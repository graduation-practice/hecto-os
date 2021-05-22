//! 内存管理模块

#[macro_use]
pub mod address;
pub mod frame_allocator;
pub mod heap;
pub mod memory_set;
pub mod page_table;

pub use address::{VARange, VARangeOrd, VPNRange, PA, PPN, VA, VPN};
pub use frame_allocator::{frame_alloc, Frame, FrameTracker};
pub use memory_set::{MapArea, MapType, MemorySet};
pub use page_table::KERNEL_PAGE_TABLE;

/// 初始化内存相关的子模块
pub fn init() {
    heap::init();
    frame_allocator::init_frame_allocator();

    info!("mod memory initialized");
}

/// bss 段清零
pub fn clear_bss() {
    use crate::board::*;
    unsafe {
        core::slice::from_raw_parts_mut(sbss as *mut usize, ebss as usize - sbss as usize).fill(0);
    }
}

pub mod interface {
    pub use super::page_table::{PageTable, PTE};

    pub trait Config<const MMIO_N: usize> {
        /// 内核使用线性映射的偏移量
        const KERNEL_MAP_OFFSET: usize;
        /// 用户栈大小
        const USER_STACK_SIZE: usize;
        /// 每个内核栈的栈顶都为 1 << KERNEL_STACK_SIZE_BITS 的倍数
        const KERNEL_STACK_ALIGN_BITS: usize;
        /// 内核栈大小，最大为 1 << KERNEL_STACK_SIZE_BITS - PAGE_SIZE
        const KERNEL_STACK_SIZE: usize;
        /// 内核堆大小
        const KERNEL_HEAP_SIZE: usize;
        /// 内存起始地址
        const MEMORY_START: usize;
        /// 内存大小
        const MEMORY_SIZE: usize;
        /// PAGE_SIZE = 1 << PAGE_SIZE_BITS
        const PAGE_SIZE_BITS: usize;
        /// MMIO 起始地址
        const MMIO: [(usize, usize); MMIO_N];
        /// 时钟频率
        const CLOCK_FREQ: usize;

        /// PAGE 大小
        const PAGE_SIZE: usize = 1 << Self::PAGE_SIZE_BITS;
        /// 内核栈对齐大小
        const KERNEL_STACK_ALIGN_SIZE: usize = 1 << Self::KERNEL_STACK_ALIGN_BITS;
        /// 内核栈之间的 guard page
        const KERNEL_STACK_GUARD_PAGE_SIZE: usize =
            Self::KERNEL_STACK_ALIGN_SIZE - Self::KERNEL_STACK_SIZE;
        /// 第 0 个内核栈的栈顶
        const KERNEL_STACK_TOP: usize = usize::MAX - Self::KERNEL_STACK_ALIGN_SIZE + 1;
        /// 可用内存末尾
        const MEMORY_END: usize = Self::MEMORY_START + Self::MEMORY_SIZE;
    }
}
