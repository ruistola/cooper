use std::path::Path;
use std::process::ExitCode;

use cooper_cli::{BuildError, Loaded, Options};
use cooper_frontend::{Diagnostic, Project};

const USAGE: &str = "usage: cooper <check|build|run> [--nil-panic] <source-file.coop | project-dir>\n       cooper test [--nil-panic] <project-dir | file_test.coop> [name-filter]";

/// The command-line driver. `check` reports diagnostics, `build` produces an
/// executable in the build directory, and `run` builds and then runs it, exiting with
/// its status. A directory is loaded as a project via its `project.toml`; a single
/// file as a one-module program.
fn main() -> ExitCode {
    let (flags, args): (Vec<String>, Vec<String>) = std::env::args().skip(1).partition(|arg| arg.starts_with("--"));
    let mut options = Options::default();
    for flag in &flags {
        match flag.as_str() {
            "--nil-panic" => options.nil_panic = true,
            _ => {
                eprintln!("unknown option {flag}\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let (command, path, filter) = match args.as_slice() {
        [command, path] => (command, path, None),
        [command, path, filter] if command == "test" => (command, path, Some(filter.as_str())),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // `check` and `test` read the test sources too.
    let load = if matches!(command.as_str(), "check" | "test") { cooper_cli::load_with_tests } else { cooper_cli::load };
    let loaded = match load(Path::new(path)) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    match command.as_str() {
        "check" => check(&loaded.project),
        "build" => match build(&loaded, options) {
            Some(executable) => {
                println!("{}", executable.display());
                ExitCode::SUCCESS
            }
            None => ExitCode::FAILURE,
        },
        "test" => test(&loaded, options, filter),
        "run" => match build(&loaded, options) {
            Some(executable) => run(&executable),
            None => ExitCode::FAILURE,
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Build the test program and run each test (those whose name contains `filter`) in a process of
/// its own, reporting each, then the totals. Fails when any test does.
fn test(loaded: &Loaded, options: Options, filter: Option<&str>) -> ExitCode {
    let built = match cooper_cli::build_tests(&loaded.project, &loaded.native, &loaded.build_dir, options) {
        Ok(built) => built,
        Err(error) => {
            report_build_error(&loaded.project, error);
            return ExitCode::FAILURE;
        }
    };
    if built.tests.is_empty() {
        println!("no tests: a test is a function `testName()` in a `*_test.coop` file");
        return ExitCode::SUCCESS;
    }
    let (mut passed, mut failed) = (0, 0);
    for (index, name) in built.tests.iter().enumerate() {
        if filter.is_some_and(|f| !name.contains(f)) {
            continue;
        }
        let output = std::process::Command::new(&built.executable).env("COOPER_TEST", index.to_string()).output();
        match output {
            Ok(output) if output.status.success() => {
                passed += 1;
                println!("ok    {name}");
            }
            Ok(output) => {
                failed += 1;
                println!("FAIL  {name}");
                for stream in [&output.stdout, &output.stderr] {
                    for line in String::from_utf8_lossy(stream).lines() {
                        println!("        {line}");
                    }
                }
            }
            Err(error) => {
                failed += 1;
                println!("FAIL  {name}\n        cannot run {}: {error}", built.executable.display());
            }
        }
    }
    println!("\n{passed} passed, {failed} failed");
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
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
fn build(loaded: &Loaded, options: Options) -> Option<std::path::PathBuf> {
    match cooper_cli::build(&loaded.project, &loaded.native, &loaded.build_dir, options) {
        Ok(executable) => Some(executable),
        Err(error) => {
            report_build_error(&loaded.project, error);
            None
        }
    }
}

/// Report why a build failed.
fn report_build_error(project: &Project, error: BuildError) {
    match error {
        BuildError::Diagnostics(diagnostics) => report(project, &diagnostics),
        error if error.location().is_some() => {
            let (file, span) = error.location().expect("checked by the guard");
            report(project, &[Diagnostic::error(span, error.to_string()).in_file(file)]);
        }
        error => eprintln!("error: {error}"),
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
