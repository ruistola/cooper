# Numeric Types

## The primitive numeric set

Cooper's built-in numeric types are the usual fixed-width grid:

* **Signed integers:** `i8`, `i16`, `i32`, `i64`
* **Unsigned integers:** `u8`, `u16`, `u32`, `u64`
* **Floating-point:** `f32`, `f64`

## Literals are typed by inference

A numeric literal's lexical form fixes its *class*; the uses of the literal across its
function body fix its *width*. Only a literal that no use constrains takes the class default.

* A literal with a fractional point or a decimal exponent (`3.14`, `1e9`) is a
  **float** literal; a hexadecimal (`0xFF`), binary (`0b1010`), or plain decimal
  (`42`) literal is an **integer** literal.
* An **integer literal** may take any numeric type — integer or float — so
  `let x: i64 = 5` and `let x: f32 = 5` both hold with no annotation on the literal.
* A **float literal** may take any float type.
* A literal that nothing constrains takes its class default once the whole body is solved:
  **`i32`** for integers, **`f32`** for floats.

Every use constrains the literal, wherever it appears in the body: annotations, `return`
types, call arguments, struct and variant field types, operands of an operator, and later
assignments. After `z := 1`, a later `z + y` with `y: i64` makes `z` an `i64`.

A literal that does not fit the range of its resolved integer type is a compile
error: `let x: u8 = 300`, `let x: i8 = 128`, and a negative literal for an unsigned
type are all rejected. A leading sign is part of the literal for this purpose, so
`-128` is checked against `i8`'s minimum and accepted.

## Arithmetic is same-type

A binary arithmetic or comparison operator requires both operands to share one
concrete numeric type. The operands constrain each other, so a literal operand takes
whatever width the other operand and the rest of the body determine, and operand order
does not matter: in `x + 1` with `x: i64`, the `1` is an `i64`. When every operand is an
unconstrained literal, they default together.

Integer arithmetic is checked: an overflowing result, division or remainder by zero, and
the signed `MIN / -1` stop the program with a runtime error. Float arithmetic follows IEEE.

The bitwise `and`, `or`, and `xor` take two operands of one integer type, like arithmetic,
and evaluate both. A shift `x << n` or `x >> n` has the type of `x`; the count `n` is not
combined with `x` arithmetically and may be of any integer type, so `x << n` needs no
conversion when `n` is an `i32` and `x` a `u64`. A count at least the width of `x`'s type,
or negative, stops the program; bits a left shift moves out are discarded rather than
reported as overflow. `>>` is arithmetic on signed integers (it copies the sign bit) and
logical on unsigned ones.

## Conversions are explicit and constructor-style

A numeric conversion is spelled as a **constructor-style call on the target
primitive**: `i64(x)`, `u8(n)`, `f64(i)`, and is the only place a value changes
representation. An integer conversion truncates to a narrower type and extends to a
wider one by the source's signedness (`u32(-7)` is `4294967289`). A float-to-integer
conversion rounds toward zero and saturates at the target's range, with NaN becoming 0
(`i32(1e10)` is `2147483647`).

`string(x)` formats a number or a `bool` as text (and converts a C string; see
[C Interop](./c-interop.md#strings-and-buffers)): integers in decimal, floats as the shortest
text that reads back as the same value (`string(0.1)` is `"0.1"`), bools as `true`/`false`.
