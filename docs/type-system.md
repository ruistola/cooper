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

## Inference

Type checking is constraint-based inference in the style of **HM(X)** (Odersky, Sulzmann, and
Wehr, 1999): Hindley–Milner parameterized by a constraint domain *X*. Checking a function body
generates constraints over type variables, and a solver then decides them. Classic
Hindley–Milner is the instance where *X* is equality, solved by unification; other instances
add subtyping, type classes, or records. Because constraints are collected before they are
solved, whether a body type-checks does not depend on the order of its statements or operands:
`x := None` can be fixed by a later `x = Maybe.Some(3)`.

Cooper's domain *X* has three kinds of constraint:

* **Equality** between types, solved eagerly by unification with an occurs check: assignments,
  returns, arguments, operand pairs, array elements, and instantiations of generics. `nil`
  unifies with any pointer type.
* **Predicates** on a type, checked once it is solved: numeric, addable (`+`), integer
  (indices, shift operands), logical (`bool` or an integer, for `and`/`or`/`xor`),
  comparable (`==`), and formattable (`string(x)`). A numeric literal is a variable
  of its class that defaults only when nothing constrains it
  ([Numeric Types](./numeric-types.md)).
* **Member obligations**: field and method access, variant construction, and `match` patterns
  on a type not yet known wait until another constraint determines it. Each has one solution,
  so solving never searches or backtracks, and a bare variant never selects its sum type
  merely because its name is unique ([Sum Types](./sum-types.md)).

A constraint that nothing determines is an error asking for an annotation, reported where it
arose. Every top-level signature is declared, so inference runs per function body and never
crosses a declaration. A function literal or nested function is part of the body it appears
in: its body shares that body's constraints, so a variable it captures is inferred from
uses inside and outside it alike.

### What Cooper leaves out of HM(X)

* **No generalization.** Hindley–Milner generalizes `let` bindings to polymorphic types. Cooper
  has no local generic functions, so local bindings stay monomorphic and only declarations
  introduce type parameters ([Generics](./generics.md)).
* **No user-extensible constraints.** There are no type classes, bounds, or overloading, so *X*
  is closed: the predicates above are built in, and generic code takes the behavior it needs
  as function arguments.
* **No subtyping.** Constraints are equalities, with the single exception of `nil` and
  pointers.
* **No inferred signatures.** Parameter and return types of declarations are written out.

## Operators and overloading

* **Functions are not overloaded.** One name denotes one function.
* **Arithmetic operators apply to built-in types only:** the numeric primitives, `string`
  concatenation with `+`, and a planned built-in vector and matrix library (`vec3`,
  `mat3x3`, …). A user-defined numeric type exposes ordinary methods (`a.mul(b)`).
* **Index and slice syntax applies to built-in collections only:** arrays, and later hash
  maps and sets. A user-defined collection exposes ordinary methods (`at`, `set`, …). The
  full index surface includes interior pointers (`&a[i]`) and shared slices (`a[lo..hi]`),
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
