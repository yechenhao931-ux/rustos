#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, get_time_us, mmap, munmap, wait};

const PROT_RW: usize = 3;
const BASE: usize = 0x3000_0000;

// Run `iters` fork+exit cycles after pre-touching `pages` pages of anonymous
// memory. We measure the elapsed time of just the fork() call (parent side)
// and the post-fork wait. With Copy-on-Write, fork latency should grow only
// linearly with the number of mapped pages (PTE rewrites + Arc clones), not
// with the number of bytes touched, because no page contents are copied
// until something is written.
fn bench(pages: usize, iters: usize) {
    let len = pages * 4096;
    assert_eq!(mmap(BASE, len, PROT_RW), 0, "mmap failed");
    // Pre-touch every page so the parent has resident frames.
    for i in 0..pages {
        unsafe {
            ((BASE + i * 4096) as *mut u64).write_volatile(0xA5A5_A5A5_A5A5_A5A5 ^ i as u64);
        }
    }

    let mut total_fork_us: i64 = 0;
    let mut total_wait_us: i64 = 0;
    for _ in 0..iters {
        let t0 = get_time_us();
        let pid = fork();
        let t1 = get_time_us();
        if pid == 0 {
            // Child: do nothing (so we don't pollute the timing with COW
            // copies the child triggers). Real apps would `exec` here.
            exit(0);
        }
        total_fork_us += (t1 - t0) as i64;
        let mut ec = 0;
        let _ = wait(&mut ec);
        let t2 = get_time_us();
        total_wait_us += (t2 - t1) as i64;
    }

    let avg_fork = total_fork_us / iters as i64;
    let avg_wait = total_wait_us / iters as i64;
    println!(
        "pages={:>4} bytes={:>7} iters={} fork_us_avg={} wait_us_avg={}",
        pages,
        len,
        iters,
        avg_fork,
        avg_wait
    );
    assert_eq!(munmap(BASE, len), 0);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== fork() latency vs resident-set size (Copy-on-Write) ===");
    println!("methodology:");
    println!("  * map an anonymous region, write one byte to each page so the");
    println!("    parent's frames are physically resident,");
    println!("  * time fork() with sys_get_time_us() (riscv mtime / CLOCK_FREQ),");
    println!("  * child exits immediately so its COW copies don't show up in");
    println!("    the parent's fork latency,");
    println!("  * average over multiple iterations to dampen scheduler jitter.");
    println!("expectation: latency grows ~linearly with #pages (PTE rewrites),");
    println!("not with bytes touched. A non-COW fork would also copy 4KB per");
    println!("page, so its growth slope would be much steeper.");
    println!("");
    bench(1, 50);
    bench(8, 50);
    bench(64, 20);
    bench(256, 10);
    bench(1024, 5);
    println!("");
    println!("fork_bench done.");
    0
}
