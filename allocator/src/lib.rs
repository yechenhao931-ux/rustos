//! `kalloc` — a small but production-shaped two-level kernel allocator.
//!
//! Layout:
//!
//! ```text
//!   +------------------------ GlobalAlloc API --------------------------+
//!   |                                                                   |
//!   |   alloc(layout)      |--- size <= 1024B ---> SlabSet (per-class)  |
//!   |   dealloc(ptr,layout)|                            |               |
//!   |                      |                            v               |
//!   |                      |                       BuddyAllocator       |
//!   |                      |--- size  > 1024B ---------^                |
//!   +-------------------------------------------------------------------+
//! ```
//!
//! Properties:
//!   * `no_std`. Optional `stats` feature adds atomic counters.
//!   * Single spinlock around the whole allocator for simplicity. The slab
//!     layer makes the lock cheap because most allocs are a single
//!     freelist pop while holding it.
//!   * Designed to be plugged in as `#[global_allocator]` in a kernel,
//!     or used standalone via `Heap::alloc`/`dealloc` for benchmarking.

#![cfg_attr(not(feature = "std"), no_std)]
#![allow(clippy::missing_safety_doc)]

extern crate alloc;

pub mod buddy;
pub mod linked_list;
pub mod slab;
pub mod stats;

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use spin::Mutex;

use buddy::BuddyAllocator;
use slab::SlabSet;

pub use stats::AllocStats;

// When the `stats` feature is off, every record_* call compiles to nothing.
#[cfg(feature = "stats")]
macro_rules! stat {
    ($self:ident . $method:ident ( $($arg:tt)* )) => {
        $self.stats.$method($($arg)*)
    };
}
#[cfg(not(feature = "stats"))]
macro_rules! stat {
    ($self:ident . $method:ident ( $($arg:tt)* )) => {
        let _ = &$self.stats;
    };
}

/// Threshold above which allocations skip slab and go straight to buddy.
pub const SLAB_THRESHOLD: usize = 1024;

pub struct Heap {
    inner: Mutex<HeapInner>,
    stats: AllocStats,
}

struct HeapInner {
    buddy: BuddyAllocator,
    slabs: SlabSet,
}

impl Heap {
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(HeapInner {
                buddy: BuddyAllocator::new(),
                slabs: SlabSet::new(),
            }),
            stats: AllocStats::new(),
        }
    }

    /// Add a region of physical memory to the heap.
    ///
    /// SAFETY: The caller must guarantee `[start, start+size)` is unique,
    /// writable memory not aliased anywhere else for the heap's lifetime.
    pub unsafe fn add_region(&self, start: usize, size: usize) {
        let mut g = self.inner.lock();
        unsafe { g.buddy.add_region(start, start + size) };
    }

    pub fn stats(&self) -> &AllocStats {
        &self.stats
    }

    /// Snapshot of fragmentation info, useful for tests/benches.
    pub fn snapshot(&self) -> HeapSnapshot {
        let g = self.inner.lock();
        let mut slab_free = 0usize;
        let mut slab_provisioned = 0usize;
        for i in 0..slab::CLASS_SIZES.len() {
            let c = g.slabs.cache(i);
            slab_free += c.free_count() * c.obj_size();
            slab_provisioned += c.provisioned() * c.obj_size();
        }
        HeapSnapshot {
            buddy_total: g.buddy.total(),
            buddy_used: g.buddy.used(),
            buddy_waste: g.buddy.waste(),
            slab_provisioned,
            slab_free,
        }
    }

    /// Allocate `layout`. Returns `None` on OOM.
    pub fn alloc(&self, layout: Layout) -> Option<ptr::NonNull<u8>> {
        let size = layout.size().max(1);
        let align = layout.align().max(1);

        let mut g = self.inner.lock();
        let result = if size <= SLAB_THRESHOLD {
            if let Some(idx) = SlabSet::class_for(size, align) {
                stat!(self.record_slab(idx));
                let HeapInner { buddy, slabs } = &mut *g;
                slabs.alloc(idx, buddy)
            } else {
                stat!(self.record_buddy());
                g.buddy.alloc(size, align)
            }
        } else {
            stat!(self.record_buddy());
            g.buddy.alloc(size, align)
        };

        if result.is_some() {
            stat!(self.record_alloc(size));
        } else {
            stat!(self.record_oom());
        }
        result
    }

    /// SAFETY: `ptr/layout` must come from a previous `alloc` on this heap.
    pub unsafe fn dealloc(&self, ptr: ptr::NonNull<u8>, layout: Layout) {
        let size = layout.size().max(1);
        let align = layout.align().max(1);
        let mut g = self.inner.lock();
        if size <= SLAB_THRESHOLD {
            if let Some(idx) = SlabSet::class_for(size, align) {
                unsafe { g.slabs.dealloc(idx, ptr) };
            } else {
                unsafe { g.buddy.dealloc(ptr, size, align) };
            }
        } else {
            unsafe { g.buddy.dealloc(ptr, size, align) };
        }
        stat!(self.record_free(size));
    }
}

/// Lightweight snapshot of internal counters, safe to log.
#[derive(Debug, Clone, Copy)]
pub struct HeapSnapshot {
    pub buddy_total: usize,
    pub buddy_used: usize,
    pub buddy_waste: usize,
    pub slab_provisioned: usize,
    pub slab_free: usize,
}

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Heap::alloc(self, layout)
            .map(|p| p.as_ptr())
            .unwrap_or(ptr::null_mut())
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(nn) = ptr::NonNull::new(ptr) {
            unsafe { Heap::dealloc(self, nn, layout) };
        }
    }
}

#[cfg(test)]
mod tests;
