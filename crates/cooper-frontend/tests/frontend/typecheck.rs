//! Type checking of bindings, calls, structs, sum types, and pointers.

use crate::{ok, err};

#[test]
fn walrus_binding_resolves_in_later_reference() {
    ok("func f(): i32 {\n  x := 5\n  return x\n}");
}

#[test]
fn undefined_variable_is_reported() {
    err("func f(): i32 {\n  return y\n}", "undefined variable: y");
}

#[test]
fn var_decl_type_mismatch_is_reported() {
    err(
        "func f() {\n  let x: i32 = true\n}",
        "declared as i32 but initialized with bool",
    );
}

#[test]
fn return_type_mismatch_is_reported() {
    err("func f(): i32 {\n  return true\n}", "return type mismatch");
}

#[test]
fn binary_operand_mismatch_is_reported() {
    err("func f(): i32 {\n  return 1 + true\n}", "invalid operands");
}

// --- numeric literals and arithmetic ---

#[test]
fn calling_non_function_is_reported() {
    err(
        "func f() {\n  x := 5\n  x()\n}",
        "cannot call non-function value",
    );
}

#[test]
fn wrong_argument_count_is_reported() {
    err(
        "func g(x: i32): i32 {\n  return x\n}\nfunc f(): i32 {\n  return g()\n}",
        "wrong number of arguments",
    );
}

#[test]
fn struct_literal_and_field_access_type_check() {
    ok("struct Point {\n  x: i32,\n  y: i32,\n}\nfunc f(): i32 {\n  p := Point{ x: 1, y: 2 }\n  return p.x\n}");
}

#[test]
fn unassigned_struct_member_is_reported() {
    err(
        "struct Point {\n  x: i32,\n  y: i32,\n}\nfunc f() {\n  p := Point{ x: 1 }\n}",
        "is not assigned a value",
    );
}

#[test]
fn generic_variant_construction_infers_arguments() {
    ok(concat!(
        "oneof Result T E {\n  Ok(T),\n  Err(E),\n}\n",
        "func f(): Result i32 string {\n  return Result.Ok(5)\n}"
    ));
}

#[test]
fn payload_free_variant_needs_annotation_context() {
    ok(concat!(
        "oneof Maybe T {\n  Some(T),\n  None,\n}\n",
        "func f(): Maybe i32 {\n  return Maybe.None\n}"
    ));
}

#[test]
fn non_exhaustive_match_is_reported() {
    err(
        concat!(
            "oneof Shape {\n  Circle(i32),\n  Rect(i32, i32),\n}\n",
            "func area(s: Shape): i32 {\n  a := match s with {\n    Shape.Circle(r) => { r }\n  }\n  return a\n}"
        ),
        "non-exhaustive match",
    );
}

#[test]
fn match_expression_binds_and_unifies_arms() {
    ok(concat!(
        "oneof Option T {\n  Some(T),\n  None,\n}\n",
        "func unwrap(o: Option i32): i32 {\n",
        "  a := match o with {\n    Option.Some(v) => { v }\n    Option.None => { 0 }\n  }\n",
        "  return a\n}"
    ));
}

#[test]
fn dereferencing_non_pointer_is_reported() {
    err("func f(): i32 {\n  x := 5\n  return x^\n}", "cannot dereference");
}

// --- bare variants under an expected type ---
