# Performance and instrumentation

## Garbage collector controls

Environment variables read by every Cooper program's runtime:

| Variable | Effect |
|---|---|
| `COOPER_GC_STATS=1` | At exit, print the number of collections, time spent marking and sweeping, the longest pause, allocations, bytes allocated, peak heap, live bytes after the last collection, and memory returned to the system |
| `COOPER_GC=trace` | Print one line per collection: mark and sweep time, live bytes, the next collection's trigger, heap size |
| `COOPER_GC=stress` | Collect at every allocation and poison freed memory (the test harness runs every golden program this way) |
| `COOPER_GC=poison` | Fill freed memory with `0xDB`, so a missed root crashes instead of silently reusing memory |
| `COOPER_GOGC=n` or `off` | The heap may grow to `n` percent over what survived the last collection (default 100, as in Go); `off` disables automatic collection |

The pacer follows Go's goal: the next collection starts at the live bytes plus `GOGC` percent of the live
bytes and the roots (stack and module-level variables), but at least 4 MiB scaled by `GOGC`. After each
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

Where the time goes (`COOPER_GC_STATS=1`, `-O2`): in `binary_trees` 63 collections take 227 ms of 440 ms, almost
all of it marking; in `live_heap` 10 collections take 330 ms, marking 8.4M live objects at about 40 ns each, the
cost of chasing pointers across the heap that Green Tea's span-at-a-time scanning is meant to reduce. Sweeping is
under 1% of collection time, so lazy sweeping would gain nothing yet.
