//! Golden programs: every `tests/programs/*.coop` file is built with the system C
//! toolchain and run, and its exit status compared with the `# exit: N` comment on
//! its first line. Requires `clang` (or `COOPER_CC`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The exit status a program's first line declares.
fn expected_exit(source: &str, path: &Path) -> i32 {
    source
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# exit:"))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("{} must start with `# exit: N`", path.display()))
}

/// Build and run one program, describing how it failed, if it did.
fn check_program(path: &Path, build_root: &Path) -> Result<(), String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let expected = expected_exit(&source, path);
    let loaded = cooper_cli::load(path).map_err(|e| e.to_string())?;
    let stem = path.file_stem().expect("a program file has a name");
    let executable = cooper_cli::build(&loaded.project, &build_root.join(stem))
        .map_err(|e| match e {
            cooper_cli::BuildError::Diagnostics(diags) => format!(
                "diagnostics: {:?}",
                diags.iter().map(|d| &d.message).collect::<Vec<_>>()
            ),
            other => other.to_string(),
        })?;
    let status = Command::new(&executable).status().map_err(|e| e.to_string())?;
    match status.code() {
        Some(code) if code == expected => Ok(()),
        code => Err(format!("expected exit status {expected}, got {code:?}")),
    }
}

#[test]
fn golden_programs_run_with_their_expected_exit_status() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    let build_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("programs");
    let mut programs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/programs exists")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "coop"))
        .collect();
    programs.sort();
    assert!(!programs.is_empty(), "no programs in {}", dir.display());
    let failures: Vec<String> = programs
        .iter()
        .filter_map(|path| {
            check_program(path, &build_root)
                .err()
                .map(|why| format!("{}: {why}", path.display()))
        })
        .collect();
    assert!(failures.is_empty(), "failing programs:\n{}", failures.join("\n"));
}
