use std::path::Path;
use std::process::ExitCode;

use cooper_cli::{BuildError, Loaded};
use cooper_frontend::{Diagnostic, Project};

const USAGE: &str = "usage: cooper <check|build|run> <source-file.coop | project-dir>";

/// The command-line driver. `check` reports diagnostics, `build` produces an
/// executable in the build directory, and `run` builds and then runs it, exiting with
/// its status. A directory is loaded as a project via its `project.toml`; a single
/// file as a one-module program.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [command, path] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let loaded = match cooper_cli::load(Path::new(path)) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    match command.as_str() {
        "check" => check(&loaded.project),
        "build" => match build(&loaded) {
            Some(executable) => {
                println!("{}", executable.display());
                ExitCode::SUCCESS
            }
            None => ExitCode::FAILURE,
        },
        "run" => match build(&loaded) {
            Some(executable) => run(&executable),
            None => ExitCode::FAILURE,
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn check(project: &Project) -> ExitCode {
    let diagnostics = cooper_frontend::analyze_project(project);
    if diagnostics.is_empty() {
        println!("no errors.");
        return ExitCode::SUCCESS;
    }
    report(project, &diagnostics);
    ExitCode::FAILURE
}

/// Build the loaded program, reporting any failure, and return its executable.
fn build(loaded: &Loaded) -> Option<std::path::PathBuf> {
    match cooper_cli::build(&loaded.project, &loaded.build_dir) {
        Ok(executable) => Some(executable),
        Err(BuildError::Diagnostics(diagnostics)) => {
            report(&loaded.project, &diagnostics);
            None
        }
        Err(error) => {
            eprintln!("error: {error}");
            None
        }
    }
}

/// Run `executable`, exiting with its status (failure if a signal ended it).
fn run(executable: &Path) -> ExitCode {
    match std::process::Command::new(executable).status() {
        Ok(status) => match status.code() {
            Some(code) => ExitCode::from(code as u8),
            None => ExitCode::FAILURE,
        },
        Err(error) => {
            eprintln!("error: cannot run {}: {error}", executable.display());
            ExitCode::FAILURE
        }
    }
}

/// Render every diagnostic against the source file it refers to.
fn report(project: &Project, diagnostics: &[Diagnostic]) {
    for diag in diagnostics {
        let source = diag.file.as_deref().and_then(|name| {
            project
                .modules
                .iter()
                .flat_map(|m| &m.files)
                .find(|f| f.name == name)
        });
        let (name, text) = source
            .map(|f| (f.name.as_str(), f.source.as_str()))
            .unwrap_or(("<unknown>", ""));
        eprint!("{}", diag.render(name, text));
    }
    eprintln!("\n{} error(s) reported.", diagnostics.len());
}
