//! Intrusive single-linked free list.
//!
//! Each free block stores the pointer to the next free block in its first
//! `usize` bytes. This is the classic "free-list-in-place" trick used by
//! Linux SLUB, jemalloc tcache, and most kernel slab allocators — it costs
//! zero metadata bytes per free object.
//!
//! # Safety
//!
//! Callers must ensure that:
//!   * Every pushed pointer is properly aligned to `align_of::<usize>()`.
//!   * The block has at least `size_of::<usize>()` bytes available.
//!   * A block is never pushed twice without an intervening `pop`.

use core::ptr;

pub struct FreeList {
    head: *mut usize,
    len: usize,
}

impl Default for FreeList {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for FreeList {}

impl FreeList {
    pub const fn new() -> Self {
        Self {
            head: ptr::null_mut(),
            len: 0,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// SAFETY: `block` must point to at least one `usize` of writable memory
    /// owned exclusively by this list until `pop`'d again.
    #[inline]
    pub unsafe fn push(&mut self, block: *mut u8) {
        let cell = block as *mut usize;
        unsafe { ptr::write(cell, self.head as usize) };
        self.head = cell;
        self.len += 1;
    }

    #[inline]
    pub fn pop(&mut self) -> Option<*mut u8> {
        if self.head.is_null() {
            return None;
        }
        let block = self.head;
        let next = unsafe { ptr::read(block) } as *mut usize;
        self.head = next;
        self.len -= 1;
        Some(block as *mut u8)
    }

    /// Iterate without modifying. Used only for tests and debug printing.
    #[cfg(test)]
    pub fn iter(&self) -> FreeListIter {
        FreeListIter { cur: self.head }
    }

    // Accessors used by the buddy coalescing path. Kept crate-private so the
    // intrusive layout never leaks into public callers.
    #[inline]
    pub(crate) fn head_ptr(&self) -> *mut usize {
        self.head
    }
    #[inline]
    pub(crate) fn set_head(&mut self, n: *mut usize) {
        self.head = n;
    }
    #[inline]
    pub(crate) fn dec_len(&mut self) {
        self.len -= 1;
    }
}

#[cfg(test)]
pub struct FreeListIter {
    cur: *mut usize,
}

#[cfg(test)]
impl Iterator for FreeListIter {
    type Item = *mut u8;
    fn next(&mut self) -> Option<*mut u8> {
        if self.cur.is_null() {
            return None;
        }
        let cur = self.cur;
        self.cur = unsafe { ptr::read(cur) } as *mut usize;
        Some(cur as *mut u8)
    }
}
