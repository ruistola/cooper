# Performance and instrumentation

## Pauses

The collector marks while the program runs and stops the world twice per collection: once to begin marking, and
once to end it. Sweeping follows while the program runs. On the benchmarks below (`-O2`, ten processors, Apple M5, ranges over three runs) the
longest stop is under 0.25 ms. Go's stops (measured with `GODEBUG=gctrace=1`, the longer of a cycle's two) are
slightly shorter on the tree benchmarks, about as long on `strings`, and longer on `scattered`:

| Benchmark | Live heap | Cooper longest stop | Go longest stop | Cooper longest assist |
|---|---|---|---|---|
| `binary_trees` | 10 MiB | 0.04 to 0.05 ms | 0.03 ms | 0.3 to 0.6 ms |
| `live_heap` | 140 MiB | 0.04 ms | 0.02 to 0.03 ms | 0.5 ms |
| `strings` | 1.4 MiB | 0.04 to 0.09 ms | 0.08 to 0.14 ms | 0.12 to 0.14 ms |
| `scattered` | 65 MiB | 0.12 to 0.21 ms | 0.6 to 1.2 ms | 25 to 53 ms (while building the graph), 0.6 to 1.8 ms after |

With one processor the stops are about as long. A thread is held up for longer by an assist: an allocation made
while marking pays for itself in marking work. The cost lands only on the thread that allocates, in proportion to
what it allocates, and one assist stops after 0.5 ms, leaving the rest as debt. A thread that does not allocate
while marking never assists. The exception is an allocation that takes the heap a whole runway past its goal: the
thread then waits for the cycle to end. `scattered` does this while it builds its graph, when growing its array
of 3M nodes takes the heap past the limit (25 to 53 ms with ten processors, 22 ms with one). Those cycles also
mark the array's previous buffer, which stale words in the runtime's stack frames point at (conservative stack
scanning), so they mark about a fifth more than is live and take that much longer.

The wall times below include marking. With one program thread on ten processors, most marking runs on idle
processors, so the wall time does not show its CPU cost. The CPU time is close to Go's (user time on `-O2`:
`binary_trees` 0.75 s for both, `live_heap` 1.37 s against 1.22 s, `scattered` 17 s against 29 s). A program
that keeps every processor busy gives a quarter of the processors' time to marking while a cycle runs.

## Garbage collector controls

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
resident memory. Every implementation prints the same output. The compiler builds unoptimized by default, and
most of the difference to Go is that: built with `-O2`, Cooper takes 1.2x Go's time on the tree benchmarks, 2x
on `strings`, and 0.6x on `scattered`.

Apple M5 with ten cores, Go 1.27, and Cooper with one processor per core:

| Benchmark | Cooper (unoptimized) | Cooper (`-O2`) | Go |
|---|---|---|---|
| `binary_trees`: 33M short-lived 16-byte nodes, one 8 MB long-lived tree | 0.98 s, 22 MiB | 0.35 s, 22 MiB | 0.29 s, 28 MiB |
| `live_heap`: 128 MB live tree, 50M short-lived allocations | 1.39 s, 262 MiB | 0.50 s, 265 MiB | 0.43 s, 278 MiB |
| `strings`: building, joining, and splitting 1.2M small strings | 0.50 s, 9 MiB | 0.18 s, 7 MiB | 0.09 s, 10 MiB |
| `scattered`: 3M-node random graph, 120M short-lived allocations | 5.54 s, 341 MiB | 2.58 s, 186 MiB | 4.11 s, 228 MiB |

Marking works a span at a time, after Go's Green Tea collector ([Garbage Collector](./garbage-collector.md)). On
`scattered`, where tracing jumps around memory, this makes marking 2.6x faster than scanning object by object. On
the tree benchmarks a collection costs about 6 ns per object, the cost of chasing pointers across a heap bigger
than the cache. Objects allocated while marking survive the cycle, so a heap that is mostly garbage is
collected more often than its live bytes alone would need.

The write barrier, while its flag is off (outside marking), costs no more than the timer's 0.01 s on the `-O2`
benchmarks (best of five runs against the build without the barrier). Unoptimized builds pay for the inline flag
test: 12% on `alloc_parallel`, which stores two pointers into every tree node it allocates, and 3% or less on the
others.
Pushing into spare capacity writes only the array's length, and a store that initializes a fresh object needs no
barrier, so neither pays for one. Because stacks are scanned conservatively, extra frame slots can retain stale pointers and change the
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
cores; Go's version is not written). Each P allocates from its own spans without a lock: 2.5 s with one P, 0.90 s
with four, 0.80 s with ten. Beyond four Ps, a collection of the 4 MiB heap every few milliseconds limits the gain; with
collections made rare (`COOPER_HEAP_GROWTH=5000`) eight Ps take 0.68 s.
