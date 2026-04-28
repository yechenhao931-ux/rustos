//! Slab allocator layered on top of the buddy allocator.
//!
//! Each `SlabCache` owns one size class. Free objects are linked through
//! their own storage (intrusive freelist), so per-object metadata is zero.
//! When a cache is empty it pulls a slab (a contiguous power-of-two run)
//! from the buddy and partitions it into objects.
//!
//! Why slab on top of buddy?
//!   * Pure buddy rounds 24 bytes up to 64 — 62% wasted per `Box<u32>`.
//!   * The kernel allocates many same-sized fixed objects (TCBs, file
//!     descriptors, dentries). Slab gives them a hot per-class freelist.
//!   * Slab refills are O(slab/obj_size) but happen rarely once steady
//!     state is reached, so amortised alloc cost is one freelist pop.

use core::ptr::NonNull;

use crate::buddy::BuddyAllocator;
use crate::linked_list::FreeList;

/// Size classes managed by slab. Anything larger goes straight to buddy.
pub const CLASS_SIZES: [usize; 8] = [8, 16, 32, 64, 128, 256, 512, 1024];

/// How many bytes to pull from buddy per slab refill, by class index.
/// Larger refills amortise lock + buddy costs across more objects.
const REFILL_BYTES: [usize; 8] = [
    4096, 4096, 4096, 4096, 4096, 4096, 8192, 8192,
];

pub struct SlabCache {
    free: FreeList,
    obj_size: usize,
    /// Total objects ever provisioned into this cache (debug stat).
    provisioned: usize,
}

impl SlabCache {
    pub const fn new(obj_size: usize) -> Self {
        Self {
            free: FreeList::new(),
            obj_size,
            provisioned: 0,
        }
    }

    pub fn obj_size(&self) -> usize {
        self.obj_size
    }
    pub fn free_count(&self) -> usize {
        self.free.len()
    }
    pub fn provisioned(&self) -> usize {
        self.provisioned
    }

    /// Refill this cache by pulling `bytes` from `buddy` and slicing it.
    /// Returns false if the buddy refused (out of memory).
    fn refill(&mut self, buddy: &mut BuddyAllocator, bytes: usize) -> bool {
        let Some(base) = buddy.alloc(bytes, self.obj_size) else {
            return false;
        };
        let base_addr = base.as_ptr() as usize;
        // Push objects high-address first. Subsequent pops then yield
        // ascending addresses, which is friendlier to hardware prefetch
        // on bursty allocation patterns (e.g. Vec growth).
        let n = bytes / self.obj_size;
        for i in (0..n).rev() {
            let addr = base_addr + i * self.obj_size;
            unsafe { self.free.push(addr as *mut u8) };
        }
        self.provisioned += n;
        true
    }

    pub fn alloc(&mut self, buddy: &mut BuddyAllocator, refill: usize) -> Option<NonNull<u8>> {
        if self.free.is_empty() && !self.refill(buddy, refill) {
            return None;
        }
        let p = self.free.pop()?;
        NonNull::new(p)
    }

    pub unsafe fn dealloc(&mut self, ptr: NonNull<u8>) {
        unsafe { self.free.push(ptr.as_ptr()) };
    }
}

pub struct SlabSet {
    caches: [SlabCache; 8],
}

impl SlabSet {
    pub const fn new() -> Self {
        Self {
            caches: [
                SlabCache::new(CLASS_SIZES[0]),
                SlabCache::new(CLASS_SIZES[1]),
                SlabCache::new(CLASS_SIZES[2]),
                SlabCache::new(CLASS_SIZES[3]),
                SlabCache::new(CLASS_SIZES[4]),
                SlabCache::new(CLASS_SIZES[5]),
                SlabCache::new(CLASS_SIZES[6]),
                SlabCache::new(CLASS_SIZES[7]),
            ],
        }
    }

    /// Class index for `size`/`align`, or `None` if the request is too big.
    ///
    /// O(1): we know the class sizes are 2^3..2^10, so the index is just
    /// `ilog2(round_up_pow2(need)) - 3`. Since every class size is a power
    /// of two and we round up to the next one, the alignment requirement is
    /// satisfied automatically (a 64-byte block is 64-byte aligned, etc.).
    #[inline]
    pub fn class_for(size: usize, align: usize) -> Option<usize> {
        const MIN_CLS: usize = 8;
        const MAX_CLS: usize = 1024;
        let need = size.max(align).max(MIN_CLS);
        if need > MAX_CLS {
            return None;
        }
        let nph = need.next_power_of_two();
        // 8 -> 0, 16 -> 1, ..., 1024 -> 7
        Some(nph.trailing_zeros() as usize - 3)
    }

    pub fn alloc(&mut self, idx: usize, buddy: &mut BuddyAllocator) -> Option<NonNull<u8>> {
        self.caches[idx].alloc(buddy, REFILL_BYTES[idx])
    }

    pub unsafe fn dealloc(&mut self, idx: usize, ptr: NonNull<u8>) {
        unsafe { self.caches[idx].dealloc(ptr) };
    }

    pub fn cache(&self, idx: usize) -> &SlabCache {
        &self.caches[idx]
    }
}
