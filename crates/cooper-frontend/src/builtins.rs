//! Compiler-blessed methods on built-in non-primitive types.
//!
//! A built-in type such as an array carries a fixed method set the compiler knows by
//! name rather than from a declaration: `xs.length()` resolves here, not through a
//! user-written method. These blessed names are the one vocabulary the surface syntax
//! and (later) user-defined "array-like" types share — a type is array-like when its
//! method set structurally covers this one, the same structural-satisfaction rule a
//! `where` clause uses, with no nominal trait. The bodies are intrinsics the lowering
//! IR realises behind the runtime boundary; only the signatures live here, so a call
//! site type-checks through the same method-access path as any other method.

use crate::types::Type;

/// The integer type array lengths and indices take.
const INDEX_TYPE: &str = "u64";

/// The bound signature of the blessed method `name` on an array, or `None` when
/// arrays carry no such method. The receiver is excluded, as for any bound method.
pub(crate) fn array_method(name: &str) -> Option<Type> {
    match name {
        "length" => Some(Type::Func {
            return_type: Box::new(Type::Primitive(INDEX_TYPE.to_string())),
            param_types: Vec::new(),
        }),
        _ => None,
    }
}
