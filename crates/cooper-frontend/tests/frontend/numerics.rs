//! Numeric literals, same-type arithmetic, and explicit conversions.

use crate::{ok, err};

#[test]
fn integer_literal_adopts_the_annotated_width() {
    // A bare integer literal takes the width its context demands, so no suffix is
    // needed to initialize a non-default integer type.
    ok("func f(): i64 {\n  let x: i64 = 5\n  return x\n}");
}

#[test]
fn float_literal_adopts_the_annotated_width() {
    ok("func f(): f64 {\n  let x: f64 = 3.14\n  return x\n}");
}

#[test]
fn integer_literal_initializes_a_float() {
    // An integer literal may seed a floating-point binding; a float literal may not
    // seed an integer one.
    ok("func f(): f32 {\n  let x: f32 = 5\n  return x\n}");
}

#[test]
fn float_literal_cannot_initialize_an_integer() {
    err(
        "func f() {\n  let x: i32 = 3.14\n}",
        "declared as i32 but initialized with f32",
    );
}

#[test]
fn unsigned_types_are_recognized() {
    ok("func f(): u8 {\n  let x: u8 = 200\n  return x\n}");
}

#[test]
fn literal_borrows_its_width_from_the_other_operand() {
    // `x + 1` types `1` as `i64` from `x`, so a widened variable needs no suffix on
    // the literals it is combined with.
    ok("func f(x: i64): i64 {\n  return x + 1\n}");
}

#[test]
fn mixed_width_arithmetic_is_rejected() {
    // Two differently-typed numbers do not combine implicitly: there is no promotion.
    err(
        "func f(x: i32, y: i64): i64 {\n  return x + y\n}",
        "invalid operands",
    );
}

#[test]
fn default_integer_literal_is_i32() {
    // With no annotation, a bare integer literal is i32; combining it with an i64
    // therefore fails, pinning the default.
    err(
        "func f(y: i64): i64 {\n  z := 1\n  return z + y\n}",
        "invalid operands",
    );
}

#[test]
fn literal_exceeding_its_type_is_reported() {
    err(
        "func f() {\n  let x: u8 = 300\n}",
        "out of range for u8",
    );
}

#[test]
fn signed_minimum_literal_is_in_range() {
    // -128 fits i8 exactly; the sign folds into the literal so it is checked against
    // the minimum, not rejected as the out-of-range positive 128.
    ok("func f(): i8 {\n  let x: i8 = -128\n  return x\n}");
}

#[test]
fn signed_maximum_boundary_is_enforced() {
    err(
        "func f() {\n  let x: i8 = 128\n}",
        "out of range for i8",
    );
}

#[test]
fn negating_an_unsigned_literal_is_reported() {
    err(
        "func f() {\n  let x: u16 = -1\n}",
        "cannot negate a literal of unsigned type u16",
    );
}

#[test]
fn hexadecimal_literal_is_range_checked() {
    // 0x1FF = 511 does not fit u8.
    err(
        "func f() {\n  let x: u8 = 0x1FF\n}",
        "out of range for u8",
    );
}

#[test]
fn numeric_conversion_widens_an_integer() {
    ok("func f(): i64 {\n  let x: i32 = 5\n  return i64(x)\n}");
}

#[test]
fn numeric_conversion_between_int_and_float() {
    ok("func f(): f64 {\n  let n: i32 = 3\n  return f64(n)\n}");
    ok("func f(): i32 {\n  let x: f64 = 3.5\n  return i32(x)\n}");
}

#[test]
fn numeric_conversion_between_signednesses() {
    ok("func f(): u8 {\n  let x: i32 = 200\n  return u8(x)\n}");
}

#[test]
fn converting_a_non_numeric_value_is_reported() {
    err(
        "struct S {}\nfunc f() {\n  let s: S = S{}\n  let x: i32 = i32(s)\n}",
        "source must be numeric",
    );
}

#[test]
fn numeric_conversion_wrong_arity_is_reported() {
    err(
        "func f() {\n  let x: i32 = i32()\n}",
        "takes exactly one argument",
    );
}
