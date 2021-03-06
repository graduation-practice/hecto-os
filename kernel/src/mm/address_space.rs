use alloc::collections::BTreeMap;
use core::cmp::max;

use core_io::SeekFrom;
use xmas_elf::program::Type;

use super::{FrameTracker, VARange, VARangeOrd, VA, VPN};
use crate::{
    arch::{
        interface::{PageTable, PTE},
        PTEImpl, PageTableImpl,
    },
    board::{interface::Config, ConfigImpl},
    frame_alloc, ElfFileExt,
};

#[derive(Clone, Copy)]
pub enum MapType {
    /// 线性映射
    Linear,
    /// 按帧映射
    Framed,
    /// 设备
    Device,
    /// 内核栈
    KernelStack,
}

/// 一段连续地址的虚拟内存映射片段，Linux 中，线性区描述符为 vm_area_struct
#[derive(Clone)]
pub struct MapArea {
    pub data_frames: BTreeMap<VPN, FrameTracker>,
    pub map_type: MapType,
    pub map_perm: PTEImpl,
}

/// 每个 proccess 的地址空间，类似于 Linux 中的 mm_struct
pub struct AddressSpace {
    /// 页表
    pub page_table: PageTableImpl,
    pub areas: BTreeMap<VARangeOrd, MapArea>,
    pub data_segment_end: VA,
    pub data_segment_max: VA,
}

impl AddressSpace {
    /// sys_brk 可以增加的内存上限
    const BRK_MAX: usize = 0x3000;

    /// 创建一个映射了内核区域的 AddressSpace
    pub fn new_kernel() -> Self {
        let mut page_table = PageTableImpl::new_kernel();
        for pair in ConfigImpl::MMIO {
            page_table.map(
                VARangeOrd(VA(pair.0)..VA(pair.0 + pair.1)),
                &mut MapArea {
                    data_frames: BTreeMap::new(),
                    map_type: MapType::Device,
                    map_perm: PTEImpl::READABLE | PTEImpl::WRITABLE,
                },
                None,
            );
        }

        Self {
            page_table,
            areas: BTreeMap::<VARangeOrd, MapArea>::new(),
            data_segment_end: VA(1),
            data_segment_max: VA(1),
        }
    }

    /// fork 一份 CoW 的 AddressSpace
    pub fn fork(&mut self) -> Self {
        let mut new_as = Self::new_kernel();

        for (range, area) in self.areas.iter() {
            let mut flags = area.map_perm;
            if flags.contains(PTEImpl::WRITABLE) || flags.contains(PTEImpl::COW) {
                flags.remove(PTEImpl::WRITABLE);
                flags.insert(PTEImpl::COW);
                // println!("{:#x?} {:?}", range.0, flags);
                for (&vpn, frame_tracker) in area.data_frames.iter() {
                    new_as.page_table.map_one(vpn, frame_tracker.ppn, flags);
                    self.page_table.remap_one(vpn, frame_tracker.ppn, flags);
                    // XXX 因为修改了当前的页表，所以此处需要 sfence.vma
                    unsafe { llvm_asm!("sfence.vma $0, x0" :: "r"(VA::from(vpn).0) :: "volatile") };
                }
                new_as.areas.insert(range.clone(), area.clone());
            } else {
                for (&vpn, frame_tracker) in area.data_frames.iter() {
                    new_as.page_table.map_one(vpn, frame_tracker.ppn, flags);
                }
                new_as.areas.insert(range.clone(), area.clone());
            }
        }

        new_as.data_segment_end = self.data_segment_end;
        new_as.data_segment_max = self.data_segment_max;

        new_as
    }

    pub fn handle_pagefault(&mut self, va: VA) {
        let vpn = va.floor();
        let pte = self.page_table.find_pte(vpn).unwrap();
        // debug!(
        //     "va {:#x} {:?} vpn {:#x} ppn {:#x}",
        //     va.0,
        //     pte,
        //     vpn.0,
        //     pte.ppn().0
        // );
        if !pte.contains(PTEImpl::COW) {
            println!(
                "va {:#x} {:?} vpn {:#x} ppn {:#x}",
                va.0,
                pte,
                vpn.0,
                pte.ppn().0
            );
            panic!("handle_pagefault error");
        }
        pte.remove(PTEImpl::COW);
        pte.insert(PTEImpl::WRITABLE);

        // 接下来判断是否需要复制页面
        let area = self.areas.get_mut(&VARangeOrd(va..va)).unwrap();
        let frame = area.data_frames.get_mut(&vpn).unwrap();
        if frame.get_ref_count() > 1 {
            let new_frame = frame_alloc().unwrap();
            VPN::from(new_frame.ppn)
                .get_array::<usize>()
                .copy_from_slice(vpn.get_array());
            pte.set_ppn(new_frame.ppn);
            // trace!("{:?}", pte);
            *frame = new_frame;
        }

        #[cfg(not(feature = "k210"))]
        unsafe {
            llvm_asm!("sfence.vma $0, x0" :: "r"(va.0) :: "volatile");
        }
        #[cfg(feature = "k210")]
        unsafe {
            // TODO 缩小 sfence.vm 范围
            asm!("fence", "fence.i", ".word 0x10400073", "fence", "fence.i");
        }
    }

    /// 移除一段 area
    pub fn remove_area(&mut self, va: VA) {
        for vpn in self
            .areas
            .remove_entry(&VARangeOrd(va..va))
            .unwrap()
            .0
            .vpn_range()
        {
            self.page_table.unmap_one(vpn);
        }
    }

    /// 在地址空间插入一段按帧映射的区域，未检查重叠区域
    pub fn insert_framed_area(
        &mut self,
        va_range: VARange,
        map_perm: PTEImpl,
        data: Option<&[u8]>,
    ) {
        let mut area = MapArea {
            data_frames: BTreeMap::new(),
            map_type: MapType::Framed,
            map_perm,
        };
        debug!(
            "insert_framed_area {} {:?}",
            VARangeOrd(va_range.clone()),
            map_perm
        );
        self.page_table
            .map(VARangeOrd(va_range.clone()), &mut area, data);
        self.areas.insert(VARangeOrd(va_range), area);
    }

    pub fn insert_kernel_stack_area(&mut self, va_range: VARange) {
        let mut area = MapArea {
            data_frames: BTreeMap::new(),
            map_type: MapType::KernelStack,
            map_perm: PTEImpl::READABLE | PTEImpl::WRITABLE,
        };
        debug!("insert_kernel_stack_area {}", VARangeOrd(va_range.clone()));
        self.page_table
            .map(VARangeOrd(va_range.clone()), &mut area, None);
        self.areas.insert(VARangeOrd(va_range), area);
    }

    /// 通过 elf 文件创建地址空间（不包括栈）
    pub fn from_elf(elf_file: &mut ElfFileExt) -> Self {
        let mut address_space = Self::new_kernel();

        // 映射所有 Segment
        for ph in elf_file.elf.program_iter() {
            if ph.get_type() != Ok(Type::Load) {
                continue;
            }
            // println!("{:?}", ph);
            let start_addr = ph.virtual_addr() as usize; // segment 在内存中加载到的虚拟地址
            let mem_size = ph.mem_size() as usize; // 所占虚拟地址大小
            let offset = ph.offset(); // segment 相对于 ELF 文件起始处的偏移
            let file_size = ph.file_size() as usize; // segment 数据在文件中所占大小

            let va_range = VARangeOrd(VA(start_addr)..VA(start_addr + mem_size));
            let bss_start = VA(start_addr + file_size); // bss section 起始处

            let mut map_perm = PTEImpl::USER;
            let flags = ph.flags(); // RWX 权限
            map_perm.set(PTEImpl::READABLE, flags.is_read());
            map_perm.set(PTEImpl::WRITABLE, flags.is_write());
            map_perm.set(PTEImpl::EXECUTABLE, flags.is_execute());

            let mut area = MapArea {
                data_frames: BTreeMap::new(),
                map_type: MapType::Framed,
                map_perm,
            };

            elf_file.file.seek(SeekFrom::Start(offset)).unwrap();
            let mut start = VA(start_addr).page_offset();
            let mut end;
            for vpn in va_range.vpn_range() {
                let dst_frame = frame_alloc().unwrap();
                let data = VPN::from(dst_frame.ppn).get_array::<u8>();
                end = if VA::from(vpn + 1) <= bss_start {
                    4096
                } else if VA::from(vpn) >= bss_start {
                    0
                } else {
                    bss_start.page_offset()
                };
                let end2 = if VA::from(vpn + 1) <= va_range.0.end {
                    4096
                } else {
                    va_range.0.end.page_offset()
                };

                // data section
                elf_file.file.read_exact(&mut data[start..end]).unwrap();
                // bss section
                data[end..end2].fill(0);
                start = 0;

                address_space
                    .page_table
                    .map_one(vpn, dst_frame.ppn, map_perm);
                area.data_frames.insert(vpn, dst_frame);
            }

            address_space.data_segment_end = va_range.0.end;
            address_space.areas.insert(va_range.clone(), area);

            debug!("insert segment {} {:?}", VARangeOrd(va_range.0), map_perm);
        }

        address_space.data_segment_max = VA(round_up!(
            address_space.data_segment_end.0,
            ConfigImpl::PAGE_SIZE
        )) + Self::BRK_MAX;

        address_space
    }

    // 在低地址区域划分一块可用的区域，返回 va_end
    pub fn alloc_user_area(&mut self, size: usize) -> VA {
        let mut va_end = self.data_segment_max + ConfigImpl::PAGE_SIZE + size;
        for area in self.areas.keys() {
            if va_end + ConfigImpl::PAGE_SIZE <= area.0.start {
                break;
            }
            va_end = max(
                va_end,
                VA(round_up!(area.0.end.0, ConfigImpl::PAGE_SIZE)) + ConfigImpl::PAGE_SIZE + size,
            );
        }

        va_end
    }

    /// addr = 0 时，返回 data 段末尾地址。否则成功返回 0，失败 -1
    pub fn brk(&mut self, addr: VA) -> isize {
        let cur_end = VA(round_up!(self.data_segment_end.0, ConfigImpl::PAGE_SIZE));
        if addr.0 == 0 {
            return cur_end.0 as isize;
        }
        // 如果 addr 超过了允许的范围
        if addr > self.data_segment_max {
            return -1;
        }

        let (mut va_range, mut area) = self
            .areas
            .remove_entry(&VARangeOrd(self.data_segment_end..self.data_segment_end))
            .unwrap();
        // 如果需要分配新的页面
        if addr >= cur_end {
            for vpn in VARangeOrd(cur_end..addr).vpn_range() {
                let dst_frame = frame_alloc().unwrap();
                self.page_table.map_one(vpn, dst_frame.ppn, area.map_perm);
                unsafe { llvm_asm!("sfence.vma $0, x0" :: "r"(VA::from(vpn).0) :: "volatile") };
                area.data_frames.insert(vpn, dst_frame);
            }
        }
        self.data_segment_end = addr;
        va_range.0.end = addr;
        self.areas.insert(va_range, area);

        addr.0 as isize
    }
}
