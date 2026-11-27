//! Constants: `const` declarations, their compile-time evaluation, and their uses.

use super::{err, ok};

#[test]
fn constants_give_values_array_lengths_and_patterns() {
    ok("const K: i64 = 1 << 10
const BUF_SIZE: i64 = 4 * K
const MIN: i8 = -128
const RATIO: f64 = f64(BUF_SIZE) / 3
const GREETING: string = \"hello, \" + \"world\"
const VERBOSE: bool = BUF_SIZE > 1000 and MIN < 0
struct Buffer {
  bytes: u8[BUF_SIZE],
}
func kind(n: i64): string {
  return match n with {
    K => \"kilo\"
    BUF_SIZE => \"buffer\"
    _ => \"other\"
  }
}
func main() {
  let b: Buffer
  n := b.bytes.length() + BUF_SIZE
  s := GREETING + kind(n)
  r := RATIO * 2.0
}");
}

#[test]
fn a_local_constant_is_scoped_like_a_variable() {
    ok("const SIZE: i64 = 8
func main() {
  const SIZE: i64 = 2 * 3
  let buf: u8[SIZE]
  show := func(): i64 {
    const INNER: i64 = SIZE + 1
    let more: i32[INNER]
    return buf.length() + more.length()
  }
  n := show()
}");
    err("func main() {\n  x := LIMIT\n  const LIMIT: i32 = 1\n}", "undefined variable: LIMIT");
    err("func main() {\n  n := 3\n  const N: i32 = n\n}", "a constant's initializer uses only literals");
}

#[test]
fn a_constant_is_evaluated_in_its_declared_type() {
    err("const BIG: i8 = 100 + 28", "constant overflows i8");
    err("const ZERO: i32 = 0\nconst BAD: i32 = 1 / ZERO", "division by zero");
    err("const WIDE: i64 = 1\nconst NARROW: i32 = WIDE", "cannot assign i64 to a constant of type i32");
    err("const HALF: i32 = 1.5", "cannot assign f32 to a constant of type i32");
}

#[test]
fn a_constant_initializer_is_a_constant_expression() {
    err("func one(): i32 {\n  return 1\n}\nconst ONE: i32 = one()", "a constant's initializer cannot call a function");
    err("let x: i32 = 1\nconst X: i32 = x", "a constant's initializer uses only literals");
    err("const A: i32 = B + 1\nconst B: i32 = A", "is defined in terms of itself");
}

#[test]
fn a_constant_is_a_named_value_not_storage() {
    err("const N: i32 = 1\nfunc main() {\n  N = 2\n}", "cannot assign to constant N");
    err("const N: i32 = 1\nfunc main() {\n  p := &N\n}", "cannot take the address");
    err("func main() {\n  const N: i32 = 1\n  N += 1\n}", "cannot assign to constant N");
    err("const SIZES: i32[] = 1", "a constant is a number, bool, rune, or string");
    err("const size: i32 = 1", "a constant's name is SCREAMING_SNAKE_CASE");
    err("const BufSize: i32 = 1", "a constant's name is SCREAMING_SNAKE_CASE");
}

#[test]
fn an_array_length_constant_is_a_non_negative_integer() {
    err("const NAME: string = \"n\"\nstruct S {\n  a: u8[NAME],\n}", "an array length is a non-negative integer");
    err("struct S {\n  a: u8[MISSING],\n}", "undefined constant MISSING");
}
