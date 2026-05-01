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

pub fn set_priority(prio: isize) -> isize {
    sys_set_priority(prio)
}

pub fn mmap(start: usize, len: usize, prot: usize) -> isize {
    sys_mmap(start, len, prot)
}

pub fn munmap(start: usize, len: usize) -> isize {
    sys_munmap(start, len)
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
