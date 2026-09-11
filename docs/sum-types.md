# Sum Types

## Decision

Cooper models tagged unions with the **`oneof`** keyword. A `oneof` declares a set
of named **variants**, each optionally carrying a positional payload. It is a
nominal type: distinct declarations never interchange, and a generic instantiation
is identified by its name together with its inferred type arguments.

```
oneof Color { Red, Green, Blue }

oneof Shape {
  Circle(i32),
  Rect(i32, i32),
}

oneof Result T E {
  Ok(T),
  Err(E),
}
```

`Circle`, `Ok`, and friends are **variants**, not types; they exist only inside their
`oneof` and are always reached through it.

## Why `oneof`

* `enum` and `union` are deliberately **reserved** for possible future C-style
  features (integer-backed enumerations, untagged unions) and are not even lexed.
  Using either here would foreclose that space.
* `oneof` reads as exactly what the type is: a value that is one of several shapes.

## Variants and payloads

A variant is a PascalCase name followed by an optional **parenthesised,
comma-delimited list of positional payload slots**:

```
None                 // payload-free
Some(T)              // one slot
Rect(i32, i32)       // two slots
```

Payloads are **positional only**. Named-field payloads are deferred — they are
entangled with the anonymous-struct and `type`-keyword decisions and will be
revisited then.

Payload slots are ordinary type expressions, so they may themselves be generic
applications:

```
oneof Holder { Full(Box i32) }
```

Juxtaposition was rejected for payloads because it collides with type-application
juxtaposition (`Some T` would be ambiguous with applying `Some` to `T`); the
parenthesised list keeps the two unambiguous.

## Type parameters

A generic `oneof` binds its type parameters by **juxtaposition on the declaration**,
identically to `struct Map K V`:

```
oneof Maybe T { None, Some(T) }
oneof Result T E { Ok(T), Err(E) }
```

The parameters are in scope throughout every variant's payload. An instantiation
`Maybe i32` is nominal by name and arguments; `Maybe i32` and `Maybe string` are
distinct types.

## Construction is type-qualified

A value is built by naming the `oneof` and then the variant:

```
Shape.Rect(3, 4)
Color.Green
Maybe.Some(5)
Maybe.None
```

Construction is always qualified by the **type**, never by a module. A payload-free
variant (`Color.Green`, `Maybe.None`) is a complete value on its own; a variant with
a payload is completed by a call whose arguments fill its slots.

## Inference and expected types

There are no type arguments at construction sites; a generic `oneof`'s arguments are
inferred. Two sources feed the inference:

1. **The payload arguments.** Unifying each slot against its argument fixes the
   parameters the payload mentions. `Maybe.Some(5)` fixes `T = i32`.
2. **The expected type**, when the construction sits at an annotated `let`
   initializer or a `return` whose function has a declared return type. This supplies
   the parameters the payload cannot:

```
func good(): Result i32 string { return Result.Ok(5) }    // payload fixes T, context fixes E
func empty(): Maybe i32 { return Maybe.None }             // context fixes T
```

The expected type is a **head-only** hint: it applies to the construction being
checked and is not propagated into sub-expressions. Without it, a construction whose
payload under-determines the parameters is rejected:

```
func f() { m := Maybe.None }   // error: cannot infer type arguments for Maybe.None
```

This mirrors the discipline used for generic structs, tuples, and destructuring —
**when in doubt, annotate; values stay clean.**

## Where errors surface

Everything beyond the shape of the declaration is a semantic concern, not a syntactic
one. The parser accepts the general form; the resolver registers the type and its
variants; the type checker enforces variant existence, payload arity, argument types,
and argument inference. An unknown variant, a wrong arity, a mismatched payload, or an
under-determined instantiation are all **type errors**.

## Deconstruction with `match`

A sum-type value is taken apart with `match`, which tests a scrutinee against a set
of arms and runs the one whose pattern matches. The scrutinee expression is
separated from the arm body by the **`with`** keyword, mirroring `if … then`:

```
match shape with {
  Shape.Circle(r) => area := pi * r * r
  Shape.Rect(w, h) => area := w * h
}
```

The `with` keyword also removes the ambiguity between the arm-body braces and a
struct literal in the scrutinee: the scrutinee is parsed up to `with`, and the body
always begins at the following `{`.

### Arms and patterns

Each arm is `Pattern => body`. The fat arrow `=>` separates the two because a
pattern is a richer sublanguage than a struct key — it carries its own parentheses
and, in future, its own colons — and `=>` is the established pattern-matching
separator. There is no fallthrough, so the C `switch` colon does not apply.

A pattern is one of:

* a **type-qualified variant pattern** `Type.Variant` or `Type.Variant(binders…)`,
  where each binder is a camelCase name that binds the corresponding positional
  payload slot, or `_` to ignore that slot;
* the **wildcard** `_`, a catch-all that matches any value and binds nothing.

```
Shape.Circle(r)      // binds r to the single payload slot
Shape.Rect(w, _)     // binds w, ignores the second slot
_                    // catch-all
```

Payload binders are scoped to their own arm: a name bound in one arm is not visible
in another, and each binder takes the type of the slot it names.

An arm body follows the same rule as an `if` branch: a single expression or
statement, or a braced `{ block }`. Arms are separated by inferred newlines or
explicit semicolons; a braced-block arm self-terminates. There are no commas.

### Statement and expression forms

Like `if`, `match` exists in two forms chosen by position. In statement position
the arm bodies are statements whose values are discarded. In expression position
the arm bodies are expressions whose types must unify, and that common type is the
type of the whole `match`:

```
kind := match shape with {
  Shape.Circle(r) => 1
  Shape.Rect(w, h) => 2
}
```

### Exhaustiveness

A `match` must be **exhaustive** in both forms: the arms must cover every variant of
the sum type, or end with a wildcard `_`. A match that leaves a variant unhandled
without a wildcard is a type error. An exhaustive match whose every arm returns
counts as returning on all paths, so it satisfies a function's return obligation.

Pattern validity, binder arity, binder types, arm-type unification, and
exhaustiveness are all **type errors**: the parser accepts the general form and the
type checker enforces the rest.

## Deferred

* **Named-field payloads**, tied to the anonymous-struct / `type` decision.
* **Bare / unqualified variant forms** (writing `None` where a `Maybe` is expected),
  which depend on the `use` name-binding system; no mandatory leading-dot syntax.
* **Richer patterns**: nested sub-patterns, literal patterns, or-patterns (`A | B`),
  guards (`if cond`), and `@`-bindings. The first iteration of `match` is flat.

