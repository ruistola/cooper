//! The runtime's heap allocator (`runtime/gc.c`), built with its own C test.

use std::path::Path;
use std::process::Command;

#[test]
fn the_allocator_passes_its_c_tests() {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("gc_test");
    let cc = std::env::var("COOPER_CC").unwrap_or_else(|_| "clang".to_string());
    let build = Command::new(&cc)
        .args(["-std=c11", "-Wall", "-Wextra", "-O0", "-I"])
        .arg(&runtime)
        .arg("-o")
        .arg(&out)
        .arg(runtime.join("gc.c"))
        .arg(runtime.join("sched.c"))
        .arg(runtime.join("gc_test.c"))
        .output()
        .expect("the C compiler runs");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&out).output().expect("the test runs");
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
