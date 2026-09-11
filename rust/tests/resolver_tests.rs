use cooper::run;

/// Run the full frontend over `src`, asserting no diagnostics are produced.
fn ok(src: &str) {
    let result = run(src);
    assert!(
        result.errors.is_empty(),
        "expected no errors, got: {:?}",
        result.errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Run the full frontend, asserting at least one diagnostic mentions `needle`.
fn err(src: &str, needle: &str) {
    let result = run(src);
    assert!(
        result.errors.iter().any(|d| d.message.contains(needle)),
        "expected an error containing {needle:?}, got: {:?}",
        result.errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn resolves_function_and_parameters() {
    ok("func add(x: i32, y: i32): i32 {\n  return x + y\n}");
}

#[test]
fn undefined_identifier_is_reported() {
    err(
        "func f(): i32 {\n  return y\n}",
        "undefined identifier: y",
    );
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
        "redeclared struct Point",
    );
}

#[test]
fn duplicate_struct_member_is_reported() {
    err(
        "struct Point {\n  x: i32,\n  x: i32,\n}",
        "duplicate member x",
    );
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

#[test]
fn walrus_binding_resolves_in_later_reference() {
    ok("func f(): i32 {\n  x := 5\n  return x\n}");
}

#[test]
fn oneof_and_match_binders_resolve() {
    ok(concat!(
        "oneof Option T {\n  Some(T),\n  None,\n}\n",
        "func unwrap(o: Option i32): i32 {\n",
        "  a := match o with {\n    Option.Some(v) => { v }\n    Option.None => { 0 }\n  }\n",
        "  return a\n}"
    ));
}

#[test]
fn block_scope_binding_does_not_leak() {
    err(
        "func f(): i32 {\n  {\n    y := 1\n  }\n  return y\n}",
        "undefined identifier: y",
    );
}
