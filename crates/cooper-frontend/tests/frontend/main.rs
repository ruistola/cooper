//! End-to-end frontend tests exercising resolution, type checking, and semantic
//! analysis through the public [`cooper_frontend::analyze`] entry point.

use cooper_frontend::analyze;

mod resolution;
mod typecheck;
mod numerics;
mod variants;
mod semantic;
mod modules;
mod control_flow;

/// Assert `src` produces no diagnostics.
pub fn ok(src: &str) {
    let diags = analyze(src);
    assert!(
        diags.is_empty(),
        "expected no errors, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Assert at least one diagnostic from `src` mentions `needle`.
pub fn err(src: &str, needle: &str) {
    let diags = analyze(src);
    assert!(
        diags.iter().any(|d| d.message.contains(needle)),
        "expected an error containing {needle:?}, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}
