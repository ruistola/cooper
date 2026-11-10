# Garbage Collector

Memory is reclaimed by a tracing collector modelled on Go's: a span-based allocator, a precise heap,
conservative stacks, and Green Tea marking. It lives in the C runtime (`runtime/gc.c`, with the
descriptor generation in `crates/cooper-llvm/src/gc.rs`). It marks in parallel while the program runs, with
two brief stops of the world, one to begin marking and one to end it and sweep. Controls and measurements are in
[Performance](./performance.md).

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
threshold when it refills, and the collector adds them when a collection starts; the sweep then returns the
cached spans to the lists.

## Collection

A cycle marks while the program runs, between two brief stops of the world, and sweeps in the second. The pacer
sets a heap goal after each cycle: the live bytes plus a growth percentage of the live bytes and roots (default
100%, at least 4 MiB). The roots counted are the module-level variables and the stacks of every thread. A cycle
begins when the allocated bytes are seven tenths of the way from the live bytes to the goal.

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
  allocation that helps, and the processor's idle time, which the scheduler spends marking instead of sleeping
  until a thread is ready to run. An allocation that reaches the heap goal while marking helps until the cycle
  ends. Objects allocated while marking are marked already (allocated black), and survive the cycle.
- **Ending.** A worker that finds no work, with no other worker marking and every stack scanned, stops the world
  again. The collector gathers the work the workers kept to themselves (mostly the barrier's), scans the stack of
  any thread not yet scanned, and if work is left, marks it on every processor: the threads stopped at safe
  points and idle OS threads for the remaining processors. It turns off the barrier only once no work is left
  while no C store is shading, so that a store from C after that finds what it shades marked already.
- **Sweep.** With the world still stopped, the marked objects become the allocated ones. Spans with nothing live
  return to a page heap that merges neighbouring free runs, large free runs go back to the operating system, and
  spans with free slots are listed per size class. Reused memory is zeroed when allocated.
- **Write barrier.** Generated code puts Go's hybrid write barrier before every store that may write a pointer
  word outside the storing function's own frame (fields, array elements, through pointers, module-level
  variables): an inline test of a flag, and while it is set, a runtime call that shades the old and new pointers
  (for a sum type, by the variant held before and the one stored). With stacks scanned once and never again in a
  cycle, the old pointer keeps alive what a thread may have copied to its stack, and the new one what it may store
  from an unscanned stack. A store that initializes an object just allocated, with no safe point since, needs
  none, since the object is allocated black. The runtime's array growth copies with the barrier and applies it
  for the array's new data pointer, and C uses `cooper_store_pointer`.

## Testing

`COOPER_GC=stress` runs a whole cycle at every allocation, and `COOPER_GC=markstress` keeps a cycle on almost
all the time: one begins at the first allocation after the last ended, and each allocation marks a little. Both
poison freed memory. The golden-program harness runs every program again under stress (with checkmark), on four
processors (`COOPER_THREADS=4`), under both, and under markstress on four processors (with checkmark), requiring
the same output.
`COOPER_GC=checkmark` (after Go's `gccheckmark`) marks a second time at the end of every cycle, serially and
object by object into separate bits, and panics naming any object reached that way that the mark missed, its
size and kind, and the object or module-level variable it was reached from. An unmarked object reached only from
a stack is passed over: a stack scanned early in the cycle may have exposed, by the end, words its frames left
behind before the scan, which can name objects that died. `gc_test.c`, run by `tests/allocator.rs`,
tests the allocator and collector alone: sizes, pointer bitmaps, reuse, interior pointers, reclamation,
poisoning, that a sum type retains only the variant it holds, threads allocating on four Ps during
collections, a live tree and a large pointer array that the four mark workers must share, that the barrier
functions shade the old and new pointers, and threads moving pointers around a shared array, and storing them
from C, while the collector marks, all with checkmark on. It runs natively and under ThreadSanitizer.
`sched_test.c`, run by `tests/scheduler.rs`, checks that while marking each thread's stack is scanned once and
before the thread runs, whether it was running, waiting, asleep or in C.
The golden harness also checks each program's LLVM IR for a store that may write a pointer outside its frame
without the barrier before it.

## Known limits

Conservative scanning can retain what a stale stack word points at. Optimized builds may keep only a derived
pointer (one past an object's end), which a stack scan would miss: not yet validated. The page heap is a
first-fit list. Refills and large allocations take one heap lock. Sweeping is serial and stops the world, so
the ending pause grows with the heap. Only idle processors and allocations that reach the goal mark: a program
that keeps every processor busy marks in that allocation, which stalls it about as long as a stop-the-world mark
would, and objects allocated black meanwhile survive the cycle (more cycles mark more). The barrier's slow path
marks directly rather than buffering. Waking the helpers costs about 40 µs per stop with ten processors, which
shows only when collections are small and frequent.
