# Garbage Collector

Memory is reclaimed by a tracing collector modelled on Go's: a span-based allocator, a precise heap,
conservative stacks, and Green Tea marking. It lives in the C runtime (`runtime/gc.c`, with the
descriptor generation in `crates/cooper-llvm/src/gc.rs`). It stops the world and then marks and sweeps on
one OS thread; parallel and concurrent marking are planned. Controls and measurements are in [Performance](./performance.md).

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

Stop-the-world mark and sweep, started by the pacer when the allocated bytes reach the live bytes plus a growth
percentage of the live bytes and roots (default 100%, at least 4 MiB).

- **Stopping.** The allocating thread that reaches the heap goal stops every other thread at a safe point: generated
  code polls a flag at each function entry and loop iteration, and the allocator, lock spins and the scheduler
  check it too. A thread stopped in its code saves its registers on its stack, as a switch does.
- **Roots.** Module-level variables, precisely, from a table of addresses and descriptors the compiler emits
  (`cooper_roots`). The stacks of every thread and the registers, conservatively, word by word (a suspended
  thread's registers are saved on its stack, and a thread that has not started is reached through its closure): nothing moves, so an ambiguous word
  only keeps alive what it appears to point into. Cooper never turns an integer back into a pointer.
- **Mark.** Green Tea: reaching a small object marks it and queues its span, first in first out; a span's
  marked-but-unscanned objects are scanned together from its mark and scan bits. Larger objects use an object
  work stack. Interior pointers resolve through the span table.
- **Sweep.** The marked objects become the allocated ones. Spans with nothing live return to a page heap that
  merges neighbouring free runs, large free runs go back to the operating system, and spans with free slots
  are listed per size class. Reused memory is zeroed when allocated.

## Testing

`COOPER_GC=stress` collects at every allocation and poisons freed memory, and the golden-program harness runs
every program again that way, on four processors (`COOPER_THREADS=4`), and both, requiring the same output. `gc_test.c`, run by `tests/allocator.rs`,
tests the allocator and collector alone: sizes, pointer bitmaps, reuse, interior pointers, reclamation,
poisoning, that a sum type retains only the variant it holds, and threads allocating on four Ps during collections.

## Known limits

Conservative scanning can retain what a stale stack word points at. Optimized builds may keep only a derived
pointer (one past an object's end), which a stack scan would miss: not yet validated. The page heap is a
first-fit list. Refills and large allocations take one heap lock, and the collector marks and sweeps on one OS thread while the
others wait, so allocation scales with the number of Ps only while collections are rare.
