use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::String,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};

use lazy_static::lazy_static;
use spin::Mutex;
use xmas_elf::ElfFile;

use crate::{
    arch::PTEImpl,
    board::{interface::Config, ConfigImpl},
    fs::{FileDescriptor, STDIN, STDOUT},
    get_current_thread,
    mm::*,
};

lazy_static! {
    /// 内核进程，所有内核线程都属于该进程。
    /// 通过此进程来进行内核栈的分配
    pub static ref KERNEL_PROCESS: Arc<Process> = {
        info!("初始化内核进程");
        Arc::new(Process {
            pid: 0,
            inner: Mutex::new(ProcessInner {
                cwd: String::from("/"),
                memory_set: MemorySet {
                    page_table: crate::mm::page_table::kernel_page_table(),
                    areas: BTreeMap::<VARangeOrd, MapArea>::new(),
                },
                fd_table: vec![Some(STDIN.clone()), Some(STDOUT.clone())],
                parent: Weak::new(),
                child: Vec::new(),
                child_exited: Vec::new(),
                wake_callbacks: Vec::new(),
            }),
        })
    };
}

pub type Pid = usize;

pub struct Process {
    pub pid: Pid,
    /// 可变的部分。如果要更高的细粒度，去掉 ProcessInner 的 Mutex，给里面的
    /// memory_set 等等分别加上
    pub inner: Mutex<ProcessInner>,
}

pub struct ProcessInner {
    /// 当前工作目录
    pub cwd: String,
    /// 进程中的线程公用页表 / 内存映射
    pub memory_set: MemorySet,
    /// 文件描述符
    pub fd_table: Vec<Option<Arc<FileDescriptor>>>,
    /// 父进程
    pub parent: Weak<Process>,
    /// 子进程
    pub child: Vec<Weak<Process>>,
    /// 已经退出了的子进程
    pub child_exited: Vec<(Pid, Weak<Process>)>,
    /// 回调
    pub wake_callbacks: Vec<Box<dyn Fn() + Send>>,
}

impl Process {
    /// 通过 ELF 文件创建用户进程
    pub fn from_elf(file: &ElfFile, pid: usize) -> Arc<Self> {
        Arc::new(Self {
            pid,
            inner: Mutex::new(ProcessInner {
                cwd: String::from("/"),
                memory_set: MemorySet::from_elf(file),
                fd_table: vec![Some(STDIN.clone()), Some(STDOUT.clone())],
                parent: Arc::downgrade(&get_current_thread().process),
                child: Vec::new(),
                child_exited: Vec::new(),
                wake_callbacks: Vec::new(),
            }),
        })
    }

    /// fork 进程
    pub fn fork(&self, pid: usize) -> Arc<Self> {
        let mut process_inner = self.inner.lock();
        Arc::new(Self {
            pid,
            inner: Mutex::new(ProcessInner {
                cwd: process_inner.cwd.clone(),
                memory_set: process_inner.memory_set.fork(),
                fd_table: process_inner.fd_table.clone(),
                parent: Arc::downgrade(&get_current_thread().process),
                child: Vec::new(),
                child_exited: Vec::new(),
                wake_callbacks: Vec::new(),
            }),
        })
    }

    /// 分配并映射线程的用户栈
    pub fn alloc_user_stack(&self) -> VA {
        let mut inner = self.inner.lock();
        let user_stack_top = inner
            .memory_set
            .alloc_user_area(ConfigImpl::USER_STACK_SIZE);
        inner.memory_set.insert_framed_area(
            user_stack_top - ConfigImpl::USER_STACK_SIZE..user_stack_top,
            PTEImpl::READABLE | PTEImpl::WRITABLE | PTEImpl::USER,
            None,
        );

        user_stack_top
    }

    #[inline]
    /// **UNSAFE**
    pub(super) unsafe fn dealloc_user_stack(&self, user_stack_top: VA) {
        self.inner.lock().memory_set.remove_area(user_stack_top);
    }
}

impl ProcessInner {
    pub const MAX_FD: usize = 101;

    pub fn fd_alloc(&mut self) -> isize {
        let len = self.fd_table.len();
        for i in 2..self.fd_table.len() {
            if self.fd_table[i].is_none() {
                return i as isize;
            }
        }
        if len == Self::MAX_FD {
            return -1;
        }
        self.fd_table.push(None);
        len as isize
    }
}
