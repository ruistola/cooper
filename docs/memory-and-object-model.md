# Memory and the Object Model

## Values live inline

Every type has a well-defined zero value, and values are stored inline. A struct field of
struct type is embedded in the enclosing struct, and an array of structs is one contiguous
run of struct bytes. Indirection is opt-in, through a pointer.

```
struct Point { x: i32, y: i32 }

let p: Point          # two i32 fields, inline
let line: Point[]     # contiguous Points
```

## Storage

Storage location is not part of a type. Escape analysis, modelled on Go's, places a value on
the (green-thread) stack when its lifetime is bounded by its scope, and on the heap only when
it must outlive it; keeping as much as possible on the stack is a goal. A tracing garbage
collector, modelled on Go's Green Tea collector, reclaims heap memory; there is no manual
free. The collector does not relocate objects, and it resolves an interior address to its
containing allocation through span metadata.

## Pointers and addressability

A pointer `T^` refers to a value of type `T`. `&x` takes the address of an **addressable**
operand: a variable, a struct field, a built-in array element, or a dereference. Temporaries
are not addressable. Because values are inline, a pointer may point into an aggregate:

```
let mid: Point^ = &line[line.length() / 2]   # into an array
let yp: i32^ = &mid.y                         # into a field
```

An interior pointer keeps its whole containing allocation alive.

Pointers are safe: there is no pointer arithmetic and no integer–pointer conversion. Raw
pointers, if offered, will belong to a separate `unsafe` facility.

## Nil is inert

`nil` is a valid value of every pointer type and means "no target". Dereferencing nil does
not fault. A nil pointer behaves as a stateless stand-in for its pointee type: reads yield
zero values, and writes are discarded.

```
let p: Point^ = nil
let x: i32 = p.x     # 0
p.x = 10             # discarded
let y: i32 = p.x     # still 0
```

Chains propagate: `a.b.c.value` through a nil link yields the zero value of `value`. Code
that depends on a pointer being live checks for nil explicitly. A build option will offer the
traditional treatment instead: stopping the program on any access through nil. Nil models only referential
absence. Optional values and failures use sum types (`Maybe T`, `Result T E`).

## Unmanaged memory (future direction)

Manually managed memory will be reached through a distinct pointer type, `NoGC T`, a
non-collected `T^`. The design rules:

* Assigning between `T^` and `NoGC T` in either direction is a type error.
* A data type is declared once. Its region is inferred from where a value is allocated, and
  it propagates inward: reading a pointer field through a `NoGC Node` yields a `NoGC Node`.
* An unmanaged region may not hold a managed pointer, so the collector never traces
  unmanaged memory. Freeing unmanaged memory is the programmer's responsibility.
* Functions are region-polymorphic: logic written against `T^` runs on a `NoGC T` unchanged.
  Only durable references across regions need care. They use integer handles into a side
  table, or an explicit copy between regions.

Because the collector knows every pointer's type from layout, it skips unmanaged pointers
without tag bits. Build modes may build on this: a `--no-gc` check that a program makes no
managed allocation, a `--trace-leaks` report of unreachable `NoGC` memory, and per-build
choices such as faulting on nil dereference or checking array bounds.
