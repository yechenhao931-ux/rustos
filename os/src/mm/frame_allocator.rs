use super::{PhysAddr, PhysPageNum};
use crate::config::MEMORY_END;
use crate::sync::UPIntrFreeCell;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};
use lazy_static::*;

pub struct FrameTracker {
    pub ppn: PhysPageNum,
}

impl FrameTracker {
    pub fn new(ppn: PhysPageNum) -> Self {
        // page cleaning
        let bytes_array = ppn.get_bytes_array();
        for i in bytes_array {
            *i = 0;
        }
        Self { ppn }
    }
}

impl Debug for FrameTracker {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("FrameTracker:PPN={:#x}", self.ppn.0))
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        frame_dealloc(self.ppn);
    }
}

trait FrameAllocator {
    fn new() -> Self;
    fn alloc(&mut self) -> Option<PhysPageNum>;
    fn alloc_more(&mut self, pages: usize) -> Option<Vec<PhysPageNum>>;
    fn dealloc(&mut self, ppn: PhysPageNum);
}

// =============================================================================
// Buddy-system frame allocator
// -----------------------------------------------------------------------------
// Replaces the original "stack" allocator (a bump pointer + recycled stack).
// Benefits over the old design:
//   * O(log N) physically-contiguous multi-page allocation. The old
//     `alloc_more` could only hand out the very next N pages from the bump
//     pointer and never looked at the recycled stack, so once any frame had
//     been freed the contiguous-multi-page path could fragment trivially.
//   * Released single pages can coalesce with their buddies, so subsequent
//     contiguous allocations succeed even after heavy churn.
//   * No O(n) duplicate scan in `dealloc` (the old code did
//     `recycled.iter().any(...)`).
//
// MAX_ORDER = 32 is enough for any practical physical-memory size (up to
// 2^32 * 4 KiB = 16 TiB).
// =============================================================================

const MAX_ORDER: usize = 32;

pub struct BuddyFrameAllocator {
    /// `free_lists[k]` holds starting PPNs of free blocks of size 2^k pages.
    /// Every block in `free_lists[k]` is aligned to 2^k pages, which makes
    /// the buddy XOR (`ppn ^ (1 << k)`) identify the matching half exactly.
    free_lists: Vec<BTreeSet<usize>>,
    base: usize,
    end: usize,
}

impl BuddyFrameAllocator {
    pub fn init(&mut self, l: PhysPageNum, r: PhysPageNum) {
        self.base = l.0;
        self.end = r.0;
        self.add_block(l.0, r.0 - l.0);
    }

    /// Greedily decompose [start, start+len) into power-of-two-aligned blocks
    /// and seed the free lists. Each emitted block is the largest one that
    /// is both alignment-fit and length-fit at the current cursor.
    fn add_block(&mut self, start: usize, len: usize) {
        let mut s = start;
        let e = start + len;
        while s < e {
            let align_order = if s == 0 {
                MAX_ORDER
            } else {
                s.trailing_zeros() as usize
            };
            let max_size = e - s;
            // floor(log2(max_size))
            let size_order = (usize::BITS - 1 - max_size.leading_zeros()) as usize;
            let k = align_order.min(size_order).min(MAX_ORDER);
            self.free_lists[k].insert(s);
            s += 1usize << k;
        }
    }

    /// Allocate one block of size 2^order pages, splitting larger free blocks
    /// as needed. Returns the starting PPN.
    fn alloc_order(&mut self, order: usize) -> Option<usize> {
        let mut j = order;
        while j <= MAX_ORDER && self.free_lists[j].is_empty() {
            j += 1;
        }
        if j > MAX_ORDER {
            return None;
        }
        let block = *self.free_lists[j].iter().next().unwrap();
        self.free_lists[j].remove(&block);
        // Split the larger block down to the requested order. Each step yields
        // a buddy of size 2^j that we put back on the corresponding free list.
        while j > order {
            j -= 1;
            self.free_lists[j].insert(block + (1usize << j));
        }
        Some(block)
    }

    /// Deallocate the block at `ppn` of size 2^order pages, coalescing with
    /// its buddy whenever the buddy is also free.
    fn dealloc_order(&mut self, ppn: usize, order: usize) {
        let mut p = ppn;
        let mut k = order;
        while k < MAX_ORDER {
            let buddy = p ^ (1usize << k);
            if self.free_lists[k].remove(&buddy) {
                p = p.min(buddy);
                k += 1;
            } else {
                break;
            }
        }
        self.free_lists[k].insert(p);
    }
}

impl FrameAllocator for BuddyFrameAllocator {
    fn new() -> Self {
        let mut v = Vec::with_capacity(MAX_ORDER + 1);
        for _ in 0..=MAX_ORDER {
            v.push(BTreeSet::new());
        }
        Self {
            free_lists: v,
            base: 0,
            end: 0,
        }
    }

    fn alloc(&mut self) -> Option<PhysPageNum> {
        self.alloc_order(0).map(PhysPageNum::from)
    }

    /// Hand out `pages` physically-contiguous frames. We allocate the
    /// smallest enclosing power-of-two block, return the first `pages`
    /// frames as individual order-0 trackers, and release the unused tail
    /// back to the buddy free lists. Each returned frame is freed
    /// independently via `FrameTracker::drop`, and the buddy coalescer
    /// rebuilds the larger block once all of them are released.
    ///
    /// The returned `Vec` is in **decreasing** PPN order to match the
    /// original `StackFrameAllocator` contract: `vec.last()` therefore
    /// gives the lowest PPN, i.e. the *base* of the contiguous range.
    /// `drivers/bus/virtio.rs::dma_alloc` relies on this.
    fn alloc_more(&mut self, pages: usize) -> Option<Vec<PhysPageNum>> {
        if pages == 0 {
            return Some(Vec::new());
        }
        let order = ceil_log2(pages);
        let start = self.alloc_order(order)?;
        let block_end = start + (1usize << order);
        let used_end = start + pages;
        if used_end < block_end {
            self.add_block(used_end, block_end - used_end);
        }
        Some(
            (start..used_end)
                .rev()
                .map(PhysPageNum::from)
                .collect(),
        )
    }

    fn dealloc(&mut self, ppn: PhysPageNum) {
        let p = ppn.0;
        if p < self.base || p >= self.end {
            panic!("Frame ppn={:#x} out of allocator range", p);
        }
        self.dealloc_order(p, 0);
    }
}

fn ceil_log2(n: usize) -> usize {
    if n <= 1 {
        0
    } else {
        (usize::BITS - (n - 1).leading_zeros()) as usize
    }
}

type FrameAllocatorImpl = BuddyFrameAllocator;

lazy_static! {
    pub static ref FRAME_ALLOCATOR: UPIntrFreeCell<FrameAllocatorImpl> =
        unsafe { UPIntrFreeCell::new(FrameAllocatorImpl::new()) };
}

pub fn init_frame_allocator() {
    unsafe extern "C" {
        safe fn ekernel();
    }
    FRAME_ALLOCATOR.exclusive_access().init(
        PhysAddr::from(ekernel as usize).ceil(),
        PhysAddr::from(MEMORY_END).floor(),
    );
}

pub fn frame_alloc() -> Option<FrameTracker> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc()
        .map(FrameTracker::new)
}

pub fn frame_alloc_more(num: usize) -> Option<Vec<FrameTracker>> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc_more(num)
        .map(|x| x.iter().map(|&t| FrameTracker::new(t)).collect())
}

pub fn frame_dealloc(ppn: PhysPageNum) {
    FRAME_ALLOCATOR.exclusive_access().dealloc(ppn);
}

#[allow(unused)]
pub fn frame_allocator_test() {
    let mut v: Vec<FrameTracker> = Vec::new();
    for _ in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:?}", frame);
        v.push(frame);
    }
    v.clear();
    for _ in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:?}", frame);
        v.push(frame);
    }
    drop(v);
    println!("frame_allocator_test passed!");
}

#[allow(unused)]
pub fn frame_allocator_alloc_more_test() {
    let mut v: Vec<FrameTracker> = Vec::new();
    let frames = frame_alloc_more(5).unwrap();
    for frame in &frames {
        println!("{:?}", frame);
    }
    v.extend(frames);
    v.clear();
    let frames = frame_alloc_more(5).unwrap();
    for frame in &frames {
        println!("{:?}", frame);
    }
    drop(v);
    println!("frame_allocator_test passed!");
}

// =============================================================================
// In-kernel benchmark for the frame allocator.
//
// Userland can't time `frame_alloc` directly (no syscall) and `mmap_bench`
// measures the *whole* page-fault path. This bench isolates the allocator:
// it leases an arena of ARENA contiguous pages from the live buddy
// allocator, then runs identical workloads against
//   (a) a fresh BuddyFrameAllocator initialized over the arena,
//   (b) a fresh LegacyStackFrameAllocator (the original rCore allocator,
//        revived here only for comparison).
// Both run inside the kernel and print microseconds via timer::get_time_us.
//
// The arena is freed when the bench returns (the FrameTrackers drop), so
// this is non-destructive and can be invoked repeatedly.
// =============================================================================

#[allow(dead_code)]
struct LegacyStackFrameAllocator {
    current: usize,
    end: usize,
    recycled: Vec<usize>,
}

#[allow(dead_code)]
impl LegacyStackFrameAllocator {
    fn new(l: usize, r: usize) -> Self {
        Self { current: l, end: r, recycled: Vec::new() }
    }
    fn alloc(&mut self) -> Option<PhysPageNum> {
        if let Some(ppn) = self.recycled.pop() {
            Some(ppn.into())
        } else if self.current == self.end {
            None
        } else {
            self.current += 1;
            Some((self.current - 1).into())
        }
    }
    fn alloc_more(&mut self, pages: usize) -> Option<Vec<PhysPageNum>> {
        // Original rCore behavior: only the bump-pointer region is consulted;
        // recycled pages are never coalesced into multi-page allocations.
        if self.current + pages >= self.end {
            None
        } else {
            self.current += pages;
            let arr: Vec<usize> = (1..pages + 1).collect();
            let v = arr.iter().map(|x| (self.current - x).into()).collect();
            Some(v)
        }
    }
    fn dealloc(&mut self, ppn: PhysPageNum) {
        self.recycled.push(ppn.0);
    }
}

pub fn run_buddy_bench() {
    use crate::timer::get_time_us;
    const ARENA: usize = 256;

    let arena = match frame_alloc_more(ARENA) {
        Some(v) => v,
        None => {
            println!("[buddy_bench] OOM acquiring {}-page arena", ARENA);
            return;
        }
    };
    // alloc_more returns ppns in decreasing order; the base is `last()`.
    let base = arena.last().unwrap().ppn.0;
    let end = base + ARENA;
    println!(
        "[buddy_bench] arena: base_ppn={:#x} end_ppn={:#x} pages={}",
        base, end, ARENA
    );

    // ---- Workload A: alloc/dealloc throughput on order-0 pages ----
    let (b_alloc_us, b_dealloc_us) = {
        let mut a = BuddyFrameAllocator::new();
        a.init(PhysPageNum::from(base), PhysPageNum::from(end));
        let mut v: Vec<PhysPageNum> = Vec::with_capacity(ARENA);
        let t0 = get_time_us();
        for _ in 0..ARENA {
            v.push(a.alloc().expect("buddy: arena exhausted"));
        }
        let t1 = get_time_us();
        for p in v.drain(..) {
            a.dealloc(p);
        }
        let t2 = get_time_us();
        (t1 - t0, t2 - t1)
    };
    let (s_alloc_us, s_dealloc_us) = {
        let mut a = LegacyStackFrameAllocator::new(base, end);
        let mut v: Vec<PhysPageNum> = Vec::with_capacity(ARENA);
        let t0 = get_time_us();
        for _ in 0..ARENA {
            v.push(a.alloc().expect("stack: arena exhausted"));
        }
        let t1 = get_time_us();
        for p in v.drain(..) {
            a.dealloc(p);
        }
        let t2 = get_time_us();
        (t1 - t0, t2 - t1)
    };
    println!(
        "[buddy_bench] === Workload A: alloc + dealloc {} order-0 pages ===",
        ARENA
    );
    println!(
        "[buddy_bench]   buddy : alloc {} us, dealloc {} us",
        b_alloc_us, b_dealloc_us
    );
    println!(
        "[buddy_bench]   stack : alloc {} us, dealloc {} us",
        s_alloc_us, s_dealloc_us
    );

    // ---- Workload B: contiguous-K alloc after full churn ----
    // Alloc the entire arena, free everything, then ask for a K-page
    // contiguous block. Buddy coalesces freed pages back into large blocks.
    // The legacy stack allocator never inspects its `recycled` stack from
    // alloc_more, so it fails as soon as the bump pointer is exhausted.
    println!("[buddy_bench] === Workload B: alloc_more(K) after full churn ===");
    let ks: [usize; 4] = [2, 4, 8, 32];
    for &k in &ks {
        let buddy_ok = {
            let mut a = BuddyFrameAllocator::new();
            a.init(PhysPageNum::from(base), PhysPageNum::from(end));
            let mut v: Vec<PhysPageNum> = (0..ARENA).map(|_| a.alloc().unwrap()).collect();
            for p in v.drain(..) {
                a.dealloc(p);
            }
            a.alloc_more(k).is_some()
        };
        let stack_ok = {
            let mut a = LegacyStackFrameAllocator::new(base, end);
            let mut v: Vec<PhysPageNum> = (0..ARENA).map(|_| a.alloc().unwrap()).collect();
            for p in v.drain(..) {
                a.dealloc(p);
            }
            a.alloc_more(k).is_some()
        };
        println!(
            "[buddy_bench]   alloc_more({:>2}): buddy={:<4} stack={:<4}",
            k,
            if buddy_ok { "OK" } else { "FAIL" },
            if stack_ok { "OK" } else { "FAIL" },
        );
    }

    // ---- Workload C: alloc/dealloc churn (mixed) ----
    // Repeated: alloc 64 pages, free a pseudo-random 32 of them, back-fill,
    // measuring time. Same workload on both allocators.
    println!("[buddy_bench] === Workload C: 1024-iter mixed churn (64-page working set) ===");
    const ITER: usize = 1024;
    let buddy_us = {
        let mut a = BuddyFrameAllocator::new();
        a.init(PhysPageNum::from(base), PhysPageNum::from(end));
        let mut held: Vec<PhysPageNum> = (0..64).map(|_| a.alloc().unwrap()).collect();
        let t0 = get_time_us();
        for i in 0..ITER {
            // pseudo-random index = (i * 1664525 + 1013904223) % 64
            let j = ((i.wrapping_mul(1664525).wrapping_add(1013904223)) % 64) as usize;
            let old = held[j];
            a.dealloc(old);
            held[j] = a.alloc().unwrap();
        }
        let t1 = get_time_us();
        for p in held.drain(..) {
            a.dealloc(p);
        }
        t1 - t0
    };
    let stack_us = {
        let mut a = LegacyStackFrameAllocator::new(base, end);
        let mut held: Vec<PhysPageNum> = (0..64).map(|_| a.alloc().unwrap()).collect();
        let t0 = get_time_us();
        for i in 0..ITER {
            let j = ((i.wrapping_mul(1664525).wrapping_add(1013904223)) % 64) as usize;
            let old = held[j];
            a.dealloc(old);
            held[j] = a.alloc().unwrap();
        }
        let t1 = get_time_us();
        for p in held.drain(..) {
            a.dealloc(p);
        }
        t1 - t0
    };
    let ops = (ITER * 2) as usize;
    println!(
        "[buddy_bench]   buddy : {} us total ({} ns/op avg over {} ops)",
        buddy_us,
        if buddy_us > 0 { (buddy_us * 1000) / ops } else { 0 },
        ops
    );
    println!(
        "[buddy_bench]   stack : {} us total ({} ns/op avg over {} ops)",
        stack_us,
        if stack_us > 0 { (stack_us * 1000) / ops } else { 0 },
        ops
    );

    // arena drops here, returning the test pages to the live allocator.
    drop(arena);
    println!("[buddy_bench] done");
}
