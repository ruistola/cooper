# Performance and instrumentation

## Garbage collector controls

Environment variables read by every Cooper program's runtime:

| Variable | Effect |
|---|---|
| `COOPER_GC_STATS=1` | At exit, print the number of collections, time spent marking (wall time, and summed over the mark workers) and sweeping, the longest pause, bytes allocated, peak heap, live bytes after the last collection, and memory returned to the system |
| `COOPER_GC=trace` | Print one line per collection: mark time (wall time, summed over the workers, and their number), sweep time, live bytes, the next collection's trigger, heap size |
| `COOPER_GC=stress` | Collect at every allocation and poison freed memory (the test harness runs every golden program this way) |
| `COOPER_GC=poison` | Fill freed memory with `0xDB`, so a missed root crashes instead of silently reusing memory |
| `COOPER_GC=checkmark` | After every mark, mark again serially and panic on any object the mark missed (combine with `stress` as `stress,checkmark`) |
| `COOPER_HEAP_GROWTH=n` or `off` | The heap may grow to `n` percent over what survived the last collection (default 100); `off` disables automatic collection |
| `COOPER_STACK_KIB=n` | Reserve `n` KiB of stack for each thread (default 8192) |
| `COOPER_THREADS=n` | Run threads on `n` processors, each an OS thread while busy (default: one per core); the collector marks on as many |

The pacer follows Go's goal: the next collection starts at the live bytes plus the growth percentage of the live
bytes and the roots (stack and module-level variables), but at least 4 MiB scaled by the percentage.
Statistics cost nothing while allocating: bytes allocated are derived at each collection, and the clock is
read only when statistics or tracing are on. After each
collection, free runs of 128 KiB or more are returned to the operating system (`madvise`).

## Benchmarks

`benchmarks/run.sh [name ...]` builds and times each benchmark in Cooper (as built by default, and with
`clang -O2`), Go, and Java when a JDK is installed, reporting the best of three wall times and the peak
resident memory. Every implementation prints the same output.

Apple M5 with ten cores, Go 1.27 (with its concurrent collector on spare cores), Cooper with one processor per
core and its stop-the-world collector marking on all of them:

| Benchmark | Cooper (default) | Cooper (`-O2`) | Go |
|---|---|---|---|
| `binary_trees`: 33M short-lived 16-byte nodes, one 8 MB long-lived tree | 0.93 s, 22 MiB | 0.34 s, 23 MiB | 0.29 s, 28 MiB |
| `live_heap`: 128 MB live tree, 50M short-lived allocations | 1.33 s, 273 MiB | 0.49 s, 273 MiB | 0.44 s, 273 MiB |
| `strings`: building, joining, and splitting 1.2M small strings | 0.47 s, 8 MiB | 0.18 s, 7 MiB | 0.09 s, 10 MiB |
| `scattered`: 3M-node random graph, 120M short-lived allocations | 5.37 s, 234 MiB | 3.02 s, 163 MiB | 4.10 s, 232 MiB |

Marking works a span at a time, after Go's Green Tea collector: reaching a small object marks it and queues its
span (first in, first out), and a span's marked-but-unscanned objects are scanned together from the span's bit
arrays. On `scattered`, where tracing jumps around memory, this makes marking 2.6x faster than scanning object
by object (7.2 s against 18.7 s over 105 collections, on one processor). On the tree benchmarks, whose nodes
are allocated in the order they are traced, it makes no difference, since a collection already costs about
6 ns per object there, the cost of chasing pointers across a heap bigger than the cache.

Mark time over all collections (`COOPER_GC_STATS=1`, `-O2`) by the number of processors:

| Benchmark | 1 | 4 | 10 | Longest pause at 10 (at 1) |
|---|---|---|---|---|
| `binary_trees` (63 collections) | 183 ms | 54 ms | 36 ms | 0.7 ms (3.7 ms) |
| `live_heap` (10 collections) | 351 ms | 113 ms | 69 ms | 12 ms (60 ms) |
| `scattered` (105 collections) | 6.5 s | 2.2 s | 1.9 s | 20 ms (82 ms) |
| `alloc_parallel` (1140 collections of a 4 MiB heap) | 183 ms | 80 ms | 76 ms | 0.1 ms (0.3 ms) |

With one processor the mark takes as long as it did before it was parallel, except in the default build of
`scattered`, where the runtime is compiled unoptimized too: there it takes 6% longer. `scattered` gains little
beyond four processors (2.2 s at four, 1.9 s at ten); its marking is bound by memory latency. With ten
processors the time summed over the workers is 5 to 10 times the wall time, much of it idle workers waiting for
work. Sweeping is serial: under 1% of collection time with one processor, but 17% on `alloc_parallel` with ten,
whose collections mark little.

## Threads

`benchmarks/threads.coop` (with a Go version, run with one OS thread) measures the scheduler. Cooper `-O2` with
`COOPER_THREADS=1`, Go 1.27, Apple M5:

| Measure | Cooper | Go |
|---|---|---|
| 5M switches between two threads | 0.38 s (76 ns each) | included below |
| 1M short-lived threads, in bursts of 1000 | 0.41 s (0.4 µs each) | included below |
| 100k threads alive at once, each holding a stack | 0.6 s, 1.65 GB | included below |
| All three | 1.0 s | 0.52 s |

The memory is the cost of the design: a stack is at least one page, 16 KiB on this machine, against Go's 2 KiB.
Time is mostly the kernel zero-filling those pages. With one P per core (ten here) all three take 0.74 s.

The safe-point polls the collector needs (a load and a branch at every function entry and loop iteration) cost
little where the work is in calls, and most in tight loops they keep LLVM from rewriting: `strings` at `-O2` takes
0.19 s with them and 0.15 s without.

`benchmarks/alloc_parallel.coop` has 8 threads each allocate 33M short-lived 16-byte nodes (`-O2`, Apple M5, ten
cores; Go's version is not written). Each P allocates from its own spans without a lock: 2.4 s with one P, 0.89 s
with four, 0.75 s with ten. Beyond four Ps, stopping the world for a collection of the 4 MiB heap every few
milliseconds limits the gain; with collections made rare (`COOPER_HEAP_GROWTH=5000`) eight Ps take 0.68 s.
