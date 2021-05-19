use alloc::{sync::Arc, vec::Vec};
use core::{
    hash::{Hash, Hasher},
    mem::size_of,
};

use core_io::Read;
use lazy_static::*;
use spin::Mutex;
use xmas_elf::ElfFile;

use super::*;
use crate::{
    arch::{
        PTEImpl, RegisterImpl, TaskContextImpl, TrapFrameImpl, __switch,
        interface::{PageTable, Register, TaskContext, TrapFrame},
    },
    board::{interface::Config, ConfigImpl},
    fs::*,
    mm::*,
};

pub struct TidAllocator {
    current: usize,
    recycled: Vec<usize>,
}

impl TidAllocator {
    pub fn new() -> Self {
        TidAllocator {
            current: 0,
            recycled: Vec::with_capacity(4),
        }
    }

    pub fn alloc(&mut self) -> Tid {
        if let Some(tid) = self.recycled.pop() {
            Tid(tid)
        } else {
            self.current += 1;
            Tid(self.current - 1)
        }
    }

    pub fn dealloc(&mut self, tid: usize) {
        assert!(tid < self.current);
        assert!(
            self.recycled.iter().find(|ptid| **ptid == tid).is_none(),
            "tid {} has been deallocated!",
            tid
        );
        self.recycled.push(tid);
    }
}

lazy_static! {
    /// 用于分配 tid
    pub(super) static ref TID_ALLOCATOR: Mutex<TidAllocator> = Mutex::new(TidAllocator::new());
}

#[derive(Debug)]
pub struct Tid(usize);
impl Drop for Tid {
    fn drop(&mut self) {
        TID_ALLOCATOR.lock().dealloc(self.0);
    }
}

pub struct Thread {
    /// 线程 ID
    pub tid: Tid,
    /// 所属的进程
    pub process: Arc<Process>,
    /// 用户栈顶
    pub user_stack_top: VA,
    /// 当线程处于 Ready 状态时，task_cx 指向保存在内核栈中的 TaskContextImpl；
    pub task_cx: &'static TaskContextImpl,
    /// 用 `Mutex` 包装一些可变的变量
    pub inner: Mutex<ThreadInner>,
}

/// 线程中需要可变的部分
pub struct ThreadInner {
    /// 线程状态
    pub status: ThreadStatus,
}

#[allow(unused)]
pub enum ThreadStatus {
    Ready,
    Running,
    Waiting,
    Zombie,
}

pub fn get_kernel_stack_range(tid: usize) -> VARange {
    let kernel_stack_top = ConfigImpl::KERNEL_STACK_TOP
        - tid * (ConfigImpl::KERNEL_STACK_SIZE + ConfigImpl::GUARD_PAGE_SIZE);
    VA(kernel_stack_top - ConfigImpl::KERNEL_STACK_SIZE)..VA(kernel_stack_top)
}

const THREAD_PTR_OFFSET: usize = size_of::<usize>();
const TRAP_FRAME_OFFSET: usize = THREAD_PTR_OFFSET + size_of::<TrapFrameImpl>();

#[inline]
pub fn get_cur_kernel_stack_top() -> VA {
    // XXX 可能的问题：sp 刚好在栈底，得到 guard page 里的内容，发生 page fault
    VA(RegisterImpl::sp() - 1 + ConfigImpl::KERNEL_STACK_SIZE
        & !(ConfigImpl::KERNEL_STACK_SIZE - 1))
}
pub fn get_current_thread() -> &'static mut Thread {
    let thread_ptr = *(get_cur_kernel_stack_top() - THREAD_PTR_OFFSET).get_mut::<usize>();
    unsafe { &mut *(thread_ptr as *mut Thread) }
}
pub fn get_current_trapframe() -> &'static mut TrapFrameImpl {
    (get_cur_kernel_stack_top() - TRAP_FRAME_OFFSET).get_mut()
}

impl Thread {
    /// 创建内核线程
    pub fn new_kernel(entry: usize, _args: Option<&[usize]>) -> Arc<Thread> {
        let tid = TID_ALLOCATOR.lock().alloc();

        // 分配内核栈
        let kernel_stack_range = get_kernel_stack_range(tid.0);
        KERNEL_PROCESS.inner.lock().memory_set.insert_framed_area(
            kernel_stack_range.clone(),
            PTEImpl::READABLE | PTEImpl::WRITABLE,
        );
        // TaskContextImpl
        let task_cx = (kernel_stack_range.end - TRAP_FRAME_OFFSET).get_mut::<TaskContextImpl>();
        task_cx.set_ra(entry);

        let new_thread = Arc::new(Self {
            tid,
            process: KERNEL_PROCESS.clone(),
            user_stack_top: VA(0), // 内核线程的用户栈顶为 0，表示没有用户栈
            task_cx,
            inner: Mutex::new(ThreadInner {
                status: ThreadStatus::Ready,
            }),
        });
        *(kernel_stack_range.end - THREAD_PTR_OFFSET).get_mut::<usize>() =
            Arc::<Thread>::as_ptr(&new_thread) as usize;

        new_thread
    }

    /// 创建用户进程
    pub fn new_thread(file_name: &str, args: Option<&[usize]>) -> Arc<Thread> {
        let tid = TID_ALLOCATOR.lock().alloc();

        // 读取 elf 文件内容
        let mut app = ROOT_DIR.open_file(file_name).unwrap();
        let mut data: Vec<u8> = Vec::new();
        app.read_to_end(&mut data).unwrap();
        let elf = ElfFile::new(data.as_slice()).unwrap();
        // 创建进程
        let process = Process::from_elf(&elf, tid.0);
        // 分配用户栈
        let user_stack_top = process.alloc_user_stack();
        // 分配内核栈
        let kernel_stack_range = get_kernel_stack_range(tid.0);
        // println!("内核栈 {:#x?}", kernel_stack_range);
        KERNEL_PROCESS.inner.lock().memory_set.insert_framed_area(
            kernel_stack_range.clone(),
            PTEImpl::READABLE | PTEImpl::WRITABLE,
        );
        // TrapFrame
        let cx = (kernel_stack_range.end - TRAP_FRAME_OFFSET).get_mut::<TrapFrameImpl>();
        cx.init(
            user_stack_top.0 - size_of::<usize>(),
            elf.header.pt2.entry_point() as usize,
            args,
            true,
        );
        // TaskContextImpl
        let task_cx = VA(cx as *const TrapFrameImpl as usize - size_of::<TaskContextImpl>())
            .get_mut::<TaskContextImpl>();
        task_cx.set_ra(crate::arch::__restore as usize);
        // println!("task_cx {:#p}", task_cx);

        let new_thread = Arc::new(Self {
            tid,
            process,
            user_stack_top,
            task_cx,
            inner: Mutex::new(ThreadInner {
                status: ThreadStatus::Ready,
            }),
        });

        *(kernel_stack_range.end - THREAD_PTR_OFFSET).get_mut::<usize>() =
            Arc::<Thread>::as_ptr(&new_thread) as usize;

        new_thread
    }

    /// TODO 切换页表，因为每个线程都有可能读写用户区的数据
    pub fn switch_to(&self, other: &Thread) {
        unsafe {
            __switch(&self.task_cx, other.task_cx);
        }
    }

    /// 准备执行一个线程
    ///
    /// 激活对应进程的页表，并返回其 TrapFrame
    pub fn prepare(&self) -> *mut TrapFrameImpl {
        self.process.inner.lock().memory_set.page_table.activate();
        let kernel_stack_top = VA(ConfigImpl::KERNEL_STACK_TOP
            - self.tid.0 * (ConfigImpl::KERNEL_STACK_SIZE + ConfigImpl::GUARD_PAGE_SIZE));
        (kernel_stack_top - size_of::<TrapFrameImpl>()).get_mut()
    }

    /// 使用此 unsafe 函数时，需满足以下几点：
    /// 1. 只有用户线程才能调用此函数
    /// 2. 确保该用户线程不会回到用户态
    #[inline]
    pub unsafe fn dealloc_user_stack(&mut self) {
        self.process.dealloc_user_stack(self.user_stack_top);
    }
}

impl Eq for Thread {}
impl PartialEq for Thread {
    fn eq(&self, other: &Self) -> bool {
        self.tid.0 == other.tid.0
    }
}
/// 通过线程 ID 来哈希
impl Hash for Thread {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.tid.0);
    }
}

/// 回收内核栈
impl Drop for Thread {
    fn drop(&mut self) {
        // debug!("{:?} 线程对象 drop", self.tid);
        // TODO 暂时不移除，留给下一个线程用？
        KERNEL_PROCESS
            .inner
            .lock()
            .memory_set
            .remove_area(get_kernel_stack_range(self.tid.0).end);
    }
}
