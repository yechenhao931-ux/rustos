//! Reproducible host-side benchmark for `kalloc` vs baseline allocators.
//!
//! We pin the workload (allocation size mix and free order) via a seeded
//! PRNG so all three allocators see the *exact same* sequence. Numbers
//! are reported as ns/op, throughput, and slab hit-rate.

use std::alloc::Layout;
use std::time::Instant;

use buddy_system_allocator::LockedHeap as BuddyHeap;
use kalloc::Heap as KHeap;
use linked_list_allocator::Heap as LLHeap;
use spin::Mutex;

const HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MiB

/// A workload is a stream of ops plus the number of slots needed.
struct Workload {
    ops: Vec<(Layout, Op)>,
    slots: usize,
}

/// Bursts of K allocations followed by K LIFO frees, repeated until ~`n` ops.
fn workload_small_lifo(n: usize, seed: u64) -> Workload {
    const BURST: usize = 256;
    let mut rng = fastrand::Rng::with_seed(seed);
    let sizes = [8usize, 16, 24, 32, 48, 64, 96, 128, 200, 256, 384, 512, 768, 1024];
    let mut ops = Vec::with_capacity(2 * n);
    let cycles = n / BURST;
    for c in 0..cycles {
        for i in 0..BURST {
            let s = sizes[rng.usize(..sizes.len())];
            let l = Layout::from_size_align(s, 8).unwrap();
            let slot = c * BURST + i;
            ops.push((l, Op::Alloc { slot }));
        }
        // LIFO free: pop the most recent allocation in this burst first.
        for i in (0..BURST).rev() {
            let s = sizes[0]; // size doesn't matter; layout is read from prior op
            let l = Layout::from_size_align(s, 8).unwrap();
            let slot = c * BURST + i;
            // Re-look-up the actual layout used for this slot by scanning
            // backwards in `ops`. The Layout in the Free op must equal the
            // Layout used at Alloc, otherwise the allocator gets a stale
            // size — this is what bit us on the first attempt.
            let real_layout = ops
                .iter()
                .rev()
                .find_map(|(l, op)| matches!(op, Op::Alloc { slot: s2 } if *s2 == slot).then(|| *l))
                .unwrap_or(l);
            ops.push((real_layout, Op::Free { slot }));
        }
    }
    Workload {
        ops,
        slots: cycles * BURST,
    }
}

/// Random alloc/free with realistic kernel-ish size mix:
/// 70% small (slab range), 25% mid (1-8 KiB), 5% large (8-64 KiB).
fn workload_mixed_random(n: usize, seed: u64) -> Workload {
    let mut rng = fastrand::Rng::with_seed(seed);
    let mut live: Vec<usize> = Vec::new();
    let mut ops: Vec<(Layout, Op)> = Vec::with_capacity(2 * n);

    let mut next_slot = 0usize;
    for _ in 0..n {
        let do_alloc = live.is_empty() || rng.f32() < 0.55;
        if do_alloc {
            let r = rng.f32();
            let size = if r < 0.70 {
                1 + rng.usize(..1024)
            } else if r < 0.95 {
                1024 + rng.usize(..(8 * 1024))
            } else {
                8 * 1024 + rng.usize(..(56 * 1024))
            };
            let l = Layout::from_size_align(size, 8).unwrap();
            let slot = next_slot;
            next_slot += 1;
            ops.push((l, Op::Alloc { slot }));
            live.push(slot);
        } else {
            let pick = rng.usize(..live.len());
            let slot = live.swap_remove(pick);
            // Find the original layout for this slot.
            let l = ops
                .iter()
                .rev()
                .find_map(|(l, op)| matches!(op, Op::Alloc { slot: s2 } if *s2 == slot).then(|| *l))
                .expect("alloc not found for slot");
            ops.push((l, Op::Free { slot }));
        }
    }
    while let Some(slot) = live.pop() {
        let l = ops
            .iter()
            .rev()
            .find_map(|(l, op)| matches!(op, Op::Alloc { slot: s2 } if *s2 == slot).then(|| *l))
            .unwrap();
        ops.push((l, Op::Free { slot }));
    }
    Workload {
        ops,
        slots: next_slot,
    }
}

/// Mimic many short-lived `Vec<u8>` growths (16, 32, ..., 4096, free).
fn workload_growth(_seed: u64) -> Workload {
    let mut ops = Vec::new();
    let mut next_slot = 0usize;
    for _ in 0..2000 {
        let mut s = 16;
        let mut these = Vec::new();
        while s <= 4096 {
            let l = Layout::from_size_align(s, 8).unwrap();
            ops.push((l, Op::Alloc { slot: next_slot }));
            these.push((l, next_slot));
            next_slot += 1;
            s *= 2;
        }
        for (l, slot) in these.into_iter().rev() {
            ops.push((l, Op::Free { slot }));
        }
    }
    Workload {
        ops,
        slots: next_slot,
    }
}

#[derive(Copy, Clone)]
enum Op {
    /// Allocate and store the resulting pointer at `slot`.
    Alloc { slot: usize },
    /// Free the pointer previously stored at `slot`.
    Free { slot: usize },
}

/// Minimal trait so all three allocators look the same to the bench loop.
trait Bench {
    fn name(&self) -> &'static str;
    unsafe fn alloc(&mut self, l: Layout) -> *mut u8;
    unsafe fn dealloc(&mut self, p: *mut u8, l: Layout);
}

struct KAlloc {
    heap: KHeap,
    _backing: Box<[u8]>,
}
impl KAlloc {
    fn new() -> Self {
        let mut backing: Box<[u8]> = vec![0u8; HEAP_SIZE].into_boxed_slice();
        let heap = KHeap::new();
        unsafe { heap.add_region(backing.as_mut_ptr() as usize, HEAP_SIZE) };
        Self {
            heap,
            _backing: backing,
        }
    }
}
impl Bench for KAlloc {
    fn name(&self) -> &'static str {
        "kalloc"
    }
    unsafe fn alloc(&mut self, l: Layout) -> *mut u8 {
        self.heap.alloc(l).map(|p| p.as_ptr()).unwrap_or(std::ptr::null_mut())
    }
    unsafe fn dealloc(&mut self, p: *mut u8, l: Layout) {
        if let Some(nn) = std::ptr::NonNull::new(p) {
            unsafe { self.heap.dealloc(nn, l) };
        }
    }
}

struct Buddy {
    heap: BuddyHeap,
    _backing: Box<[u8]>,
}
impl Buddy {
    fn new() -> Self {
        let mut backing: Box<[u8]> = vec![0u8; HEAP_SIZE].into_boxed_slice();
        let heap: BuddyHeap = BuddyHeap::new();
        unsafe { heap.lock().init(backing.as_mut_ptr() as usize, HEAP_SIZE) };
        Self {
            heap,
            _backing: backing,
        }
    }
}
impl Bench for Buddy {
    fn name(&self) -> &'static str {
        "buddy_system (locked)"
    }
    unsafe fn alloc(&mut self, l: Layout) -> *mut u8 {
        self.heap
            .lock()
            .alloc(l)
            .map(|p| p.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }
    unsafe fn dealloc(&mut self, p: *mut u8, l: Layout) {
        if let Some(nn) = std::ptr::NonNull::new(p) {
            self.heap.lock().dealloc(nn, l);
        }
    }
}

struct LinkedList {
    heap: Mutex<LLHeap>,
    _backing: Box<[u8]>,
}
impl LinkedList {
    fn new() -> Self {
        let mut backing: Box<[u8]> = vec![0u8; HEAP_SIZE].into_boxed_slice();
        let heap = unsafe { LLHeap::new(backing.as_mut_ptr(), HEAP_SIZE) };
        Self {
            heap: Mutex::new(heap),
            _backing: backing,
        }
    }
}
impl Bench for LinkedList {
    fn name(&self) -> &'static str {
        "linked_list (locked)"
    }
    unsafe fn alloc(&mut self, l: Layout) -> *mut u8 {
        self.heap
            .lock()
            .allocate_first_fit(l)
            .map(|p| p.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }
    unsafe fn dealloc(&mut self, p: *mut u8, l: Layout) {
        if let Some(nn) = std::ptr::NonNull::new(p) {
            self.heap.lock().deallocate(nn, l);
        }
    }
}

fn run<B: Bench>(b: &mut B, w: &Workload) -> (u64, usize) {
    let mut slots: Vec<*mut u8> = vec![std::ptr::null_mut(); w.slots];
    let mut allocs = 0usize;
    let start = Instant::now();
    for (l, op) in &w.ops {
        match op {
            Op::Alloc { slot } => {
                let p = unsafe { b.alloc(*l) };
                assert!(!p.is_null(), "OOM in {} (size={})", b.name(), l.size());
                slots[*slot] = p;
                allocs += 1;
            }
            Op::Free { slot } => {
                let p = slots[*slot];
                unsafe { b.dealloc(p, *l) };
                slots[*slot] = std::ptr::null_mut();
            }
        }
    }
    let ns = start.elapsed().as_nanos() as u64;
    (ns, allocs)
}

fn header() {
    println!(
        "{:<24} {:<22} {:>12} {:>12} {:>12} {:>12}",
        "workload", "allocator", "ops", "total ms", "ns/op", "Mops/s"
    );
    println!("{}", "-".repeat(96));
}

fn report(workload: &str, name: &str, ns: u64, ops: usize) {
    let total_ms = ns as f64 / 1.0e6;
    let nsop = ns as f64 / ops as f64;
    let mops = ops as f64 / (ns as f64 / 1.0e9) / 1.0e6;
    println!(
        "{:<24} {:<22} {:>12} {:>12.2} {:>12.1} {:>12.2}",
        workload, name, ops, total_ms, nsop, mops
    );
}

fn run_all(workload: &str, w: &Workload) {
    let mut k = KAlloc::new();
    let (ns, n) = run(&mut k, w);
    report(workload, k.name(), ns, n);
    let s = k.heap.snapshot();
    let st = k.heap.stats();
    println!(
        "  -> kalloc snapshot: buddy_used={}KB buddy_total={}KB slab_provisioned={}KB \
         slab_free={}KB slab_hit_rate={:.1}% buddy_calls={} oom={}",
        s.buddy_used / 1024,
        s.buddy_total / 1024,
        s.slab_provisioned / 1024,
        s.slab_free / 1024,
        st.slab_hit_rate_pct(),
        st.buddy_calls(),
        st.oom(),
    );

    let mut b = Buddy::new();
    let (ns, n) = run(&mut b, w);
    report(workload, b.name(), ns, n);

    let mut l = LinkedList::new();
    let (ns, n) = run(&mut l, w);
    report(workload, l.name(), ns, n);
    println!();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args
        .iter()
        .skip(1)
        .find_map(|s| s.parse::<usize>().ok())
        .unwrap_or(50_000);
    let only: Option<&str> = args
        .iter()
        .skip(1)
        .find(|s| !s.chars().all(|c| c.is_ascii_digit()))
        .map(|s| s.as_str());

    println!("kalloc benchmarks  heap=16MiB  N={}\n", n);
    header();

    let want = |name: &str| only.map(|o| o == name).unwrap_or(true);

    if want("small-lifo") {
        run_all("small-lifo", &workload_small_lifo(n, 0xC0FFEE));
    }
    if want("mixed-random") {
        // linked_list first-fit degrades to O(N) per op as fragmentation
        // grows, so we cap N here.
        let nm = n.min(10_000);
        run_all("mixed-random", &workload_mixed_random(nm, 0xDEADBEEF));
    }
    if want("vec-growth") {
        let g = workload_growth(0);
        println!("(growth uses fixed 2000 cycles; {} ops)", g.ops.len());
        run_all("vec-growth", &g);
    }
}
