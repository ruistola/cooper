# Performance and instrumentation

## Garbage collector controls

Environment variables read by every Cooper program's runtime:

| Variable | Effect |
|---|---|
| `COOPER_GC_STATS=1` | At exit, print the number of collections, time spent marking and sweeping, the longest pause, allocations, bytes allocated, peak heap, live bytes after the last collection, and memory returned to the system |
| `COOPER_GC=trace` | Print one line per collection: mark and sweep time, live bytes, the next collection's trigger, heap size |
| `COOPER_GC=stress` | Collect at every allocation and poison freed memory (the test harness runs every golden program this way) |
| `COOPER_GC=poison` | Fill freed memory with `0xDB`, so a missed root crashes instead of silently reusing memory |
| `COOPER_HEAP_GROWTH=n` or `off` | The heap may grow to `n` percent over what survived the last collection (default 100); `off` disables automatic collection |
| `COOPER_STACK_KIB=n` | Reserve `n` KiB of stack for each task (default 8192) |
| `COOPER_GC=objects` | Mark object by object as they are reached, instead of a span at a time: for comparing the two |

The pacer follows Go's goal: the next collection starts at the live bytes plus the growth percentage of the live
bytes and the roots (stack and module-level variables), but at least 4 MiB scaled by the percentage.
Statistics cost nothing while allocating: bytes allocated are derived at each collection, and the clock is
read only when statistics or tracing are on. After each
collection, free runs of 128 KiB or more are returned to the operating system (`madvise`).

## Benchmarks

`benchmarks/run.sh [name ...]` builds and times each benchmark in Cooper (as built by default, and with
`clang -O2`), Go, and Java when a JDK is installed, reporting the best of three wall times and the peak
resident memory. Every implementation prints the same output.

Baseline, Apple M5, Go 1.26 (with its concurrent collector on spare cores), Cooper's collector stop-the-world
and single-threaded:

| Benchmark | Cooper (default) | Cooper (`-O2`) | Go |
|---|---|---|---|
| `binary_trees`: 33M short-lived 16-byte nodes, one 8 MB long-lived tree | 1.31 s, 22 MiB | 0.44 s, 21 MiB | 0.29 s, 28 MiB |
| `live_heap`: 128 MB live tree, 50M short-lived allocations | 1.94 s, 269 MiB | 0.65 s, 269 MiB | 0.43 s, 275 MiB |
| `strings`: building, joining, and splitting 1.2M small strings | 0.43 s, 8 MiB | 0.15 s, 7 MiB | 0.08 s, 10 MiB |

Marking works a span at a time, after Go's Green Tea collector: reaching a small object marks it and queues its
span (first in, first out), and a span's marked-but-unscanned objects are scanned together from the span's bit
arrays. On `scattered`, a random graph of 3M nodes where tracing jumps around memory, this makes marking 2.6x
faster than scanning object by object (7.2 s against 18.7 s over 105 collections). On the tree benchmarks,
whose nodes are allocated in the order they are traced, it makes no difference, since a collection already
costs about 6 ns per object there. Cooper's mark is single-threaded; Go's runs on other cores concurrently
(on `scattered` Go takes 4.2 s of wall time and 29 s of CPU, Cooper 8.0 s of wall time and CPU).

| Benchmark | Cooper (`-O2`) | Go |
|---|---|---|
| `scattered`: 3M-node random graph, 120M short-lived allocations | 8.03 s, 173 MiB | 4.21 s, 249 MiB |

Where the time goes (`COOPER_GC_STATS=1`, `-O2`): in `binary_trees` 63 collections take 227 ms of 440 ms, almost
all of it marking; in `live_heap` 10 collections take 330 ms, marking 8.4M live objects at about 6 ns each, the
cost of chasing pointers across a heap bigger than the cache. Sweeping is
under 1% of collection time, so lazy sweeping would gain nothing yet.

## Tasks

`benchmarks/tasks.coop` (with a Go version, run with one thread) measures the scheduler. Cooper `-O2`, Go 1.27,
Apple M5:

| Measure | Cooper | Go |
|---|---|---|
| 5M switches between two tasks | 0.38 s (76 ns each) | included below |
| 1M short-lived tasks, in bursts of 1000 | 0.41 s (0.4 µs each) | included below |
| 100k tasks alive at once, each holding a stack | 0.6 s, 1.65 GB | included below |
| All three | 1.0 s | 0.52 s |

The memory is the cost of the design: a stack is at least one page, 16 KiB on this machine, against Go's 2 KiB.
Time is mostly the kernel zero-filling those pages.
