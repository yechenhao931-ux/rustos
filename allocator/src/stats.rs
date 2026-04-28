//! Atomic, lock-free per-allocator statistics.

use core::sync::atomic::{AtomicU64, Ordering};

pub struct AllocStats {
    allocs: AtomicU64,
    frees: AtomicU64,
    bytes_allocated: AtomicU64,
    bytes_freed: AtomicU64,
    slab_hits: AtomicU64,
    buddy_calls: AtomicU64,
    oom: AtomicU64,
    /// Per-class slab hits for the 8 classes (8..1024).
    pub class_hits: [AtomicU64; 8],
}

impl AllocStats {
    pub const fn new() -> Self {
        const Z: AtomicU64 = AtomicU64::new(0);
        Self {
            allocs: Z,
            frees: Z,
            bytes_allocated: Z,
            bytes_freed: Z,
            slab_hits: Z,
            buddy_calls: Z,
            oom: Z,
            class_hits: [Z, Z, Z, Z, Z, Z, Z, Z],
        }
    }

    #[inline]
    pub fn record_alloc(&self, bytes: usize) {
        self.allocs.fetch_add(1, Ordering::Relaxed);
        self.bytes_allocated
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }
    #[inline]
    pub fn record_free(&self, bytes: usize) {
        self.frees.fetch_add(1, Ordering::Relaxed);
        self.bytes_freed.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    #[inline]
    pub fn record_slab(&self, idx: usize) {
        self.slab_hits.fetch_add(1, Ordering::Relaxed);
        self.class_hits[idx].fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn record_buddy(&self) {
        self.buddy_calls.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn record_oom(&self) {
        self.oom.fetch_add(1, Ordering::Relaxed);
    }

    pub fn allocs(&self) -> u64 {
        self.allocs.load(Ordering::Relaxed)
    }
    pub fn frees(&self) -> u64 {
        self.frees.load(Ordering::Relaxed)
    }
    pub fn bytes_allocated(&self) -> u64 {
        self.bytes_allocated.load(Ordering::Relaxed)
    }
    pub fn bytes_freed(&self) -> u64 {
        self.bytes_freed.load(Ordering::Relaxed)
    }
    pub fn slab_hits(&self) -> u64 {
        self.slab_hits.load(Ordering::Relaxed)
    }
    pub fn buddy_calls(&self) -> u64 {
        self.buddy_calls.load(Ordering::Relaxed)
    }
    pub fn oom(&self) -> u64 {
        self.oom.load(Ordering::Relaxed)
    }

    /// `slab_hits / (slab_hits + buddy_calls)` as percent.
    pub fn slab_hit_rate_pct(&self) -> f64 {
        let s = self.slab_hits() as f64;
        let b = self.buddy_calls() as f64;
        let total = s + b;
        if total == 0.0 {
            0.0
        } else {
            100.0 * s / total
        }
    }
}
