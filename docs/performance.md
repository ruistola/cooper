# Performance and instrumentation

## Garbage collector controls

Environment variables read by every Cooper program's runtime:

| Variable | Effect |
|---|---|
| `COOPER_GC_STATS=1` | At exit, print the number of collections and of stops of the world, the time the world was stopped (in all, sweeping, and the longest pause), how long marking ran and the time summed over the mark workers (assists, with the longest an allocation spent assisting; background workers; idle processors), bytes allocated, peak heap and the most the heap went over its goal, live bytes after the last collection, and memory returned to the system |
| `COOPER_GC=trace` | Print one line per collection: how long marking ran and the time summed over the workers (assists, with the longest; background workers; idle processors), the cycle's stops (how many, the longest, the last with its sweep and scavenging), the heap when marking began, when it ended, and its goal, then live bytes, the next trigger and goal, and heap size |
| `COOPER_GC=stress` | Run a whole collection at every allocation and poison freed memory |
| `COOPER_GC=markstress` | Keep marking on almost all the time (a collection begins at the first allocation after one ends, and each allocation marks a little), and poison freed memory |
| `COOPER_GC=poison` | Fill freed memory with `0xDB`, so a missed root crashes instead of silently reusing memory |
| `COOPER_GC=checkmark` | At the end of every collection, mark again serially and panic on any object the mark missed (combine with a stress mode as `stress,checkmark`; the test harness runs every golden program this way) |
| `COOPER_HEAP_GROWTH=n` or `off` | The heap may grow to `n` percent over what survived the last collection (default 100); `off` disables automatic collection |
| `COOPER_STACK_KIB=n` | Reserve `n` KiB of stack for each thread (default 8192) |
| `COOPER_THREADS=n` | Run threads on `n` processors, each an OS thread while busy (default: one per core); the collector marks on as many |

The pacer follows Go's: the heap may reach the live bytes plus the growth percentage of the live bytes and the
roots (stack and module-level variables), but at least 4 MiB scaled by the percentage. Marking begins early enough
to end at the goal with a quarter of the processors' time, and allocations assist in proportion to their bytes
([Garbage Collector](./garbage-collector.md)).
Statistics cost nothing while allocating: bytes allocated are derived at each collection. After each collection,
free runs of 128 KiB or more are returned to the operating system (`madvise`) while the heap keeps more than a
tenth over its next goal.

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
wall time of three, `-O2`), by the number of processors. The assist is the longest an allocation spent paying
for itself in scan work, during which that thread does no other work:

| Benchmark | Processors | Wall time | Longest pause | Longest assist | Collections |
|---|---|---|---|---|---|
| `live_heap` | 1 | 0.81 s (0.76 s) | 0.3 ms (61 ms) | 0.6 ms | 10 (10) |
| | 4 | 0.54 s (0.55 s) | 0.3 ms (20 ms) | 2.1 ms | 11 (10) |
| | 10 | 0.50 s (0.49 s) | 0.3 ms (12 ms) | 0.7 ms | 12 (10) |
| `scattered` | 1 | 6.49 s (7.69 s) | 0.4 ms (81 ms) | 15 ms | 75 (105) |
| | 4 | 3.10 s (3.58 s) | 0.5 ms (26 ms) | 19 ms | 81 (105) |
| | 10 | 3.16 s (3.14 s) | 0.7 ms (20 ms) | 20 ms | 106 (105) |
| `binary_trees` | 1 | 0.48 s (0.46 s) | 0.08 ms (3.4 ms) | 0.2 ms | 51 (64) |
| | 10 | 0.34 s (0.35 s) | 0.06 ms (0.7 ms) | 0.3 ms | 71 (64) |
| `alloc_parallel` | 1 | 2.54 s (2.42 s) | 0.01 ms (0.3 ms) | 0.02 ms | 1266 (1155) |
| | 4 | 0.90 s (0.87 s) | 0.04 ms (0.12 ms) | 0.1 ms | 1197 (1136) |
| | 10 | 0.80 s (0.76 s) | 0.11 ms (0.14 ms) | 0.3 ms | 1237 (1144) |

The pauses no longer grow with the live heap: what remains in them is the sweep and stopping the threads. These
benchmarks run one thread (except `alloc_parallel`), so with several processors the idle ones do most of the
marking. Objects allocated while marking survive the cycle, which raises the live heap the next goal is computed
from: the peak heap of `live_heap` on one processor is 354 MiB against 260 MiB, and its wall time 7% more. On
`alloc_parallel`, whose eight threads keep a 4 MiB heap that is mostly garbage, collections rise 5% to 10% and wall
time 4% to 5%; unoptimized builds pay 16% to 21%, as marking takes longer against their slower mutator. A large
array's allocation pays its share of the mark at once, which on `scattered` stalls up to 20 ms (34 ms
unoptimized), and its heap reaches up to 20% over the goal; its higher goals on fewer processors mean fewer
collections. Within a stop, every processor marks; marking with the world stopped scaled to 2.9x faster at four processors
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
with four, 0.75 s with ten, measured with the world stopped for marking (with concurrent marking: 2.5 s, 0.90 s
and 0.80 s, above). Beyond four Ps, a collection of the 4 MiB heap every few milliseconds limits the gain; with
collections made rare (`COOPER_HEAP_GROWTH=5000`) eight Ps take 0.68 s.
