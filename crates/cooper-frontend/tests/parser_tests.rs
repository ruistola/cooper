use cooper_frontend::ast::{ExprKind, Stmt, StmtKind};
use cooper_frontend::{lexer, parser};

/// Lex and parse `src` (without resolution), asserting it produces no diagnostics,
/// and return the module.
fn ok(src: &str) -> Vec<Stmt> {
    let tokens = lexer::tokenize(src).expect("lexing succeeds");
    let result = parser::parse(tokens);
    assert!(
        result.errors.is_empty(),
        "expected no errors, got: {:?}",
        result.errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    result.decls
}

/// Lex and parse `src`, asserting at least one diagnostic mentions `needle`.
fn err(src: &str, needle: &str) {
    let tokens = lexer::tokenize(src).expect("lexing succeeds");
    let result = parser::parse(tokens);
    assert!(
        result.errors.iter().any(|d| d.message.contains(needle)),
        "expected an error containing {needle:?}, got: {:?}",
        result.errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn parses_free_function() {
    let m = ok("func add(x: i32, y: i32): i32 {\n  return x + y\n}");
    assert_eq!(m.len(), 1);
    assert!(matches!(&m[0].kind, StmtKind::FuncDecl(f) if f.name == "add" && f.params.len() == 2));
}

#[test]
fn parses_method_declaration() {
    let m = ok("(p: Point) func norm(): i32 {\n  return p.x\n}");
    assert!(matches!(&m[0].kind, StmtKind::FuncDecl(f) if f.receiver.is_some()));
}

#[test]
fn parses_if_expression() {
    let m = ok("func f(b: bool): i32 {\n  x := if b then 1 else 2\n  return x\n}");
    assert_eq!(m.len(), 1);
}

#[test]
fn parses_if_expression_with_block_branches() {
    ok("func f(b: bool): i32 {\n  x := if b then {\n    y := 1\n    y\n  } else {\n    2\n  }\n  return x\n}");
}

#[test]
fn parses_value_block_with_statements() {
    ok("func f(): i32 {\n  x := {\n    y := 1\n    y\n  }\n  return x\n}");
}

#[test]
fn parses_oneof_and_match_expression_with_braced_arms() {
    ok("oneof Shape {\n  Circle(i32),\n  Rect(i32, i32),\n}\nfunc area(s: Shape): i32 {\n  a := match s with {\n    Shape.Circle(r) => { r }\n    Shape.Rect(w, h) => { w }\n  }\n  return a\n}");
}

#[test]
fn parses_generics_and_tuples() {
    let m = ok("struct Pair K V {\n  key: K,\n  val: V,\n}");
    assert!(matches!(&m[0].kind, StmtKind::StructDecl { type_params, .. } if type_params.len() == 2));
}

#[test]
fn missing_terminator_is_reported_not_panicked() {
    // `y := 1 y` lacks a separator between the two expressions.
    err("func f(): i32 {\n  x := { y := 1 y }\n  return x\n}", "terminator");
}

#[test]
fn recovers_and_reports_multiple_errors() {
    // Two separate malformed declarations (missing binding name); recovery should
    // surface both and still recover the well-formed third.
    let tokens = lexer::tokenize("let = 1\nlet = 2\nlet z = 3\n").expect("lexing succeeds");
    let result = parser::parse(tokens);
    assert!(
        result.errors.len() >= 2,
        "expected multiple errors, got {}: {:?}",
        result.errors.len(),
        result.errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    // Recovery still recovered the well-formed final declaration.
    assert!(result
        .decls
        .iter()
        .any(|s| matches!(&s.kind, StmtKind::VarDecl { name, .. } if name == "z")));
}

#[test]
fn rejects_bare_top_level_expression() {
    // A file's top level admits only declarations; a bare expression is rejected.
    err("2 + 2\n", "top-level declaration");
}

/// Parse a use block and return its flattened bindings.
fn uses(src: &str) -> Vec<cooper_frontend::ast::UseSpec> {
    let tokens = lexer::tokenize(src).expect("lexing succeeds");
    let result = parser::parse(tokens);
    assert!(
        result.errors.is_empty(),
        "expected no errors, got: {:?}",
        result.errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    result.uses
}

#[test]
fn parses_use_paths_groups_and_aliases() {
    // A group flattens into one binding per item; `as` renames the local surface
    // name; a bare path defaults its local name to the final segment.
    let specs = uses(
        "use {\n  std.io,\n  http as web,\n  std.types.{ Maybe, Result as CoreResult },\n}\n",
    );
    let bindings: Vec<(Vec<&str>, &str)> = specs
        .iter()
        .map(|s| {
            (
                s.path.iter().map(String::as_str).collect::<Vec<_>>(),
                s.local_name(),
            )
        })
        .collect();
    assert_eq!(
        bindings,
        vec![
            (vec!["std", "io"], "io"),
            (vec!["http"], "web"),
            (vec!["std", "types", "Maybe"], "Maybe"),
            (vec!["std", "types", "Result"], "CoreResult"),
        ]
    );
}

#[test]
fn use_after_declaration_is_rejected() {
    err(
        "func f() {}\nuse {\n  std.io,\n}\n",
        "use declarations must appear at the top of the file",
    );
}

#[test]
fn walrus_binds_identifier() {
    let m = ok("func f() {\n  x := 41 + 1\n}");
    let StmtKind::FuncDecl(f) = &m[0].kind else {
        panic!("expected a function declaration");
    };
    assert!(matches!(
        &f.body[0].kind,
        StmtKind::Expression(e) if matches!(&e.kind, ExprKind::Let { name, .. } if name == "x")
    ));
}

/// The statements of a single function `f`, for inspecting loop bodies.
fn body_of(src: &str) -> Vec<Stmt> {
    let m = ok(src);
    let StmtKind::FuncDecl(f) = &m[0].kind else {
        panic!("expected a function declaration");
    };
    f.body.clone()
}

#[test]
fn parses_range_for_with_tuple_and_single_bindings() {
    let body = body_of("func f() {\n  for i in 0..10 do {}\n  for (idx, val) in xs do {}\n}");
    assert!(matches!(
        &body[0].kind,
        StmtKind::ForIn { bindings, iterable, .. }
            if bindings == &["i"] && matches!(&iterable.kind, ExprKind::Range { inclusive: false, .. })
    ));
    assert!(matches!(
        &body[1].kind,
        StmtKind::ForIn { bindings, .. } if bindings == &["idx", "val"]
    ));
}

#[test]
fn inclusive_range_sets_the_inclusive_flag() {
    let body = body_of("func f() {\n  for i in 0..=10 do {}\n}");
    assert!(matches!(
        &body[0].kind,
        StmtKind::ForIn { iterable, .. }
            if matches!(&iterable.kind, ExprKind::Range { inclusive: true, .. })
    ));
}

#[test]
fn parses_all_four_conditional_loops() {
    let body = body_of(
        "func f() {\n  while a do {}\n  until b repeat {}\n  do {} while c\n  repeat {} until d\n}",
    );
    let flags: Vec<(bool, bool)> = body
        .iter()
        .map(|s| match &s.kind {
            StmtKind::While { post_test, until, .. } => (*post_test, *until),
            other => panic!("expected a while-family loop, got {other:?}"),
        })
        .collect();
    assert_eq!(flags, vec![(false, false), (false, true), (true, false), (true, true)]);
}

#[test]
fn parses_break_and_continue_and_single_statement_body() {
    let body = body_of("func f() {\n  while a do break\n  while a do continue\n}");
    let StmtKind::While { body: first, .. } = &body[0].kind else {
        panic!("expected a while loop");
    };
    assert!(matches!(first[0].kind, StmtKind::Break));
    let StmtKind::While { body: second, .. } = &body[1].kind else {
        panic!("expected a while loop");
    };
    assert!(matches!(second[0].kind, StmtKind::Continue));
}

#[test]
fn header_expressions_span_multiple_lines() {
    // A condition or iterable may wrap across lines, and its delimiter keyword may
    // sit on a later line, because newlines are insignificant inside a header.
    ok("func f() {\n  while a\n    and b\n  do total += 1\n  for i in\n    0..10\n  do total += i\n}");
}

#[test]
fn malformed_variant_payload_recovers_without_hanging() {
    // A sum-type payload must be parenthesised (`Some(T)`); the juxtaposition
    // `Some T` is rejected. Recovery must terminate: the stray closing brace that
    // ends the declaration cannot begin a top-level declaration, so the parser has
    // to consume it rather than retry it forever.
    err("oneof Maybe T { Some T, None }", "expected");
}

#[test]
fn parses_an_array_literal_across_lines_with_a_trailing_comma() {
    let m = ok("func f() {\n  let xs: i32[] = [\n    1,\n    2,\n  ]\n  let e: i32[] = []\n}");
    let StmtKind::FuncDecl(f) = &m[0].kind else { panic!("expected a function") };
    let lens: Vec<usize> = f
        .body
        .iter()
        .map(|s| match &s.kind {
            StmtKind::VarDecl { init: Some(init), .. } => match &init.kind {
                ExprKind::Array(elems) => elems.len(),
                other => panic!("expected an array literal, got {other:?}"),
            },
            other => panic!("expected a variable declaration, got {other:?}"),
        })
        .collect();
    assert_eq!(lens, [2, 0]);
}

#[test]
fn a_post_test_loop_may_have_a_single_statement_body() {
    // The closing `while`/`until` ends the body statement, as `else` ends a `then`
    // branch; nesting the two still pairs each keyword with its own construct.
    ok("func f() {\n  do n += 1 while n < 3\n  repeat n -= 1 until n == 0\n  do if a then b() else c() while d\n}");
}

#[test]
fn a_match_arm_ends_at_a_newline_before_a_negative_pattern() {
    let m = ok("func f(n: i32): i32 {\n  return match n with {\n    1 => 10\n    -1 => 20\n    _ => 0\n  }\n}");
    let StmtKind::FuncDecl(f) = &m[0].kind else { panic!("expected a function") };
    let StmtKind::Return(Some(ret)) = &f.body[0].kind else { panic!("expected a return") };
    let ExprKind::Match { arms, .. } = &ret.kind else { panic!("expected a match") };
    assert_eq!(arms.len(), 3, "`10` and `-1` are separate arms, not `10 - 1`");
}

#[test]
fn a_line_may_begin_with_negation() {
    let m = ok("func f() {\n  x := ready\n  !x\n}");
    let StmtKind::FuncDecl(f) = &m[0].kind else { panic!("expected a function") };
    assert_eq!(f.body.len(), 2);
}
