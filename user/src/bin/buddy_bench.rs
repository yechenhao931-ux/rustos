#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::buddy_bench;

// Trampoline that triggers the kernel-side frame-allocator benchmark.
// All measurement and printing happens inside the kernel
// (mm::run_buddy_bench); see RCORE_OPTIMIZATIONS.md for the methodology.
#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== buddy-vs-stack frame allocator benchmark ===");
    println!("methodology:");
    println!("  Userland cannot call frame_alloc directly, so the bench runs");
    println!("  inside the kernel via syscall 2500. The kernel:");
    println!("    1. leases a contiguous arena of 256 pages from the live");
    println!("       allocator (frame_alloc_more),");
    println!("    2. constructs three FRESH allocators over that same arena:");
    println!("         - LegacyStackFrameAllocator (original rCore baseline)");
    println!("         - BuddyFrameAllocator (anti-fragmentation tier)");
    println!("         - HybridFrameAllocator (L1 stack cache + L2 buddy,");
    println!("           the LIVE in-kernel allocator)");
    println!("    3. runs three workloads against each, prints microseconds");
    println!("       via timer::get_time_us, then drops the arena.");
    println!("");
    println!("  Workload A (throughput): alloc + dealloc 256 order-0 pages.");
    println!("    Stack: amortized O(1).");
    println!("    Buddy: log-N per op (split + merge bookkeeping).");
    println!("    Hybrid: O(1) L1 hits + a single log-N refill per 16 ops");
    println!("      => approaches stack speed at ~1/16 of buddy's overhead.");
    println!("");
    println!("  Workload B (contig-after-churn): alloc all + dealloc all,");
    println!("    then ask for K contiguous pages. Buddy/hybrid coalesce");
    println!("    freed pages back into large blocks, so they satisfy any");
    println!("    K up to ARENA. The legacy stack alloc_more never inspects");
    println!("    its `recycled` stack, so it FAILS once the bump pointer");
    println!("    is exhausted -- the headline fragmentation win.");
    println!("");
    println!("  Workload C (mixed churn): 1024 random alloc/dealloc pairs");
    println!("    over a 64-page working set. Most ops should hit hybrid's");
    println!("    L1 cache and never touch buddy.");
    println!("");
    println!("  Numbers are qemu-virtualized and meant for relative");
    println!("  comparison only; report median of N=5 runs in real reports.");
    println!("");
    let r = buddy_bench();
    if r != 0 {
        println!("buddy_bench failed: rc={}", r);
        return 1;
    }
    0
}
