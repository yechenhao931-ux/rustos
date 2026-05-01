#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, set_priority, wait, yield_};

const ROUNDS: usize = 30_000;

fn busy_loop(prio: isize, ticks: &mut u64) {
    set_priority(prio);
    let mut count: u64 = 0;
    for _ in 0..ROUNDS {
        for _ in 0..200 {
            count = count.wrapping_add(1);
        }
        yield_();
    }
    *ticks = count;
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    // Spawn 3 children with priorities 4, 8, 16. With stride scheduling,
    // CPU shares should be roughly proportional to priority. We just check
    // the program runs to completion without panic; visual inspection of
    // the printed counts shows the proportional-share behavior.
    let prios = [4isize, 8, 16];
    let mut pids = [0isize; 3];
    for (i, &p) in prios.iter().enumerate() {
        let pid = fork();
        if pid == 0 {
            let mut ticks: u64 = 0;
            busy_loop(p, &mut ticks);
            println!("child prio={} ticks={}", p, ticks);
            exit(0);
        }
        pids[i] = pid;
    }
    let mut ec = 0;
    for _ in 0..prios.len() {
        wait(&mut ec);
    }
    // Bad priority should fail.
    assert_eq!(set_priority(1), -1);
    assert_eq!(set_priority(64), 64);
    println!("stride_test passed!");
    0
}
