# rCore Kernel Optimizations — Design and Measurement Methodology

This branch (`claude/rcore-optimization-Y3cUX`) layers four independent
kernel-level optimizations onto baseline rCore and ships a benchmark suite
to measure each one. The methodology section below explains *what* is
measured, *why*, and *how to interpret* the resulting numbers — useful as a
talking point in interviews and as a template for adding more experiments.

## 1. Optimizations

| # | Optimization | Files | Replaces |
|---|---|---|---|
| 1 | Stride scheduler with priority | `os/src/task/{manager,task}.rs`, `sys_set_priority` | FIFO scheduler |
| 2 | Buddy-system frame allocator | `os/src/mm/frame_allocator.rs` | Bump + recycled-stack allocator |
| 3 | mmap/munmap with demand paging | `os/src/mm/memory_set.rs`, `os/src/trap/mod.rs`, `sys_mmap` / `sys_munmap` | (no equivalent in baseline) |
| 4 | Copy-on-Write `fork()` | `os/src/mm/memory_set.rs`, `os/src/task/process.rs`, fault-handler hook | Eager-copy `from_existed_user` |

All four reuse the existing rCore abstractions (`MemorySet`, `MapArea`,
`UPIntrFreeCell`, etc.) so they do not regress correctness on the baseline
test apps.

### 1.1 Stride scheduler

Each `TaskControlBlockInner` now carries `stride: u64` and `priority: u64`
(default 16, minimum 2). On every `fetch()`, the manager linearly scans the
ready queue, picks the task with the smallest stride, and advances it by
`BIG_STRIDE / priority` (BIG_STRIDE = 2²⁰). Higher priority ⇒ slower
stride growth ⇒ more frequent selection.

`sys_set_priority(prio)` (syscall 140) lets a task tune its own share at
runtime; values `< 2` are rejected to avoid divide-by-tiny.

### 1.2 Buddy frame allocator

Replaces the bump+`recycled: Vec` allocator. `free_lists[k]` is a
`BTreeSet<usize>` of starting PPNs of free blocks of size 2ᵏ pages.

* **Init** decomposes `[ekernel, MEMORY_END)` greedily into power-of-two
  blocks at their natural alignment.
* **alloc_order(k)** finds the smallest non-empty `free_lists[j]` with
  `j ≥ k`, pops a block, and splits down to order `k`, pushing the
  successive halves back into the free lists.
* **dealloc_order(p, k)** keeps merging with `p ^ (1 << k)` while the
  buddy is also free.
* **alloc_more(n)** allocates a single block of order `⌈log₂ n⌉` and
  releases the unused tail back to the free lists, so multi-page virtio
  buffers stay physically contiguous *after* per-page deallocs and
  reallocs.

### 1.3 mmap with demand paging

`MapArea` gained a `lazy: bool` flag. `MapArea::map()` is a no-op when
`lazy` is set; the page-fault handler calls
`MemorySet::handle_lazy_page_fault(vpn)`, which materializes a frame for
the first access and lets the user instruction retry. `fork()` (both the
COW fast path and the eager fallback) propagates lazy areas, copying only
the pages the parent has actually faulted in.

### 1.4 Copy-on-Write `fork()`

`MapArea::data_frames` was upgraded from `BTreeMap<VPN, FrameTracker>` to
`BTreeMap<VPN, Arc<FrameTracker>>` so a frame can be co-owned by multiple
address spaces. `from_existed_user_cow` shares all user-accessible
(`MapPermission::U`) framed pages between parent and child, demoting both
PTEs to read-only. The trap handler chains `handle_lazy_page_fault →
handle_cow_fault → SIGSEGV`; `handle_cow_fault` re-promotes the PTE
without a copy when `Arc::strong_count == 1` (sole owner) or allocates a
private page and copies otherwise.

The trap-context page is *not* COW (no `U` bit, kernel writes via direct
PPN access); it is eagerly copied so kernel-side writes never silently
leak between parent and child.

## 2. Benchmark Suite

Programs live in `user/src/bin/`:

| Program | Optimization under test | Metric |
|---|---|---|
| `cow_test.rs` | COW correctness | functional assertions |
| `mmap_test.rs` | mmap/munmap correctness | functional assertions |
| `stride_test.rs` | scheduler correctness | runs to completion |
| `fork_bench.rs` | COW fork latency | μs/fork vs resident-set size |
| `mmap_bench.rs` | demand paging | mmap μs, first-touch μs/page, retouch μs |
| `stride_bench.rs` | scheduler fairness | work_hi / work_lo vs P_hi / P_lo |
| `buddy_bench.rs` | frame allocator (kernel-side) | μs/op + contiguous-after-churn success matrix |

A microsecond timer (`sys_get_time_us`, syscall 170) was added because
`sys_get_time` only returns milliseconds, which is too coarse for fork and
mmap measurements.

### 2.1 How to run

```
cd os
make run        # boots rCore in qemu with the user/initproc shell
# inside the shell:
> cow_test
> mmap_test
> fork_bench
> mmap_bench
> stride_bench
```

## 3. Measurement Methodology

### 3.1 Time source

`sys_get_time_us` reads the RISC-V `mtime` CSR and converts to
microseconds via `time::read() * 1_000_000 / CLOCK_FREQ`. This avoids
losing precision on short kernel paths (a fork costs single-digit μs).
qemu's `mtime` is monotonic but the conversion can drift relative to
wall clock; for relative measurements (latency, ratios) this is fine.

### 3.2 Reducing noise

* **Repeat & average.** Each `fork_bench` data point averages 5–50
  iterations. The inner loop of `stride_bench` runs for a 2-second
  wall-clock window.
* **Warm-up touches.** `fork_bench` writes one byte into every page
  *before* timing fork, so frames are physically resident — otherwise
  the post-fork COW faults would be measuring the lazy mmap path
  instead of the COW path.
* **Child-side isolation.** In `fork_bench` the child calls `exit(0)`
  immediately so its own COW faults do not pollute the parent's
  measurement window.
* **No interference processes.** Run benches one at a time. A
  background `huge_write_mt` will skew scheduler benches.

### 3.3 What each metric tells you

#### `fork_bench` — `fork_us_avg`

* Expected to grow roughly **linearly with the number of mapped pages**
  (each page costs two PTE rewrites + one `Arc::clone`) and to be
  **independent of bytes written**.
* If you reverted to the eager `from_existed_user`, you would see a
  much steeper slope (each page now also pays a 4 KB `copy_from_slice`,
  i.e. ~4 KB / memory bandwidth ≈ several μs/page on top).
* A useful comparison: switch `sys_fork` between
  `from_existed_user_cow` and `from_existed_user`, rerun, plot two
  lines. The gap is the COW win.

#### `mmap_bench` — `mmap_us`, `first_touch_us`, `retouch_us`

* `mmap_us` should be **flat** across page counts. It only inserts a
  `MapArea` into `memory_set.areas` and runs an overlap check
  (O(n_areas)).
* `first_touch_us / pages` is the **per-page demand-paging cost**:
  trap entry, allocator hit, page-zero, `map_one`, sfence. On qemu
  this is typically tens of μs/page.
* `retouch_us` is the user-mode store throughput baseline (no kernel
  involvement). The ratio `first_touch_us / retouch_us` shows how
  expensive a page fault is relative to a hot store.

#### `buddy_bench` — μs/op + contiguous-after-churn

Userland can't call `frame_alloc` directly, so this bench runs entirely
inside the kernel (syscall 2500). The kernel:

1. Leases a 256-page contiguous arena from the live buddy allocator.
2. For each workload below, builds a **fresh local allocator** of each
   kind over that same arena and runs the same operation sequence:
   * `BuddyFrameAllocator` — the new implementation.
   * `LegacyStackFrameAllocator` — the original rCore allocator,
     compiled in only for this benchmark (`#[allow(dead_code)]`).
3. Prints microsecond timings via `timer::get_time_us`.

Workloads:

* **A — throughput.** Alloc all 256 order-0 pages, dealloc all. Both
  allocators are O(N), but the buddy carries log-N split/merge
  overhead per op while the stack is amortized O(1). Expect the stack
  to be 2–5× faster on this micro-benchmark; this is the **cost** you
  pay for buddy.

* **B — contiguous-after-churn.** Alloc all 256 pages, free all 256,
  then `alloc_more(K)` for K ∈ {2, 4, 8, 32}.
  * Buddy: every `K` succeeds — freed pages coalesce back into large
    blocks.
  * Legacy stack: every `K` **FAILs** — `alloc_more` only consults the
    bump pointer (`current..end`), never the recycled stack, so once
    the bump pointer is exhausted contiguous allocation is dead. This
    is the **headline win** for buddy and the qualitative
    correctness/fragmentation difference to point to in an interview.

* **C — mixed churn.** 1024 pseudo-random alloc/dealloc pairs over a
  64-page working set. Reports total μs and ns/op average; mirrors a
  realistic kernel hot path.

Run via `> buddy_bench` in the rCore shell. The arena is freed when
the bench returns, so the test is non-destructive and can be re-run.

#### `stride_bench` — `work` ratio per priority pair

* Children with priorities `P_lo` and `P_hi` should produce a work
  ratio approaching `P_hi / P_lo` over a sufficiently long window.
* Deviations come from:
  * **Quantum granularity** — at 100 Hz, each tick is 10 ms;
    differences swallowed by a single tick.
  * **`yield()` boundaries** — busy work is interrupted by explicit
    yields, not strict preemption.
  * **Fork start skew** — children begin a few μs apart; longer
    windows wash this out.
* A baseline FIFO scheduler would give roughly **equal** work counts
  regardless of priority, which is the qualitative difference to point
  to in an interview.

### 3.4 Reproducibility checklist

For a credible result table:

1. Same qemu version and `CLOCK_FREQ` (qemu virt: 10 MHz).
2. Same `MAKE` flags (`MODE=release`, no GUI).
3. Run each bench twice; report the second run (caches warm, allocator
   patterns settled).
4. Report median of N=5, plus min/max, not just mean.
5. State which optimizations are active. The four are independent — you
   can A/B by reverting individual files.

### 3.5 Limitations

* qemu `mtime` is virtualized; absolute μs values won't match real
  hardware.
* Single-hart only; the buddy allocator and `UPIntrFreeCell` would
  need real spin-locks to scale.
* `fork_bench` only fork+exit; a real workload (`fork → exec`) hides
  the COW win behind exec's address-space teardown.
* Stride scheduler scans the ready queue linearly (O(n)); for
  hundreds of tasks a min-heap would be the next step.

## 4. Future Work (talking points)

* **Min-heap stride scheduler** — drop `fetch()` from O(n) to
  O(log n). Easy with `BinaryHeap<Reverse<(u64, Arc<TCB>)>>`.
* **Page cache in easy-fs** — currently every `read_at` round-trips
  through the block device.
* **File-backed mmap** — extend the `lazy` path to populate from an
  inode rather than zero-fill.
* **TLB shootdown batching** — the COW fork currently does a single
  blanket `sfence.vma`. With ASIDs we could narrow to per-vpn flushes.
* **Slab-style kernel-object cache** — reuse fixed-size allocations
  (TaskControlBlock, etc.) to skip the buddy allocator's split/merge
  cost on hot paths.
