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
* **Arithmetic operator overloading: no.** Arithmetic operators are defined only on
  built-in types: the numeric primitives, `string` concatenation, and a planned
  built-in linear-algebra library (vectors such as `vec3`, matrices such as `mat3x3`)
  that carries operator syntax as a built-in privilege, the same way index syntax
  belongs to the built-in collections. A user type cannot give `*` a meaning; a
  user-defined numeric type exposes ordinary methods (`a.mul(b)`), visibly userspace.
  Overloaded arithmetic hides arbitrary cost and semantics behind the most familiar
  notation, and the types that benefit from it most — small fixed-size vectors and
  matrices — are a closed, well-known set, better served by one built-in, optimisable
  implementation than by every library writing its own.
* **No protocol methods.** A user type does not hook into equality, ordering,
  hashing, or copying by declaring a method with a compiler-known name. Built-in
  collections carry their own fixed method sets; algorithms that need an operation on
  a user type (sorting, custom equality) take it as a function.
* **Index/slice syntax: built-in types only.** `a[i]`, `a[i] = v`, and range
  slicing/views are a privilege of the compiler-known collections (static and dynamic
  arrays, hashmaps, hashsets), not user-extensible sugar. A user-defined collection
  stays explicit — it exposes ordinary methods (`at`, `set`, …), visibly userspace.
  The reason is coherence, not capability envy: the full array-like surface includes
  `&a[i]` (an interior pointer) and `&a[lo..hi]` (a borrowed view), and a view is only
  "free" because the compiler privately represents it as an array aliasing existing
  storage. A user type has no such representation, so extending the sugar would either
  demand a general borrowed-view language feature or leave a partial surface (`a[i]`
  works, `&a[lo..hi]` does not) that ambushes the reader. Better a visible method call
  than magic with a hidden hole. If ergonomics demand more, the answer is *more and
  better built-ins*, not user-defined operators.

Because arithmetic operators are not method sugar, "T supports `+`" cannot be phrased
as a method-set constraint. Generic arithmetic over a numeric type parameter is an
open question; until it is settled, the operation is **passed explicitly**, like any
other capability. Zero/identity (needed by e.g. a generic `sum`) has no value to
dispatch on and is passed the same way (`fold(xs, 0, add)`). A type-associated
function is a fallback only if ergonomics demand it.

## Equality and hashing are built in

`==` and `!=` are structural, shallow, and field-wise; no user code participates.
Primitives compare by value (`string` by content, floats by IEEE semantics, so
`NaN != NaN` and `+0.0 == -0.0`), pointers by address, tuples and structs component by
component, and sum types by variant tag, then payload. A pointer field compares by
address, not by pointee. Field-wise rather than bitwise is deliberate: padding bytes,
float encodings, a `string`'s storage address, and a sum type's inactive payload bytes
all carry bits that equality must ignore. A backend may still compare raw memory where
a layout makes the two coincide.

A type is **comparable** when every component is. Excluded are function types
(closure identity has no meaning), dynamic arrays `T[]` (a view's identity and its
elements are both plausible readings, so neither is chosen), and unconstrained type
parameters (generic code that needs equality takes an `eq` function). A struct or sum
type containing any of these is not comparable either.

The comparable types are also the **hashable** ones, with hashing consistent with
`==`: equal values hash equally, `+0.0` and `-0.0` hash alike, and a `NaN` key can be
inserted but never found. Hashing a pointer by address requires the runtime to keep
object identity stable, which rules out a moving collector unless it supplies stable
identities. Ordering (`<`, `<=`, `>`, `>=`) is defined on numeric types only;
algorithms that order user types take a comparator.

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

* **Instantiation** takes no type arguments in value position: types are inferred,
  and when inference under-determines a parameter, the binding is annotated —
  `let r: Result i32 string = Ok(5)`. Same "annotate the `let`" rule already
  used for tuples and destructuring.
* **`where` clauses** infer and thread the required operations structurally, so
  bounded generics read clean while lowering to explicit dictionary passing.
* **Derivation** of common functions (equality, ordering) may later be provided as
  pure codegen sugar; it is never a nominal trait mechanism.

## Explicitly out of scope

Higher-kinded types; associated types; nominal traits/typeclasses with `impl`
blocks; coherence/orphan rules; global instance resolution; type-level computation
(TypeScript-style type algebra); subtyping; inheritance; variance; general function
overloading.

## Staged plan

* **Stage 0** — parametric generics, no constraints. Enough for `Maybe T`,
  `Result T E`, `List T`, and any generic that does not inspect `T`. Capability
  needs met by passing functions/values. (Sum types need only this stage.)
* **Stage 1** — structural method-set constraints in a `where` clause, implicit
  satisfaction, checked at instantiation.
* **Stage 2** — operator capabilities on type parameters, if a real need appears:
  most likely closed, compiler-known categories (comparable, ordered, numeric) rather
  than user-extensible ones. Zero/identity is passed explicitly (or via a
  type-associated function if needed).
