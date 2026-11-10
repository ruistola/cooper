# Performance and instrumentation

## Garbage collector controls

Environment variables read by every Cooper program's runtime:

| Variable | Effect |
|---|---|
| `COOPER_GC_STATS=1` | At exit, print the number of collections, the time the world was stopped (in all, sweeping, and the longest pause), how long marking ran and the time summed over the mark workers, bytes allocated, peak heap, live bytes after the last collection, and memory returned to the system |
| `COOPER_GC=trace` | Print one line per collection: how long marking ran and the time summed over the workers, the ending pause and the sweep in it, live bytes, the next heap goal, heap size |
| `COOPER_GC=stress` | Run a whole collection at every allocation and poison freed memory |
| `COOPER_GC=markstress` | Keep marking on almost all the time (a collection begins at the first allocation after one ends, and each allocation marks a little), and poison freed memory |
| `COOPER_GC=poison` | Fill freed memory with `0xDB`, so a missed root crashes instead of silently reusing memory |
| `COOPER_GC=checkmark` | At the end of every collection, mark again serially and panic on any object the mark missed (combine with a stress mode as `stress,checkmark`; the test harness runs every golden program this way) |
| `COOPER_HEAP_GROWTH=n` or `off` | The heap may grow to `n` percent over what survived the last collection (default 100); `off` disables automatic collection |
| `COOPER_STACK_KIB=n` | Reserve `n` KiB of stack for each thread (default 8192) |
| `COOPER_THREADS=n` | Run threads on `n` processors, each an OS thread while busy (default: one per core); the collector marks on as many |

The pacer follows Go's goal: the heap may reach the live bytes plus the growth percentage of the live bytes and
the roots (stack and module-level variables), but at least 4 MiB scaled by the percentage. Marking begins seven
tenths of the way there from the live bytes, and an allocation that reaches the goal while marking helps until
the collection ends.
Statistics cost nothing while allocating: bytes allocated are derived at each collection, and the clock is
read only when statistics or tracing are on. After each
collection, free runs of 128 KiB or more are returned to the operating system (`madvise`).

## Benchmarks

`benchmarks/run.sh [name ...]` builds and times each benchmark in Cooper (as built by default, and with
`clang -O2`), Go, and Java when a JDK is installed, reporting the best of three wall times and the peak
resident memory. Every implementation prints the same output.

Apple M5 with ten cores, Go 1.27, and Cooper with one processor per core, both collectors marking concurrently on
the cores the program leaves idle:

| Benchmark | Cooper (default) | Cooper (`-O2`) | Go |
|---|---|---|---|
| `binary_trees`: 33M short-lived 16-byte nodes, one 8 MB long-lived tree | 0.98 s, 22 MiB | 0.34 s, 22 MiB | 0.28 s, 28 MiB |
| `live_heap`: 128 MB live tree, 50M short-lived allocations | 1.47 s, 258 MiB | 0.50 s, 274 MiB | 0.42 s, 278 MiB |
| `strings`: building, joining, and splitting 1.2M small strings | 0.46 s, 8 MiB | 0.17 s, 7 MiB | 0.09 s, 10 MiB |
| `scattered`: 3M-node random graph, 120M short-lived allocations | 5.86 s, 309 MiB | 2.41 s, 187 MiB | 4.16 s, 232 MiB |

Marking works a span at a time, after Go's Green Tea collector: reaching a small object marks it and queues its
span (first in, first out), and a span's marked-but-unscanned objects are scanned together from the span's bit
arrays. On `scattered`, where tracing jumps around memory, this makes marking 2.6x faster than scanning object
by object (7.2 s against 18.7 s over 105 collections, on one processor). On the tree benchmarks, whose nodes
are allocated in the order they are traced, it makes no difference, since a collection already costs about
6 ns per object there, the cost of chasing pointers across a heap bigger than the cache.

Concurrent marking against marking with the world stopped (the same collector otherwise, interleaved runs, best
wall time of three, `-O2`), by the number of processors. The stall is the longest an allocation spent helping
to finish a cycle that reached its heap goal, during which that thread does no other work:

| Benchmark | Processors | Wall time | Longest pause | Longest stall | Collections |
|---|---|---|---|---|---|
| `live_heap` | 1 | 0.74 s (0.76 s) | 1.2 ms (60 ms) | 62 ms | 9 (10) |
| | 4 | 0.52 s (0.54 s) | 1.2 ms (20 ms) | 7.5 ms | 10 (10) |
| | 10 | 0.49 s (0.49 s) | 0.8 ms (12 ms) | none | 12 (10) |
| `scattered` | 1 | 6.37 s (7.63 s) | 0.7 ms (82 ms) | 86 ms | 73 (105) |
| | 4 | 3.06 s (3.64 s) | 0.8 ms (26 ms) | 21 ms | 73 (105) |
| | 10 | 2.52 s (3.16 s) | 0.8 ms (21 ms) | 16 ms | 73 (105) |
| `binary_trees` | 1 | 0.48 s (0.46 s) | 0.1 ms (3.4 ms) | 6.7 ms | 45 (64) |
| | 10 | 0.36 s (0.34 s) | 0.1 ms (0.7 ms) | none | 75 (64) |
| `alloc_parallel` | 1 | 2.87 s (2.41 s) | 0.02 ms (0.3 ms) | | 1489 (1142) |
| | 4 | 1.13 s (0.95 s) | 0.06 ms (0.15 ms) | | 1484 (1137) |
| | 10 | 0.96 s (0.78 s) | 0.08 ms (0.26 ms) | | 1487 (1146) |

The pauses no longer grow with the live heap: what remains in them is the sweep and stopping the threads. These
benchmarks run one thread (except `alloc_parallel`), so with several processors the idle ones do most of the
marking. With one processor nothing marks until an allocation reaches the goal and finishes the whole mark, so
the program stalls about as long as the stop-the-world pause did. Objects allocated while marking survive the
cycle, which raises the live heap the next goal is computed from: the peak heap of `live_heap` on one processor
is 362 MiB against 260 MiB. On `alloc_parallel`, whose eight threads keep a 4 MiB heap that is mostly garbage,
this means 30% more collections, each with two stops instead of one, and 19% to 23% more wall time (35% in the
default build). `scattered` gains from the same effect: its higher goals mean 73 collections instead of 105.
Within a stop, every processor marks; marking with the world stopped scaled to 2.9x faster at four processors
and 3.4x at ten on `scattered`, bound by memory latency beyond four.

The write barrier, while its flag is off (outside marking), costs no more than the timer's 0.01 s on the `-O2`
benchmarks (best of five runs against the build without the barrier). Unoptimized builds pay for the inline flag
test: 12% on `alloc_parallel`, which stores two pointers into every tree node it allocates, and 3% or less on the
others.
Pushing into spare capacity writes only the array's length, and a store that initializes a fresh object needs no
barrier, so neither pays for one. The barrier's slow path passes the runtime values rather than the address stored to, which
keeps an inlined local in registers, and shades strings, arrays and closures word by word rather than from copies
on the stack. Because stacks are scanned conservatively, extra frame slots can retain stale pointers and change the
number of collections noticeably.

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
with four, 0.75 s with ten, measured with the world stopped for marking (with concurrent marking: 2.9 s, 1.1 s
and 0.96 s, above). Beyond four Ps, a collection of the 4 MiB heap every few milliseconds limits the gain; with
collections made rare (`COOPER_HEAP_GROWTH=5000`) eight Ps take 0.68 s.
