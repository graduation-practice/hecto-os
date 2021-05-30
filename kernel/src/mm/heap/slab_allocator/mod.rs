//! 来自 https://github.com/weclaw1/slab_allocator

#![allow(dead_code)]

mod slab;

use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::NonNull,
};

use slab::Slab;

use crate::sync::SpinLock;

pub const NUM_OF_SLABS: usize = 8;
pub const MIN_SLAB_SIZE: usize = 4096;
pub const MIN_HEAP_SIZE: usize = NUM_OF_SLABS * MIN_SLAB_SIZE;

pub struct LockedHeap(SpinLock<Option<Heap>>);

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0.lock().as_mut().unwrap().allocate(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.0.lock().as_mut().unwrap().deallocate(ptr, layout)
    }
}

impl LockedHeap {
    pub const fn empty() -> Self {
        Self(SpinLock::new(None))
    }
    pub unsafe fn init(&mut self, heap_start_addr: usize, size: usize) {
        *self.0.lock() = Some(Heap::new(heap_start_addr, size));
    }
}

pub enum HeapAllocator {
    Slab64Bytes,
    Slab128Bytes,
    Slab256Bytes,
    Slab512Bytes,
    Slab1024Bytes,
    Slab2048Bytes,
    Slab4096Bytes,
    LinkedListAllocator,
}

/// A fixed size heap backed by multiple slabs with blocks of different sizes.
/// Allocations over 4096 bytes are served by linked list allocator.
struct Heap {
    slab_64_bytes: Slab,
    slab_128_bytes: Slab,
    slab_256_bytes: Slab,
    slab_512_bytes: Slab,
    slab_1024_bytes: Slab,
    slab_2048_bytes: Slab,
    slab_4096_bytes: Slab,
    linked_list_allocator: linked_list_allocator::Heap,
}

impl Heap {
    /// Creates a new heap with the given `heap_start_addr` and `heap_size`. The
    /// start address must be valid and the memory in the `[heap_start_addr,
    /// heap_start_addr + heap_size)` range must not be used for
    /// anything else. This function is unsafe because it can cause undefined
    /// behavior if the given address is invalid.
    pub unsafe fn new(heap_start_addr: usize, heap_size: usize) -> Heap {
        assert!(
            heap_start_addr % 4096 == 0,
            "Start address should be page aligned"
        );
        assert!(
            heap_size >= MIN_HEAP_SIZE,
            "Heap size should be greater or equal to minimum heap size"
        );
        assert!(
            heap_size % MIN_HEAP_SIZE == 0,
            "Heap size should be a multiple of minimum heap size"
        );
        let slab_size = heap_size / NUM_OF_SLABS;
        Heap {
            slab_64_bytes: Slab::new(heap_start_addr, slab_size, 64),
            slab_128_bytes: Slab::new(heap_start_addr + slab_size, slab_size, 128),
            slab_256_bytes: Slab::new(heap_start_addr + 2 * slab_size, slab_size, 256),
            slab_512_bytes: Slab::new(heap_start_addr + 3 * slab_size, slab_size, 512),
            slab_1024_bytes: Slab::new(heap_start_addr + 4 * slab_size, slab_size, 1024),
            slab_2048_bytes: Slab::new(heap_start_addr + 5 * slab_size, slab_size, 2048),
            slab_4096_bytes: Slab::new(heap_start_addr + 6 * slab_size, slab_size, 4096),
            linked_list_allocator: linked_list_allocator::Heap::new(
                heap_start_addr + 7 * slab_size,
                slab_size,
            ),
        }
    }

    /// Adds memory to the heap. The start address must be valid
    /// and the memory in the `[mem_start_addr, mem_start_addr + heap_size)`
    /// range must not be used for anything else.
    /// In case of linked list allocator the memory can only be extended.
    /// This function is unsafe because it can cause undefined behavior if the
    /// given address is invalid.
    pub unsafe fn grow(&mut self, mem_start_addr: usize, mem_size: usize, slab: HeapAllocator) {
        match slab {
            HeapAllocator::Slab64Bytes => self.slab_64_bytes.grow(mem_start_addr, mem_size),
            HeapAllocator::Slab128Bytes => self.slab_128_bytes.grow(mem_start_addr, mem_size),
            HeapAllocator::Slab256Bytes => self.slab_256_bytes.grow(mem_start_addr, mem_size),
            HeapAllocator::Slab512Bytes => self.slab_512_bytes.grow(mem_start_addr, mem_size),
            HeapAllocator::Slab1024Bytes => self.slab_1024_bytes.grow(mem_start_addr, mem_size),
            HeapAllocator::Slab2048Bytes => self.slab_2048_bytes.grow(mem_start_addr, mem_size),
            HeapAllocator::Slab4096Bytes => self.slab_4096_bytes.grow(mem_start_addr, mem_size),
            HeapAllocator::LinkedListAllocator => self.linked_list_allocator.extend(mem_size),
        }
    }

    /// Allocates a chunk of the given size with the given alignment. Returns a
    /// pointer to the beginning of that chunk if it was successful. Else it
    /// returns `Err`. This function finds the slab of lowest size which can
    /// still accomodate the given chunk. The runtime is in `O(1)` for
    /// chunks of size <= 4096, and `O(n)` when chunk size is > 4096,
    pub fn allocate(&mut self, layout: Layout) -> *mut u8 {
        match Heap::layout_to_allocator(&layout) {
            HeapAllocator::Slab64Bytes => self.slab_64_bytes.allocate(layout),
            HeapAllocator::Slab128Bytes => self.slab_128_bytes.allocate(layout),
            HeapAllocator::Slab256Bytes => self.slab_256_bytes.allocate(layout),
            HeapAllocator::Slab512Bytes => self.slab_512_bytes.allocate(layout),
            HeapAllocator::Slab1024Bytes => self.slab_1024_bytes.allocate(layout),
            HeapAllocator::Slab2048Bytes => self.slab_2048_bytes.allocate(layout),
            HeapAllocator::Slab4096Bytes => self.slab_4096_bytes.allocate(layout),
            HeapAllocator::LinkedListAllocator => self
                .linked_list_allocator
                .allocate_first_fit(layout)
                .unwrap()
                .as_ptr(),
        }
    }

    /// Frees the given allocation. `ptr` must be a pointer returned
    /// by a call to the `allocate` function with identical size and alignment.
    /// Undefined behavior may occur for invalid arguments, thus this
    /// function is unsafe.
    ///
    /// This function finds the slab which contains address of `ptr` and adds
    /// the blocks beginning with `ptr` address to the list of free blocks.
    /// This operation is in `O(1)` for blocks <= 4096 bytes and `O(n)` for
    /// blocks > 4096 bytes.
    pub unsafe fn deallocate(&mut self, ptr: *mut u8, layout: Layout) {
        match Heap::layout_to_allocator(&layout) {
            HeapAllocator::Slab64Bytes => self.slab_64_bytes.deallocate(ptr),
            HeapAllocator::Slab128Bytes => self.slab_128_bytes.deallocate(ptr),
            HeapAllocator::Slab256Bytes => self.slab_256_bytes.deallocate(ptr),
            HeapAllocator::Slab512Bytes => self.slab_512_bytes.deallocate(ptr),
            HeapAllocator::Slab1024Bytes => self.slab_1024_bytes.deallocate(ptr),
            HeapAllocator::Slab2048Bytes => self.slab_2048_bytes.deallocate(ptr),
            HeapAllocator::Slab4096Bytes => self.slab_4096_bytes.deallocate(ptr),
            HeapAllocator::LinkedListAllocator => self
                .linked_list_allocator
                .deallocate(NonNull::new(ptr).unwrap(), layout),
        }
    }

    /// Returns bounds on the guaranteed usable size of a successful
    /// allocation created with the specified `layout`.
    pub fn usable_size(&self, layout: &Layout) -> (usize, usize) {
        match Heap::layout_to_allocator(&layout) {
            HeapAllocator::Slab64Bytes => (layout.size(), 64),
            HeapAllocator::Slab128Bytes => (layout.size(), 128),
            HeapAllocator::Slab256Bytes => (layout.size(), 256),
            HeapAllocator::Slab512Bytes => (layout.size(), 512),
            HeapAllocator::Slab1024Bytes => (layout.size(), 1024),
            HeapAllocator::Slab2048Bytes => (layout.size(), 2048),
            HeapAllocator::Slab4096Bytes => (layout.size(), 4096),
            HeapAllocator::LinkedListAllocator => (layout.size(), layout.size()),
        }
    }

    ///Finds allocator to use based on layout size and alignment
    pub fn layout_to_allocator(layout: &Layout) -> HeapAllocator {
        if layout.size() > 4096 {
            HeapAllocator::LinkedListAllocator
        } else if layout.size() <= 64 && layout.align() <= 64 {
            HeapAllocator::Slab64Bytes
        } else if layout.size() <= 128 && layout.align() <= 128 {
            HeapAllocator::Slab128Bytes
        } else if layout.size() <= 256 && layout.align() <= 256 {
            HeapAllocator::Slab256Bytes
        } else if layout.size() <= 512 && layout.align() <= 512 {
            HeapAllocator::Slab512Bytes
        } else if layout.size() <= 1024 && layout.align() <= 1024 {
            HeapAllocator::Slab1024Bytes
        } else if layout.size() <= 2048 && layout.align() <= 2048 {
            HeapAllocator::Slab2048Bytes
        } else {
            HeapAllocator::Slab4096Bytes
        }
    }
}
