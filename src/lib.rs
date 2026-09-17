//! The Cooper compiler frontend.
//!
//! [`analyze_project`] runs the whole pipeline over an in-memory [`Project`] —
//! lexing, parsing, declaration resolution, type checking, and semantic analysis —
//! and returns every diagnostic it found. Each module is analyzed independently;
//! within a module each semantic phase runs only when the previous one produced no
//! errors, so diagnostics stay meaningful rather than cascading, while within a
//! phase collection continues past the first error. [`analyze`] is a thin
//! convenience wrapping a single source snippet as a one-module program.

pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;
pub mod project;
pub mod resolve;
pub mod semantic;
pub mod typecheck;
pub mod types;

pub use diag::{Diagnostic, Span};
pub use project::{Module, Project, ProjectKind, ProjectManifest, SourceFile};

/// Run the full frontend over a single source snippet and return all diagnostics.
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
    analyze_project(&Project::snippet(source))
}

/// Run the full frontend over every module of `project`, returning all diagnostics
/// in module order.
pub fn analyze_project(project: &Project) -> Vec<Diagnostic> {
    project
        .modules
        .iter()
        .flat_map(analyze_module)
        .collect()
}

/// Analyze one module: parse its files, unite their top-level declarations, then
/// run resolution, type checking, and semantic analysis. `use` imports are parsed
/// but not yet resolved — cross-module binding arrives with module interfaces.
fn analyze_module(module: &Module) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    let mut decls = Vec::new();
    for file in &module.files {
        let tokens = match lexer::tokenize(&file.source) {
            Ok(tokens) => tokens,
            Err(diag) => {
                errors.push(diag);
                continue;
            }
        };
        let parsed = parser::parse(tokens);
        errors.extend(parsed.errors);
        decls.extend(parsed.decls);
    }
    if !errors.is_empty() {
        return errors;
    }

    let (globals, resolve_errors) = resolve::resolve(&decls);
    if !resolve_errors.is_empty() {
        return resolve_errors;
    }

    let type_errors = typecheck::check(&decls, &globals);
    if !type_errors.is_empty() {
        return type_errors;
    }

    semantic::analyze(&decls, &globals)
}
