#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, get_time_us, set_priority, wait, yield_};

// Each child loops, accumulating a counter. The kernel records a tick
// whenever it picks the child to run; here we approximate the CPU share by
// counting how many work-units the child completes within a fixed wall-clock
// window. Under stride scheduling the ratio of completed work between two
// children should approach the ratio of their priorities.
fn worker(prio: isize, deadline_us: isize) -> u64 {
    set_priority(prio);
    let mut work: u64 = 0;
    loop {
        // 200-iter inner loop = roughly one quantum's worth of work.
        for _ in 0..200 {
            work = work.wrapping_add(1);
        }
        if get_time_us() >= deadline_us {
            return work;
        }
        yield_();
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== stride scheduler proportional-share benchmark ===");
    println!("methodology:");
    println!("  * fork two children with priorities P_lo and P_hi,");
    println!("  * each child runs a tight loop yielding once per quantum,");
    println!("  * after a fixed wall-clock window each prints its work count,");
    println!("  * expected ratio: work_hi / work_lo  ~=  P_hi / P_lo.");
    println!("  * deviation reveals scheduler bias / quantum granularity.");
    println!("");

    let pairs: [(isize, isize); 3] = [(2, 16), (4, 16), (8, 16)];
    for (lo, hi) in pairs {
        let window_us = 2_000_000isize; // 2 seconds
        let deadline = (get_time_us() + window_us) as isize;

        let pid_lo = fork();
        if pid_lo == 0 {
            let w = worker(lo, deadline);
            println!("  child prio={:>2} work={}", lo, w);
            exit(0);
        }
        let pid_hi = fork();
        if pid_hi == 0 {
            let w = worker(hi, deadline);
            println!("  child prio={:>2} work={}", hi, w);
            exit(0);
        }
        let mut ec = 0;
        wait(&mut ec);
        wait(&mut ec);
        println!(
            "pair P_lo={} P_hi={} expected_ratio={}",
            lo,
            hi,
            hi / lo
        );
        println!("");
    }
    println!("stride_bench done.");
    0
}
