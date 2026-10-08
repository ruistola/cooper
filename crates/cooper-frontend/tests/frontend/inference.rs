//! Body completion diagnoses unresolved obligations at their source expressions.

use cooper_frontend::{analyze, Span};

const ZERO: &str = "func zero T (): T {\n  let value: T\n  return value\n}\n";
const ANNOTATE: &str = "cannot infer the type of this value; add a type annotation";

fn diagnostic_at(source: &str, expression: &str, message: &str) {
    let start = source.rfind(expression).expect("the expression appears in the source");
    let span = Span::new(start, start + expression.len());
    let diagnostics = analyze(source);
    assert!(
        diagnostics.iter().any(|d| d.span == span && d.message.contains(message)),
        "expected {message:?} at {span:?}, got {diagnostics:?}"
    );
}

#[test]
fn an_unresolved_field_reports_the_field_access() {
    let source = format!("{ZERO}func f() {{\n  value := zero()\n  value.member\n}}");
    diagnostic_at(&source, "value.member", ANNOTATE);
}

#[test]
fn an_unresolved_method_reports_the_member_access() {
    let source = format!("{ZERO}func f() {{\n  value := zero()\n  value.method(1)\n}}");
    diagnostic_at(&source, "value.method", ANNOTATE);
}

#[test]
fn an_unresolved_bare_variant_reports_its_name() {
    let source = "oneof Maybe T { Some(T), None }\nfunc f() {\n  value := Some(1)\n}";
    diagnostic_at(source, "Some", "cannot infer sum type for bare variant Some");
}

#[test]
fn an_unresolved_match_reports_its_scrutinee() {
    let source = format!(
        "{ZERO}oneof Maybe T {{ Some(T), None }}\nfunc f() {{\n  value := zero()\n  result := match value with {{\n    Some(n) => n\n    None => 0\n  }}\n}}"
    );
    diagnostic_at(&source, "value", ANNOTATE);
}

#[test]
fn an_unconstrained_empty_array_reports_its_binding() {
    diagnostic_at("func f() {\n  values := []\n}", "values := []", ANNOTATE);
}

#[test]
fn a_deferred_method_argument_error_reports_the_argument() {
    let source = format!(
        "{ZERO}struct Counter {{ n: i64 }}\n(c: Counter) func add(n: i64) {{ c.n += n }}\nfunc f() {{\n  counter := zero()\n  counter.add(true)\n  counter = Counter{{n: 0}}\n}}"
    );
    diagnostic_at(&source, "true", "argument 1 type mismatch: expected i64, found bool");
}
