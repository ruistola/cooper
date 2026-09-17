# Numeric Types

## The primitive numeric set

Cooper's built-in numeric types are the usual fixed-width grid:

* **Signed integers:** `i8`, `i16`, `i32`, `i64`
* **Unsigned integers:** `u8`, `u16`, `u32`, `u64`
* **Floating-point:** `f32`, `f64`

## Literals are typed by context

A numeric literal's lexical form fixes its *class*; the surrounding context fixes its
*width*. Absent a usable context, the class default applies.

* A literal with a fractional point or a decimal exponent (`3.14`, `1e9`) is a
  **float** literal; a hexadecimal (`0xFF`), binary (`0b1010`), or plain decimal
  (`42`) literal is an **integer** literal.
* An **integer literal** adopts any expected numeric type — integer or float — so
  `let x: i64 = 5` and `let x: f32 = 5` both hold with no annotation on the literal.
* A **float literal** adopts an expected float type.
* With **no expected type**, a literal takes its class default: **`i32`** for
  integers, **`f32`** for floats.

The expected type flows from annotations, `return` types, call-argument types, and
struct and variant field types.

A literal that does not fit the range of its resolved integer type is a compile
error: `let x: u8 = 300`, `let x: i8 = 128`, and a negative literal for an unsigned
type are all rejected. A leading sign is part of the literal for this purpose, so
`-128` is checked against `i8`'s minimum and accepted.

## Arithmetic is same-type

A binary arithmetic or comparison operator requires both operands to share one
concrete numeric type. To keep this ergonomic, a bare numeric literal on one side of
an operator **borrows its width from the other, non-literal operand**: in `x + 1`
with `x: i64`, the `1` is typed `i64`. When both sides are literals, or neither is,
the enclosing expected type guides both equally.

## Conversions are explicit and constructor-style

A numeric conversion is spelled as a **constructor-style call on the target
primitive**: `i64(x)`, `u8(n)`, `f64(i)`. Conversions are value-changing — truncation,
sign reinterpretation, and integer/float rounding all happen here — and are always
written explicitly at the point they occur.
