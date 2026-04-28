//! Kernel heap.
//!
//! Replaces the upstream `buddy_system_allocator::LockedHeap` with the
//! project's two-level `kalloc::Heap` (slab caches over a buddy backend).
//! See `docs/allocator.md` for design notes and host-side benchmarks.

use crate::config::KERNEL_HEAP_SIZE;
use core::ptr::addr_of_mut;
use kalloc::Heap;

#[global_allocator]
static HEAP_ALLOCATOR: Heap = Heap::new();

#[alloc_error_handler]
pub fn handle_alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

static mut HEAP_SPACE: [u8; KERNEL_HEAP_SIZE] = [0; KERNEL_HEAP_SIZE];

pub fn init_heap() {
    unsafe {
        HEAP_ALLOCATOR.add_region(addr_of_mut!(HEAP_SPACE) as usize, KERNEL_HEAP_SIZE);
    }
}

/// Snapshot the kernel heap state for debug shell / panic dump.
#[allow(dead_code)]
pub fn heap_stats() -> (kalloc::HeapSnapshot, &'static kalloc::AllocStats) {
    (HEAP_ALLOCATOR.snapshot(), HEAP_ALLOCATOR.stats())
}

#[allow(unused)]
pub fn heap_test() {
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    unsafe extern "C" {
        safe fn sbss();
        safe fn ebss();
    }
    let bss_range = sbss as usize..ebss as usize;
    let a = Box::new(5);
    assert_eq!(*a, 5);
    assert!(bss_range.contains(&(a.as_ref() as *const _ as usize)));
    drop(a);
    let mut v: Vec<usize> = Vec::new();
    for i in 0..500 {
        v.push(i);
    }
    for (i, val) in v.iter().take(500).enumerate() {
        assert_eq!(*val, i);
    }
    assert!(bss_range.contains(&(v.as_ptr() as usize)));
    drop(v);
    println!("heap_test passed!");
}
