#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{mmap, munmap};

const BASE: usize = 0x1000_0000;
const LEN: usize = 4 * 4096;
const PROT_R: usize = 1;
const PROT_W: usize = 2;

#[unsafe(no_mangle)]
fn main() -> i32 {
    // Reserve a 4-page demand-paged region.
    assert_eq!(mmap(BASE, LEN, PROT_R | PROT_W), 0);

    // Touch each page; the kernel should fault-in frames lazily.
    for i in 0..(LEN / 4096) {
        let p = (BASE + i * 4096) as *mut u64;
        unsafe {
            p.write_volatile(0xDEAD_BEEF + i as u64);
        }
    }
    for i in 0..(LEN / 4096) {
        let p = (BASE + i * 4096) as *const u64;
        let v = unsafe { p.read_volatile() };
        assert_eq!(v, 0xDEAD_BEEF + i as u64);
    }

    assert_eq!(munmap(BASE, LEN), 0);
    // Overlap rejection.
    assert_eq!(mmap(BASE, LEN, PROT_R | PROT_W), 0);
    assert_eq!(mmap(BASE + 4096, 4096, PROT_R | PROT_W), -1);
    assert_eq!(munmap(BASE, LEN), 0);
    // Invalid permission bits.
    assert_eq!(mmap(BASE, LEN, 0), -1);
    assert_eq!(mmap(BASE, LEN, 0x10), -1);
    // Unaligned.
    assert_eq!(mmap(BASE + 1, LEN, PROT_R), -1);

    println!("mmap_test passed!");
    0
}
