//! Compiler-blessed methods on built-in non-primitive types.
//!
//! A built-in type such as an array carries a fixed method set the compiler knows by
//! name rather than from a declaration: `xs.length()` and the indexing `xs[i]` /
//! `xs[i] = v` resolve here, not through a user-written method. These blessed names
//! are the one vocabulary the surface syntax and (later) user-defined "array-like"
//! types share — a type is array-like when its method set structurally covers this
//! one, the same structural-satisfaction rule a `where` clause uses, with no nominal
//! trait. Index syntax binds to the blessed names `get`/`set`, so `a[i]` is `a.get(i)`
//! and `a[i] = v` is `a.set(i, v)`; a future built-in map reuses the same names with a
//! key-typed signature. The bodies are intrinsics the lowering IR realises behind the
//! runtime boundary; only the signatures live here, so a call site type-checks through
//! the same method-access path as any other method.

use crate::types::Type;

/// The integer type array indices and lengths take. Provisional pending a dedicated
/// pointer-sized index type.
const INDEX_TYPE: &str = "u64";

/// The integer type an array index and length take.
pub(crate) fn index_type() -> Type {
    Type::Primitive(INDEX_TYPE.to_string())
}

/// The bound signature of the blessed method `name` on an array of `elem`, or `None`
/// when arrays carry no such method. The receiver is excluded, as for any bound
/// method. `get`/`set` are the methods the `a[i]` / `a[i] = v` index syntax binds to.
pub(crate) fn array_method(name: &str, elem: &Type) -> Option<Type> {
    let func = |param_types: Vec<Type>, return_type: Type| Type::Func {
        return_type: Box::new(return_type),
        param_types,
    };
    match name {
        "length" => Some(func(Vec::new(), index_type())),
        "get" => Some(func(vec![index_type()], elem.clone())),
        "set" => Some(func(vec![index_type(), elem.clone()], Type::Unit)),
        _ => None,
    }
}
