# Memory and the Object Model

How objects are laid out, where they live, and how they are reached. This is a design
record, not a formal specification.

## Values live inline

Every type has a complete, well-defined zero value, and values are stored inline by
default. A struct variable is the struct's bytes; a struct field of struct type is
embedded directly in the enclosing struct; an array of structs is a single contiguous
run of struct bytes, not an array of references to separately allocated structs.

```
struct Point { x: i32, y: i32 }

let p: Point          // two i32 fields, inline
let line: Point[]     // N * sizeof(Point) contiguous bytes
```

This is the cache-friendly default: iterating `line` walks linear memory with no pointer
chasing. Indirection is something you opt into (with a pointer), not something imposed by
the type system.

## Stack if you can, heap if you must

Storage location is an implementation concern, not part of a value's type. A value is
placed on the stack when its lifetime is provably bounded by the enclosing scope, and
promoted to the heap only when it must outlive that scope (for example, when its address
escapes through a returned pointer). The decision is made by escape analysis; the
programmer writes the same code either way.

Heap memory is reclaimed by a garbage collector. There is no manual free, and no
lifetime annotations in the common case. A type-system-based escape hatch for the
constrained cases where a tracing collector is unacceptable is a planned future
direction (see "Escaping the collector" below).

## Pointers and addressability

A pointer `T^` is an explicit indirection to a value of type `T`. Address-of `&x`
produces one from an **addressable** operand: a variable, a struct field, an array
element, or a dereference. Temporaries (call results, arithmetic results, literals) are
not addressable, because they name no storage.

Because values are stored inline, `&` can point *into* an aggregate — an interior
pointer to a field or an array element:

```
let line: Point[] = ...
let mid: Point^ = &line[len(line) / 2]   // interior pointer into the array
let yp: i32^ = &mid.y                     // interior pointer into a field
```

Interior pointers keep the target aggregate alive as far as the collector is concerned;
a pointer to one element retains the whole backing array. This is the price of allowing
indirection into contiguous storage, and it is the intended trade: contiguity and
interior pointers are more valuable than fine-grained per-element reclamation.

The collector is a span-based tracing GC that does not relocate heap objects, which is
what lets an arbitrary interior address be resolved back to its containing allocation via
span metadata.

## Nil as an inert value

A pointer may be `nil` — the absence of a target. Nil is a valid inhabitant of every
pointer type, not an error state the type system tries to eliminate.

Dereferencing nil does **not** panic. Instead, the null-object pattern (long used in
practice as a hand-written userspace convention) is promoted to a language default: a
nil pointer behaves as an inert stand-in for its pointee type. Reads yield that type's
zero values; writes are absorbed and discarded.

```
let p: Point^ = nil
let x: i32 = p.x     // 0 — the zero value of i32
p.x = 10             // absorbed; writes nothing
let y: i32 = p.x     // still 0
```

The nil object has **no state**. It is a `/dev/null` sink and a source of defaults only;
it never becomes a storage cell, so there is nothing for a write to persist, and nothing
for a later read to recover. When `p` is `nil`, `p.x = 10` followed by reading
`p.x` still yields the default `0`, not `10`.

Dereference chains propagate the same way: each step through a nil pointer yields a typed
nil, so `a.b.c.value` resolves to the zero value of `value` rather than panicking
partway through. The point is not that inert nil is superior to panicking — it is that
panic need not be the *only* outcome, and it should be for userspace code to decide.

You still of course may need to track whether a pointer refers to a live object; you have
simply swapped the consequence of getting it wrong from a crash to a default. This can
let nil checks be omitted or deferred where correctness does *not* depend on liveness: a
`filter`/`map`/`reduce` over a collection containing nils applies cleanly, producing
results built from zero values rather than aborting. Where liveness *does* matter, you
check for nil explicitly — the same check as before, now a domain decision rather than a
language mandate.

Nil is confined to this role: "a pointer legitimately has no target." It is not the tool
for modelling optional-ness or errors in general. A "maybe a bool" is an `Option<bool>`, not
a `bool^`; a fallible computation should return a `Result<T,E>`, not a bare pointer as a
success/failure signal. Those constructs (planned separately) model *semantic* absence;
nil models *referential* absence, and inertness is simply how it behaves under
dereference.

## Safety

Pointers are safe and GC-tracked: no pointer arithmetic, no integer/pointer casts. Raw,
unchecked pointers — if offered at all — belong to a separate `unsafe` facility (TBD).

## Escaping the collector (future direction)

GC-by-default with a manual escape hatch — a managed core with an unmanaged shell — is
the intended shape. The escape hatch is expressed **in the type system**, not through
runtime pointer tricks.

The distinction is a *static* one. Because Cooper has a precise, monomorphizing tracing
GC, the compiler always knows which words are pointers from type layout alone; it never
guesses from bit patterns. That same static knowledge lets the collector skip unmanaged
pointers for free, so no spare-pointer-bit tagging or per-dereference masking is needed —
tag bits only help conservative collectors, which this is not. A dynamic check, where one
is ever required, is a region/page test against allocator metadata, not a pointer flag.

The sketch, for posterity:

* A pointer into manually managed memory has a distinct type, written `NoGC T` as sugar
  for a non-collected `T^` (`NoGC` is pointer-only, so the caret is implicit). Assigning
  between `T^` and `NoGC T` is a type error in either direction: the two live in
  different regions and the language keeps them apart.
* `NoGC` is **infectious**: a pointer reached through an unmanaged structure is itself
  unmanaged, without re-annotation. A data type is declared once and used in either
  region — no managed/unmanaged twin structs, and no forcing every field to be spelled
  `NoGC` (the way `Maybe Foo` would not force every field to be `Maybe`).
* The load-bearing rule: **an unmanaged region may not hold a managed pointer.** This is
  what makes arenas fully opaque to the collector — it never has to trace into unmanaged
  memory — and it is enforced by the infectious typing above. Freeing unmanaged memory is
  the programmer's responsibility.
* This static membrane is what powers build-mode conveniences: a `--no-gc` mode is a pure
  compile-time reachability check (does the program contain any managed allocation?), and
  a debug `--trace-leaks` mode reuses the collector's reachability machinery to report
  `NoGC` pointers that became unreachable without being freed.

Also left open, to be settled alongside a build-mode concept: whether compiler flags
should let the programmer trade safety for speed per build — panic-on-nil-dereference and
array bounds checking are the obvious candidates (debug builds fault eagerly, release
builds run the inert/unchecked path).
