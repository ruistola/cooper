//! The runtime's scheduler (`runtime/sched.c`), built with its own C test, natively and, where the
//! host can run it, as x86-64 under Rosetta.

use std::path::Path;
use std::process::Command;

fn build_and_run(target: Option<&str>) -> Option<(bool, String)> {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let name = format!("sched_test_{}", target.unwrap_or("native"));
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let cc = std::env::var("COOPER_CC").unwrap_or_else(|_| "clang".to_string());
    let mut build = Command::new(&cc);
    build.args(["-std=c11", "-Wall", "-Wextra", "-O1", "-I"]).arg(&runtime);
    if let Some(target) = target {
        build.args(["-target", target]);
    }
    let build = build
        .arg("-o")
        .arg(&out)
        .arg(runtime.join("sched.c"))
        .arg(runtime.join("sched_test.c"))
        .output()
        .expect("the C compiler runs");
    if target.is_some() && !build.status.success() {
        return None;
    }
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let mut run = if target.is_some() { Command::new("arch") } else { Command::new(&out) };
    if target.is_some() {
        run.args(["-x86_64"]).arg(&out);
    }
    let run = run.output().ok()?;
    Some((run.status.success(), format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr))))
}

#[test]
fn the_scheduler_passes_its_c_tests() {
    let (ok, output) = build_and_run(None).expect("native run");
    assert!(ok, "{output}");
}

#[test]
fn the_x86_64_context_switch_works_under_rosetta() {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        if let Some((ok, output)) = build_and_run(Some("x86_64-apple-macos")) {
            assert!(ok, "{output}");
        }
    }
}
