//! Loops, ranges, and array iteration, indexing, and builtins.

use crate::{ok, err};

#[test]
fn range_for_binds_an_integer_element() {
    ok("func f(): i32 {\n  total := 0\n  for i in 0..10 do total += i\n  return total\n}");
}

#[test]
fn inclusive_range_for_binds_an_integer_element() {
    ok("func f(): i32 {\n  total := 0\n  for i in 0..=10 do total += i\n  return total\n}");
}

#[test]
fn range_over_non_integer_bounds_is_reported() {
    err(
        "func f() {\n  for x in 0..true do {}\n}",
        "range bounds must",
    );
}

#[test]
fn array_iteration_binds_element_and_index_value_pair() {
    // The index is an `i64`, like an array's length.
    ok("func f(xs: i32[]): i64 {\n  let total: i64 = 0\n  for v in xs do total += i64(v)\n  for (i, v) in xs do total += i + i64(v)\n  return total\n}");
}

#[test]
fn array_exposes_a_blessed_length_method() {
    // `length` is a compiler-blessed method on any array, yielding an `i64` count.
    ok("func f(xs: i32[]): i64 {\n  return xs.length()\n}");
}

#[test]
fn array_index_syntax_binds_to_blessed_get_and_set() {
    // `a[i]` reads like the blessed `get` and `a[i] = v` writes like `set`; the
    // explicit methods take an `i64` index.
    ok("func f(xs: i32[], i: i64): i32 {\n  xs[i] = xs.get(i) + 1\n  xs.set(i, 0)\n  return xs[i]\n}");
}

#[test]
fn index_syntax_accepts_any_integer_type_but_nothing_else() {
    ok("func f(xs: i32[], a: i32, b: u8, c: i64): i32 {\n  return xs[a] + xs[b] + xs[c] + xs[0]\n}");
    err(
        "func f(xs: i32[], i: f64): i32 {\n  return xs[i]\n}",
        "index must be an integer, found f64",
    );
}

#[test]
fn a_user_type_is_not_indexable() {
    // Index syntax is a built-in-array privilege: a user struct is not indexable even
    // when it declares a method named `get` — declaring the name grants no sugar.
    err(
        "struct Slots {\n  data: i32[],\n}\n(m: Slots) func get(i: u64): i32 {\n  return m.data[i]\n}\nfunc f(m: Slots): i32 {\n  return m[0]\n}",
        "type Slots cannot be indexed",
    );
}

#[test]
fn a_user_type_is_not_index_assignable() {
    // Likewise `m[i] = v`: a user struct with a `set` method is not index-assignable.
    err(
        "struct Slots {\n  data: i32[],\n}\n(m: Slots) func set(i: u64, v: i32) {\n  m.data[i] = v\n}\nfunc f(m: Slots) {\n  m[0] = 7\n}",
        "type Slots cannot be assigned by index",
    );
}

#[test]
fn addressing_a_built_in_array_element_is_allowed() {
    // A built-in array's element lives in the backing buffer, so `&xs[i]` is a place.
    ok("func f(xs: i32[]): i32^ {\n  return &xs[0]\n}");
}



#[test]
fn array_push_follows_the_return_value_idiom() {
    // `push` hands back the possibly-new array, reassigned in place; the element type
    // flows into the argument.
    ok("func f(xs: i32[]): i32 {\n  xs = xs.push(9)\n  return xs.get(0)\n}");
}

#[test]
fn pushing_the_wrong_element_type_is_reported() {
    err(
        "func f(xs: i32[], s: string): i32[] {\n  return xs.push(s)\n}",
        "expected i32, found string",
    );
}



#[test]
fn unknown_array_method_is_reported() {
    err(
        "func f(xs: i32[]): i32 {\n  return xs.size()\n}",
        "size is not a method of array type",
    );
}


#[test]
fn iterating_a_non_iterable_type_is_reported() {
    err("func f() {\n  for x in true do {}\n}", "cannot iterate over type bool");
}

#[test]
fn conditional_loops_check_their_bodies() {
    ok("func f(): i32 {\n  n := 0\n  while n < 5 do {\n    n += 1\n    if n == 3 then continue\n    if n == 4 then break\n  }\n  repeat { n -= 1 } until n == 0\n  return n\n}");
}

#[test]
fn break_outside_a_loop_is_reported() {
    err("func f() {\n  break\n}", "'break' outside of a loop");
}

#[test]
fn continue_outside_a_loop_is_reported() {
    err("func f() {\n  continue\n}", "'continue' outside of a loop");
}
