//! Compiler-blessed methods on built-in non-primitive types.
//!
//! A built-in type such as an array carries a fixed method set the compiler knows by
//! name rather than from a declaration: `xs.length()` and the indexing `xs[i]` /
//! `xs[i] = v` resolve here, not through a user-written method. Index (and, later,
//! slice) syntax is a *built-in privilege*: it binds to the blessed names `get`/`set`,
//! so `a[i]` is `a.get(i)` and `a[i] = v` is `a.set(i, v)`, but it is not extended to
//! user types. A user-defined collection stays explicit — it exposes ordinary methods
//! (`at`, `set`, …), visibly userspace — because the ergonomic surface (addressable
//! elements, copy/view slicing) is reachable only for types whose representation the
//! compiler controls. The planned built-in set that carries this sugar is static and
//! dynamic arrays, hashmaps, and hashsets. Growth (`push`) follows the return-value
//! idiom. The bodies are intrinsics the lowering IR realises behind the runtime
//! boundary; only the signatures live here, so a call site type-checks through the same
//! method-access path as any other method.
//!
//! The only array type modelled today is the dynamic `T[]`, which carries the full set
//! including `push`. A fixed-size static array `T[N]` — not yet a distinct type — would
//! carry the static subset (everything but growth), realising the static- vs
//! dynamic-array-like split once it exists.

use crate::types::{Type, INDEX_INT};

/// The integer type of an array's length and of the `get`/`set` index parameter.
/// Index syntax (`a[i]`) accepts any integer type.
pub(crate) fn index_type() -> Type {
    Type::Primitive(INDEX_INT.to_string())
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
        // Growth follows the return-value idiom: an append may hand back the same
        // array (spare capacity) or a fresh one (a full owner, or any view), so `push`
        // returns the possibly-new array rather than mutating in place.
        "push" => Some(func(vec![elem.clone()], Type::Array(Box::new(elem.clone())))),
        _ => None,
    }
}
