//! Symbol mangling: the linker-level name of every function instance.
//!
//! A symbol encodes a function's identity and its type arguments, so two instances of a
//! generic, or two same-named functions in different modules, never share a name. The
//! scheme follows the length-prefixed style of Itanium C++ and Rust v0 mangling: every
//! identifier is written as its byte length followed by its bytes, which keeps symbols
//! within `[A-Za-z0-9_]` and makes them unambiguous to decode.
//!
//! ```text
//! symbol   ::= "_C" item [ "I" type+ "E" ]
//! item     ::= "F" path ident             free function: module, name
//!            | "M" path ident ident       method: module, receiver type, name
//! path     ::= "N" ident+ "E"             module path segments
//! ident    ::= <decimal byte length> <bytes>
//! type     ::= "p" ident                  primitive, by name (p3i32)
//!            | "u"                        unit
//!            | "P" type                   pointer
//!            | "A" type                   array
//!            | "T" type+ "E"              tuple
//!            | "K" type* "E" type         function: parameters, then return type
//!            | "Q" path ident [ "I" type+ "E" ]   struct or sum type, with arguments
//!            | "G" ident                  type parameter (absent after monomorphization)
//! ```
//!
//! `main.add` mangles to `_CFN4mainE3add`; `Box.get` in module `util` at `i32` to
//! `_CMN4utilE3Box3getIp3i32E`.

use std::fmt::Write;

use cooper_frontend::resolve::Callable;
use cooper_frontend::types::Type;

/// The symbol of the instance of `item` at `type_args`.
pub fn symbol(item: &Callable, type_args: &[Type]) -> String {
    let mut out = String::from("_C");
    match item {
        Callable::Func { module, name } => {
            out.push('F');
            path(&mut out, module);
            ident(&mut out, name);
        }
        Callable::Method { receiver, name } => {
            out.push('M');
            path(&mut out, &receiver.module);
            ident(&mut out, &receiver.name);
            ident(&mut out, name);
        }
    }
    args(&mut out, type_args);
    out
}

/// The mangled encoding of `ty` alone (the `type` production), which names its LLVM
/// struct type.
pub(crate) fn type_name(ty: &Type) -> String {
    let mut out = String::new();
    self::ty(&mut out, ty);
    out
}

fn ident(out: &mut String, name: &str) {
    write!(out, "{}{name}", name.len()).expect("writing to a String cannot fail");
}

/// A dotted module path as its segments.
fn path(out: &mut String, module: &str) {
    out.push('N');
    module.split('.').for_each(|segment| ident(out, segment));
    out.push('E');
}

fn args(out: &mut String, type_args: &[Type]) {
    if type_args.is_empty() {
        return;
    }
    out.push('I');
    type_args.iter().for_each(|t| ty(out, t));
    out.push('E');
}

fn ty(out: &mut String, t: &Type) {
    match t {
        Type::Primitive(name) => {
            out.push('p');
            ident(out, name);
        }
        Type::Unit => out.push('u'),
        Type::Pointer(elem) => {
            out.push('P');
            ty(out, elem);
        }
        Type::Array(elem) => {
            out.push('A');
            ty(out, elem);
        }
        Type::Tuple(elems) => {
            out.push('T');
            elems.iter().for_each(|e| ty(out, e));
            out.push('E');
        }
        Type::Func {
            return_type,
            param_types,
        } => {
            out.push('K');
            param_types.iter().for_each(|p| ty(out, p));
            out.push('E');
            ty(out, return_type);
        }
        Type::Struct { id, type_args } | Type::Oneof { id, type_args } => {
            out.push('Q');
            path(out, &id.module);
            ident(out, &id.name);
            args(out, type_args);
        }
        Type::TypeParam(name) => {
            out.push('G');
            ident(out, name);
        }
        // None of these names a value a function instance can be generic over.
        Type::Unknown | Type::Nil | Type::Module(_) | Type::Infer(_) => {
            unreachable!("a type argument is a concrete value type, not {t:?}")
        }
    }
}

#[cfg(test)]
mod tests {
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
}
