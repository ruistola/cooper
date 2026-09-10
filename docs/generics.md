# Generics

## Decision

Cooper expresses generic types and functions with **juxtaposition** for type
application, **PascalCase type parameters introduced by an explicit binder**, and
**inference-only instantiation in value position**. There are no angle brackets, no
turbofish, and no type arguments written at call sites.

```
struct Map K V { … }                         // K, V are type parameters
let scores: Map string i32 = …               // Map applied to string and i32
let maybe: Maybe bool = Some(true)            // constructor; type args inferred
```

## Why not brackets

* `<>` is out: `<` is the comparison operator, so disambiguation needs backtracking
  or a turbofish. It is also the "ASCII art" the language tries to avoid.
* `[]` is out for Cooper specifically: postfix `[]` is already array construction
  and indexing. `Foo[Bar]` would collide with "array of Foo" and, in value
  position, with indexing. Overloading the most load-bearing bracket is worse here
  than in languages that do not use `[]` for arrays.
* Call-style `Map(string, i32)` parses fine in type position but looks like a
  function call, overloads parentheses (already tuples, grouping, unit, calls, and
  function-type parameter lists), and flattens the visual hierarchy between a type
  constructor and its arguments. It remains an acceptable fallback with identical
  semantics, but juxtaposition is preferred.

Juxtaposition reads cleanly, adds no symbols, and gives type parameters an almost
keyword-like prominence in a signature.

## What the pipeline makes possible

Type application is parsed as a left-associative spine and **arity is never checked
during parsing**. `Map string i32` becomes `App(App(Map, string), i32)`; whether
`Map` actually takes two parameters is a resolver question. This deferral is what
lets a bounded-lookahead parser accept juxtaposition at all — a single-pass compiler
could not.

Parsing of a type-application atom stops at any token that cannot begin a type atom:
a comma, a closing delimiter, `=`, `{`, or a statement terminator. Those existing
boundaries make the common cases unambiguous:

```
func f(m: Map string i32, n: i32): i32 { … }  // comma ends the first type
let x: Map string i32 = …                     // `=` ends the type
): Map string i32 { … }                       // `{` ends the return type
```

### Grouping requires parentheses

Juxtaposition is left-associative and binds looser than the postfix operators, so a
compound argument must be parenthesised:

```
(Map string i32)[]        // array of maps  (vs. Map string i32[] = Map of string and i32[])
Map (i32, string) bool    // a tuple as one argument
List (func(i32): bool)    // a function type as one argument
Map string (List i32)     // nesting: Map of string to (List of i32)
```

This is the ML trade-off: clean in the common case, parentheses for anything
compound. One consequence is that a missing statement terminator surfaces as a
resolver error (`Point y` → "Point is not generic") rather than a syntax error.

## Type parameters: PascalCase + explicit binder

A type parameter is neither a concrete type nor a value, but Cooper has no separate
lexical class for it. Rather than break the "types are PascalCase" rule with
lowercase ML-style variables, parameters stay PascalCase and an **explicit binder**
marks them. The binder is what distinguishes a parameter from a concrete type; its
locality on the same declaration keeps intent clear.

```
struct Map K V { … }                     // struct binder: after the name
func map T U (f: func(T): U, xs: List T): List U { … }   // free-function binder
```

The binder is a run of identifiers between the declared name and the value-parameter
list. It is LL(1): after `func <name>`, any identifiers before `(` are type
parameters; a value-parameter list always starts at `(` with `name: Type` entries.

## Generic methods

A method receiver already introduces the receiver **value**; it also introduces the
receiver type's **type parameters**, by pattern. Method-local parameters (those not
derivable from the receiver) use the same after-the-name binder as free functions.

```
(m: Map K V) func get(key: K): V { … }

(m: Map K V) func transform T U (other: Map T U): Map K U { … }
```

* `K, V` are bound by the receiver pattern `Map K V`.
* `T, U` are method-local, bound after the method name.
* The signature freely mixes them, including several distinct instantiations of the
  same constructor (`Map T U` and `Map K U`).

That last point is decisive: type parameters bind **per declaration**, never
globally to a type. Binding `K, V` to `Map` module-wide would make it impossible to
mention a second instantiation of `Map` in one signature, as `transform` does.

### The `_` wildcard

A receiver must name the parameters it references. When a method ignores one, `_`
stands in for it rather than forcing an unused name:

```
(m: Map K _) func keys(): List K { … }   // value type irrelevant here
```

## Instantiation in value position

There are **no type arguments at call sites**. Instantiation types appear only in
type position; in value position the compiler infers them, and when inference
under-determines a parameter, the binding is annotated:

```
x := Some(true)                          // x : Maybe bool, fully inferred
let r: Result i32 string = Result.Ok(5)  // Ok fixes only T; E annotated on the let
```

This is the same discipline already used for tuples and destructuring — **when in
doubt, annotate the `let`; values stay clean** — so one rule covers all three
features.

## Constraints (deferred)

Bounded type parameters are **not yet designed**. Two commitments hold regardless:

* Constraints go in a **separate `where`-style clause**, never inline in the
  signature. Keeping bounds out of the signature is what preserves the readability
  that juxtaposition and the receiver-pattern binders buy.
* Because Cooper has no interface construct, a bound cannot name an interface. The
  eventual mechanism will most likely be **structural**, and its interaction with
  operators (a numeric parameter needing `+`, a zero, an identity) and with the
  compile-time/runtime boundary is an open question. Generics start fully
  parametric (unbounded); bounds are added later.
