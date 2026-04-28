# rustos — kernel allocator optimisation on rCore-Tutorial-v3

This branch takes the [rCore-Tutorial-v3](https://github.com/rcore-os/rCore-Tutorial-v3)
RISC-V teaching kernel and replaces its kernel heap allocator with a
purpose-built two-level allocator (`kalloc`) that combines a slab cache
with a buddy backend.

The change is small and self-contained: a new `kalloc` crate, a new host
benchmark harness (`allocator-bench`), one file changed under `os/src/mm/`,
and the rest of rCore unmodified. The point of the project is to do real
performance engineering on a single concrete kernel subsystem and report
numbers, not to rewrite the kernel.

## Headline results

Single-threaded host benchmark, `spin::Mutex`-wrapped baselines, 16 MiB
heap. Lower is better.

| Workload     | kalloc   | `buddy_system_allocator` | `linked_list_allocator` |
| ------------ | -------- | ------------------------ | ----------------------- |
| small-lifo   | **27.4** | 42.9 (1.57×)             | 35.3 (1.29×)            |
| mixed-random | **121.5**| 156.6 (1.29×)            | 536.2 (4.41×)           |
| vec-growth   | 27.6     | **23.1**                 | 35.3                    |

ns/op; bold = winner. With stats instrumentation the slab caches absorb
**100% of small-lifo**, **70% of mixed-random**, and **78% of vec-growth**
calls — only the misses fall through to buddy. Full design notes,
methodology, and stats-on numbers are in
[`docs/allocator.md`](docs/allocator.md).

## Repository layout

```
allocator/         kalloc — no_std two-level (slab + buddy) allocator
allocator-bench/   host-side benchmark vs buddy_system / linked_list
docs/allocator.md  design + benchmarks
os/                rCore-Tutorial-v3 kernel (unmodified except heap)
user/              rCore user programs
easy-fs/           rCore filesystem
```

## Reproduce

```sh
# Unit tests (host)
( cd allocator     && cargo test --features std,stats )

# Production-mode benchmark
( cd allocator-bench && cargo run --release )

# Stats-mode benchmark (slower, prints slab hit rates)
( cd allocator-bench && cargo run --release --features stats )

# Build the rCore kernel with kalloc as the global allocator
cp os/src/linker-qemu.ld os/src/linker.ld
( cd os && cargo build --release --target riscv64gc-unknown-none-elf )

# Full kernel run (requires qemu-system-riscv64)
cd os && make run
```

## End-to-end test from a user program

Two new syscalls let userland exercise and observe the kernel heap:

* `sys_sbrk(delta)` (id 214) — grow/shrink the per-process user heap.
  Each page added triggers `frame_alloc` + `BTreeMap` insert + page-table
  walk, which together generate a handful of kernel-heap allocations of
  varied sizes — exactly the slab+buddy fast path.
* `sys_heap_stats(*out)` (id 4000) — copy the kernel-heap counters
  (allocs, frees, slab hits, buddy calls, peak usage) into the user.

The `heaptest` user program (`user/src/bin/heaptest.rs`) takes a
before-snapshot, runs grow/shrink/churn phases, takes an after-snapshot,
and prints kernel-side throughput plus slab hit rate. Run it from the
rCore shell after booting (`>> heaptest`). Source-of-truth for the
syscall layout lives next to the syscall, in
`os/src/syscall/process.rs::UserHeapStats` /
`user/src/task.rs::KernelHeapStats`.

## What's in `kalloc`

* **Buddy allocator** (`allocator/src/buddy.rs`). Per-order intrusive
  free lists, split-on-alloc, optimistic O(N) coalescing on free. Safe
  on heap regions that aren't power-of-two aligned.
* **Slab caches** (`allocator/src/slab.rs`). Eight size classes
  (8..1024 B). Cache miss pulls a 4-8 KiB slab from buddy and slices it,
  pushing high address first so subsequent pops give cache-friendly
  ascending addresses.
* **Router** (`allocator/src/lib.rs`). `O(1)` class lookup via
  `next_power_of_two().trailing_zeros()`. Single `spin::Mutex` for the
  whole heap; per-class locks are a future-step.
* **Stats** (`allocator/src/stats.rs`). Atomic counters behind a Cargo
  feature, so production builds don't pay for them on the hot path.

## What's in `allocator-bench`

A single binary that runs three workloads (`small-lifo`, `mixed-random`,
`vec-growth`) against `kalloc`, `buddy_system_allocator::LockedHeap`,
and `spin::Mutex<linked_list_allocator::Heap>`. The harness records
ns/op, throughput, and (with `--features stats`) slab hit rate.

The workloads are seeded with a fixed PRNG so all three allocators see
the exact same allocation sequence; numbers are directly comparable.

## Original rCore notes

The kernel below `os/` is unmodified rCore-Tutorial-v3.6 except for
`os/src/mm/heap_allocator.rs`. See the upstream
[Chinese documentation](https://rcore-os.github.io/rCore-Tutorial-Book-v3/)
for everything else (boot flow, paging, scheduler, fs, syscalls).
