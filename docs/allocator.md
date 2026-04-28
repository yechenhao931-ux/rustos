# `kalloc`: a two-level kernel heap (slab + buddy)

This document covers the design of `kalloc`, the replacement for rCore's
default `buddy_system_allocator::LockedHeap`, and the host-side benchmarks
that drive the optimisation work.

## Why replace the default?

rCore-Tutorial-v3 ships with a single buddy allocator for the kernel heap.
Buddy is fine for medium-to-large allocations but has two weaknesses on
the kernel's actual workload:

1. **Internal fragmentation on small objects.** Buddy rounds up to the next
   power of two. A 24-byte `Box<u32>` consumes 64 bytes (62% wasted). Kernel
   data structures — task control blocks, file descriptors, page-table
   nodes, log lines — are dominated by sub-1-KiB allocations.
2. **Lookup overhead per call.** Even on a freelist hit the buddy walks
   order classes and hits the same lock as every other allocation,
   so allocation rate is bounded by lock + cacheline traffic per op.

A slab cache absorbs these problems for fixed-size hot objects: each cache
keeps a freelist of pre-sized objects and allocations become a single
freelist pop. When a cache empties it pulls a slab page from the buddy
backend and partitions it.

## Layout

```
+------------------------------ GlobalAlloc API ---------------------------+
|                                                                          |
|   alloc(layout)        |--- size <= 1024 B ---> SlabSet (8..1024 B)      |
|   dealloc(ptr,layout)  |                              |                  |
|                        |                              v                  |
|                        |                         BuddyAllocator          |
|                        |--- size  > 1024 B -----------^                  |
+--------------------------------------------------------------------------+
```

* `BuddyAllocator` (`buddy.rs`) — manages the entire heap region. Per-order
  intrusive freelists, split-on-alloc, coalesce-on-free.
* `SlabSet` (`slab.rs`) — eight size-class caches at 8, 16, 32, 64, 128,
  256, 512, 1024 bytes. Each cache pulls a 4-8 KiB slab from buddy on miss
  and slices it into objects.
* `Heap` (`lib.rs`) — routes by size and holds a single `spin::Mutex`
  around both layers. Stats are gated behind a Cargo feature so the hot
  path stays branch-clean in production builds.

## Why a single mutex?

Multiple locks would help on SMP, but rCore is largely uniprocessor-shaped
in chapters 1-8 and the slab layer already eliminates lock-acquire
amortisation by making the critical section a single freelist pop. A
per-class lock can be added later without changing the public API; the
freelists already live behind `&mut SlabCache`.

## O(1) class lookup

Choosing a slab class for `(size, align)` is a hot-path operation. The
naïve approach iterates the class array — eight branches in the worst
case. We instead use the fact that all class sizes are powers of two:

```rust
let need = size.max(align).max(8);
if need > 1024 { return None; }
let idx = need.next_power_of_two().trailing_zeros() as usize - 3;
```

This compiles to a `lzcnt` plus arithmetic, ~1-2 ns. Because every class
size is a power of two and we round up, alignment up to the class size is
satisfied automatically — no `cls % align` check needed.

## Coalescing safety

The classic risk in buddy coalescing is merging buddies whose parent block
was never owned by the allocator (e.g. when the heap region is not
power-of-two aligned). `BuddyAllocator::dealloc` takes an *optimistic*
approach: it walks the freelist at the current order looking for the
exact buddy address. If the buddy is not present we stop. This makes
coalescing safe for any region we can `add_region`, including heaps that
straddle a non-aligned base address.

The cost is an O(N) freelist walk per coalesce attempt, but in steady
state freelists at any single order stay short (< 64 entries on the
benchmark workloads), and the walk happens only on free, not alloc.

## Benchmarks

Three workloads, single-threaded host build, `cargo run --release` from
`allocator-bench/`. Heap = 16 MiB. All allocators are wrapped in a
`spin::Mutex` (`buddy_system_allocator::LockedHeap` and
`spin::Mutex<linked_list_allocator::Heap>`) so the comparison reflects
the kernel's lock-required path.

### Production hot path (stats off)

```
small-lifo  (256-element burst, alloc then LIFO free, 8..1024 B random)
  kalloc                      27.4 ns/op   36.5 Mops/s
  buddy_system (locked)       42.9 ns/op   23.3 Mops/s   (kalloc 1.57x)
  linked_list  (locked)       35.3 ns/op   28.3 Mops/s   (kalloc 1.29x)

mixed-random  (70% small / 25% mid / 5% large, random alloc + free)
  kalloc                     121.5 ns/op    8.2 Mops/s
  buddy_system (locked)      156.6 ns/op    6.4 Mops/s   (kalloc 1.29x)
  linked_list  (locked)      536.2 ns/op    1.9 Mops/s   (kalloc 4.41x)

vec-growth  (Vec<u8> 16,32,64..4096 then drop, x2000)
  kalloc                      27.6 ns/op   36.2 Mops/s
  buddy_system (locked)       23.1 ns/op   43.2 Mops/s   (kalloc 0.84x)
  linked_list  (locked)       35.3 ns/op   28.3 Mops/s   (kalloc 1.28x)
```

`vec-growth` is the one workload where buddy alone wins — the request
sizes are exact powers of two and never escape the buddy fast path, so
the slab-routing decision is pure overhead. Real kernels have far fewer
exact-power-of-two requests than this microbenchmark.

### With instrumentation (stats on)

```
workload        slab hit rate   buddy calls
small-lifo      100.0%          0
mixed-random     70.0%       1635
vec-growth       77.8%       4000
```

`small-lifo` confirms the design intent: once the slab caches are warm,
zero work reaches the buddy. `mixed-random` shows the realistic case —
most requests are absorbed by slab; the remainder (mid/large sizes) go
straight to buddy. Stats overhead is ~30 ns/op of relaxed atomic ops; in
production they should be off, in development on.

## Reproduce

```sh
# Unit tests
( cd allocator && cargo test --features std )
( cd allocator && cargo test --features std,stats )

# Production-mode bench
( cd allocator-bench && cargo run --release )

# Stats-mode bench (slower, prints hit rates)
( cd allocator-bench && cargo run --release --features stats )

# Kernel build (rCore integration)
cp os/src/linker-qemu.ld os/src/linker.ld
( cd os && cargo build --release --target riscv64gc-unknown-none-elf )
```

## End-to-end benchmark via `sys_sbrk` + `heaptest`

Host benchmarks measure the allocator on a synthetic workload. To check
that the win shows up in the actual kernel, we added two syscalls:

| syscall id | name             | semantics                                   |
| ---------- | ---------------- | ------------------------------------------- |
| 214        | `sys_sbrk(d)`    | grow/shrink user heap by `d` bytes (Linux-style) |
| 4000       | `sys_heap_stats(*out)` | copy a snapshot of kernel heap counters out |

`sys_sbrk` walks the kernel allocator hard: every page mapped requires a
`frame_alloc`, a `BTreeMap<VirtPageNum, FrameTracker>` insert (kernel-heap
allocation, slab class 64 B), and zero-or-more `PageTable::map` calls
that may allocate mid-level page-table frames. A loop of `sbrk(+page)`
calls is a clean way to drive a known number of small kernel-heap
allocations from a user program.

`sys_heap_stats` returns:

```rust
struct KernelHeapStats {
    buddy_total, buddy_used,         // bytes managed / in use by buddy
    slab_provisioned, slab_free,     // bytes pulled into slab caches
    allocs, frees,                   // total kernel-heap calls
    bytes_allocated, bytes_freed,
    slab_hits, buddy_calls,          // routing distribution
    oom,
}
```

Take a snapshot before and after a phase and the diff tells you exactly
how much kernel-heap traffic the workload generated and how the slab
absorbed it.

The `heaptest` user program (`user/src/bin/heaptest.rs`) does this in
three phases:

1. Grow heap by 64 pages, touch each page, shrink back, record diff.
2. Grow heap by 256 pages, same.
3. 500 rounds of churn: `sbrk(+8 KiB); sbrk(-8 KiB)`.

Run it from the rCore shell:

```
>> heaptest
```

The output reports kernel allocs/frees, slab hit rate, and average µs
per `sbrk` call — directly comparable across allocator implementations.

## Possible next steps

* **Per-CPU magazines.** Linux SLUB caches one slab per CPU to drop the
  shared lock entirely on the fast path. Worth an experiment once rCore
  enters the SMP chapters.
* **Page-aligned slab refill.** Currently slab refill is `4-8 KiB` of
  whatever buddy gives us. Aligning to 4 KiB and tagging the page lets
  `dealloc` infer the size class from the pointer (Linux's `kfree` trick),
  eliminating the layout argument's role at free time.
* **Adaptive class sizes.** The current 8-class table is uniform powers
  of two. Profiling rCore would reveal request-size hotspots (e.g. 48 B
  for `Arc<TaskControlBlock>`) and let us tune the table for less waste.
