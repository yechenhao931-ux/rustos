//! Host-side unit tests. We allocate a real `Vec<u8>` as the backing region.

use crate::buddy::BuddyAllocator;
use crate::slab::{SlabSet, CLASS_SIZES};
use crate::Heap;
use core::alloc::Layout;

fn region(size: usize) -> Vec<u8> {
    vec![0u8; size]
}

#[test]
fn buddy_basic_alloc_free() {
    let mut buf = region(1 << 20);
    let mut a = BuddyAllocator::new();
    unsafe {
        a.add_region(buf.as_mut_ptr() as usize, buf.as_mut_ptr() as usize + buf.len());
    }
    let p = a.alloc(64, 8).unwrap();
    assert_eq!(a.used(), 64);
    unsafe { a.dealloc(p, 64, 8) };
    assert_eq!(a.used(), 0);
}

#[test]
fn buddy_coalesces_buddies() {
    let mut buf = region(1 << 20);
    let mut a = BuddyAllocator::new();
    unsafe {
        a.add_region(buf.as_mut_ptr() as usize, buf.as_mut_ptr() as usize + buf.len());
    }
    let total = a.total();
    let mut ptrs = vec![];
    for _ in 0..16 {
        ptrs.push(a.alloc(64, 8).unwrap());
    }
    for p in ptrs {
        unsafe { a.dealloc(p, 64, 8) };
    }
    // After freeing in any order the allocator should not have leaked.
    assert_eq!(a.used(), 0);
    assert_eq!(a.total(), total);
    // We should be able to satisfy a large allocation again, proving that
    // small frees coalesced back up to a big block.
    let big = a.alloc(1 << 19, 8).unwrap();
    unsafe { a.dealloc(big, 1 << 19, 8) };
}

#[test]
fn slab_class_routing() {
    assert_eq!(SlabSet::class_for(1, 1), Some(0)); // -> 8
    assert_eq!(SlabSet::class_for(8, 8), Some(0));
    assert_eq!(SlabSet::class_for(9, 1), Some(1)); // -> 16
    assert_eq!(SlabSet::class_for(1024, 8), Some(7));
    assert_eq!(SlabSet::class_for(1025, 8), None);
    // Alignment may force a larger class.
    assert_eq!(SlabSet::class_for(8, 16), Some(1));
}

#[test]
fn heap_round_trip_many_layouts() {
    let mut buf = region(4 * 1024 * 1024);
    let heap = Heap::new();
    unsafe { heap.add_region(buf.as_mut_ptr() as usize, buf.len()) };
    let layouts = [
        Layout::from_size_align(1, 1).unwrap(),
        Layout::from_size_align(7, 1).unwrap(),
        Layout::from_size_align(64, 8).unwrap(),
        Layout::from_size_align(200, 16).unwrap(),
        Layout::from_size_align(1024, 8).unwrap(),
        Layout::from_size_align(4096, 4096).unwrap(),
        Layout::from_size_align(65536, 8).unwrap(),
    ];
    let mut allocs = vec![];
    for l in &layouts {
        let p = heap.alloc(*l).expect("alloc failed");
        // Touch the memory to ensure it's writable (catches mis-aligned bugs).
        unsafe {
            core::ptr::write_bytes(p.as_ptr(), 0xAB, l.size());
        }
        allocs.push((p, *l));
    }
    for (p, l) in allocs {
        unsafe { heap.dealloc(p, l) };
    }
    // After full release, slab caches keep memory cached but buddy should
    // see used == 0 once the slabs eventually relinquish (we don't free
    // slabs back, mimicking Linux SLUB behaviour for stable caches).
}

#[cfg(feature = "stats")]
#[test]
fn slab_hit_rate_dominates_for_small_allocs() {
    let mut buf = region(4 * 1024 * 1024);
    let heap = Heap::new();
    unsafe { heap.add_region(buf.as_mut_ptr() as usize, buf.len()) };
    for _ in 0..10_000 {
        let l = Layout::from_size_align(48, 8).unwrap();
        let p = heap.alloc(l).unwrap();
        unsafe { heap.dealloc(p, l) };
    }
    // 10000 allocs, only the first one (and refills) hit buddy.
    assert!(heap.stats().slab_hit_rate_pct() > 99.0);
    assert!(heap.stats().buddy_calls() < 10);
}

#[test]
fn class_sizes_are_monotonic_powers_of_two() {
    for w in CLASS_SIZES.windows(2) {
        assert!(w[1] == w[0] * 2);
    }
}

#[test]
fn alignment_is_respected() {
    let mut buf = region(1 << 20);
    let heap = Heap::new();
    unsafe { heap.add_region(buf.as_mut_ptr() as usize, buf.len()) };
    for align_log2 in 0..12 {
        let align = 1usize << align_log2;
        let l = Layout::from_size_align(64, align).unwrap();
        let p = heap.alloc(l).expect("alloc");
        assert_eq!(p.as_ptr() as usize & (align - 1), 0, "align={align}");
        unsafe { heap.dealloc(p, l) };
    }
}
