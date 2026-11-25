//! Generic functions and methods: type arguments inferred at each reference.

use crate::{err, ok};

const BOX: &str = "struct Box T {\n  v: T,\n}\n";

#[test]
fn a_generic_call_infers_its_type_argument_from_an_argument() {
    ok("func id T (x: T): T {\n  return x\n}\nfunc f(): string {\n  return id(\"s\")\n}");
}

#[test]
fn a_literal_argument_takes_its_width_from_the_expected_result() {
    // The annotation fixes `T` before the argument is checked, so `3` is an `i64`.
    ok("func id T (x: T): T {\n  return x\n}\nfunc f() {\n  let a: i64 = id(3)\n}");
}

#[test]
fn a_type_argument_only_in_the_result_comes_from_the_expected_type() {
    ok(concat!(
        "union Maybe T {\n  None,\n  Some(T),\n}\n",
        "func none T (): Maybe T {\n  return None\n}\n",
        "func f() {\n  let m: Maybe i32 = none()\n}",
    ));
}

#[test]
fn an_undetermined_type_argument_is_reported() {
    err(
        "func make T (): T[] {\n  return []\n}\nfunc f() {\n  let xs = make()\n}",
        "cannot infer type argument T for function make",
    );
}

#[test]
fn arguments_binding_one_parameter_inconsistently_are_reported() {
    err(
        "func pick T (a: T, b: T): T {\n  return a\n}\nfunc f() {\n  let x = pick(1, \"a\")\n}",
        "argument 2 type mismatch: expected i32, found string",
    );
}

#[test]
fn a_method_on_a_generic_struct_takes_the_receivers_type_arguments() {
    ok(&format!(
        "{BOX}(b: Box T) func get(): T {{\n  return b.v\n}}\nfunc f(b: Box i32): i32 {{\n  return b.get()\n}}"
    ));
}

#[test]
fn a_generic_method_infers_its_own_type_parameter() {
    ok(&format!(
        "{BOX}(b: Box T) func map U (f: func(T): U): Box U {{\n  return Box{{v: f(b.v)}}\n}}\n\
         func len(s: string): i32 {{\n  return 1\n}}\n\
         func f(b: Box string): Box i32 {{\n  return b.map(len)\n}}"
    ));
}

#[test]
fn a_callers_type_parameter_is_distinct_from_a_same_named_callee_parameter() {
    // `conv`'s `T` and `U` swap roles relative to `Box`/`map`'s; inference keeps
    // the callee's binders apart from the caller's.
    ok(&format!(
        "{BOX}(b: Box T) func map U (f: func(T): U): Box U {{\n  return Box{{v: f(b.v)}}\n}}\n\
         func conv T U (b: Box U, f: func(U): T): Box T {{\n  return b.map(f)\n}}"
    ));
}

#[test]
fn a_generic_function_passed_as_an_argument_is_instantiated_by_its_parameter() {
    ok(concat!(
        "func id T (x: T): T {\n  return x\n}\n",
        "func apply T (f: func(T): T, x: T): T {\n  return f(x)\n}\n",
        "func f(): i32 {\n  return apply(id, 2)\n}",
    ));
}

#[test]
fn a_generic_function_value_needs_an_expected_function_type() {
    ok("func id T (x: T): T {\n  return x\n}\nfunc f() {\n  let g: func(i64): i64 = id\n}");
    err(
        "func id T (x: T): T {\n  return x\n}\nfunc f() {\n  let g = id\n}",
        "cannot infer type argument T for function id",
    );
}

#[test]
fn unit_is_a_type_argument_like_any_other() {
    ok(concat!(
        "union Result T E {\n  Ok(T),\n  Err(E),\n}\n",
        "struct Box T {\n  v: T,\n}\n",
        "func done(): Result i32 () {\n  return Err(())\n}\n",
        "func call T (h: func(): T): T {\n  return h()\n}\n",
        "func nothing() { }\n",
        "func f() {\n  let b: Box () = Box{v: ()}\n  call(nothing)\n}",
    ));
}

#[test]
fn a_generic_body_requires_of_its_type_parameters_what_it_does_with_them() {
    let same = "func same T (a: T, b: T): bool {\n  return a == b\n}\n";
    ok(&format!("{same}struct P {{\n  x: i32,\n}}\nfunc f() {{\n  same(1, 2)\n  same(P{{x: 1}}, P{{x: 2}})\n}}"));
    err(&format!("{same}func f() {{\n  same([1], [2])\n}}"), "values of type i32[] are not comparable");
}

#[test]
fn a_generic_caller_passes_on_the_requirements_of_what_it_calls() {
    let max = "func max T (a: T, b: T): T {\n  if a < b then return b\n  return a\n}\n";
    let largest = "func largest T (xs: T[]): T {\n  m := xs[0]\n  for x in xs do m = max(m, x)\n  return m\n}\n";
    ok(&format!("{max}{largest}func f() {{\n  largest([1.5, 2.5])\n}}"));
    err(&format!("{max}{largest}func f() {{\n  largest([true])\n}}"), "values of type bool cannot be ordered");
}

#[test]
fn a_literal_requires_a_type_parameter_that_holds_it() {
    let big = "func isBig T (x: T): bool {\n  return x > 300\n}\n";
    ok(&format!("{big}func f() {{\n  isBig(5)\n  isBig(2.5)\n}}"));
    err(&format!("{big}func f() {{\n  isBig(u8(5))\n}}"), "integer literal 300 is out of range for u8");
}

#[test]
fn a_nested_generic_function_cannot_require_anything_of_its_type_parameters() {
    err(
        "func f() {\n  func less U (a: U, b: U): bool {\n    return a < b\n  }\n}",
        "invalid operands for <: U and U",
    );
}

#[test]
fn the_float_functions_of_std_math_take_floats_and_make_integer_literals_floats() {
    ok("use { std.math as math }\nfunc f() {\n  a := math.sqrt(4)\n  b := math.sqrt(f32(2))\n}");
    err("use { std.math as math }\nfunc f() {\n  a := math.sqrt(i64(4))\n}", "i64 is not a float type");
}
