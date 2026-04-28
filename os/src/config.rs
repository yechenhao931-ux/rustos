#[allow(unused)]

pub const USER_STACK_SIZE: usize = 4096 * 2;
pub const KERNEL_STACK_SIZE: usize = 4096 * 2;
pub const KERNEL_HEAP_SIZE: usize = 0x100_0000;
pub const PAGE_SIZE: usize = 0x1000;
pub const PAGE_SIZE_BITS: usize = 0xc;

pub const TRAMPOLINE: usize = usize::MAX - PAGE_SIZE + 1;
pub const TRAP_CONTEXT_BASE: usize = TRAMPOLINE - PAGE_SIZE;

/// Base of the per-process user heap. Sits well above any plausible thread
/// stack region (which grows from `ustack_base` ≈ data_end). Sv39 user
/// space goes up to ~512 GiB so 1 GiB is safe.
pub const USER_HEAP_BASE: usize = 0x4000_0000;
/// Hard cap on user heap size (1 GiB). `sbrk` past this returns -1.
pub const USER_HEAP_MAX_SIZE: usize = 0x4000_0000;

pub use crate::board::{CLOCK_FREQ, MEMORY_END, MMIO};
