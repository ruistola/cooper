# Performance and instrumentation

## Pauses

The collector marks while the program runs and stops the world twice per collection: once to begin marking, and
once to end it. Sweeping follows while the program runs. On the benchmarks below (`--release`, ten processors, Apple M5;
stops range over ten runs, assists over three) the longest stop is under 0.2 ms. Go's stops (measured with
`GODEBUG=gctrace=1`, the longer of a cycle's two) are shorter on the tree benchmarks and `strings`, and longer on
`scattered`:

| Benchmark | Live heap | Cooper longest stop | Go longest stop | Cooper longest assist |
|---|---|---|---|---|
| `binary_trees` | 10 MiB | 0.04 to 0.05 ms | 0.02 ms | 0.3 ms |
| `live_heap` | 140 MiB | 0.03 to 0.04 ms | 0.02 ms | 0.5 ms |
| `strings` | 1.2 MiB | 0.02 to 0.06 ms | 0.02 ms | 0.11 to 0.13 ms |
| `scattered` | 80 MiB | 0.10 to 0.14 ms | 0.7 to 1.4 ms | 9 to 11 ms (while building the graph), 1.3 to 1.6 ms after |

In one run in ten, Go's longest stop on `binary_trees` and `strings` was 0.13 to 0.15 ms. With one processor the
stops are shorter (0.01 ms on `scattered`). A thread is held up for longer by an assist: an allocation made
while marking pays for itself in marking work. The cost lands only on the thread that allocates, in proportion to
what it allocates, and one assist stops after 0.5 ms, leaving the rest as debt. A thread that does not allocate
while marking never assists. The exception is an allocation that takes the heap a whole runway past its goal: the
thread then waits for the cycle to end. `scattered` does this while it builds its graph, when growing its array
of 3M nodes takes the heap past the limit (9 to 11 ms with ten processors, 17 to 20 ms with one).

The wall times below include marking. With one program thread on ten processors, most marking runs on idle
processors, so the wall time does not show its CPU cost. The CPU time is close to Go's (user time with `--release`:
`binary_trees` 0.75 s against 0.74 s, `live_heap` 1.38 s against 1.26 s, `scattered` 17 s against 29 s). A program
that keeps every processor busy gives a quarter of the processors' time to marking while a cycle runs.

## Garbage collector controls

A program controls the collector with `std.gc`, after Go's `runtime.GC` and `debug.SetGCPercent`: `collect()`
collects now, `setGrowth(Some(n))` sets the growth percentage, `setGrowth(None)` turns automatic collection off
(`collect` still works), each giving the previous setting, and `stats()` gives the collections, the stops of the
world and their time, and the bytes allocated, live, and in the heap. A game, for one, can turn collection off
during a level and collect at a transition.

Environment variables read by every Cooper program's runtime:

| Variable | Effect |
|---|---|
| `COOPER_GC_STATS=1` | At exit, print the number of collections and of stops of the world, the time the world was stopped (in all, and the longest pause), the time spent sweeping, how long marking ran and the time summed over the mark workers (assists, with the longest an allocation spent assisting; background workers; idle processors), bytes allocated, peak heap and the most the heap went over its goal, live bytes after the last collection, and memory returned to the system |
| `COOPER_GC=trace` | Print one line per collection: how long marking ran and the time summed over the workers (assists, with the longest; background workers; idle processors), the cycle's stops (how many, the longest, the last), the heap when marking began, when it ended, and its goal, then live bytes, the next trigger and goal, and heap size |
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
Statistics cost nothing while allocating: bytes allocated are derived at each collection. Once a collection's
sweep is done, free runs of 128 KiB or more are returned to the operating system (`madvise`) while the heap keeps
more than a tenth over its next goal.

## Benchmarks

`benchmarks/run.sh [name ...]` builds and times each benchmark in Cooper (as built by default, and with
`--release`, which compiles it with `clang -O2`), Go, and Java when a JDK is installed, reporting the best of three wall times and the peak
resident memory. Every implementation prints the same output. The runtime is always optimized, so an unoptimized
build pays only for its own code: 1.2x to 2x the time of `--release`. With `--release`, Cooper takes 1.1x to 1.2x
Go's time on the tree benchmarks, 2x on `strings`, and 0.6x on `scattered`. The unoptimized `scattered` is faster
than its `--release` build: its frames keep more stale pointers, so more survives each collection (108 to 151 MiB
against 82 to 85 MiB at the end), the heap goal grows with it, and it collects 54 to 61 times instead of 84 or 85,
with a peak heap of 290 to 310 MiB instead of 170 to 180 MiB.

Apple M5 with ten cores, Go 1.27, and Cooper with one processor per core:

| Benchmark | Cooper (unoptimized) | Cooper (`--release`) | Go |
|---|---|---|---|
| `binary_trees`: 33M short-lived 16-byte nodes, one 8 MB long-lived tree | 0.43 s, 23 MiB | 0.34 s, 22 MiB | 0.30 s, 28 MiB |
| `live_heap`: 128 MB live tree, 50M short-lived allocations | 0.62 s, 265 MiB | 0.50 s, 263 MiB | 0.43 s, 276 MiB |
| `strings`: building, joining, and splitting 1.2M small strings | 0.35 s, 10 MiB | 0.18 s, 8 MiB | 0.09 s, 10 MiB |
| `scattered`: 3M-node random graph, 120M short-lived allocations | 2.07 s, 305 MiB | 2.55 s, 188 MiB | 4.17 s, 195 MiB |

Marking works a span at a time, after Go's Green Tea collector ([Garbage Collector](./garbage-collector.md)). On
`scattered`, where tracing jumps around memory, this makes marking 2.6x faster than scanning object by object. On
the tree benchmarks a collection costs about 6 ns per object, the cost of chasing pointers across a heap bigger
than the cache. Objects allocated while marking survive the cycle, so a heap that is mostly garbage is
collected more often than its live bytes alone would need.

The write barrier costs little with `--release` (best of five alternating runs against a build without it): nothing
measurable on `binary_trees`, `strings` and `alloc_parallel`, and 4% on `live_heap` and `scattered`. That includes the
shading the barrier does while marking runs (three fifths of the time on `scattered`), and code layout alone moves
times by a few percent. Unoptimized builds pay for the inline flag test: 2 to 5% on every benchmark, 4% on
`alloc_parallel`, which stores two pointers into every tree node it allocates.
Pushing into spare capacity writes only the array's length, and a store that initializes a fresh object needs no
barrier, so neither pays for one. Because stacks are scanned conservatively, extra frame slots can retain stale pointers and change the
number of collections noticeably.

## Threads

`benchmarks/threads.coop` (with a Go version, run with one OS thread) measures the scheduler. Cooper `--release` with
`COOPER_THREADS=1`, Go 1.27, Apple M5:

| Measure | Cooper | Go |
|---|---|---|
| 5M switches between two threads | 0.11 s (22 ns each) | included below |
| 1M short-lived threads, in bursts of 1000 | 0.09 s (0.09 µs each) | included below |
| 100k threads alive at once, each holding a stack | 0.67 s, 1.65 GB | included below |
| All three | 0.88 s | 0.51 s |

The memory is the cost of the design: a stack is at least one page, 16 KiB on this machine, against Go's 2 KiB.
Time is mostly the kernel zero-filling those pages. With one P per core (ten here) all three take 0.72 s.

The safe-point polls the collector needs (a load and a branch at every function entry and loop iteration) cost
little where the work is in calls, and most in tight loops they keep LLVM from rewriting: `strings` with `--release` takes
0.18 s with them and 0.17 s without.

`benchmarks/alloc_parallel.coop` has 8 threads each allocate 33M short-lived 16-byte nodes (`--release`, Apple M5, ten
cores; Go's version is not written). Each P allocates from its own spans without a lock: 2.5 s with one P, 0.81 s
with four, 0.74 s with ten. Beyond four Ps, a collection of the 4 MiB heap every few milliseconds limits the gain; with
collections made rare (`COOPER_HEAP_GROWTH=5000`) eight Ps take 0.57 s.
