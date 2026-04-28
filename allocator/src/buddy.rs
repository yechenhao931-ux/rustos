//! Buddy allocator.
//!
//! The classic Knuth scheme: memory is split into power-of-two blocks; each
//! order keeps a free list; on alloc we split a larger block when needed,
//! on free we coalesce with the buddy if it is also free.
//!
//! Compared to a flat free list this gives:
//!   * O(log MAX_ORDER) alloc/free in the worst case.
//!   * Bounded external fragmentation (every block is a 2^k aligned region).
//!
//! Limitations of *pure* buddy:
//!   * Allocating 33 bytes wastes 31 bytes (rounds up to 64). The slab layer
//!     on top of this addresses small-object internal fragmentation.

use core::cmp::max;
use core::mem::size_of;
use core::ptr::NonNull;

use crate::linked_list::FreeList;

/// Smallest block this allocator hands out.
pub const MIN_ORDER: usize = 6; // 64 B
/// Largest block. `1 << 31` = 2 GiB which covers any plausible kernel heap.
pub const MAX_ORDER: usize = 32;

pub struct BuddyAllocator {
    free_lists: [FreeList; MAX_ORDER],
    /// Total managed bytes (sum of every region added).
    total: usize,
    /// Currently-in-use bytes (sum of granted block sizes).
    used: usize,
    /// Bytes lost to alignment when adding regions.
    waste: usize,
}

impl BuddyAllocator {
    pub const fn new() -> Self {
        const EMPTY: FreeList = FreeList::new();
        Self {
            free_lists: [EMPTY; MAX_ORDER],
            total: 0,
            used: 0,
            waste: 0,
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }
    pub fn used(&self) -> usize {
        self.used
    }
    pub fn waste(&self) -> usize {
        self.waste
    }

    /// Add a memory region `[start, end)` to be managed.
    ///
    /// SAFETY: The region must be a unique, valid, writable mapping for the
    /// lifetime of this allocator. Overlapping regions are a bug.
    pub unsafe fn add_region(&mut self, mut start: usize, end: usize) {
        // Align start up to MIN_ORDER granularity.
        let min = 1 << MIN_ORDER;
        let aligned = (start + min - 1) & !(min - 1);
        self.waste += aligned - start;
        start = aligned;

        while start + min <= end {
            // Pick the largest power-of-two block that:
            //   (a) is aligned to its own size starting at `start`, and
            //   (b) fits within [start, end).
            let low_bits = start.trailing_zeros() as usize;
            let span = end - start;
            let span_order = (usize::BITS - 1 - span.leading_zeros()) as usize;
            let order = max(MIN_ORDER, low_bits.min(span_order));
            if order >= MAX_ORDER {
                break;
            }
            let size = 1usize << order;
            unsafe { self.free_lists[order].push(start as *mut u8) };
            self.total += size;
            start += size;
        }
        self.waste += end - start;
    }

    /// Round `n` up to the next power of two and return its order.
    /// Returns at least `MIN_ORDER`.
    #[inline]
    fn order_for(size: usize, align: usize) -> usize {
        let need = max(size.max(size_of::<usize>()), align);
        let order = (need - 1).max(1).ilog2() as usize + 1;
        order.max(MIN_ORDER)
    }

    /// Allocate a block large enough to satisfy `size`/`align`.
    pub fn alloc(&mut self, size: usize, align: usize) -> Option<NonNull<u8>> {
        let order = Self::order_for(size, align);
        if order >= MAX_ORDER {
            return None;
        }

        // Find the smallest order that has a free block.
        let mut found = None;
        for o in order..MAX_ORDER {
            if !self.free_lists[o].is_empty() {
                found = Some(o);
                break;
            }
        }
        let mut have = found?;

        // Pop one block and split down to `order`.
        let block = self.free_lists[have].pop()? as usize;
        while have > order {
            have -= 1;
            let buddy = block + (1 << have);
            unsafe { self.free_lists[have].push(buddy as *mut u8) };
        }

        self.used += 1 << order;
        NonNull::new(block as *mut u8)
    }

    /// Free a block previously returned by `alloc`.
    ///
    /// SAFETY: `ptr/size/align` must match a prior `alloc`.
    pub unsafe fn dealloc(&mut self, ptr: NonNull<u8>, size: usize, align: usize) {
        let order = Self::order_for(size, align);
        let mut addr = ptr.as_ptr() as usize;
        let mut o = order;
        self.used -= 1 << order;

        // Coalesce upwards while the buddy is free.
        while o + 1 < MAX_ORDER {
            let buddy = addr ^ (1 << o);
            // Walk the free list at order `o` looking for `buddy`. This is
            // O(N) per coalesce; for typical free-list lengths (< 64 in
            // steady state) it is faster than maintaining a bitmap because
            // we touch fewer cache lines.
            let mut found = false;
            // SAFETY: we hold &mut self and the free list is private.
            unsafe {
                let list = &mut self.free_lists[o];
                let mut prev: *mut usize = core::ptr::null_mut();
                let mut node = list.head_ptr();
                while !node.is_null() {
                    if node as usize == buddy {
                        let next = core::ptr::read(node) as *mut usize;
                        if prev.is_null() {
                            list.set_head(next);
                        } else {
                            core::ptr::write(prev, next as usize);
                        }
                        list.dec_len();
                        found = true;
                        break;
                    }
                    prev = node;
                    node = core::ptr::read(node) as *mut usize;
                }
            }
            if !found {
                break;
            }
            addr &= !(1 << o);
            o += 1;
        }
        unsafe { self.free_lists[o].push(addr as *mut u8) };
    }
}

