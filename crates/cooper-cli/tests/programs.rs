//! Golden programs: every `tests/programs/*.coop` file is built with the system C
//! toolchain and run, and its exit status compared with the `# exit: N` comment on
//! its first line. The header lines that follow may give `# stdout: text`, one line of
//! the exact standard output each, `# stderr: text`, text its standard error must
//! contain, and `# env: NAME=value`, a variable to run it with. Requires `clang` (or `COOPER_CC`).
//!
//! Each program is built twice, unoptimized and with `--release`, since some faults in the runtime show only when
//! optimized code keeps pointers in registers. Its LLVM IR is also checked for stores that need a write barrier and
//! lack one.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// How long one run of a program may take, under any of the collector's stress modes, before it counts as hung.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Run `command` to completion, as `Command::output` does, but kill it once it runs longer than RUN_TIMEOUT. A hung
/// program's threads are sampled first (with macOS's `sample`, where it exists) into `stacks`, which the error names.
fn run(command: &mut Command, stacks: &Path) -> Result<Output, String> {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    };
    let stdout = read(Box::new(child.stdout.take().expect("piped")));
    let stderr = read(Box::new(child.stderr.take().expect("piped")));
    let deadline = Instant::now() + RUN_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            let sampled = Command::new("sample")
                .args([&child.id().to_string(), "1", "-file"])
                .arg(stacks)
                .output()
                .is_ok_and(|o| o.status.success());
            let _ = child.kill();
            let _ = child.wait();
            let stderr = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned();
            return Err(format!(
                "still running after {} s{}; stderr: {stderr}",
                RUN_TIMEOUT.as_secs(),
                if sampled { format!(", its threads sampled into {}", stacks.display()) } else { String::new() }
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Output { status, stdout: stdout.join().unwrap_or_default(), stderr: stderr.join().unwrap_or_default() })
}

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
fn check_program(path: &Path, build_root: &Path, release: bool) -> Result<(), String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let expected = expected_exit(&source, path);
    let loaded = cooper_cli::load(path).map_err(|e| e.to_string())?;
    let stem = path.file_stem().expect("a program file has a name").to_string_lossy();
    let build_dir = build_root.join(if release { format!("{stem}-release") } else { stem.into_owned() });
    let options = cooper_cli::Options { release, ..Default::default() };
    let executable = cooper_cli::build(&loaded.project, &loaded.native, &build_dir, options)
        .map_err(|e| match e {
            cooper_cli::BuildError::Diagnostics(diags) => format!(
                "diagnostics: {:?}",
                diags.iter().map(|d| &d.message).collect::<Vec<_>>()
            ),
            other => other.to_string(),
        })?;
    let ir = std::fs::read_to_string(executable.with_extension("ll")).map_err(|e| e.to_string())?;
    let unguarded = unguarded_stores(&ir);
    if !unguarded.is_empty() {
        return Err(format!("stores without a write barrier:\n  {}", unguarded.join("\n  ")));
    }
    let variables: Vec<(&str, &str)> = header(&source, "# env:").into_iter().filter_map(|v| v.split_once('=')).collect();
    let stacks = |run: &str| executable.with_extension(format!("{run}.stacks.txt"));
    let output = run(Command::new(&executable).envs(variables.clone()), &stacks("default"))
        .map_err(|e| format!("as is: {e}"))?;
    // The same program collecting garbage at every allocation, or marking almost all the time while it runs, with
    // freed memory poisoned and every mark checked against a serial one, and on four processors (the default is
    // one per core), must behave exactly as it does normally. A program's own `# env:` lines take precedence.
    let reruns: [&[(&str, &str)]; 4] = [
        &[("COOPER_GC", "stress,checkmark")],
        &[("COOPER_THREADS", "4")],
        &[("COOPER_THREADS", "4"), ("COOPER_GC", "stress,checkmark")],
        &[("COOPER_THREADS", "4"), ("COOPER_GC", "markstress,checkmark")],
    ];
    for (i, rerun) in reruns.into_iter().enumerate() {
        let again = run(
            Command::new(&executable).envs(rerun.iter().copied()).envs(variables.iter().copied()),
            &stacks(&format!("rerun{i}")),
        )
        .map_err(|e| format!("with {rerun:?}: {e}"))?;
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

/// The stores in the LLVM module `ir` that may write a pointer word outside the storing function's own frame
/// (its `alloca`s and the addresses computed from them, and in a C callback's `.run` function the `%frame` it is
/// given, which is on its thread's stack) with no write barrier on their address before them, each
/// as `function: store`. A barrier is a block named `shade...`, which reads the value it overwrites, or else
/// `cooper_array_grow`, which applies the barrier for storing its result over an array's data pointer. A store
/// into an object `cooper_alloc` returned in the same block, with no call since, needs none: it overwrites
/// nothing, and an object allocated during marking is marked.
///
/// A named type holds pointers if its definition mentions `ptr` or one that does, or if the module defines its
/// collector descriptor (`@cooper.type.<name>`), as it does for every type whose pointer words its LLVM type does
/// not show (sum types, `extern union`s).
fn unguarded_stores(ir: &str) -> Vec<String> {
    let mut bodies = HashMap::new();
    let mut described = HashSet::new();
    for line in ir.lines() {
        if let Some((name, body)) = line.strip_prefix('%').and_then(|l| l.split_once(" = type ")) {
            bodies.insert(name, body);
        } else if let Some(rest) = line.strip_prefix("@cooper.type.") {
            described.insert(rest.split(' ').next().unwrap_or_default());
        }
    }
    let holds_pointers = |ty: &str| {
        let mut pending = vec![ty];
        let mut seen = HashSet::new();
        while let Some(ty) = pending.pop() {
            for token in ty.split(|c: char| !(c.is_alphanumeric() || "%._$".contains(c))) {
                if token == "ptr" {
                    return true;
                }
                if let Some(name) = token.strip_prefix('%') {
                    if described.contains(name) {
                        return true;
                    }
                    if let Some(body) = bodies.get(name).filter(|_| seen.insert(name)) {
                        pending.push(body);
                    }
                }
            }
        }
        false
    };
    let mut unguarded = Vec::new();
    let mut function = "";
    let mut frame = HashSet::new();
    let mut guarded = HashSet::new();
    let mut grown = HashSet::new();
    let mut fresh = HashSet::new();
    let mut block = "";
    for line in ir.lines() {
        if let Some(label) = line.strip_suffix(':') {
            block = label;
            fresh.clear();
            continue;
        }
        if line.contains(" call ") || line.trim_start().starts_with("call ") {
            fresh.clear();
        }
        if line.starts_with("define ") {
            function = line.split('@').nth(1).and_then(|rest| rest.split('(').next()).unwrap_or(line);
            frame.clear();
            guarded.clear();
            grown.clear();
            if function.ends_with(".c.run") {
                frame.insert("%frame".to_string());
            }
            continue;
        }
        let line = line.trim();
        if let Some((name, rhs)) = line.split_once(" = ") {
            let base = operand_after(rhs, ", ptr ").filter(|_| rhs.starts_with("getelementptr "));
            if rhs.starts_with("alloca ") || base.is_some_and(|base| frame.contains(base)) {
                frame.insert(name.to_string());
            }
            if rhs.starts_with("call ptr @cooper_alloc(") || base.is_some_and(|base| fresh.contains(base)) {
                fresh.insert(name.to_string());
            }
            if block.starts_with("shade") && rhs.starts_with("load ") {
                guarded.extend(operand_after(rhs, ", ptr ").map(str::to_string));
            } else if rhs.starts_with("call ptr @cooper_array_grow(") {
                grown.insert(name.to_string());
            }
        } else if let Some(stored) = line.strip_prefix("store ") {
            let (ty, value) = split_type(stored);
            let value = value.split(',').next().unwrap_or_default();
            let destination = operand_after(stored, ", ptr ").unwrap_or_default();
            let exempt = frame.contains(destination) || fresh.contains(destination) || grown.contains(value);
            if holds_pointers(ty) && !exempt && !guarded.contains(destination) {
                unguarded.push(format!("{function}: {line}"));
            }
        }
    }
    unguarded
}

/// The operand following the last `marker` in an instruction.
fn operand_after<'l>(line: &'l str, marker: &str) -> Option<&'l str> {
    let (_, rest) = line.rsplit_once(marker)?;
    rest.split([',', ')', ' ']).next()
}

/// The leading LLVM type of `text` (a literal struct or array type is bracketed), and the rest.
fn split_type(text: &str) -> (&str, &str) {
    let mut depth = 0;
    for (at, c) in text.char_indices() {
        match c {
            '{' | '[' | '<' => depth += 1,
            '}' | ']' | '>' => depth -= 1,
            ' ' if depth == 0 => return (&text[..at], &text[at + 1..]),
            _ => {}
        }
    }
    (text, "")
}

#[test]
fn golden_programs_run_with_their_expected_exit_status() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    let build_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("programs");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/programs exists")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "coop"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no programs in {}", dir.display());
    let programs: Vec<(&PathBuf, bool)> = paths.iter().flat_map(|path| [(path, false), (path, true)]).collect();
    // The programs run a few at a time; each may itself use several OS threads.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failures = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                while let Some(&(path, release)) = programs.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    if let Err(why) = check_program(path, &build_root, release) {
                        let build = if release { " (--release)" } else { "" };
                        failures.lock().unwrap().push(format!("{}{build}: {why}", path.display()));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "failing programs:\n{}", failures.join("\n"));
}
