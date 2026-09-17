# Numeric Types

## The primitive numeric set

Cooper's built-in numeric types are the usual fixed-width grid:

* **Signed integers:** `i8`, `i16`, `i32`, `i64`
* **Unsigned integers:** `u8`, `u16`, `u32`, `u64`
* **Floating-point:** `f32`, `f64`

Pointer-width `isize`/`usize` are deliberately deferred until there is a concrete
need (indexing/`len` against real arrays); adding them later is additive.

## No untyped constants; literals are typed by context

Cooper does **not** adopt Go-style arbitrary-precision untyped constants. That
machinery — an arbitrary-precision constant-evaluation layer that only narrows to a
concrete type at the assignment boundary — buys reusable constant expressions at
every width, but its cost is out of proportion to its value for a compiled,
fixed-width language. What actually makes strongly-typed numeric code ergonomic is
the far cheaper **expected-type literal inference**, which Cooper does adopt:

> A numeric literal's lexical form fixes its *class* (integer vs floating-point); the
> surrounding context fixes its *width*. Absent a usable context, the class default
> applies.

* A literal with a fractional point or a decimal exponent (`3.14`, `1e9`) is a
  **float** literal; a hexadecimal (`0xFF`), binary (`0b1010`), or plain decimal
  (`42`) literal is an **integer** literal.
* An **integer literal** adopts any expected numeric type — integer *or* float — so
  `let x: i64 = 5` and `let x: f32 = 5` both hold with no suffix or cast.
* A **float literal** adopts only an expected float type. `let x: i32 = 3.14` is an
  error: a fractional literal cannot seed an integer.
* With **no expected type**, a literal falls to its class default: **`i32`** for
  integers, **`f32`** for floats. (32-bit defaults suit the graphics/physics/GPU
  domains where 32-bit values remain the norm.)

The expected type flows from annotations, `return` types, call-argument types, and
struct/variant field types — the same head-only contextual hint the checker already
uses to infer generic sum-type arguments.

## No implicit conversions; arithmetic is same-type

There is **no implicit numeric conversion and no promotion**. A binary arithmetic or
comparison operator requires both operands to share one concrete numeric type;
`i32 + i64` is an error, not a silent widening. To keep this ergonomic, a bare
numeric literal on one side of an operator **borrows its width from the other,
non-literal operand**: in `x + 1` with `x: i64`, the `1` is typed `i64`. When both
sides are literals, or neither is, the enclosing expected type guides both equally.

This follows Rust/Zig rather than C/Go: implicit width and signedness conversions are
a well-known defect source, and the cost of forbidding them is a few explicit
conversions at genuine width boundaries.

## Reference baseline (cast + const across inspiring languages)

| Language | Literal typing | Numeric conversion | Cast syntax |
| --- | --- | --- | --- |
| Rust | expected-type inference, suffixes (`5i64`) | explicit only | `x as i64` (also `T::from`/`try_into`) |
| Go | untyped constants (arbitrary precision) | explicit for typed values | `int64(x)` constructor-style |
| Swift | expected-type inference | explicit only | `Int64(x)` constructor-style |
| Haskell | `Num`-polymorphic literals via `fromInteger` | explicit (`fromIntegral`) | function, not syntax |
| OCaml/SML | monomorphic; separate `int`/`float` ops (`+.`) | explicit (`float_of_int`) | function, not syntax |
| TS/JS | one `number` type | n/a | n/a |

Two design axes stand out. On **cast syntax**, the constructor-style `i64(x)` (Go,
Swift) reads without precedence ambiguity, whereas `x as i64` (Rust) invites the
mental "where does the cast bind?" question. On **constants**, only Go pays for
arbitrary precision; everyone else gets by with typed constants plus literal
inference.

## Stance on the remaining work (range checking, conversions, `const`)

### Range/overflow checking of literals — MVP-adjacent, do it

Once a literal has a concrete type, an out-of-range literal (`let x: u8 = 300`, a
negative literal for an unsigned type) is a compile error. This needs the literal's
text parsed into a value (staged through `i128`/`u128`) and bounds-checked against
its resolved type. It is cheap, high-value, and catches real bugs — planned as the
next numeric step.

### Explicit conversions — constructor-style `i64(x)`

Cooper will spell an explicit numeric conversion as a **constructor-style call on the
target primitive**: `i64(x)`, `u8(n)`, `f64(i)`. Rationale: it parenthesizes its
operand, sidesteps the precedence ambiguity of an `as`-operator, and matches the
constructor intuition already used for struct and variant construction — one fewer
special form than introducing a cast operator. The `as` keyword is left to its
existing `use`-aliasing role. Conversions are value-changing and always explicit:
truncation, sign reinterpretation, and int↔float rounding all happen only here.

### `const` and immutable values — coming, but typed

A typed `const` (and immutable bindings generally) is on the near roadmap: beyond
expressing intent, immutability opens optimization opportunities (constant folding,
propagation). Cooper's `const` will be **typed** — either annotated or inferred by
the same literal rules above — **not** an untyped arbitrary-precision constant. This
keeps the door open to compile-time constant folding without importing Go's constant
system. Arbitrary-precision untyped constants remain explicitly out of scope, likely
permanently.
