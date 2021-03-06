use bitflags::bitflags;

use crate::{
    arch::interface::{PageTable, PTE},
    frame_alloc, FrameTracker, KERNEL_PAGE_TABLE, PPN, VPN,
};

bitflags! {
    /// TODO 使用 tock_registers 的 register_bitfields
    /// 页表项中的 8 个标志位
    #[derive(Default)]
    pub struct PTEImpl: usize {
        const EMPTY =            0;
        /// 有效位
        const VALID =       1 << 0;
        /// 可读位
        const READABLE =    1 << 1;
        /// 可写位
        const WRITABLE =    1 << 2;
        /// 可执行位
        const EXECUTABLE =  1 << 3;
        /// 用户位
        const USER =        1 << 4;
        /// 全局位
        const GLOBAL =      1 << 5;
        /// 已使用位
        const ACCESSED =    1 << 6;
        /// 已修改位
        const DIRTY =       1 << 7;

        /// Copy-on-Write
        const COW =         1 << 8;
    }
}

impl PTE for PTEImpl {
    const EMPTY: Self = Self::EMPTY;
    const EXECUTABLE: Self = Self::EXECUTABLE;
    const READABLE: Self = Self::READABLE;
    const USER: Self = Self::USER;
    const VALID: Self = Self::VALID;
    const WRITABLE: Self = Self::WRITABLE;

    fn new(ppn: PPN, flags: Self) -> Self {
        unsafe { Self::from_bits_unchecked(ppn.0 << 10) | Self::from_bits_truncate(flags.bits) }
    }

    fn set_ppn(&mut self, ppn: PPN) {
        *self =
            unsafe { Self::from_bits_unchecked(ppn.0 << 10) | Self::from_bits_truncate(self.bits) };
    }

    fn ppn(self) -> PPN {
        // K210 的 Sv39 物理地址长度为 50 位而不是 56 位
        #[cfg(feature = "k210")]
        return (self.bits >> 10 & ((1usize << 38) - 1)).into();

        #[cfg(not(feature = "k210"))]
        return (self.bits >> 10 & ((1usize << 44) - 1)).into();
    }

    fn is_valid(self) -> bool {
        (self & Self::VALID) != Self::EMPTY
    }
}

use alloc::{vec, vec::Vec};

/// 用来映射一个地址空间的页表
///
/// TODO field 都改成 private
pub struct PageTableImpl {
    /// 根页表的页框
    pub root: FrameTracker,
    /// 其他子页表的页框
    pub frames: Vec<FrameTracker>,
}

impl PageTable for PageTableImpl {
    /// 创建一个映射了高 256 G的内核区域地址的页表
    fn new_kernel() -> Self {
        let frame = frame_alloc().unwrap();
        // 前 256 个 PTE 清零
        VPN::from(frame.ppn).get_array::<PTEImpl>()[..256].fill(PTEImpl::EMPTY);
        // 从 KERNEL_PAGE_TABLE 复制高 256G 的内核区域的映射关系
        VPN::from(frame.ppn).get_array::<PTEImpl>()[256..]
            .copy_from_slice(&VPN::from(KERNEL_PAGE_TABLE.root.ppn).get_array::<PTEImpl>()[256..]);

        PageTableImpl {
            root: frame,
            frames: vec![],
        }
    }

    /// 查找 vpn 虚拟页号对应的 pte，若不存在会创建页表
    fn find_pte_create(&mut self, vpn: VPN) -> Option<&mut PTEImpl> {
        let idxs = vpn.indexes();
        let mut pte: &mut PTEImpl = &mut VPN::from(self.root.ppn).get_array()[idxs[0]];
        for &idx in &idxs[1..] {
            if !pte.is_valid() {
                let frame = frame_alloc().unwrap();
                VPN::from(frame.ppn)
                    .get_array::<PTEImpl>()
                    .fill(PTEImpl::EMPTY);
                *pte = PTEImpl::new(frame.ppn, PTEImpl::VALID);
                self.frames.push(frame);
            }
            pte = &mut VPN::from(pte.ppn()).get_array()[idx];
        }
        Some(pte)
    }

    /// 查找 vpn 虚拟页号对应的 pte
    fn find_pte(&self, vpn: VPN) -> Option<&mut PTEImpl> {
        let idxs = vpn.indexes();
        let mut pte: &mut PTEImpl = &mut VPN::from(self.root.ppn).get_array()[idxs[0]];
        for &idx in &idxs[1..] {
            if !pte.is_valid() {
                return None;
            }
            pte = &mut VPN::from(pte.ppn()).get_array()[idx];
        }
        if !pte.is_valid() {
            return None;
        }
        Some(pte)
    }

    /// 激活页表
    fn activate(&self) {
        #[cfg(feature = "k210")]
        {
            // Sv39
            let sptbr = self.root.ppn.0 & ((1usize << 38) - 1);
            unsafe {
                // 写入 sptbr 寄存器
                llvm_asm!("csrw 0x180, $0"::"r"(sptbr));
                // sfence.vm
                asm!("fence", "fence.i", ".word 0x10400073", "fence", "fence.i");
            }
        }

        #[cfg(not(feature = "k210"))]
        {
            // Sv39
            let satp = 8usize << 60 | self.root.ppn.0;
            riscv::register::satp::write(satp);
            unsafe { asm!("sfence.vma") }
        }
    }
}
