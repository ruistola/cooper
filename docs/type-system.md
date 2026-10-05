# Type System

Cooper's type system is first-order and has no subtyping. Generics are parametric and
monomorphized. A type gets per-type behavior from its own methods, and generic code takes
the behavior it needs as function arguments.

## Nominal and structural types

Declared aggregates (structs and sum types) are **nominal**. Two structs with identical
fields are distinct types, and a generic instantiation is identified by its declaration and
its type arguments. Anonymous composites (tuples and function types) are **structural** and
compared by shape.

There is no subtyping, inheritance, or variance: types are equal or they are not. Sum types
cover closed heterogeneity, and function values cover open behavior (see
[Polymorphism](./polymorphism.md)).

## Operators and overloading

* **Functions are not overloaded.** One name denotes one function.
* **Arithmetic operators apply to built-in types only:** the numeric primitives, `string`
  concatenation with `+`, and a planned built-in vector and matrix library (`vec3`,
  `mat3x3`, …). A user-defined numeric type exposes ordinary methods (`a.mul(b)`).
* **Index and slice syntax applies to built-in collections only:** arrays, and later hash
  maps and sets. A user-defined collection exposes ordinary methods (`at`, `set`, …). The
  full index surface includes interior pointers (`&a[i]`) and shared views (`&a[lo..hi]`),
  which rely on a representation only the compiler controls.
* **User types do not hook into equality, ordering, hashing, or copying through methods.**
  Equality and hashing are built in (below). An algorithm that needs another operation on a
  user type, such as an ordering for sorting, takes it as a function.

## Equality and hashing

`==` and `!=` are structural, shallow, and field-wise. Primitives compare by value: `string`
by content, and floats by IEEE semantics (`NaN != NaN`, `+0.0 == -0.0`). Pointers compare by
address, tuples and structs component by component, and sum types by variant, then payload.
Comparison is field-wise rather than bitwise because padding, float encodings, a `string`'s
storage address, and inactive payload bytes must not affect it.

A type is **comparable** when all its components are. Function types, dynamic arrays `T[]`,
and unconstrained type parameters are not comparable, and neither is any type containing
them. Generic code that needs equality takes an `eq` function.

The comparable types are also the **hashable** types, and hashing agrees with `==`:
`+0.0` and `-0.0` hash alike, and a `NaN` key can be inserted but never found. Pointers hash
by address, which requires the runtime to keep object identity stable.

Ordering operators (`<`, `<=`, `>`, `>=`) apply to numeric types only.
