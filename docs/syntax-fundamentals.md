# Syntax Fundamentals

## Source encoding

* Program sources are UTF-8.
  * Current lexer only tokenizes a narrow ASCII subset (to be expanded).
* Strings are by default encoded in UTF-8.
  * TODO: `rune` type for representing an individual code point (or grapheme — TBD).

## Semicolon inference

Semicolons act as statement separators and/or terminators. However, an endline can be
automatically converted into a semicolon, so explicit semicolons are rarely needed.

One reason to type an explicit semicolon: when a function or block expression is intended to
return nothing (the unit type).

* `foo` at the end of a block → the block's type is the type of `foo` (e.g. `i32`).
* `foo;` at the end of a block → the block's type is the unit type.

### Inference algorithm

Based on the Ahnfelt variant of the Scala implementation:

Given two consecutive tokens `a` and `b` separated by an `EOL` token (endline):

1. If `a` is in the `beforeSemicolon` category (a semicolon immediately after `a` is syntactically valid), **and**
2. If `b` is in the `afterSemicolon` category (a semicolon immediately before `b` is syntactically valid),

then the `EOL` token is converted into a semicolon.

### Additional details

* Redundant consecutive endlines are eliminated in the tokenization phase, as are all other
  whitespace tokens.
* Endline-to-semicolon conversions are disabled within parentheses (allowing multi-line
  expressions without escaping).

## Naming conventions

* **Types**: PascalCase (e.g., `MyStruct`, `Color`). Capitalization disambiguates types from
  instances — not used for visibility/access control (unlike Go).
* **Functions, methods, variables, fields**: camelCase (e.g., `myFunc`, `isValid`).
* **Visibility**: Not determined by capitalization. Separate mechanism TBD.

## Type expressions

### Array notation

An **array** is Cooper's default sequence type: a dynamic, growable run of elements
(conceptually `{ data, length, capacity }`). It is written with postfix brackets:

```
let items: i32[]
let matrix: i32[][]
let callbacks: func(i32)[]
```

This follows C#/TypeScript convention (`Type[]`) rather than Go's prefix (`[]Type`).

A **static array** has a fixed compile-time length, written by placing that length inside
the brackets. It is the exceptional case — a known, non-resizable block of memory — chosen
when the size is fixed and the extra guarantees enable optimization:

```
let rgb: u8[3]           # exactly three bytes, never reallocated
let grid: i32[8][8]      # fixed 8×8
```

### Array literals

An array value is written as a bracketed, comma-separated list. This keeps `[...]` for
sequences and reserves `{...}` for records/aggregates:

```
let xs: i32[] = [0, 1, 2, 4, 8]
ys := [0, 1, 2, 4, 8]        # ys : i32[], element type inferred
empty := []                  # needs an annotation to fix the element type
```

There is no `new` or `make` keyword. Zero-defaults, literals, and (later) a small set of
ordinary builtin constructor *functions* for capacity hints cover construction; the
programmer never chooses stack versus heap.

### Subranging: copy by default, share with `&`

Indexing with a range yields a **new array** — an independent copy — not a view onto the
original backing store:

```
let xs: i32[] = [0, 1, 2, 4, 8]
let ys: i32[] = xs[2..]      # a fresh i32[] holding 2, 4, 8; independent of xs
```

Arrays have value semantics, so the safe thing happens by default: mutating `ys` never
touches `xs`. Sharing a backing store is the less common, more hazardous case, so it costs
one visible sigil — the `&` "reference into" operator — applied at the point the aliasing
is introduced:

```
let ws: i32[] = &xs[2..]     # a *view*: shares xs' backing, no copy
ws[0] = 99                   # writes through — xs[2] is now 99
```

The shape of the selection picks what `&` produces: a scalar index yields a single-element
pointer, a range yields a shared array view.

```
let p: i32^  = &xs[2]        # pointer to one element
let v: i32[] = &xs[3..]      # shared view of a run of elements
```

A view is **not a new type.** It is an ordinary `i32[]` whose data pointer aliases another
array's storage, so it passes to any function expecting `i32[]` and needs no separate
annotation. Under the tracing collector, two headers sharing one backing store is memory
safe; the only question is behavior, which one rule settles:

> **`&` shares elements; growth always forks.**

* **Element writes are shared.** Writing through a view mutates the common backing — that
  is exactly what you asked for by typing `&`, and it is visible at the creation site.
* **Growth never is.** A view is born with `capacity == length` (no spare room), so *any*
  append to it must reallocate into fresh storage and detach. A view can never grow into —
  and clobber — the array it borrows from.

Growth therefore follows an explicit return-value idiom, since an append may hand back the
same array (if it had spare capacity) or an entirely new one (a full owner, or any view):

```
xs = xs.push(9)              # reassign: the result may or may not be the original xs
```

A function that appends must return the possibly-new array; a signature that does **not**
return a `T[]` thereby signals it will not grow what it was given. The one residual sharp
edge is inherited and mild: pushing to an array that has outstanding views may reallocate
it, after which those views observe the pre-growth backing rather than the new one.

### Pointer notation

Pointers use postfix caret notation, consistent with the postfix array notation
(the caret is dedicated to pointers, so `^` is never multiplication or bitwise
operations):

```
let p: Point^        # pointer to a Point
let pp: Point^^      # pointer to a pointer to a Point
let ps: Point^[]     # array of pointers to Point
let sp: Point[]^     # pointer to an array of Point
```

The same caret is the postfix dereference operator in value position, mirroring
how `[]` constructs an array type but indexes an array value:

```
let x: i32 = p^.value   # dereference (explicit form: (p^).value)
p^.value = 10           # assign through a pointer
```

Related operators and values:

* **`&expr`** — prefix address-of, yielding a pointer to an addressable operand (a
  variable, struct field, array element, or dereference). Temporaries are not addressable.
* **`p^`** — postfix dereference. Member access auto-dereferences, so `p.value`
  works directly on a `Point^`; the explicit `p^.value` is equivalent.
* **`nil`** — the absent value of a pointer type. Dereferencing nil does not panic;
  it behaves as an inert stand-in yielding zero values. See
  [Memory and the Object Model](./memory-and-object-model.md) for the semantics of
  nil, addressability, and how objects live in memory.

### Function type expressions

A function type carries only the parameter types and return type — no parameter names:

```
let handler: func(Request, Duration): Response
```

Parameter names are **not allowed** in function type expressions. Rationale: if names are
inconsequential (not checked or matched), their presence is misleading. Names belong in
function _declarations_ (where they're used in the body), not in type expressions.

```
# Function declaration — names required (used in body)
func process(req: Request, timeout: Duration): Response { ... }

# The same signature as a type — types only, written inline where it is needed
func run(step: func(Request, Duration): Response) { ... }
```

Cooper has **no `type` keyword** and no cosmetic type aliases: a function type (or any
other type) is written directly at the binding or parameter that uses it, keeping the
shape local to its point of use rather than behind an inert nickname. Naming a distinct
*nominal* type over an existing one (e.g. a `LengthMm` backed by `i32`) is a separate,
deferred feature that will lead with its own declaring keyword.

### Tuple types

A tuple is an anonymous, structural product type written as a parenthesised list of
element types. Tuples are compared by shape: arity and element types, positionally.

```
let pair: (i32, string)          # a 2-tuple
let nested: (i32, (string, bool)) # tuples nest
let pairs: (i32, string)[]       # array of tuples
```

Parentheses without a comma are not tuples: `(T)` collapses to `T` (grouping) and
`()` is the unit type. A tuple therefore always has at least two elements. Tuple
_values_ mirror the type syntax: `(1, "a")` is a `(i32, string)`.

## Variable bindings

A `let` binds a name with an explicit type, an optional initializer, or both:

```
let count: i32 = 0
let name: string        # declared, no initializer
```

The walrus operator `:=` declares and initializes in one step, inferring the type
from the right-hand side (no annotation is permitted):

```
total := 0              # total: i32
label := "start"        # label: string
```

### Destructuring

A parenthesised pattern binds the elements of a tuple positionally. Patterns are
untyped — any type annotation lives on the enclosing `let`, never inside the
pattern. The pattern's arity must match the tuple's.

```
(x, y) := origin()              # types inferred from the result tuple
let (a, b): (i32, string) = row # annotation on the `let`, checked element-wise
```

The same pattern shape reassigns existing variables when the left of `=` is a tuple
of assignable targets:

```
(a, b) = (b, a)                 # positional reassignment
```

## Logical and bitwise operators

Cooper keeps a small operator vocabulary by a single rule: **a symbol is a unary prefix,
a keyword is a binary infix.** Negation is therefore the symbol `!`; the binary logical and
bitwise connectives are the keywords `and`, `or`, `xor`.

```
if !ready and pending or !blocked { ... }
```

`and`, `or`, `xor`, and `!` are **type-directed**: on `bool` operands they are logical, on
integer operands they are bitwise. Mixed operands are a type error, so there is never
ambiguity about which meaning applies.

* On `bool`, `and` and `or` **short-circuit** (the right operand is skipped when the left
  already decides the result); `!` is logical negation.
* On integers, `and`/`or`/`xor` are **eager** bitwise operations (both operands always
  evaluated) and `!` is bitwise complement. `xor` has no short-circuiting form in either
  case.

Bit shifts keep their conventional symbols, which are unambiguous: `x << 2`, `x >> 1`.

## Chained comparison

Comparisons may be chained when they run in a single direction, which reads better than a
repeated conjunction:

```
if min <= x <= max { ... }        # equivalent to: min <= x and x <= max
```

A chain desugars to the conjunction of its adjacent pairs, and each middle operand is
evaluated exactly once. Only **monotonic** chains are allowed: the links must all be
`<`/`<=` or all be `>`/`>=`. A direction-mixing chain such as `a < b > c` is rejected.

