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

// --- numeric literals and arithmetic ---

#[test]
fn integer_literal_adopts_the_annotated_width() {
    // A bare integer literal takes the width its context demands, so no suffix is
    // needed to initialize a non-default integer type.
    ok("func f(): i64 {\n  let x: i64 = 5\n  return x\n}");
}

#[test]
fn float_literal_adopts_the_annotated_width() {
    ok("func f(): f64 {\n  let x: f64 = 3.14\n  return x\n}");
}

#[test]
fn integer_literal_initializes_a_float() {
    // An integer literal may seed a floating-point binding; a float literal may not
    // seed an integer one.
    ok("func f(): f32 {\n  let x: f32 = 5\n  return x\n}");
}

#[test]
fn float_literal_cannot_initialize_an_integer() {
    err(
        "func f() {\n  let x: i32 = 3.14\n}",
        "declared as i32 but initialized with f32",
    );
}

#[test]
fn unsigned_types_are_recognized() {
    ok("func f(): u8 {\n  let x: u8 = 200\n  return x\n}");
}

#[test]
fn literal_borrows_its_width_from_the_other_operand() {
    // `x + 1` types `1` as `i64` from `x`, so a widened variable needs no suffix on
    // the literals it is combined with.
    ok("func f(x: i64): i64 {\n  return x + 1\n}");
}

#[test]
fn mixed_width_arithmetic_is_rejected() {
    // Two differently-typed numbers do not combine implicitly: there is no promotion.
    err(
        "func f(x: i32, y: i64): i64 {\n  return x + y\n}",
        "invalid operands",
    );
}

#[test]
fn default_integer_literal_is_i32() {
    // With no annotation, a bare integer literal is i32; combining it with an i64
    // therefore fails, pinning the default.
    err(
        "func f(y: i64): i64 {\n  z := 1\n  return z + y\n}",
        "invalid operands",
    );
}

#[test]
fn literal_exceeding_its_type_is_reported() {
    err(
        "func f() {\n  let x: u8 = 300\n}",
        "out of range for u8",
    );
}

#[test]
fn signed_minimum_literal_is_in_range() {
    // -128 fits i8 exactly; the sign folds into the literal so it is checked against
    // the minimum, not rejected as the out-of-range positive 128.
    ok("func f(): i8 {\n  let x: i8 = -128\n  return x\n}");
}

#[test]
fn signed_maximum_boundary_is_enforced() {
    err(
        "func f() {\n  let x: i8 = 128\n}",
        "out of range for i8",
    );
}

#[test]
fn negating_an_unsigned_literal_is_reported() {
    err(
        "func f() {\n  let x: u16 = -1\n}",
        "cannot negate a literal of unsigned type u16",
    );
}

#[test]
fn hexadecimal_literal_is_range_checked() {
    // 0x1FF = 511 does not fit u8.
    err(
        "func f() {\n  let x: u8 = 0x1FF\n}",
        "out of range for u8",
    );
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

// --- bare variants under an expected type ---

/// A shared sum-type preamble for the bare-variant cases.
const RESULT: &str = "oneof Result T E {\n  Ok(T),\n  Err(E),\n}\n";
const MAYBE: &str = "oneof Maybe T {\n  Some(T),\n  None,\n}\n";

#[test]
fn bare_variant_construction_uses_return_context() {
    ok(&format!(
        "{RESULT}func f(): Result i32 string {{\n  return Ok(5)\n}}"
    ));
}

#[test]
fn bare_payload_free_variant_uses_return_context() {
    ok(&format!("{MAYBE}func f(): Maybe i32 {{\n  return None\n}}"));
}

#[test]
fn bare_variant_uses_let_annotation_context() {
    ok(&format!(
        "{MAYBE}func f() {{\n  let a: Maybe i32 = Some(5)\n  let b: Maybe i32 = None\n}}"
    ));
}

#[test]
fn bare_variant_uses_call_argument_context() {
    ok(&format!(
        "{MAYBE}func take(m: Maybe i32): i32 {{\n  return 0\n}}\nfunc f() {{\n  take(Some(1))\n  take(None)\n}}"
    ));
}

#[test]
fn bare_and_qualified_variants_coexist() {
    ok(&format!(
        "{MAYBE}func f() {{\n  let a: Maybe i32 = Some(5)\n  let b: Maybe i32 = Maybe.None\n}}"
    ));
}

#[test]
fn bare_variant_patterns_match_on_scrutinee() {
    ok(&format!(
        "{RESULT}func f(r: Result i32 string): i32 {{\n  a := match r with {{\n    Ok(n) => {{ n }}\n    Err(e) => {{ 0 }}\n  }}\n  return a\n}}"
    ));
}

#[test]
fn bare_variant_without_context_is_reported() {
    err(
        &format!("{RESULT}func f() {{\n  let x = Ok(5)\n}}"),
        "cannot infer sum type for bare variant Ok",
    );
}

#[test]
fn bare_payload_free_variant_without_context_is_reported() {
    err(
        &format!("{MAYBE}func f() {{\n  let x = None\n}}"),
        "cannot infer sum type for bare variant None",
    );
}

#[test]
fn bare_variant_foreign_to_expected_type_is_reported() {
    err(
        &format!("{RESULT}{MAYBE}func f(): Maybe i32 {{\n  return Ok(5)\n}}"),
        "cannot infer sum type for bare variant Ok",
    );
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

// --- projects and modules ---

/// A module spanning several source files unites their top-level declarations into
/// one namespace, so a function in one file may call one defined in another.
#[test]
fn module_unites_declarations_across_files() {
    use cooper::{Module, Project, ProjectKind, SourceFile};

    let project = Project::new(
        "multifile",
        ProjectKind::Program,
        vec![Module::new(
            "main",
            vec![
                SourceFile::new("a", "func one(): i32 {\n  return 1\n}"),
                SourceFile::new("b", "func two(): i32 {\n  return one() + 1\n}"),
            ],
        )],
    );

    let diags = cooper::analyze_project(&project);
    assert!(
        diags.is_empty(),
        "expected no errors, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Build a program from one-file modules, each given as `(module path, source)`, and
/// return its diagnostics.
fn project_diags(modules: &[(&str, &str)]) -> Vec<cooper::Diagnostic> {
    use cooper::{Module, Project, ProjectKind, SourceFile};
    let modules = modules
        .iter()
        .map(|(path, src)| Module::new(path, vec![SourceFile::new(*path, *src)]))
        .collect();
    cooper::analyze_project(&Project::new("multimodule", ProjectKind::Program, modules))
}

/// Assert the multi-module program is clean.
fn project_ok(modules: &[(&str, &str)]) {
    let diags = project_diags(modules);
    assert!(
        diags.is_empty(),
        "expected no errors, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Assert some diagnostic of the multi-module program mentions `needle`.
fn project_err(modules: &[(&str, &str)], needle: &str) {
    let diags = project_diags(modules);
    assert!(
        diags.iter().any(|d| d.message.contains(needle)),
        "expected an error containing {needle:?}, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn name_binding_imports_a_function_for_bare_use() {
    project_ok(&[
        ("util", "func inc(x: i32): i32 {\n  return x + 1\n}"),
        (
            "main",
            "use {\n  util.inc,\n}\nfunc main(): i32 {\n  return inc(41)\n}",
        ),
    ]);
}

#[test]
fn name_binding_imports_a_type_for_construction_and_fields() {
    project_ok(&[
        ("geo", "struct Point {\n  x: i32,\n  y: i32,\n}"),
        (
            "main",
            "use {\n  geo.Point,\n}\nfunc origin(): i32 {\n  p := Point { x: 0, y: 0 }\n  return p.x\n}",
        ),
    ]);
}

#[test]
fn imported_name_may_be_aliased() {
    project_ok(&[
        ("util", "func inc(x: i32): i32 {\n  return x + 1\n}"),
        (
            "main",
            "use {\n  util.inc as bump,\n}\nfunc main(): i32 {\n  return bump(41)\n}",
        ),
    ]);
}

#[test]
fn use_of_unknown_module_is_reported() {
    project_err(
        &[("main", "use {\n  missing.thing,\n}\nfunc main() {}")],
        "no module `missing.thing` in this project",
    );
}

#[test]
fn use_of_unexported_item_is_reported() {
    project_err(
        &[
            ("util", "func inc(x: i32): i32 {\n  return x + 1\n}"),
            ("main", "use {\n  util.dec,\n}\nfunc main() {}"),
        ],
        "module `util` exports no item named `dec`",
    );
}

#[test]
fn imports_do_not_re_export_transitively() {
    // `mid` uses `base.Thing`, but does not re-export it; `main` using `mid.Thing`
    // must fail — a name reaches a module only through that module's own use.
    project_err(
        &[
            ("base", "struct Thing {\n  v: i32,\n}"),
            ("mid", "use {\n  base.Thing,\n}\nfunc wrap(t: Thing): i32 {\n  return t.v\n}"),
            ("main", "use {\n  mid.Thing,\n}\nfunc main() {}"),
        ],
        "module `mid` exports no item named `Thing`",
    );
}

#[test]
fn colliding_imports_are_reported() {
    // Two imports aliased to the same local name collide.
    project_err(
        &[
            ("a", "func f(): i32 {\n  return 1\n}"),
            ("b", "func g(): i32 {\n  return 2\n}"),
            (
                "main",
                "use {\n  a.f as dup,\n  b.g as dup,\n}\nfunc main() {}",
            ),
        ],
        "bound by more than one use",
    );
}

#[test]
fn import_colliding_with_a_declaration_is_reported_at_the_use() {
    // An imported name that shadows one of the importing module's own declarations is
    // rejected at the `use` site, not at the declaration.
    let diags = multifile_main_diags(
        &[(
            "main",
            "use {\n  util.inc,\n}\nfunc inc(x: i32): i32 {\n  return x\n}\nfunc main() {}",
        )],
        &[("util", "func inc(x: i32): i32 {\n  return x + 1\n}")],
    );
    let collision = diags
        .iter()
        .find(|d| d.message.contains("collides with a declaration"))
        .expect("expected a collision error");
    assert_eq!(collision.file.as_deref(), Some("main"));
}

#[test]
fn module_dependency_cycle_is_reported() {
    let diags = multifile_main_diags(
        &[("main", "use {\n  b.fromB,\n}\nfunc fromA(): i32 {\n  return fromB()\n}")],
        &[("b", "use {\n  main.fromA,\n}\nfunc fromB(): i32 {\n  return fromA()\n}")],
    );
    // The cycle is reported against a real `use` site, tagged with its file, rather
    // than an anonymous zero span.
    let cycle = diags
        .iter()
        .find(|d| d.message.contains("dependency cycle"))
        .expect("expected a dependency-cycle error");
    assert!(cycle.file.is_some(), "cycle error should name its file");
    assert!(
        cycle.span.start != 0 || cycle.span.end != 0,
        "cycle error should carry a real use span"
    );
}

#[test]
fn module_binding_reaches_a_function_qualified() {
    project_ok(&[
        ("util", "func inc(x: i32): i32 {\n  return x + 1\n}"),
        (
            "main",
            "use {\n  util,\n}\nfunc main(): i32 {\n  return util.inc(41)\n}",
        ),
    ]);
}

#[test]
fn module_binding_reaches_a_type_qualified() {
    project_ok(&[
        ("geo", "struct Point {\n  x: i32,\n  y: i32,\n}"),
        (
            "main",
            "use {\n  geo,\n}\nfunc origin(): i32 {\n  p := geo.Point { x: 0, y: 0 }\n  return p.x\n}",
        ),
    ]);
}

#[test]
fn module_binding_reaches_a_variant_qualified() {
    project_ok(&[
        ("shapes", "oneof Shape {\n  Circle(i32),\n  Rect(i32, i32),\n}"),
        (
            "main",
            "use {\n  shapes,\n}\nfunc main() {\n  s := shapes.Shape.Circle(5)\n}",
        ),
    ]);
}

#[test]
fn module_binding_may_be_aliased() {
    project_ok(&[
        ("helpers", "func inc(x: i32): i32 {\n  return x + 1\n}"),
        (
            "main",
            "use {\n  helpers as h,\n}\nfunc main(): i32 {\n  return h.inc(41)\n}",
        ),
    ]);
}

#[test]
fn multi_segment_module_is_reached_by_full_spelling() {
    project_ok(&[
        ("server.auth", "func token(): i32 {\n  return 7\n}"),
        (
            "main",
            "use {\n  server.auth,\n}\nfunc main(): i32 {\n  return server.auth.token()\n}",
        ),
    ]);
}

#[test]
fn qualified_access_to_unknown_member_is_reported() {
    project_err(
        &[
            ("util", "func inc(x: i32): i32 {\n  return x + 1\n}"),
            (
                "main",
                "use {\n  util,\n}\nfunc main(): i32 {\n  return util.dec(41)\n}",
            ),
        ],
        "module `util` exports no item named `dec`",
    );
}

/// Build a program whose `main` module spans several `(file name, source)` files
/// alongside any number of one-file dependency modules, and return its diagnostics.
fn multifile_main_diags(
    main_files: &[(&str, &str)],
    deps: &[(&str, &str)],
) -> Vec<cooper::Diagnostic> {
    use cooper::{Module, Project, ProjectKind, SourceFile};
    let main = Module::new(
        "main",
        main_files
            .iter()
            .map(|(name, src)| SourceFile::new(*name, *src))
            .collect(),
    );
    let mut modules = vec![main];
    modules.extend(
        deps.iter()
            .map(|(path, src)| Module::new(path, vec![SourceFile::new(*path, *src)])),
    );
    cooper::analyze_project(&Project::new("multifile", ProjectKind::Program, modules))
}

#[test]
fn a_use_is_visible_only_in_its_own_file() {
    // `a.coop` imports `inc`; `b.coop`, in the same module, may not use it bare — a
    // `use` is file-scoped, so the name is undefined there.
    let diags = multifile_main_diags(
        &[
            ("a", "use {\n  util.inc,\n}\nfunc first(): i32 {\n  return inc(1)\n}"),
            ("b", "func second(): i32 {\n  return inc(2)\n}"),
        ],
        &[("util", "func inc(x: i32): i32 {\n  return x + 1\n}")],
    );
    assert!(
        diags.iter().any(|d| d.message.contains("undefined")
            && d.file.as_deref() == Some("b")),
        "expected an 'undefined' error in file b, got: {:?}",
        diags
            .iter()
            .map(|d| (&d.file, &d.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn sibling_files_may_import_the_same_name_independently() {
    // The same bare name imported in two files of one module is not a collision:
    // each `use` is file-scoped.
    let diags = multifile_main_diags(
        &[
            ("a", "use {\n  util.inc,\n}\nfunc first(): i32 {\n  return inc(1)\n}"),
            ("b", "use {\n  util.inc,\n}\nfunc second(): i32 {\n  return inc(2)\n}"),
        ],
        &[("util", "func inc(x: i32): i32 {\n  return x + 1\n}")],
    );
    assert!(
        diags.is_empty(),
        "expected no errors, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn a_declaration_in_one_file_is_visible_across_the_module() {
    // A type imported in `a.coop` is used there; a function declared in `b.coop`
    // (module-global) is callable from `a.coop`, confirming declarations unite while
    // imports do not.
    let diags = multifile_main_diags(
        &[
            (
                "a",
                "use {\n  geo.Point,\n}\nfunc origin(): i32 {\n  p := Point { x: 0, y: 0 }\n  return helper(p.x)\n}",
            ),
            ("b", "func helper(n: i32): i32 {\n  return n + 1\n}"),
        ],
        &[("geo", "struct Point {\n  x: i32,\n  y: i32,\n}")],
    );
    assert!(
        diags.is_empty(),
        "expected no errors, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}
