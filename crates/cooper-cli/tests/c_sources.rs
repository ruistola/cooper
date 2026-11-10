//! A project's own C sources (`[c] sources`) compile and link with its program, and include the runtime's
//! `cooper_ffi.h` to store Cooper pointers into Cooper memory.

use std::path::Path;
use std::process::Command;

#[test]
fn c_sources_store_cooper_pointers_through_the_runtime() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("c_sources");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("native")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap();
    write("project.toml", "name = \"csources\"\n\n[c]\nsources = [\"native/link.c\"]\n");
    write(
        "native/link.c",
        "#include <stdint.h>\n\
         #include \"cooper_ffi.h\"\n\
         struct Node { int32_t value; struct Node *next; };\n\
         void link(struct Node *from, struct Node *to) { cooper_store_pointer((void **)&from->next, to); }\n",
    );
    write(
        "main.coop",
        "struct Node { value: i32, next: Node^ }\n\
         extern func link(from: Node^, to: Node^)\n\
         func make(value: i32): Node^ {\n\
             node := Node{value: value, next: nil}\n\
             return &node\n\
         }\n\
         func main(): i32 {\n\
             let head: Node^ = nil\n\
             for i in 1..=100 do {\n\
                 node := make(i)\n\
                 link(node, head)\n\
                 head = node\n\
             }\n\
             total := 0\n\
             while head != nil do {\n\
                 total += head.value\n\
                 head = head.next\n\
             }\n\
             return if total == 5050 then 0 else 1\n\
         }\n",
    );

    let loaded = cooper_cli::load(&root).unwrap();
    let executable = cooper_cli::build(&loaded.project, &loaded.native, &loaded.build_dir, Default::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let run = Command::new(&executable).env("COOPER_GC", "stress,checkmark").output().unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
}
