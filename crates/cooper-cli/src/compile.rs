//! Building native executables: checking, lowering, code generation, and the C
//! toolchain.
//!
//! A program is checked by the frontend, lowered and monomorphized into the IR,
//! emitted as one textual LLVM IR module, and compiled together with the C runtime by
//! the system `clang` (or the compiler named by `COOPER_CC`). Every intermediate file
//! is kept in the build directory, so the emitted IR can be read when debugging.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use cooper_frontend::diag::Span;
use cooper_frontend::{Diagnostic, Project, ProjectKind};
use cooper_ir::ProgramError;
use cooper_llvm::CodegenError;

/// The environment variable naming the C compiler, overriding `clang`.
const CC_VAR: &str = "COOPER_CC";
const DEFAULT_CC: &str = "clang";
const RUNTIME_FILE: &str = "cooper_rt.c";

/// Why a program could not be built.
#[derive(Debug)]
pub enum BuildError {
    /// The program has errors, reported as diagnostics against its sources.
    Diagnostics(Vec<Diagnostic>),
    /// The project is a library, which has no entry point to build an executable from.
    Library,
    Lower(ProgramError),
    Codegen(CodegenError),
    /// The C toolchain could not be run, or rejected the generated code.
    Toolchain(String),
    Io { path: PathBuf, source: std::io::Error },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Diagnostics(diags) => write!(f, "{} error(s) reported", diags.len()),
            BuildError::Library => write!(f, "a library has no `main` to build an executable from"),
            BuildError::Lower(error) => write!(f, "{error}"),
            BuildError::Codegen(error) => write!(f, "{error}"),
            BuildError::Toolchain(message) => write!(f, "{message}"),
            BuildError::Io { path, source } => write!(f, "cannot write {}: {source}", path.display()),
        }
    }
}

impl BuildError {
    /// The source file and span the error refers to, if it is a located error other
    /// than diagnostics (which carry their own).
    pub fn location(&self) -> Option<(&str, Span)> {
        match self {
            BuildError::Lower(error) => Some(error.location()),
            BuildError::Codegen(error) => error.location(),
            _ => None,
        }
    }
}

impl std::error::Error for BuildError {}

/// Build `project` into an executable in `build_dir`, returning the executable's path.
/// The emitted IR (`<name>.ll`) and the runtime source are left beside it.
pub fn build(project: &Project, build_dir: &Path) -> Result<PathBuf, BuildError> {
    if project.manifest.kind == ProjectKind::Library {
        return Err(BuildError::Library);
    }
    let checked = cooper_frontend::check_project(project).map_err(BuildError::Diagnostics)?;
    let program = cooper_ir::lower_program(&checked).map_err(BuildError::Lower)?;
    let cc = std::env::var(CC_VAR).unwrap_or_else(|_| DEFAULT_CC.to_string());
    let triple = target_triple(&cc)?;
    let ir = cooper_llvm::emit(&program, &triple, project).map_err(BuildError::Codegen)?;

    let name = &project.manifest.name;
    let ir_path = build_dir.join(format!("{name}.ll"));
    let runtime_path = build_dir.join(RUNTIME_FILE);
    let executable = build_dir.join(name);
    create_dir(build_dir)?;
    write(&ir_path, &ir)?;
    write(&runtime_path, cooper_llvm::RUNTIME_C)?;

    let output = Command::new(&cc)
        .arg("-o")
        .arg(&executable)
        .arg(&ir_path)
        .arg(&runtime_path)
        .output()
        .map_err(|e| BuildError::Toolchain(format!("cannot run `{cc}`: {e}")))?;
    if !output.status.success() {
        // Generated code the toolchain rejects is a compiler bug, not a user error.
        return Err(BuildError::Toolchain(format!(
            "`{cc}` rejected the generated code ({}):\n{}",
            ir_path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(executable)
}

/// The target triple `cc` compiles for by default: the host.
fn target_triple(cc: &str) -> Result<String, BuildError> {
    let output = Command::new(cc)
        .arg("-print-target-triple")
        .output()
        .map_err(|e| {
            BuildError::Toolchain(format!(
                "cannot run `{cc}` (set {CC_VAR} to a clang-compatible compiler): {e}"
            ))
        })?;
    if !output.status.success() {
        return Err(BuildError::Toolchain(format!("`{cc} -print-target-triple` failed")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn create_dir(path: &Path) -> Result<(), BuildError> {
    std::fs::create_dir_all(path).map_err(|source| BuildError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write(path: &Path, contents: &str) -> Result<(), BuildError> {
    std::fs::write(path, contents).map_err(|source| BuildError::Io {
        path: path.to_path_buf(),
        source,
    })
}
