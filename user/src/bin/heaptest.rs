#![no_std]
#![no_main]

//! Kernel heap stress + benchmark program.
//!
//! Exercises the kernel allocator from userspace via two paths:
//!
//! 1. `sys_sbrk(+page)` — extends the user heap one page at a time. The
//!    kernel's response touches `frame_alloc` (one frame), the `MapArea`
//!    `BTreeMap<VirtPageNum, FrameTracker>` (one entry per page → repeated
//!    small kernel-heap allocations), and the page-table walk for
//!    `PageTable::map` (potentially allocating mid-level page-table
//!    frames). Each `sbrk` therefore generates a handful of kernel
//!    allocations of varied sizes — exactly the slab+buddy hotspot.
//!
//! 2. `sys_sbrk(-page)` — releases pages back. The matching `frame_dealloc`
//!    plus `BTreeMap::remove` exercises the kernel free path.
//!
//! After the timed phase we read the kernel heap counters via the new
//! `sys_heap_stats` syscall and report:
//!   * total allocs/frees the kernel did
//!   * how many of those hit the slab cache vs fell through to buddy
//!   * peak buddy usage (a proxy for kernel memory footprint)
//!   * average kernel-side latency per `sbrk` call
//!
//! Used as a regression harness for the kalloc replacement; see
//! `docs/allocator.md` for the host-side equivalent.

#[macro_use]
extern crate user_lib;

use user_lib::{KernelHeapStats, get_time, read_heap_stats, sbrk};

const PAGE_SIZE: usize = 4096;

fn time_ms() -> isize {
    get_time()
}

fn print_stats_diff(label: &str, before: &KernelHeapStats, after: &KernelHeapStats) {
    let allocs = after.allocs - before.allocs;
    let frees = after.frees - before.frees;
    let bytes_a = after.bytes_allocated - before.bytes_allocated;
    let bytes_f = after.bytes_freed - before.bytes_freed;
    let slab = after.slab_hits - before.slab_hits;
    let buddy = after.buddy_calls - before.buddy_calls;
    let total_routed = slab + buddy;
    let hit_pct = if total_routed > 0 {
        100u64 * slab / total_routed
    } else {
        0
    };
    let buddy_used_kb = after.buddy_used / 1024;
    let buddy_total_kb = after.buddy_total / 1024;
    let slab_prov_kb = after.slab_provisioned / 1024;

    println!(
        "  [{}] kernel allocs={}  frees={}  bytes_alloc={}  bytes_free={}",
        label, allocs, frees, bytes_a, bytes_f
    );
    println!(
        "  [{}] slab_hits={}  buddy_calls={}  hit_rate={}%  oom={}",
        label, slab, buddy, hit_pct, after.oom
    );
    println!(
        "  [{}] buddy_used={}KB / {}KB  slab_provisioned={}KB",
        label, buddy_used_kb, buddy_total_kb, slab_prov_kb
    );
}

fn run_grow_shrink(pages: usize) -> bool {
    println!("\n--- grow-then-shrink, {} pages ({} KB) ---", pages, pages * 4);

    let mut s0 = KernelHeapStats::default();
    if !read_heap_stats(&mut s0) {
        println!("  sys_heap_stats failed");
        return false;
    }

    // Grow phase
    let t0 = time_ms();
    for _ in 0..pages {
        let prev = sbrk(PAGE_SIZE as i32);
        if prev < 0 {
            println!("  sbrk(+{}) failed at iter (heap full?)", PAGE_SIZE);
            return false;
        }
    }
    let t1 = time_ms();

    let mut s1 = KernelHeapStats::default();
    read_heap_stats(&mut s1);

    // Touch each page to make sure mappings work end-to-end.
    let base_after = sbrk(0) as usize;
    let heap_start = base_after - pages * PAGE_SIZE;
    let mut sum: u64 = 0;
    let mut p = heap_start;
    while p < base_after {
        unsafe {
            (p as *mut u32).write_volatile(p as u32);
            sum = sum.wrapping_add((p as *const u32).read_volatile() as u64);
        }
        p += PAGE_SIZE;
    }

    // Shrink phase
    let t2 = time_ms();
    for _ in 0..pages {
        if sbrk(-(PAGE_SIZE as i32)) < 0 {
            println!("  sbrk(-{}) failed", PAGE_SIZE);
            return false;
        }
    }
    let t3 = time_ms();

    let mut s2 = KernelHeapStats::default();
    read_heap_stats(&mut s2);

    let grow_ms = (t1 - t0) as u64;
    let shrink_ms = (t3 - t2) as u64;
    let grow_us_per = if pages > 0 { grow_ms * 1000 / pages as u64 } else { 0 };
    let shrink_us_per = if pages > 0 { shrink_ms * 1000 / pages as u64 } else { 0 };

    println!(
        "  grow: {} ms total, {} us / sbrk(+page)   shrink: {} ms total, {} us / sbrk(-page)",
        grow_ms, grow_us_per, shrink_ms, shrink_us_per
    );
    println!("  (touch checksum = 0x{:x})", sum);
    print_stats_diff("grow", &s0, &s1);
    print_stats_diff("net (after shrink)", &s0, &s2);
    true
}

fn run_churn(rounds: usize) {
    println!(
        "\n--- churn: {} rounds of [sbrk(+8KB) sbrk(-8KB)] ---",
        rounds
    );
    let mut s0 = KernelHeapStats::default();
    read_heap_stats(&mut s0);

    let t0 = time_ms();
    for _ in 0..rounds {
        sbrk(8192);
        sbrk(-8192);
    }
    let t1 = time_ms();

    let mut s1 = KernelHeapStats::default();
    read_heap_stats(&mut s1);

    let total_ms = (t1 - t0) as u64;
    let ops = rounds as u64 * 2;
    let us_per = if ops > 0 { total_ms * 1000 / ops } else { 0 };
    println!(
        "  {} sbrk calls in {} ms  ({} us/call)",
        ops, total_ms, us_per
    );
    print_stats_diff("churn", &s0, &s1);
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("== heaptest: kernel heap stress via sys_sbrk ==");

    let mut s = KernelHeapStats::default();
    if !read_heap_stats(&mut s) {
        println!("sys_heap_stats not supported; aborting");
        return 1;
    }
    println!(
        "initial: buddy_total={}KB  buddy_used={}KB  slab_provisioned={}KB",
        s.buddy_total / 1024,
        s.buddy_used / 1024,
        s.slab_provisioned / 1024,
    );

    // Warm-up: a single small allocation so first-call costs (slab refill)
    // don't dominate.
    sbrk(PAGE_SIZE as i32);
    sbrk(-(PAGE_SIZE as i32));

    if !run_grow_shrink(64) {
        return 1;
    }
    if !run_grow_shrink(256) {
        return 1;
    }
    run_churn(500);

    println!("\nheaptest passed!");
    0
}
