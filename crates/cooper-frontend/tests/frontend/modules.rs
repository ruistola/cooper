//! Projects and modules: multi-file namespaces, `use` bindings, and qualified access.

/// A module spanning several source files unites their top-level declarations into
/// one namespace, so a function in one file may call one defined in another.
#[test]
fn module_unites_declarations_across_files() {
    use cooper_frontend::{Module, Project, ProjectKind, SourceFile};

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

    let diags = cooper_frontend::analyze_project(&project);
    assert!(
        diags.is_empty(),
        "expected no errors, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Build a program from one-file modules, each given as `(module path, source)`, and
/// return its diagnostics.
fn project_diags(modules: &[(&str, &str)]) -> Vec<cooper_frontend::Diagnostic> {
    use cooper_frontend::{Module, Project, ProjectKind, SourceFile};
    let modules = modules
        .iter()
        .map(|(path, src)| Module::new(path, vec![SourceFile::new(*path, *src)]))
        .collect();
    cooper_frontend::analyze_project(&Project::new("multimodule", ProjectKind::Program, modules))
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
) -> Vec<cooper_frontend::Diagnostic> {
    use cooper_frontend::{Module, Project, ProjectKind, SourceFile};
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
    cooper_frontend::analyze_project(&Project::new("multifile", ProjectKind::Program, modules))
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

// --- control flow ---

#[test]
fn same_named_types_from_two_modules_stay_distinct_and_read_qualified() {
    project_err(
        &[
            ("a", "struct Node {\n  x: i32,\n}"),
            ("b", "struct Node {\n  x: i32,\n}"),
            (
                "main",
                "use {\n  a.Node as ANode,\n  b.Node as BNode,\n}\nfunc f(x: ANode): BNode {\n  return x\n}",
            ),
        ],
        "return type mismatch: expected b.Node, found a.Node",
    );
}

#[test]
fn a_type_reaches_another_it_names_without_importing_it() {
    project_ok(&[
        ("a", "struct Item {\n  tag: i32,\n}\nstruct Node {\n  item: Item,\n}"),
        ("main", "use {\n  a.Node,\n}\nfunc f(n: Node): i32 {\n  return n.item.tag\n}"),
    ]);
}

#[test]
fn a_generic_function_is_called_through_a_module_binding() {
    project_ok(&[
        ("util", "func id T (x: T): T {\n  return x\n}"),
        ("main", "use {\n  util,\n}\nfunc f(): i32 {\n  return util.id(1)\n}"),
    ]);
}

#[test]
fn the_standard_io_module_is_provided_by_the_compiler() {
    project_ok(&[(
        "main",
        "use {\n  std.io,\n  std.io.println,\n}\nfunc main() {\n  std.io.print(\"a\")\n  println(\"b\")\n}",
    )]);
    project_err(
        &[("main", "use {\n  std.io,\n}\nfunc main() {\n  std.io.shout(\"a\")\n}")],
        "module `std.io` exports no item named `shout`",
    );
}
