//! The runtime's heap allocator and collector (`runtime/gc.c`), built with its own C test, natively and
//! under ThreadSanitizer.

use std::path::Path;
use std::process::Command;

/// Builds the test with `flags` and runs it, giving whether it passed and its output; `None` if a sanitizer
/// build is not possible here.
fn build_and_run(flags: &[&str]) -> Option<(bool, String)> {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gc_test{}", flags.concat()));
    let cc = std::env::var("COOPER_CC").unwrap_or_else(|_| "clang".to_string());
    let build = Command::new(&cc)
        .args(["-std=c11", "-Wall", "-Wextra", "-O0", "-I"])
        .arg(&runtime)
        .args(flags)
        .arg("-o")
        .arg(&out)
        .arg(runtime.join("gc.c"))
        .arg(runtime.join("sched.c"))
        .arg(runtime.join("gc_test.c"))
        .output()
        .expect("the C compiler runs");
    if !flags.is_empty() && !build.status.success() {
        return None;
    }
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&out).output().expect("the test runs");
    let output = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    Some((run.status.success(), output))
}

#[test]
fn the_allocator_passes_its_c_tests() {
    let (ok, output) = build_and_run(&[]).expect("native run");
    assert!(ok, "{output}");
}

#[test]
fn the_collector_has_no_data_races_thread_sanitizer_finds() {
    if let Some((ok, output)) = build_and_run(&["-fsanitize=thread"]) {
        assert!(ok && !output.contains("ThreadSanitizer"), "{output}");
    }
}
