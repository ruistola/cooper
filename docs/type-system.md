# Type System

## Decision

Cooper is a **first-order** type system: types and type parameters, but no
abstraction over type constructors, no nominal abstraction hierarchy, and no runtime
type polymorphism. Generics add parametric polymorphism; per-type behavior is either
a fixed-name method you implement or a function you pass.

One rule underpins the rest:

> **Capability is passed, not inherited. Substitutability is checked structurally,
> at compile time, at the generic instantiation boundary.**

A constraint/typeclass/trait is just compiler-synthesized dictionary passing.
Cooper's polymorphism principle is the opposite — pass the function — so constraints
are, at most, thin sugar over explicit passing, never a separate nominal system with
global instance resolution.

## Nominal vs structural

* **Named, declared aggregates are nominal**: structs and sum types. Two structs
  with identical fields (`Coord` and `Point`) are distinct and **not
  substitutable**; convert explicitly if needed.
* **Anonymous, ad-hoc composites are structural**: tuples and function types,
  compared by shape.

Rule of thumb: named things are nominal, anonymous things are structural.

## No subtyping, no variance

There is no subtyping and no inheritance. Composition and first-class functions
cover the use cases; if ergonomic composition is wanted later, struct embedding is
added as pure field/method **forwarding** — explicitly not a subtype relationship.

Declining subtyping removes an entire apparatus for free:

* `List T` is invariant by construction (monomorphized; no subtype of `T` to
  smuggle in), so the covariant-array store hazard cannot occur.
* Function types are compared by exact structural match, not by
  contravariant-parameter / covariant-return subtyping.
* **Variance never needs to exist**, because there are no subtype relationships for
  containers or functions to preserve or invert.

## Substitutability lives at instantiation, at compile time

Cooper removed interfaces, so "does X stand in for Y" is not a runtime question. It
is answered when a generic is instantiated: a type argument satisfies a `where`
clause by structurally having the required methods. The mental model is
**monomorphization** — generics are stamped out per concrete type before runtime, so
the compile-time/runtime line stays sharp and value semantics are preserved (no
boxing).

The price of monomorphization — no heterogeneous runtime container of "anything with
method M" — is paid instead by the features built for it: **sum types** for closed
heterogeneity, **closures** for open behavior.

## No general overloading

* **Function overloading: no.** It fights Cooper's inference-heavy value position
  (walrus, tuple destructuring, no value-position type arguments). Generics and
  method sets already express "same operation, many types."
* **Operator overloading: yes, but only as fixed-name method sugar.** A closed set
  of built-in operators desugars to conventional method names on the left operand
  (`a + b` → `a.add(b)`, `a == b` → `a.equals(b)`, `a < b` → `a.compare(b) < 0`).
  Users opt into existing syntax by implementing a known method; they cannot invent
  operators or change precedence. This is the one controlled exception, and it is
  what makes "T supports `+`" expressible as "T has method `add`" — so constraints
  never need a Go-style type-set escape hatch (`~int | ~float64`).

Zero/identity (needed by e.g. a generic `sum`) has no value to dispatch on and is
**passed explicitly** (`fold(xs, 0, add)`) rather than resolved implicitly. A
type-associated function is a fallback only if ergonomics demand it.

## Constraints: structural, implicit, sugar over passing

When a type parameter needs capabilities, the substrate is always **explicit
passing** of functions/values. A `where` clause is optional ergonomic sugar over
that: it asserts **structural method-set membership**, satisfied **implicitly** and
checked **at instantiation** — no named trait, no `impl` block.

```
func max T (a: T, b: T): T where T: { compare(T): i32 } { … }
```

`T` satisfies this if its method set contains `compare(T): i32`. Because methods must
be declared in the same module as their receiver type, foreign types cannot be
retrofitted — so the orphan/coherence problems that force Rust's rules **cannot
arise**, and satisfaction is unambiguous for free.

You never *depend* on a global resolution algorithm: the operations can always be
passed by hand, and the `where` clause merely fills them in. That boundary is what
keeps Cooper between Go and Rust and off the meta-ladder.

## Developer experience: sugar and inference on top of a lean core

The core stays explicit; day-to-day ergonomics come from codegen sugar and inference
that lower to it:

* **Operators** lower to method calls (above).
* **Instantiation** takes no type arguments in value position: types are inferred,
  and when inference under-determines a parameter, the binding is annotated —
  `let r: Result i32 string = Result.Ok(5)`. Same "annotate the `let`" rule already
  used for tuples and destructuring.
* **`where` clauses** infer and thread the required operations structurally, so
  bounded generics read clean while lowering to explicit dictionary passing.
* **Derivation** of common methods (equality, ordering) may later be provided as
  pure codegen sugar; it is never a nominal trait mechanism.

## Explicitly out of scope

Higher-kinded types; associated types; nominal traits/typeclasses with `impl`
blocks; coherence/orphan rules; global instance resolution; type-level computation
(TypeScript-style type algebra); subtyping; inheritance; variance; general function
overloading.

## Staged plan

* **Stage 0** — parametric generics, no constraints. Enough for `Option T`,
  `Result T E`, `List T`, and any generic that does not inspect `T`. Capability
  needs met by passing functions/values. (Sum types need only this stage.)
* **Stage 1** — structural method-set constraints in a `where` clause, implicit
  satisfaction, checked at instantiation.
* **Stage 2** — operators as fixed-name method sugar, so numeric constraints work;
  zero/identity passed explicitly (or via a type-associated function if needed).
