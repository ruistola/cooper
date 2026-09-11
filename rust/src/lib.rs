//! The Cooper compiler frontend.
//!
//! [`analyze`] runs the whole pipeline over a source string — lexing, parsing,
//! declaration resolution, type checking, and semantic analysis — and returns
//! every diagnostic it found. Each semantic phase runs only when the previous one
//! produced no errors, so diagnostics stay meaningful rather than cascading, while
//! within a phase collection continues past the first error.

pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;
pub mod resolve;
pub mod semantic;
pub mod typecheck;
pub mod types;

pub use diag::{Diagnostic, Span};

/// Run the full frontend over `source` and return all diagnostics, in order.
///
/// ```
/// let diags = cooper::analyze("func main() { }");
/// assert!(diags.is_empty());
/// ```
///
/// ```
/// let diags = cooper::analyze("func main() { let x: i32 = true }");
/// assert_eq!(diags.len(), 1);
/// ```
pub fn analyze(source: &str) -> Vec<Diagnostic> {
    let tokens = match lexer::tokenize(source) {
        Ok(tokens) => tokens,
        Err(diag) => return vec![diag],
    };

    let parsed = parser::parse(tokens);
    if !parsed.errors.is_empty() {
        return parsed.errors;
    }

    let (globals, resolve_errors) = resolve::resolve(&parsed.module);
    if !resolve_errors.is_empty() {
        return resolve_errors;
    }

    let type_errors = typecheck::check(&parsed.module, &globals);
    if !type_errors.is_empty() {
        return type_errors;
    }

    semantic::analyze(&parsed.module, &globals)
}
