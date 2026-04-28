use super::*;

pub fn exit(exit_code: i32) -> ! {
    sys_exit(exit_code);
}
pub fn yield_() -> isize {
    sys_yield()
}
pub fn get_time() -> isize {
    sys_get_time()
}
pub fn getpid() -> isize {
    sys_getpid()
}
pub fn fork() -> isize {
    sys_fork()
}

/// Mirror of the kernel `UserHeapStats` struct. Field order MUST match
/// `os/src/syscall/process.rs::UserHeapStats`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct KernelHeapStats {
    pub buddy_total: u64,
    pub buddy_used: u64,
    pub slab_provisioned: u64,
    pub slab_free: u64,
    pub allocs: u64,
    pub frees: u64,
    pub bytes_allocated: u64,
    pub bytes_freed: u64,
    pub slab_hits: u64,
    pub buddy_calls: u64,
    pub oom: u64,
}

/// Grow (or shrink) the user heap. Returns the previous heap top, or -1.
pub fn sbrk(delta: i32) -> isize {
    sys_sbrk(delta)
}

/// Snapshot the kernel heap counters into `out`. Returns true on success.
pub fn read_heap_stats(out: &mut KernelHeapStats) -> bool {
    sys_heap_stats(out as *mut KernelHeapStats as *mut u8) == 0
}
pub fn exec(path: &str, args: &[*const u8]) -> isize {
    sys_exec(path, args)
}

pub fn wait(exit_code: &mut i32) -> isize {
    loop {
        match sys_waitpid(-1, exit_code as *mut _) {
            -2 => {
                yield_();
            }
            // -1 or a real pid
            exit_pid => return exit_pid,
        }
    }
}

pub fn waitpid(pid: usize, exit_code: &mut i32) -> isize {
    loop {
        match sys_waitpid(pid as isize, exit_code as *mut _) {
            -2 => {
                yield_();
            }
            // -1 or a real pid
            exit_pid => return exit_pid,
        }
    }
}

pub fn waitpid_nb(pid: usize, exit_code: &mut i32) -> isize {
    sys_waitpid(pid as isize, exit_code as *mut _)
}

bitflags! {
    pub struct SignalFlags: i32 {
        const SIGINT    = 1 << 2;
        const SIGILL    = 1 << 4;
        const SIGABRT   = 1 << 6;
        const SIGFPE    = 1 << 8;
        const SIGSEGV   = 1 << 11;
    }
}

pub fn kill(pid: usize, signal: i32) -> isize {
    sys_kill(pid, signal)
}

pub fn sleep(sleep_ms: usize) {
    sys_sleep(sleep_ms);
}

pub fn thread_create(entry: usize, arg: usize) -> isize {
    sys_thread_create(entry, arg)
}
pub fn gettid() -> isize {
    sys_gettid()
}
pub fn waittid(tid: usize) -> isize {
    loop {
        match sys_waittid(tid) {
            -2 => {
                yield_();
            }
            exit_code => return exit_code,
        }
    }
}
