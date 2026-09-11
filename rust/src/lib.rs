pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;
pub mod resolver;
pub mod types;

pub use diag::{Diagnostic, Span};

/// Run the frontend over `source`: tokenize, parse, then resolve. Lexical, parse,
/// and resolve diagnostics are returned together; unlike the Go frontend, no stage
/// aborts on the first error. Resolution runs only when parsing produced a module
/// free of errors, mirroring the Go pipeline's phase gating.
pub struct Frontend {
    pub module: Vec<ast::Stmt>,
    pub errors: Vec<Diagnostic>,
}

pub fn run(source: &str) -> Frontend {
    let tokens = match lexer::tokenize(source) {
        Ok(tokens) => tokens,
        Err(span) => {
            return Frontend {
                module: Vec::new(),
                errors: vec![Diagnostic::error(span, "unexpected character")],
            };
        }
    };
    let parsed = parser::parse(tokens);
    if !parsed.errors.is_empty() {
        return Frontend {
            module: parsed.module,
            errors: parsed.errors,
        };
    }
    let resolved = resolver::resolve(&parsed.module);
    Frontend {
        module: parsed.module,
        errors: resolved.errors,
    }
}
