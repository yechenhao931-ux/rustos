#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, mmap, munmap, wait};

const PROT_RW: usize = 3;
const BASE: usize = 0x5000_0000;
const PAGES: usize = 8;
const LEN: usize = PAGES * 4096;

// Functional COW correctness test: parent maps a region, writes a value,
// forks. Child writes a different value into the SAME virtual addresses.
// Both writes must produce private copies; neither should see the other's
// modifications.
#[unsafe(no_mangle)]
fn main() -> i32 {
    assert_eq!(mmap(BASE, LEN, PROT_RW), 0);
    for i in 0..PAGES {
        unsafe {
            ((BASE + i * 4096) as *mut u64).write_volatile(0xAA00 + i as u64);
        }
    }

    let pid = fork();
    if pid == 0 {
        // Child: write a fresh pattern, then verify only its own writes are
        // visible (neither parent's pre-fork values nor parent's post-fork
        // overwrites should leak in).
        for i in 0..PAGES {
            unsafe {
                ((BASE + i * 4096) as *mut u64).write_volatile(0xCC00 + i as u64);
            }
        }
        for i in 0..PAGES {
            let v = unsafe { ((BASE + i * 4096) as *const u64).read_volatile() };
            assert_eq!(v, 0xCC00 + i as u64, "child saw {:x} at page {}", v, i);
        }
        exit(0);
    }

    // Parent: overwrite with a third pattern AFTER fork. With correct COW,
    // these writes also fault and produce a private copy that the child
    // cannot see.
    for i in 0..PAGES {
        unsafe {
            ((BASE + i * 4096) as *mut u64).write_volatile(0xBB00 + i as u64);
        }
    }
    let mut ec = 0;
    wait(&mut ec);
    assert_eq!(ec, 0, "child failed COW correctness check");

    for i in 0..PAGES {
        let v = unsafe { ((BASE + i * 4096) as *const u64).read_volatile() };
        assert_eq!(v, 0xBB00 + i as u64);
    }
    assert_eq!(munmap(BASE, LEN), 0);
    println!("cow_test passed!");
    0
}
