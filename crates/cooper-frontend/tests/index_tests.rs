//! The position index, documentation comments, and lenient analysis, through the public API.

use cooper_frontend::index::{DeclKind, Index};
use cooper_frontend::{analyze_lenient, Project};

const SOURCE: &str = "\
# A point on the plane.
# Its fields are the coordinates.
struct Point {
    # The horizontal coordinate.
    x: i32,
    y: i32,
}

# The distance from the origin, roughly.
func norm(p: Point): i32 {
    return p.x + p.y
}

x := 3  # not documentation

# Not attached: a blank line follows.

func main() {
    p := Point{x: 1, y: 2}
    norm(p)
}
";

#[test]
fn doc_comments_attach_to_the_declaration_below_them() {
    let analysis = analyze_lenient(&Project::snippet(SOURCE));
    let index = Index::new(&analysis.project);
    let doc = |name: &str| {
        let found = index.declarations().iter().find(|d| d.name == name && d.file == "main").unwrap();
        found.doc.clone()
    };
    assert_eq!(doc("Point").as_deref(), Some("A point on the plane.\nIts fields are the coordinates."));
    assert_eq!(doc("x").as_deref(), Some("The horizontal coordinate."));
    assert_eq!(doc("y"), None);
    assert_eq!(doc("norm").as_deref(), Some("The distance from the origin, roughly."));
    assert_eq!(doc("main"), None);
}

#[test]
fn a_call_resolves_to_its_declaration() {
    let analysis = analyze_lenient(&Project::snippet(SOURCE));
    let index = Index::new(&analysis.project);
    let call = SOURCE.rfind("norm(p)").unwrap();
    let declaration = index.definition_at("main", call + 1).expect("the call resolves");
    assert_eq!((declaration.name.as_str(), declaration.kind), ("norm", DeclKind::Function));
    assert_eq!(declaration.ty.as_deref(), Some("func(Point):i32"));
    // The cursor on a declaration's own name finds it.
    let name = SOURCE.find("norm(p: Point)").unwrap();
    assert_eq!(index.definition_at("main", name).unwrap().name_span.start, name);
}

#[test]
fn analysis_continues_past_a_parse_error() {
    let source = "func good(): i32 { return 1 }\nfunc bad( {\nfunc main() { good() }\n";
    let analysis = analyze_lenient(&Project::snippet(source));
    assert!(!analysis.diagnostics.is_empty());
    let index = Index::new(&analysis.project);
    assert!(index.declarations().iter().any(|d| d.name == "good"));
}
