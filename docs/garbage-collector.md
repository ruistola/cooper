# Garbage Collector

Memory is reclaimed by a tracing collector modelled on Go's: a span-based allocator, a precise heap,
conservative stacks, and Green Tea marking. It lives in the C runtime (`runtime/gc.c`, with the
descriptor generation in `crates/cooper-llvm/src/gc.rs`). It marks in parallel while the program runs, with
two brief stops of the world, one to begin marking and one to end it, and sweeps while the program runs; each stop
is under 1 ms on the benchmarks. Pause measurements against Go, controls and other measurements are in [Performance](./performance.md).

## Typed allocation

Every heap allocation is `cooper_alloc(size, type)`. `type` is a constant descriptor of one value of the
type allocated, tiled over an array's storage, or null when the allocation holds no pointers (string
bytes, arrays of numbers), which the collector never scans. A descriptor has the type's size, a bitmap with
one bit per 8-byte word that is a pointer whatever the value holds, and a list of *variant sites*.

A sum type's pointer words depend on its variant. Each sum type value with a pointer in some variant is a
site: where its tag and payload are, and a descriptor of each variant's payload (null when that variant has
no pointers). The collector reads the tag and scans only the payload held; payloads may have sites of their
own. A type with sites cannot record its pointers in a span bitmap, so it is allocated with a header word.

The compiler computes each layout as LLVM lays the type out, and the descriptor carries LLVM's own size beside
it; the runtime checks that they agree on every allocation, which validates the computation.

## Allocator

Go's design: 64 MiB arenas of 8 KiB pages grouped into spans; small objects (up to 32 KiB) in slots of one of
Go's 67 size classes, large objects in spans of their own; an arena table finds the span of any address,
and per-span allocation bits with a free index find free slots. A span is one of three kinds: pointer-free,
scannable with its pointer bitmap at the span's end (objects up to 512 bytes), or scannable with a type header
word before each object (larger objects and types with sites). Objects larger than 32 KiB keep their type in
the span.

Each P has an allocation cache holding the span it is allocating from for each class and kind, so the common
allocation takes no lock. Taking a new span (a partly free one, or fresh pages), a large object, and anything else
that changes the span structure take one heap lock. The bytes a P has allocated count toward the collection
threshold when it refills, and the collector adds them when a collection starts; the end of marking empties the
caches, and their spans are swept like the rest.

## Collection

A cycle marks while the program runs, between two brief stops of the world, and then sweeps while the program
runs. The pacer
sets a heap goal after each cycle: the live bytes plus a growth percentage of the live bytes and roots (default
100%, at least 4 MiB). The roots counted are the module-level variables and the stacks of every thread. The
pacer, after Go's, also begins each cycle early enough that marking ends as the heap reaches the goal, with
background workers taking a quarter of the processors' time. It judges that from the bytes allocated per byte of
scan work in the last four cycles (cons/mark, as Go measures it), and begins between seven tenths and nineteen
twentieths of the way from the live bytes to the goal.

- **Stopping.** The allocating thread that reaches the trigger stops every other thread at a safe point:
  generated code polls a flag at each function entry and loop iteration, and the allocator, lock spins and the
  scheduler check it too. A thread stopped in its code saves its registers on its stack, as a switch does. A
  thread in a C call counts as stopped without waiting: it recorded its stack pointer and registers when the call
  began, and waits for the world to restart if the call returns while it is stopped.
- **Beginning.** With the world stopped, the collector turns on the write barrier (below), waits for any C store
  that may have missed it, scans the module-level variables precisely from a table of addresses and descriptors
  the compiler emits (`cooper_roots`), and the timers, and restarts the world.
- **Stacks.** Every thread's stack is scanned once in a cycle, conservatively, word by word, and before the
  thread runs Cooper code again: a thread stopped to begin marking scans its own stack as it resumes, a thread is
  scanned before the scheduler switches to it, and idle processors scan the threads that are waiting, and those in
  C, whose Cooper frames are above the stack pointer they recorded (C changes them only through
  `cooper_store_pointer`). A thread leaving C waits for a scan in progress. A suspended thread's registers are
  saved on its stack, or with the thread record for one in C, and a thread that has not started is reached through
  its closure. Nothing moves, so an ambiguous word only keeps alive what it appears to point into. Cooper never
  turns an integer back into a pointer.
- **Mark.** Green Tea: reaching a small object marks it and queues its span, first in first out; a span's
  marked-but-unscanned objects are scanned together from its mark and scan bits. Larger objects use an object
  work stack, and an object over 128 KiB is scanned in pieces that different workers can take. Interior pointers
  resolve through the span table. Mark and scan bits are set by atomic OR, so each object is marked and scanned
  once.
- **Marking while the program runs.** Each processor has a mark worker, with its own span queue and object stack,
  which publishes surplus work in fixed-size blocks to global lists and steals half of another worker's span
  queue when it runs out. Whatever runs on the processor uses it: the write barrier of the code it runs, an
  allocation that assists, and the background workers. As Go divides a quarter of the processors' time, a quarter
  of the processors (rounded) are dedicated workers, which mark until no work is left; when rounding would be off
  by more than 30% (fewer than four processors), each processor instead has a fractional worker that marks for its
  share of the time. As marking begins, the scheduler takes processors for the dedicated workers, idle ones first,
  and the monitor preempts a thread whose processor's fractional worker is due (it looks every millisecond while
  marking). A processor with no thread to run marks instead of sleeping, until a thread is ready.
- **Assists.** While marking, each allocated byte owes scan work: what is left of the work the last cycle took,
  over what is left of the bytes to the goal. Credit and debt are each thread's, so a thread pays only for what it
  allocates, and one that does not allocate while marking never assists. A thread that runs out of credit pays its
  debt from the scan work the background workers did and nobody claimed, which it keeps claiming as they add to it,
  and by marking itself, at least 64 KiB of scan work at a time; what it pays beyond the debt is credit. An assist
  stops after 0.5 ms and leaves the rest as debt, for later allocations to pay or for the cycle's end to forgive.
  When at most 1 MiB of the expected work is left, an assist does as much of it as it can, which ends the cycle
  sooner. Past the goal each byte owes so much work that every allocation assists for the full 0.5 ms. At the
  limit, a whole runway past the goal, a thread in debt waits until the cycle ends, which bounds the heap; a
  thread that is not in debt goes on. Objects allocated while marking are marked already (allocated black), and
  survive the cycle.
- **Ending.** A worker that finds no work, with no other worker marking, stops the world again. The collector
  gathers the work the workers kept to themselves (mostly the barrier's) and scans the stack of any thread not yet
  scanned. If that finds work, the world restarts and marking goes on (Go's mark termination); otherwise the
  collector turns off the barrier, once no work is left while no C store is shading, so that a store from C after
  that finds what it shades marked already. If no worker has marked for a millisecond, the monitor has one
  processor look for work, so that work only one processor sees, or none at all, still ends the cycle.
- **Sweep.** After Go's, while the program runs. The ending stop leaves every span unswept: its mark bits say
  which objects survived, and its allocation bits still say which were allocated. Sweeping a span makes the marked
  objects the allocated ones, returns the span to a page heap that merges neighbouring free runs if nothing in it
  is live, and otherwise lists it per size class, with free slots or without. An allocation that needs a span
  sweeps spans of its size class until one has a free slot (up to 100) before it takes new pages. Allocations also
  sweep spans of any class in proportion to the bytes they allocate, so that sweeping ends 1 MiB before the heap
  reaches the next trigger, and a processor with no thread to run sweeps until a thread is ready. Marking needs
  every span swept, so the allocation that begins the next cycle sweeps what is left first. `cooper_gc_collect`
  also finishes the sweep. The ending stop sets the live bytes from what the mark workers counted as they marked,
  plus the bytes allocated while marking, which checkmark compares with the mark bits. Once all spans are swept,
  large free runs go back to the operating system while the heap keeps more than a tenth over its next goal.
  Reused memory is zeroed when allocated.
- **Write barrier.** Generated code puts Go's hybrid write barrier before every store that may write a pointer
  word outside the storing function's own frame (fields, array elements, through pointers, module-level
  variables): an inline test of a flag, and while it is set, a runtime call that shades the old and new pointers
  (for a sum type, by the variant held before and the one stored). With stacks scanned once and never again in a
  cycle, the old pointer keeps alive what a thread may have copied to its stack, and the new one what it may store
  from an unscanned stack. A store that initializes an object just allocated, with no safe point since, needs
  none, since the object is allocated black. The runtime's array growth copies with the barrier and applies it
  for the array's new data pointer, in pieces with a safe point between them, and C uses `cooper_store_pointer`.

## Testing

`COOPER_GC=stress` runs a whole cycle at every allocation, and `COOPER_GC=markstress` keeps a cycle on almost
all the time: one begins at the first allocation after the last ended, and each allocation marks a little. Both
poison freed memory. The golden-program harness runs every program again under stress (with checkmark), on four
processors (`COOPER_THREADS=4`), under both, and under markstress on four processors (with checkmark), requiring
the same output.
`COOPER_GC=checkmark` (after Go's `gccheckmark`) marks a second time at the end of every cycle, serially and
object by object into separate bits, and panics naming any object reached that way that the mark missed, its
size and kind, and the object or module-level variable it was reached from. It also panics if the bytes the mark
bits hold differ from the live bytes the workers counted. An unmarked object reached only from
a stack is passed over: a stack scanned early in the cycle may have exposed, by the end, words its frames left
behind before the scan, which can name objects that died. `gc_test.c`, run by `tests/allocator.rs`,
tests the allocator and collector alone: sizes, pointer bitmaps, reuse, interior pointers, reclamation,
poisoning, that a sum type retains only the variant it holds, threads allocating on four Ps during
collections, a live tree and a large pointer array that the four mark workers must share, that the barrier
functions shade the old and new pointers, threads moving pointers around a shared array, and storing them from C,
while the collector marks, and that marking ends while threads keep every processor busy without allocating, all
with checkmark on. It runs natively on four processors and on two (fractional workers only), and under
ThreadSanitizer.
`sched_test.c`, run by `tests/scheduler.rs`, checks that while marking each thread's stack is scanned once and
before the thread runs, whether it was running, waiting, asleep or in C.
The golden harness also checks each program's LLVM IR for a store that may write a pointer outside its frame
without the barrier before it.

## Known limits

Conservative scanning can retain what a stale stack word points at. The scan of a thread's own stack as it begins
a cycle reads the runtime's frames too, words those frames have not written included: on `scattered`, they keep
the previous buffer of the growing array alive for a cycle, a fifth of the live heap. Optimized builds may keep only a derived
pointer (one past an object's end), which a stack scan would miss: not yet validated. The page heap is a
first-fit list. Refills, large allocations and sweeping take one heap lock, and one processor at a time sweeps
when idle. An allocation that takes the heap a whole runway past its goal, such as
growing a large array while every allocation survives, makes the allocating thread wait for the cycle to end (25
to 53 ms on `scattered` with ten processors, 22 ms with one).
Objects allocated black survive the cycle, so a program whose heap is mostly garbage collects more often than its
live bytes alone would need. The barrier's slow path marks directly rather than buffering. Waking the helpers costs about 40 µs per stop with ten processors, which
shows only when collections are small and frequent.
