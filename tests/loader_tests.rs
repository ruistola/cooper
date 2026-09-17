//! Integration test for the filesystem loader: the in-repo `examples/demo` project
//! is loaded from disk and analyzed, exercising the whole pipeline over a real,
//! multi-module source tree with both module-binding and name-binding `use`.

use std::path::Path;

use cooper::ProjectKind;

fn demo_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/demo")
}

#[test]
fn loads_and_analyzes_the_demo_project() {
    let project = cooper::load_project(&demo_dir()).expect("demo project loads");

    assert_eq!(project.manifest.name, "demo");
    assert_eq!(project.manifest.kind, ProjectKind::Program);

    // main, mathx, geometry — the root module plus its two subdirectories.
    let mut paths: Vec<String> = project.modules.iter().map(|m| m.path.to_string()).collect();
    paths.sort();
    assert_eq!(paths, vec!["geometry", "main", "mathx"]);

    let diags = cooper::analyze_project(&project);
    assert!(
        diags.is_empty(),
        "expected the demo project to analyze clean, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}
