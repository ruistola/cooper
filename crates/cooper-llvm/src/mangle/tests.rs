use super::*;
use cooper_frontend::types::TypeId;

fn func(module: &str, name: &str) -> Callable {
    Callable::Func {
        module: module.to_string(),
        name: name.to_string(),
    }
}

fn prim(name: &str) -> Type {
    Type::Primitive(name.to_string())
}

#[test]
fn symbols_follow_the_documented_grammar() {
    assert_eq!(symbol(&func("main", "add"), &[]), "_CFN4mainE3add");
    assert_eq!(symbol(&func("server.auth", "login"), &[]), "_CFN6server4authE5login");
    let get = Callable::Method {
        receiver: TypeId {
            module: "util".to_string(),
            name: "Box".to_string(),
        },
        name: "get".to_string(),
    };
    assert_eq!(symbol(&get, &[prim("i32")]), "_CMN4utilE3Box3getIp3i32E");
}

#[test]
fn distinct_instances_get_distinct_symbols() {
    // Same name in two modules, and one generic at two type arguments.
    assert_ne!(symbol(&func("a", "f"), &[]), symbol(&func("b", "f"), &[]));
    let id = func("main", "id");
    assert_ne!(symbol(&id, &[prim("i32")]), symbol(&id, &[prim("i64")]));
    // Length prefixes keep segment boundaries unambiguous: `ab.c` is not `a.bc`.
    assert_ne!(symbol(&func("ab.c", "f"), &[]), symbol(&func("a.bc", "f"), &[]));
    // A tuple of two arguments is not two arguments.
    let pair = Type::Tuple(vec![prim("i32"), prim("bool")]);
    assert_ne!(
        symbol(&id, &[pair]),
        symbol(&id, &[prim("i32"), prim("bool")])
    );
}
