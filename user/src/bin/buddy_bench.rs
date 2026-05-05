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
    println!("       buddy allocator (frame_alloc_more),");
    println!("    2. constructs two FRESH allocators over that same arena:");
    println!("         - BuddyFrameAllocator (the new implementation)");
    println!("         - LegacyStackFrameAllocator (the original rCore code)");
    println!("    3. runs three workloads against each, prints microseconds");
    println!("       via timer::get_time_us, then drops the arena.");
    println!("");
    println!("  Workload A (throughput): alloc all 256 pages, dealloc all.");
    println!("    Both should be O(N); buddy carries log-N overhead per op,");
    println!("    stack carries amortized O(1).");
    println!("");
    println!("  Workload B (contig-after-churn): alloc all + dealloc all,");
    println!("    then ask for K contiguous pages. Buddy coalesces freed");
    println!("    pages back into large blocks, so it satisfies any K up");
    println!("    to ARENA. The legacy stack alloc_more never inspects its");
    println!("    `recycled` stack, so it FAILS once the bump pointer is");
    println!("    exhausted -- the headline correctness/fragmentation win.");
    println!("");
    println!("  Workload C (mixed churn): 1024 random alloc/dealloc pairs");
    println!("    over a 64-page working set. Reports ns/op for both.");
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
