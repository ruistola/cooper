//! Whole-program lowering through the public API: a checked project lowered and
//! monomorphized into the IR a backend consumes.

use cooper_frontend::resolve::Callable;
use cooper_frontend::{check_project, Module, Project, ProjectKind, SourceFile};
use cooper_ir::{lower_program, Program};

/// Check and lower a program built from one-file modules given as `(path, source)`.
fn program(modules: &[(&str, &str)]) -> Program {
    let modules = modules
        .iter()
        .map(|(path, src)| Module::new(path, vec![SourceFile::new(*path, *src)]))
        .collect();
    let project = Project::new("program", ProjectKind::Program, modules);
    let checked = check_project(&project).unwrap_or_else(|diags| {
        panic!("program checks: {:?}", diags.iter().map(|d| &d.message).collect::<Vec<_>>())
    });
    lower_program(&checked).expect("program lowers")
}

#[test]
fn a_generic_function_from_another_module_is_instantiated_per_use() {
    let program = program(&[
        (
            "util",
            "struct Box T {\n  v: T,\n}\nfunc wrap T (x: T): Box T {\n  return Box{v: x}\n}",
        ),
        (
            "main",
            "use {\n  util,\n}\nfunc main(): i32 {\n  b := util.wrap(1)\n  s := util.wrap(\"s\")\n  return b.v\n}",
        ),
    ]);
    let wraps: Vec<String> = program
        .functions
        .iter()
        .filter(|f| matches!(&f.item, Callable::Func { module, name } if module == "util" && name == "wrap"))
        .map(|f| f.return_type.to_string())
        .collect();
    assert_eq!(wraps, ["Box i32", "Box string"]);
    assert!(
        program.functions.iter().all(|f| f.type_args.len() == f.type_params.len()),
        "every function is a concrete instance"
    );
    let boxed = program
        .defs
        .structs
        .keys()
        .find(|id| id.name == "Box")
        .expect("the program carries Box's definition");
    assert_eq!(boxed.module, "util");
}
