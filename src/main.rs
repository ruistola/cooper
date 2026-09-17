use std::path::Path;
use std::process::ExitCode;

use cooper::{Module, Project, ProjectKind, SourceFile};

/// Demo driver: given a path (first argument), run the frontend and render any
/// diagnostics with `ariadne`. A directory is loaded as a project via its
/// `project.toml`; a single file is analyzed as a one-module program.
fn main() -> ExitCode {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: cooper <source-file.coop | project-dir>");
            return ExitCode::FAILURE;
        }
    };

    let project = match load(&path) {
        Ok(project) => project,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    let diagnostics = cooper::analyze_project(&project);
    if diagnostics.is_empty() {
        println!("no errors.");
        return ExitCode::SUCCESS;
    }

    let sources = source_index(&project);
    for diag in &diagnostics {
        let (name, source) = diag
            .file
            .as_deref()
            .and_then(|f| sources.iter().find(|(n, _)| *n == f))
            .map(|(n, s)| (n.as_str(), s.as_str()))
            .unwrap_or(("<unknown>", ""));
        eprint!("{}", diag.render(name, source));
    }
    eprintln!("\n{} error(s) reported.", diagnostics.len());
    ExitCode::FAILURE
}

/// Load a project from a directory, or wrap a single source file as a program.
fn load(path: &str) -> Result<Project, String> {
    let path = Path::new(path);
    if path.is_dir() {
        return cooper::load_project(path).map_err(|e| e.to_string());
    }
    let source = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let name = path.to_string_lossy().into_owned();
    let module = Module::new("main", vec![SourceFile::new(name, source)]);
    Ok(Project::new("snippet", ProjectKind::Program, vec![module]))
}

/// Every source file of `project` as `(name, source)`, for locating a diagnostic's
/// file when rendering.
fn source_index(project: &Project) -> Vec<(String, String)> {
    project
        .modules
        .iter()
        .flat_map(|m| &m.files)
        .map(|f: &SourceFile| (f.name.clone(), f.source.clone()))
        .collect()
}
