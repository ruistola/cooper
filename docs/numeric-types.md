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

Every conversion is total: it yields a value of the target type, lossy where it must be.
`rune(n)` gives U+FFFD, the replacement character, for an `n` that is not a Unicode scalar
value. Each conversion that can lose the number has a checked form in `std.math` that returns a
`Maybe`: `n.toU8()`, `f.toI32()`, `n.toRune()`, and so on
([Methods and Functions](./methods-and-functions.md#methods-of-built-in-types)), so the cautious
form is available without taxing every `u8(x)`.

A conversion changes the representation to keep the number. To keep the representation
instead, `f.bits()` is a float's IEEE 754 bits as the unsigned integer type of its width (`u32`
for `f32`, `u64` for `f64`), and `b.floatFromBits()` on a `u32` or `u64` is the float those
bits encode. For two finite floats of the same sign, `i64(b.bits()) - i64(a.bits())` is their
distance in units in the last place.

`string(x)` formats a number or a `bool` as text: integers in decimal, floats as the shortest
text that reads back as the same value (`string(0.1)` is `"0.1"`), bools as `true`/`false`.
It formats any other value too ([Type System](./type-system.md#formatting)), and a hole in a
string literal can choose a radix, precision, or width
([Syntax Fundamentals](./syntax-fundamentals.md#format-specs)).

`u64(p)` converts a pointer to its address, which no other conversion produces and nothing
converts back to a pointer: `"{u64(p):x}"` shows it in hexadecimal, and `u64(nil)` is `0`.

## Math functions

`std.math` has the mathematical functions as generic free functions, called with the module
prefix (`use { std.math }`):

```
math.min(a, b)      math.max(a, b)      math.clamp(x, lo, hi)      # numbers and runes
math.abs(x)                                                        # numbers
math.sqrt(x)        math.pow(x, y)      math.sin(x)                # f32 and f64
math.isNaN(x)       math.isInf(x)       math.isFinite(x)           # f32 and f64
```

Which types a function takes follows from what its body does with them
([Generics](./generics.md#requirements)), and an error names the operation that needs it:
`math.sqrt(i64(4))` is rejected because `sqrt` takes `f32` or `f64`. An integer literal that a
float function takes from the call site is a float, so `math.sqrt(4)` is `2.0`; that applies to
the functions of other modules, whose requirements are known when the call is checked.

The float functions are those of C's `math.h`, by the same names: `sqrt`, `cbrt`, `pow`,
`hypot`, `exp`, `exp2`, `log`, `log2`, `log10`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`,
`atan2`, `sinh`, `cosh`, `tanh`, `floor`, `ceil`, `round` (halves away from zero), and `trunc`,
each computed in the argument's type. `min` and `max` give NaN when either argument is NaN,
and `abs` of a signed integer type's most negative value overflows.

The constants `PI`, `E`, `SQRT2`, `LN2`, `LN10`, `INF`, and `NAN` are `f64`; `f32(math.PI)` is
the nearest `f32`.
