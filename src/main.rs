use std::process::ExitCode;

/// Demo driver: read a Cooper source file (path as the first argument), run the
/// frontend, and either pretty-print the recovered AST or render every collected
/// diagnostic with `ariadne`.
fn main() -> ExitCode {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: cooper <source-file.coop>");
            return ExitCode::FAILURE;
        }
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let diagnostics = cooper::analyze(&source);

    if diagnostics.is_empty() {
        println!("no errors.");
        ExitCode::SUCCESS
    } else {
        for diag in &diagnostics {
            eprint!("{}", diag.render(&path, &source));
        }
        eprintln!("\n{} error(s) reported.", diagnostics.len());
        ExitCode::FAILURE
    }
}
