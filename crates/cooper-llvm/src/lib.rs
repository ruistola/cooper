//! The Cooper LLVM backend.
//!
//! [`emit`] turns a monomorphized [`Program`] into one textual LLVM IR module; the
//! driver compiles that with the system `clang`, linking in the C runtime
//! [`RUNTIME_C`]. Every function instance becomes an LLVM function named by its
//! mangled symbol, and the program's `main` is exported to the runtime as
//! `cooper_main`, returning the process exit status.
//!
//! Runtime errors — integer overflow, division by zero — stop the program through the
//! runtime's `cooper_panic`, with a message naming the source location. Code generation
//! covers a growing subset of the IR; anything outside it is reported as
//! [`CodegenError::Unsupported`] rather than miscompiled.

mod function;
mod mangle;

use std::collections::BTreeSet;
use std::fmt::{self, Write};

use cooper_frontend::diag::{line_col, Span};
use cooper_frontend::resolve::Callable;
use cooper_frontend::types::Type;
use cooper_frontend::Project;
use cooper_ir::{Function, Program};

/// The C runtime every program links against: the process entry point and the
/// operations the IR's intrinsics lower to.
pub const RUNTIME_C: &str = include_str!("../runtime/cooper_rt.c");

/// The module and name of the function a program starts in.
const ENTRY_MODULE: &str = "main";
const ENTRY_NAME: &str = "main";

/// Why a program could not be compiled to LLVM IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodegenError {
    /// The program has no `main` function in its `main` module.
    NoEntry,
    /// `main` takes parameters, or returns something other than `i32` or unit.
    BadEntry { file: String, span: Span },
    /// A construct code generation does not handle yet.
    Unsupported { what: String, file: String, span: Span },
}

impl CodegenError {
    /// The source file and span the error refers to, if any.
    pub fn location(&self) -> Option<(&str, Span)> {
        match self {
            CodegenError::NoEntry => None,
            CodegenError::BadEntry { file, span } | CodegenError::Unsupported { file, span, .. } => {
                Some((file, *span))
            }
        }
    }
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::NoEntry => write!(f, "the program has no `main` function in module `main`"),
            CodegenError::BadEntry { .. } => {
                write!(f, "`main` must take no parameters and return `i32` or nothing")
            }
            CodegenError::Unsupported { what, .. } => {
                write!(f, "code generation does not support {what} yet")
            }
        }
    }
}

impl std::error::Error for CodegenError {}

/// Emit `program` as a textual LLVM IR module for the target `triple`. `project`
/// supplies the source text that runtime error messages locate their cause in.
pub fn emit(program: &Program, triple: &str, project: &Project) -> Result<String, CodegenError> {
    let mut module = Module {
        project,
        declarations: BTreeSet::new(),
        strings: Vec::new(),
    };
    let mut functions = String::new();
    for function in &program.functions {
        functions.push('\n');
        functions.push_str(&function::emit(&mut module, function)?);
    }
    let mut out = format!("target triple = \"{triple}\"\n");
    if !module.strings.is_empty() {
        out.push('\n');
        module.strings.iter().for_each(|s| out.push_str(s));
    }
    if !module.declarations.is_empty() {
        out.push('\n');
        module.declarations.iter().for_each(|d| {
            out.push_str(d);
            out.push('\n');
        });
    }
    out.push_str(&functions);
    out.push('\n');
    out.push_str(&entry(&program.functions)?);
    Ok(out)
}

/// Module-wide emission state shared by every function: external declarations and
/// string constants, each emitted once at the top of the module.
struct Module<'p> {
    project: &'p Project,
    declarations: BTreeSet<String>,
    strings: Vec<String>,
}

impl Module<'_> {
    /// Declare an external function (an LLVM intrinsic or a runtime function).
    fn declare(&mut self, declaration: &str) {
        self.declarations.insert(declaration.to_string());
    }

    /// A private, NUL-terminated string constant holding `text`, returning its name.
    fn string(&mut self, text: &str) -> String {
        let name = format!("@.str.{}", self.strings.len());
        let mut bytes = String::new();
        for &b in text.as_bytes() {
            if b.is_ascii_graphic() && b != b'"' && b != b'\\' || b == b' ' {
                bytes.push(b as char);
            } else {
                write!(bytes, "\\{b:02X}").expect("writing to a String cannot fail");
            }
        }
        self.strings.push(format!(
            "{name} = private unnamed_addr constant [{} x i8] c\"{bytes}\\00\"\n",
            text.len() + 1
        ));
        name
    }

    /// `file:line:column` for a span in one of the project's files.
    fn locate(&self, file: &str, span: Span) -> String {
        let source = self
            .project
            .modules
            .iter()
            .flat_map(|m| &m.files)
            .find(|f| f.name == file)
            .map_or("", |f| f.source.as_str());
        let (line, column) = line_col(source, span.start);
        format!("{file}:{line}:{column}")
    }
}

/// The `cooper_main` export the runtime calls: the program's `main`, with a unit
/// result turned into exit status 0.
fn entry(functions: &[Function]) -> Result<String, CodegenError> {
    let main = functions
        .iter()
        .find(|f| {
            matches!(&f.item, Callable::Func { module, name }
                if module == ENTRY_MODULE && name == ENTRY_NAME)
        })
        .ok_or(CodegenError::NoEntry)?;
    let symbol = mangle::symbol(&main.item, &main.type_args);
    let call = match &main.return_type {
        _ if !main.params.is_empty() || main.receiver.is_some() => None,
        Type::Primitive(name) if name == "i32" => Some(format!(
            "  %status = call i32 @{symbol}()\n  ret i32 %status\n"
        )),
        Type::Unit => Some(format!("  call void @{symbol}()\n  ret i32 0\n")),
        _ => None,
    };
    let body = call.ok_or_else(|| CodegenError::BadEntry {
        file: main.file.clone(),
        span: main.span,
    })?;
    Ok(format!("define i32 @cooper_main() {{\n{body}}}\n"))
}
