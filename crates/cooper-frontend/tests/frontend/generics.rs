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
        "oneof Maybe T {\n  None,\n  Some(T),\n}\n",
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
        "oneof Result T E {\n  Ok(T),\n  Err(E),\n}\n",
        "struct Box T {\n  v: T,\n}\n",
        "func done(): Result i32 () {\n  return Err(())\n}\n",
        "func call T (h: func(): T): T {\n  return h()\n}\n",
        "func nothing() { }\n",
        "func f() {\n  let b: Box () = Box{v: ()}\n  call(nothing)\n}",
    ));
}
