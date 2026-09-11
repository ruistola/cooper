use cooper::ast::{ExprKind, Stmt, StmtKind};
use cooper::{lexer, parser};

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
    result.module
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
        .module
        .iter()
        .any(|s| matches!(&s.kind, StmtKind::VarDecl { name, .. } if name == "z")));
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
