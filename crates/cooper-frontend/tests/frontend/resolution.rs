//! Declaration resolution: symbol collection, signatures, and generic arity.

use crate::{ok, err};

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

#[test]
fn a_type_parameter_may_name_a_concrete_instantiation_in_a_parameter() {
    // A concrete instantiation is a legal parameter type: the function accepts only
    // `Pair i32 bool`, with `i32`/`bool` resolved concretely (not as binders).
    ok("struct Pair A B {\n  first: A,\n  second: B,\n}\n\
        func f(p: Pair i32 bool): i32 {\n  return 0\n}");
}

#[test]
fn a_type_parameter_colliding_with_a_type_is_reported() {
    // A binder may not reuse a concrete type's name; the distinction between a
    // parameter and a concrete type rests on the binder, not on casing.
    err(
        "struct Celsius {\n}\nfunc convert Celsius (x: Celsius): Celsius {\n  return x\n}",
        "collides with a type of the same name",
    );
}

#[test]
fn a_receiver_binder_naming_a_concrete_type_is_reported() {
    // A receiver slot is a binder, so a concrete type there would silently bind a
    // phantom parameter; it is rejected instead.
    err(
        "struct Box A {\n  value: A,\n}\n(b: Box i32) func get(): i32 {\n  return 0\n}",
        "collides with a type of the same name",
    );
}

#[test]
fn a_duplicate_type_parameter_is_reported() {
    err(
        "func f T T (x: T): T {\n  return x\n}",
        "duplicate type parameter T",
    );
}

#[test]
fn a_type_may_refer_to_one_declared_later() {
    ok("struct A {\n  b: B,\n}\nstruct B {\n  x: i32,\n}");
}

#[test]
fn a_type_may_refer_to_itself_through_a_pointer_or_an_array() {
    ok(concat!(
        "struct Node {\n  next: Node^,\n  kids: Node[],\n}\n",
        "oneof List T {\n  Nil,\n  Cons(T, (List T)^),\n}\n",
        "func f(n: Node): i32 {\n  return 0\n}",
    ));
}

#[test]
fn a_type_containing_itself_by_value_is_reported() {
    err("struct P {\n  n: P,\n}", "type P contains itself by value");
    // Through a generic argument: `Box Q` holds a `Q` inline.
    err(
        "struct Box T {\n  v: T,\n}\nstruct Q {\n  b: Box Q,\n}",
        "type Q contains itself by value",
    );
    err(
        "struct G T {\n  g: G (T, T),\n}",
        "recurse through a pointer `(G T)^`",
    );
}

#[test]
fn a_function_named_like_a_type_is_reported() {
    err("struct B {\n  x: i32,\n}\nfunc B() { }", "redeclared function B");
}
