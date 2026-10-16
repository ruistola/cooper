//! `cooper test`: a project's `*_test.coop` functions are found, built into one program that
//! runs the test `COOPER_TEST` selects, and left out of an ordinary build.

use std::path::Path;
use std::process::Command;

fn write(root: &Path, name: &str, text: &str) {
    std::fs::write(root.join(name), text).unwrap();
}

#[test]
fn tests_run_one_at_a_time_and_report_their_own_status() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("test_runner");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "project.toml", "name = \"runner\"\n");
    write(&root, "main.coop", "func add(a: i32, b: i32): i32 { return a + b }\nfunc main() {}\n");
    write(
        &root,
        "main_test.coop",
        "use { std.testing.check }\n\
         func testPasses() { check(add(1, 2) == 3, \"sum\") }\n\
         func testFails() { check(add(1, 2) == 4, \"wrong sum\") }\n\
         func notATest() { check(false, \"never run\") }\n",
    );

    let loaded = cooper_cli::load_with_tests(&root).unwrap();
    let built = cooper_cli::build_tests(&loaded.project, &loaded.native, &loaded.build_dir, Default::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(built.tests, ["main.testPasses", "main.testFails"]);
    let run = |index: usize| {
        Command::new(&built.executable).env("COOPER_TEST", index.to_string()).output().unwrap()
    };
    assert!(run(0).status.success());
    let failing = run(1);
    assert_eq!(failing.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&failing.stderr);
    assert!(stderr.contains("FAIL: wrong sum"));
    assert!(stderr.contains("main_test.coop:3:"), "the failure says where: {stderr}");

    // An ordinary load leaves the test sources out.
    let plain = cooper_cli::load(&root).unwrap();
    assert!(plain.project.modules.iter().flat_map(|m| &m.files).all(|f| !f.name.ends_with("_test.coop")));
}
