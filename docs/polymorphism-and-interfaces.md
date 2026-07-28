# Polymorphism Without Interfaces

## Decision

Cooper does **not** have a dedicated interface construct. Polymorphism is achieved by passing
functions (first-class citizens) as arguments. Type aliases for function signatures provide
naming and documentation.

## Context and rationale

Go's interfaces are an elegant and non-invasive polymorphism mechanism compared to C++ vtables
embedded in object memory layouts. However, several observations suggest a simpler approach
can achieve similar expressiveness with less machinery:

1. **Go best practice converged on single-method interfaces.** The community widely recommends
   interfaces as thin as possible (one, at most two methods). `io.Reader`, `io.Writer`,
   `fmt.Stringer` — the most successful interfaces have exactly one method.

2. **Single-method interfaces are isomorphic to closures.** The runtime representation is
   identical: a function pointer plus an environment/data pointer. Go's itable indirection
   exists to support multi-method dispatch, but for single-method interfaces it's pure overhead
   (chasing a pointer to a table containing one entry).

3. **Cooper has first-class functions.** Passing a function where an interface would be
   expected in Go is natural and requires no new language concept.

## The design

```
// Named function type — documentation and intent, compile-time only
type Reader = func([]u8): (i32, Error)

// Functions accept capabilities explicitly
func process(read: Reader) {
    n, err := read(buf)
    // ...
}

// Call sites are explicit about what capability they provide
process(myfile.read)                           // method binding → closure
process(func(b: []u8): (i32, Error) { ... })   // ad hoc lambda
process(myMockRead)                            // any matching function
```

## What this gives us

* **Zero runtime overhead** beyond what closures already cost (func_ptr + env_ptr, one
  indirection — strictly less than Go's itable approach).
* **No new language concepts** — functions are values, type aliases provide names. Both are
  needed regardless.
* **Structural typing for free** — signature match equals satisfaction.
* **Trivial testability** — pass lambdas instead of constructing mock structs.
* **Principle of least authority** — callers provide exactly the capability needed, not an
  entire object with potentially many methods.

## What this costs

* **No runtime type recovery.** You cannot ask "does the underlying value also support Close?"
  at runtime (Go's type assertions / type switches). In Cooper, if you need `close`, take it
  as a parameter — require capabilities statically rather than discovering them dynamically.
  Sum types with pattern matching address the "optional capability" case differently.

* **Multi-method polymorphism is manual.** For the rare case where 2-3 tightly coupled
  functions must travel together, the user constructs a struct with function-typed fields or
  passes multiple function parameters. This is intentionally slightly verbose — it nudges
  toward the single-capability design that produces more flexible, composable code.

* **No self-documenting "type X satisfies interface Y" relationship.** Satisfaction is checked
  at call sites (wrong signature → type error), not at definition sites.

## Performance comparison

```
// Go single-method interface call: 2 loads + indirect call
MOV RAX, [interface]       // load itable pointer
MOV RBX, [RAX+offset]     // load func pointer from itable
CALL RBX with data_ptr

// Cooper function value call: 1 load + indirect call
MOV RAX, [closure]         // load func pointer directly
CALL RAX with env_ptr
```

One fewer pointer chase per polymorphic call. Marginal in most code, potentially meaningful
in hot loops — and more importantly, it means reaching for this pattern carries no guilt in
performance-sensitive code.

## Interaction with sum types

Sum types (tagged unions) use closed, static dispatch — fundamentally different from open
interface dispatch. When all variants of a union define a method with matching signature, the
compiler can verify completeness at compile time and generate a tag-based branch (or jump
table) — no itable, no pointer chasing.

```
enum Color {
  Red(RedPayload),
  Green(GreenPayload),
  Blue(BluePayload),
}

// If all payload types define is_valid(): bool, then:
func validate(colors: Color[]) {
    for c in colors {
        check(c.is_valid)  // compiler generates tag dispatch, binds variant method
    }
}
```

If a variant lacks the required method, the compiler emits an error:
`not all variants of Color define is_valid(): bool`.

This is an interface-like compile-time contract with zero runtime abstraction cost — just a
branch on a tag, which modern CPUs predict well.

## Design philosophy

The goal is to make "pass the function you need" the easy, obvious, zero-friction path.
Multi-method bundles are possible (struct of function fields) but intentionally require more
ceremony — discouraging elaborate abstract contracts and encouraging concrete, composable,
single-capability designs.

This aligns with empirical evidence from Go's ecosystem: programs designed around thin
interfaces (one method) are more flexible and less bloated than those built on elaborate
multi-method contracts typical of enterprise Java patterns.
