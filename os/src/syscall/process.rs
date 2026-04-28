use crate::config::{USER_HEAP_BASE, USER_HEAP_MAX_SIZE};
use crate::fs::{OpenFlags, open_file};
use crate::mm::{
    heap_stats, translated_byte_buffer, translated_ref, translated_refmut, translated_str,
};
use crate::task::{
    SignalFlags, current_process, current_task, current_user_token, exit_current_and_run_next,
    pid2process, suspend_current_and_run_next,
};
use crate::timer::get_time_ms;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub fn sys_exit(exit_code: i32) -> ! {
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    suspend_current_and_run_next();
    0
}

pub fn sys_get_time() -> isize {
    get_time_ms() as isize
}

pub fn sys_getpid() -> isize {
    current_task().unwrap().process.upgrade().unwrap().getpid() as isize
}

pub fn sys_fork() -> isize {
    let current_process = current_process();
    let new_process = current_process.fork();
    let new_pid = new_process.getpid();
    // modify trap context of new_task, because it returns immediately after switching
    let new_process_inner = new_process.inner_exclusive_access();
    let task = new_process_inner.tasks[0].as_ref().unwrap();
    let trap_cx = task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    new_pid as isize
}

pub fn sys_exec(path: *const u8, mut args: *const usize) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let mut args_vec: Vec<String> = Vec::new();
    loop {
        let arg_str_ptr = *translated_ref(token, args);
        if arg_str_ptr == 0 {
            break;
        }
        args_vec.push(translated_str(token, arg_str_ptr as *const u8));
        unsafe {
            args = args.add(1);
        }
    }
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let process = current_process();
        let argc = args_vec.len();
        process.exec(all_data.as_slice(), args_vec);
        // return argc because cx.x[10] will be covered with it later
        argc as isize
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    let process = current_process();
    // find a child process

    let mut inner = process.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

pub fn sys_kill(pid: usize, signal: u32) -> isize {
    if let Some(process) = pid2process(pid) {
        if let Some(flag) = SignalFlags::from_bits(signal) {
            process.inner_exclusive_access().signals |= flag;
            0
        } else {
            -1
        }
    } else {
        -1
    }
}

/// `sys_sbrk(delta)` — extend (or shrink) the user heap by `delta` bytes.
///
/// Semantics match Linux/POSIX `sbrk(2)`: returns the *previous* heap top
/// on success, or `-1` on failure. `delta == 0` queries the current top.
/// Heap grows from `USER_HEAP_BASE`; the kernel rejects requests that
/// would shrink below the base or grow past `USER_HEAP_MAX_SIZE`.
///
/// On grow, kernel does:
///   1. allocate frames via `frame_alloc` (one per page),
///   2. write PTEs into the user page table (which uses kernel heap for
///      mid-level page table nodes),
///   3. record `FrameTracker`s in the heap `MapArea`.
/// All three steps stress the kernel heap allocator — `sys_heap_stats`
/// below lets userspace observe the resulting allocation traffic.
pub fn sys_sbrk(delta: i32) -> isize {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let cur = inner.heap_top;
    if delta == 0 {
        return cur as isize;
    }
    let new_top = if delta > 0 {
        cur.checked_add(delta as usize)
    } else {
        cur.checked_sub((-(delta as i64)) as usize)
    };
    let Some(new_top) = new_top else { return -1 };
    if new_top < USER_HEAP_BASE || new_top > USER_HEAP_BASE + USER_HEAP_MAX_SIZE {
        return -1;
    }
    if !inner.memory_set.resize_heap(USER_HEAP_BASE, cur, new_top) {
        return -1;
    }
    inner.heap_top = new_top;
    cur as isize
}

/// Layout exposed to user space by `sys_heap_stats`. Keep field order and
/// types in sync with `user/src/heap_stats.rs`.
#[repr(C)]
pub struct UserHeapStats {
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

/// `sys_heap_stats(out)` — copy a snapshot of the kernel heap counters into
/// the user buffer at `out`. Returns 0 on success, -1 on bad pointer.
///
/// The point of this syscall is to give the userland benchmark program a
/// way to see exactly how many bytes/allocs the kernel performed in
/// response to its work, without printf or RDTSC tricks.
pub fn sys_heap_stats(out: *mut u8) -> isize {
    let token = current_user_token();
    let bytes = core::mem::size_of::<UserHeapStats>();
    let buffers = translated_byte_buffer(token, out, bytes);
    if buffers.is_empty() {
        return -1;
    }
    let (snap, st) = heap_stats();
    let s = UserHeapStats {
        buddy_total: snap.buddy_total as u64,
        buddy_used: snap.buddy_used as u64,
        slab_provisioned: snap.slab_provisioned as u64,
        slab_free: snap.slab_free as u64,
        allocs: st.allocs(),
        frees: st.frees(),
        bytes_allocated: st.bytes_allocated(),
        bytes_freed: st.bytes_freed(),
        slab_hits: st.slab_hits(),
        buddy_calls: st.buddy_calls(),
        oom: st.oom(),
    };
    let src = unsafe {
        core::slice::from_raw_parts(&s as *const _ as *const u8, bytes)
    };
    let mut written = 0;
    for chunk in buffers {
        let n = chunk.len().min(bytes - written);
        chunk[..n].copy_from_slice(&src[written..written + n]);
        written += n;
        if written == bytes {
            break;
        }
    }
    if written == bytes { 0 } else { -1 }
}
