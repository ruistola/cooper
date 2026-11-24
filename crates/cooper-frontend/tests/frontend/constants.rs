//! Constants: `const` declarations, their compile-time evaluation, and their uses.

use super::{err, ok};

#[test]
fn constants_give_values_array_lengths_and_patterns() {
    ok("const K: i64 = 1 << 10
const BufSize: i64 = 4 * K
const Min: i8 = -128
const Ratio: f64 = f64(BufSize) / 3
const Greeting: string = \"hello, \" + \"world\"
const Verbose: bool = BufSize > 1000 and Min < 0
struct Buffer {
  bytes: u8[BufSize],
}
func kind(n: i64): string {
  return match n with {
    K => \"kilo\"
    BufSize => \"buffer\"
    _ => \"other\"
  }
}
func main() {
  let b: Buffer
  n := b.bytes.length() + BufSize
  s := Greeting + kind(n)
  r := Ratio * 2.0
}");
}

#[test]
fn a_local_constant_is_scoped_like_a_variable() {
    ok("const Size: i64 = 8
func main() {
  const Size: i64 = 2 * 3
  let buf: u8[Size]
  show := func(): i64 {
    const Inner: i64 = Size + 1
    let more: i32[Inner]
    return buf.length() + more.length()
  }
  n := show()
}");
    err("func main() {\n  x := Limit\n  const Limit: i32 = 1\n}", "undefined variable: Limit");
    err("func main() {\n  n := 3\n  const N: i32 = n\n}", "a constant's initializer uses only literals");
}

#[test]
fn a_constant_is_evaluated_in_its_declared_type() {
    err("const Big: i8 = 100 + 28", "constant overflows i8");
    err("const Zero: i32 = 0\nconst Bad: i32 = 1 / Zero", "division by zero");
    err("const Wide: i64 = 1\nconst Narrow: i32 = Wide", "cannot assign i64 to a constant of type i32");
    err("const Half: i32 = 1.5", "cannot assign f32 to a constant of type i32");
}

#[test]
fn a_constant_initializer_is_a_constant_expression() {
    err("func one(): i32 {\n  return 1\n}\nconst One: i32 = one()", "a constant's initializer cannot call a function");
    err("let x: i32 = 1\nconst X: i32 = x", "a constant's initializer uses only literals");
    err("const A: i32 = B + 1\nconst B: i32 = A", "is defined in terms of itself");
}

#[test]
fn a_constant_is_a_named_value_not_storage() {
    err("const N: i32 = 1\nfunc main() {\n  N = 2\n}", "cannot assign to constant N");
    err("const N: i32 = 1\nfunc main() {\n  p := &N\n}", "cannot take the address");
    err("func main() {\n  const N: i32 = 1\n  N += 1\n}", "cannot assign to constant N");
    err("const Sizes: i32[] = 1", "a constant is a number, bool, rune, or string");
    err("const size: i32 = 1", "a constant's name is PascalCase");
}

#[test]
fn an_array_length_constant_is_a_non_negative_integer() {
    err("const Name: string = \"n\"\nstruct S {\n  a: u8[Name],\n}", "an array length is a non-negative integer");
    err("struct S {\n  a: u8[Missing],\n}", "undefined constant Missing");
}
