//! End-to-end frontend tests exercising resolution, type checking, and semantic
//! analysis through the public [`cooper::analyze`] entry point.

use cooper::analyze;

/// Assert `src` produces no diagnostics.
fn ok(src: &str) {
    let diags = analyze(src);
    assert!(
        diags.is_empty(),
        "expected no errors, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Assert at least one diagnostic from `src` mentions `needle`.
fn err(src: &str, needle: &str) {
    let diags = analyze(src);
    assert!(
        diags.iter().any(|d| d.message.contains(needle)),
        "expected an error containing {needle:?}, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// --- resolution ---

#[test]
fn resolves_function_and_parameters() {
    ok("func add(x: i32, y: i32): i32 {\n  return x + y\n}");
}

#[test]
fn undefined_type_is_reported() {
    err("func f(p: Widget): i32 {\n  return 0\n}", "undefined type: Widget");
}

#[test]
fn redeclared_function_is_reported() {
    err(
        "func f(): i32 {\n  return 0\n}\nfunc f(): i32 {\n  return 1\n}",
        "redeclared function f",
    );
}

#[test]
fn resolves_struct_members_and_methods() {
    ok("struct Point {\n  x: i32,\n  y: i32,\n}\n(p: Point) func sum(): i32 {\n  return p.x + p.y\n}");
}

#[test]
fn redeclared_struct_is_reported() {
    err(
        "struct Point {\n  x: i32,\n}\nstruct Point {\n  y: i32,\n}",
        "redeclared type Point",
    );
}

#[test]
fn duplicate_struct_member_is_reported() {
    err("struct Point {\n  x: i32,\n  x: i32,\n}", "duplicate member x");
}

#[test]
fn method_on_non_struct_receiver_is_reported() {
    err(
        "(x: i32) func f(): i32 {\n  return 0\n}",
        "must be a struct or pointer-to-struct",
    );
}

#[test]
fn redeclared_method_is_reported() {
    err(
        "struct Point {\n  x: i32,\n}\n(p: Point) func f(): i32 {\n  return 0\n}\n(p: Point) func f(): i32 {\n  return 1\n}",
        "redeclared method f",
    );
}

#[test]
fn method_field_collision_is_reported() {
    err(
        "struct Point {\n  x: i32,\n}\n(p: Point) func x(): i32 {\n  return 0\n}",
        "collides with a field",
    );
}

#[test]
fn generic_struct_requires_type_arguments() {
    err(
        "struct Box T {\n  value: T,\n}\nfunc f(b: Box): i32 {\n  return 0\n}",
        "requires 1 type argument",
    );
}

#[test]
fn generic_struct_instantiation_resolves() {
    ok("struct Box T {\n  value: T,\n}\nfunc f(b: Box i32): i32 {\n  return 0\n}");
}

#[test]
fn generic_arity_mismatch_is_reported() {
    err(
        "struct Pair A B {\n  first: A,\n  second: B,\n}\nfunc f(p: Pair i32): i32 {\n  return 0\n}",
        "expects 2 type argument(s), got 1",
    );
}

// --- type checking ---

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

// --- semantic analysis ---

#[test]
fn missing_return_in_all_paths_is_reported() {
    err(
        "func f(b: bool): i32 {\n  if b then {\n    return 1\n  }\n}",
        "does not return a value in all code paths",
    );
}

#[test]
fn return_in_both_branches_satisfies_all_paths() {
    ok("func f(b: bool): i32 {\n  if b then {\n    return 1\n  } else {\n    return 2\n  }\n}");
}

#[test]
fn unreachable_code_after_return_is_reported() {
    err(
        "func f(): i32 {\n  return 1\n  x := 2\n}",
        "unreachable code",
    );
}
