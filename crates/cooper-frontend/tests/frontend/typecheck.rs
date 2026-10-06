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

#[test]
fn an_array_literal_takes_its_element_type_from_an_annotation_or_its_elements() {
    ok("func f(y: i64) {\n  let xs: u8[] = [1, 2]\n  ys := [1, y]\n  let zs: i64[] = ys\n  let fs = [1, 2.5]\n}");
}

#[test]
fn an_array_literal_must_be_homogeneous() {
    // The first element that is not a bare literal fixes the element type.
    err(
        "func f(x: i32) {\n  let xs = [x, \"s\"]\n}",
        "array element type mismatch: expected i32, found string",
    );
}

#[test]
fn an_empty_array_literal_needs_an_annotation() {
    ok("func f() {\n  let xs: f64[] = []\n}");
    err("func f() {\n  let xs = []\n}", "cannot infer the element type of an empty array literal");
}

#[test]
fn structs_tuples_and_sum_types_of_comparable_parts_compare() {
    ok(concat!(
        "struct P {\n  x: i32,\n  s: string,\n  n: i32^,\n}\n",
        "func f(a: P, b: P, p: i32^): bool {\n  return a == b and p == nil and (1, \"x\") == (1, \"x\")\n}",
    ));
}

#[test]
fn functions_arrays_and_aggregates_containing_them_are_not_comparable() {
    err("func f(xs: i32[]): bool {\n  return xs == xs\n}", "values of type i32[] are not comparable");
    err(
        "struct S {\n  xs: i32[],\n}\nfunc f(s: S): bool {\n  return s == s\n}",
        "values of type S are not comparable (it contains i32[])",
    );
    err(
        "func g() { }\nfunc f(): bool {\n  return g == g\n}",
        "are not comparable",
    );
}

#[test]
fn a_type_parameter_is_not_comparable() {
    err("func same T (a: T, b: T): bool {\n  return a == b\n}", "values of type T are not comparable");
}

#[test]
fn a_unit_function_returns_only_unit_values() {
    ok("func nothing() { }\nfunc f() {\n  return nothing()\n}");
    err("func f() {\n  return 5\n}", "cannot return a value of type i32 from a function returning unit");
}
