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

use crate::Native;

/// The environment variable naming the C compiler, overriding `clang`.
const CC_VAR: &str = "COOPER_CC";
const DEFAULT_CC: &str = "clang";
const RUNTIME_FILE: &str = "cooper_rt.c";
const SHIMS_FILE: &str = "cooper_shims.c";

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
/// The emitted IR (`<name>.ll`) and the runtime source are left beside it. The program
/// is linked with the C code `native` names.
pub fn build(project: &Project, native: &Native, build_dir: &Path, options: cooper_llvm::Options) -> Result<PathBuf, BuildError> {
    if project.manifest.kind == ProjectKind::Library {
        return Err(BuildError::Library);
    }
    // The standard library modules the program uses compile with it.
    let project = &cooper_frontend::stdlib::with_source_modules(project);
    let checked = cooper_frontend::check_project(project).map_err(BuildError::Diagnostics)?;
    let program = cooper_ir::lower_program(&checked).map_err(BuildError::Lower)?;
    link(project, &program, native, build_dir, options, &project.manifest.name)
}

/// A built test program: the executable, and the name of each test, in the order the
/// executable numbers them (`COOPER_TEST=n` runs the `n`th).
pub struct TestBuild {
    pub executable: PathBuf,
    pub tests: Vec<String>,
}

/// Build the test program of `project`, which holds its `*_test.coop` sources: every function
/// `testName()` of those files is a test. The executable runs one test, chosen by the
/// `COOPER_TEST` environment variable, and exits with status 0 if it returns.
pub fn build_tests(project: &Project, native: &Native, build_dir: &Path, options: cooper_llvm::Options) -> Result<TestBuild, BuildError> {
    let project = &cooper_frontend::stdlib::with_source_modules(project);
    let checked = cooper_frontend::check_project(project).map_err(BuildError::Diagnostics)?;
    let tests = discover_tests(&checked)?;
    let names = tests.iter().map(|t| match t {
        cooper_frontend::resolve::Callable::Func { module, name } => format!("{module}.{name}"),
        _ => unreachable!("a test is a function"),
    });
    let names: Vec<String> = names.collect();
    if tests.is_empty() {
        return Ok(TestBuild { executable: PathBuf::new(), tests: names });
    }
    let program = cooper_ir::lower_program_for(&checked, &cooper_ir::Entry::Tests(tests)).map_err(BuildError::Lower)?;
    let name = format!("{}-test", project.manifest.name);
    let executable = link(project, &program, native, build_dir, options, &name)?;
    Ok(TestBuild { executable, tests: names })
}

/// The tests of a checked project: the functions named `test` and a capital letter or digit
/// (`testParse`) in its `*_test.coop` files, which take nothing and return nothing.
fn discover_tests(checked: &cooper_frontend::CheckedProject) -> Result<Vec<cooper_frontend::resolve::Callable>, BuildError> {
    use cooper_frontend::ast::StmtKind;
    let mut tests = Vec::new();
    let mut errors = Vec::new();
    for file in checked.files.iter().filter(|f| f.name.ends_with(crate::loader::TEST_SUFFIX)) {
        for decl in &file.decls {
            let StmtKind::FuncDecl(func) = &decl.kind else { continue };
            let named = func.name.strip_prefix("test").and_then(|rest| rest.chars().next());
            if func.receiver.is_some() || !named.is_some_and(|c| c.is_uppercase() || c.is_ascii_digit()) {
                continue;
            }
            if !func.params.is_empty() || func.return_type.is_some() || !func.type_params.is_empty() {
                errors.push(
                    Diagnostic::error(func.name_span, "a test function takes no parameters, has no type parameters, and returns nothing")
                        .in_file(&file.name),
                );
                continue;
            }
            tests.push(cooper_frontend::resolve::Callable::Func { module: file.module.to_string(), name: func.name.clone() });
        }
    }
    if errors.is_empty() { Ok(tests) } else { Err(BuildError::Diagnostics(errors)) }
}

/// Compile and link `program` into the executable `name` in `build_dir`.
fn link(
    project: &Project,
    program: &cooper_ir::Program,
    native: &Native,
    build_dir: &Path,
    options: cooper_llvm::Options,
    name: &str,
) -> Result<PathBuf, BuildError> {
    let cc = std::env::var(CC_VAR).unwrap_or_else(|_| DEFAULT_CC.to_string());
    let triple = target_triple(&cc)?;
    let emitted = cooper_llvm::emit(program, &triple, project, options).map_err(BuildError::Codegen)?;

    let ir_path = build_dir.join(format!("{name}.ll"));
    let runtime_path = build_dir.join(RUNTIME_FILE);
    let executable = build_dir.join(name);
    create_dir(build_dir)?;
    write(&ir_path, &emitted.ir)?;
    write(&runtime_path, cooper_llvm::RUNTIME_C)?;
    let mut runtime_sources = Vec::new();
    for (file, text) in cooper_llvm::RUNTIME_FILES {
        let path = build_dir.join(file);
        write(&path, text)?;
        if file.ends_with(".c") {
            runtime_sources.push(path);
        }
    }
    // Wrappers for C functions that pass structs by value, compiled with the program.
    let shims_path = build_dir.join(SHIMS_FILE);
    let shims = (!emitted.c_shims.is_empty()).then_some(&shims_path);
    if shims.is_some() {
        write(&shims_path, &emitted.c_shims)?;
    }

    let output = Command::new(&cc)
        .arg("-o")
        .arg(&executable)
        .arg(&ir_path)
        .arg(&runtime_path)
        .args(&runtime_sources)
        .args(shims)
        .args(&native.sources)
        .args(native.libraries.iter().map(|library| format!("-l{library}")))
        .output()
        .map_err(|e| BuildError::Toolchain(format!("cannot run `{cc}`: {e}")))?;
    if !output.status.success() {
        // Generated code the toolchain rejects is a compiler bug, unless it is the
        // program's own C code or libraries that fail.
        return Err(BuildError::Toolchain(format!(
            "`{cc}` failed to compile or link the program ({}):\n{}",
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
