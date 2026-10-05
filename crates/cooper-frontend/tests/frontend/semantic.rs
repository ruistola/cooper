//! Semantic analysis: all-paths return and unreachable code.

use crate::{ok, err};

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
