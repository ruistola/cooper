//! The Cooper compiler frontend.
//!
//! [`analyze_project`] runs the whole pipeline over an in-memory [`Project`] —
//! lexing, parsing, declaration resolution, type checking, and semantic analysis —
//! and returns every diagnostic it found. Each module is analyzed independently;
//! within a module each semantic phase runs only when the previous one produced no
//! errors, so diagnostics stay meaningful rather than cascading, while within a
//! phase collection continues past the first error. [`analyze`] is a thin
//! convenience wrapping a single source snippet as a one-module program.
//! [`check_project`] runs the same pipeline and, for a clean project, hands back its
//! checked form for lowering.

pub mod ast;
mod builtins;
pub mod diag;
pub mod lexer;
pub mod modules;
pub mod parser;
pub mod project;
pub mod resolve;
pub mod semantic;
pub mod typecheck;
pub mod types;

pub use diag::{Diagnostic, Span};
pub use modules::{CheckedFile, CheckedProject};
pub use project::{Module, Project, ProjectKind, ProjectManifest, SourceFile};

/// Run the full frontend over a single source snippet and return all diagnostics.
///
/// ```
/// let diags = cooper_frontend::analyze("func main() { }");
/// assert!(diags.is_empty());
/// ```
///
/// ```
/// let diags = cooper_frontend::analyze("func main() { let x: i32 = true }");
/// assert_eq!(diags.len(), 1);
/// ```
pub fn analyze(source: &str) -> Vec<Diagnostic> {
    analyze_project(&Project::snippet(source))
}

/// Run the full frontend over every module of `project`, resolving the module
/// dependency graph and checking each module against its dependencies' interfaces.
pub fn analyze_project(project: &Project) -> Vec<Diagnostic> {
    modules::analyze(project)
}

/// Run the full frontend over `project`, returning its checked form — every file's
/// declarations, symbol table, and checker tables — when it is free of diagnostics,
/// and every diagnostic otherwise.
pub fn check_project(project: &Project) -> Result<CheckedProject, Vec<Diagnostic>> {
    modules::check(project)
}
