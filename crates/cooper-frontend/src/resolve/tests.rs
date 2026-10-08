use super::*;
use crate::{lexer, parser};

#[test]
fn struct_members_keep_their_declaration_order() {
    // The member order is the struct's layout, so it must survive resolution
    // exactly as written, never sorted or hashed.
    let source = "struct S {\n  zeta: i32,\n  alpha: bool,\n  mid: string,\n}";
    let decls = parser::parse(lexer::tokenize(source).expect("lexes")).decls;
    let (globals, diags) = resolve_into(Globals::default(), "main", &decls);
    assert!(diags.is_empty(), "{diags:?}");
    let s = globals.lookup_struct("S").expect("S is declared");
    let names: Vec<String> = globals
        .defs
        .struct_members(&s)
        .expect("S has members")
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["zeta", "alpha", "mid"]);
}
