pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;

pub use diag::{Diagnostic, Span};

/// Run the frontend over `source`: tokenize, then parse. Lexical errors and
/// collected parse diagnostics are returned together; unlike the Go frontend,
/// neither stage aborts on the first error.
pub struct Frontend {
    pub module: Vec<ast::Stmt>,
    pub errors: Vec<Diagnostic>,
}

pub fn run(source: &str) -> Frontend {
    match lexer::tokenize(source) {
        Ok(tokens) => {
            let parsed = parser::parse(tokens);
            Frontend {
                module: parsed.module,
                errors: parsed.errors,
            }
        }
        Err(span) => Frontend {
            module: Vec::new(),
            errors: vec![Diagnostic::error(span, "unexpected character")],
        },
    }
}
