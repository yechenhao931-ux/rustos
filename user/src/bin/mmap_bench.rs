#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{get_time_us, mmap, munmap};

const PROT_RW: usize = 3;
const BASE: usize = 0x4000_0000;

fn run(pages: usize) {
    let len = pages * 4096;

    // 1) mmap latency: should be ~constant since we don't allocate frames.
    let t0 = get_time_us();
    assert_eq!(mmap(BASE, len, PROT_RW), 0);
    let t1 = get_time_us();
    let mmap_us = t1 - t0;

    // 2) first-touch latency: pay for one demand-page fault per page.
    let t2 = get_time_us();
    for i in 0..pages {
        unsafe {
            ((BASE + i * 4096) as *mut u8).write_volatile(1);
        }
    }
    let t3 = get_time_us();
    let touch_us = t3 - t2;

    // 3) re-touch latency: PTEs are present, no kernel involvement.
    let t4 = get_time_us();
    for i in 0..pages {
        unsafe {
            ((BASE + i * 4096) as *mut u8).write_volatile(2);
        }
    }
    let t5 = get_time_us();
    let retouch_us = t5 - t4;

    let touch_us_per_page = if pages == 0 { 0 } else { touch_us / pages as isize };
    println!(
        "pages={:>4} mmap_us={:>5} first_touch_us={:>7} (~{} us/page) retouch_us={}",
        pages, mmap_us, touch_us, touch_us_per_page, retouch_us
    );

    assert_eq!(munmap(BASE, len), 0);
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== mmap() latency and per-page demand-page cost ===");
    println!("methodology:");
    println!("  * mmap_us measures the syscall itself; with lazy allocation");
    println!("    it should not depend on `pages` (the kernel only registers");
    println!("    a MapArea, no frames touched).");
    println!("  * first_touch_us measures `pages` page-fault round-trips,");
    println!("    each one ~= (trap + alloc + zero + map_one). Per-page cost");
    println!("    should be roughly constant.");
    println!("  * retouch_us measures PTE-resident store throughput as a");
    println!("    sanity baseline.");
    println!("");
    run(1);
    run(8);
    run(64);
    run(256);
    run(1024);
    println!("");
    println!("mmap_bench done.");
    0
}
