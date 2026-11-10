//! Golden programs: every `tests/programs/*.coop` file is built with the system C
//! toolchain and run, and its exit status compared with the `# exit: N` comment on
//! its first line. The header lines that follow may give `# stdout: text`, one line of
//! the exact standard output each, `# stderr: text`, text its standard error must
//! contain, and `# env: NAME=value`, a variable to run it with. Requires `clang` (or `COOPER_CC`).

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

/// The header lines after the first that carry `prefix`, with the prefix and the one
/// space after it removed.
fn header<'s>(source: &'s str, prefix: &str) -> Vec<&'s str> {
    source
        .lines()
        .skip(1)
        .take_while(|line| ["# stdout:", "# stderr:", "# env:"].iter().any(|p| line.starts_with(p)))
        .filter_map(|line| line.strip_prefix(prefix))
        .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
        .collect()
}

/// Build and run one program, describing how it failed, if it did.
fn check_program(path: &Path, build_root: &Path) -> Result<(), String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let expected = expected_exit(&source, path);
    let loaded = cooper_cli::load(path).map_err(|e| e.to_string())?;
    let stem = path.file_stem().expect("a program file has a name");
    let executable = cooper_cli::build(&loaded.project, &loaded.native, &build_root.join(stem), Default::default())
        .map_err(|e| match e {
            cooper_cli::BuildError::Diagnostics(diags) => format!(
                "diagnostics: {:?}",
                diags.iter().map(|d| &d.message).collect::<Vec<_>>()
            ),
            other => other.to_string(),
        })?;
    let variables: Vec<(&str, &str)> = header(&source, "# env:").into_iter().filter_map(|v| v.split_once('=')).collect();
    let output = Command::new(&executable).envs(variables.clone()).output().map_err(|e| e.to_string())?;
    // The same program collecting garbage at every allocation, with freed memory poisoned and every mark
    // checked against a serial one, and on four processors (the default is one per core), must behave exactly
    // as it does normally. A program's own `# env:` lines take precedence.
    let reruns: [&[(&str, &str)]; 3] = [
        &[("COOPER_GC", "stress,checkmark")],
        &[("COOPER_THREADS", "4")],
        &[("COOPER_THREADS", "4"), ("COOPER_GC", "stress,checkmark")],
    ];
    for rerun in reruns {
        let again = Command::new(&executable)
            .envs(rerun.iter().copied())
            .envs(variables.iter().copied())
            .output()
            .map_err(|e| e.to_string())?;
        if again.status.code() != output.status.code() || again.stdout != output.stdout {
            return Err(format!(
                "with {rerun:?}: exit {:?} (not {:?}); stdout {:?} (not {:?}); stderr: {}",
                again.status.code(),
                output.status.code(),
                String::from_utf8_lossy(&again.stdout),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&again.stderr)
            ));
        }
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.code() != Some(expected) {
        return Err(format!(
            "expected exit status {expected}, got {:?}; stderr: {stderr}",
            output.status.code()
        ));
    }
    let expected_stdout = header(&source, "# stdout:");
    if !expected_stdout.is_empty() {
        let want = expected_stdout.iter().map(|line| format!("{line}\n")).collect::<String>();
        if stdout != want {
            return Err(format!("expected stdout {want:?}, got {stdout:?}"));
        }
    }
    match header(&source, "# stderr:").into_iter().find(|text| !stderr.contains(text)) {
        Some(missing) => Err(format!("stderr lacks {missing:?}: {stderr}")),
        None => Ok(()),
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
    // The programs run a few at a time; each may itself use several OS threads.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failures = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                while let Some(path) = programs.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    if let Err(why) = check_program(path, &build_root) {
                        failures.lock().unwrap().push(format!("{}: {why}", path.display()));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "failing programs:\n{}", failures.join("\n"));
}
