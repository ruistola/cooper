//! Bare variants resolved against an expected sum type.

use crate::{ok, err};

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
fn a_comparison_supplies_its_left_operands_type_to_a_variant_on_the_right() {
    ok(&format!(
        "{MAYBE}func f(m: Maybe i32): bool {{\n  return m == Maybe.None or m != None\n}}"
    ));
}
