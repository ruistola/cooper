//! The standard library modules the compiler provides.
//!
//! A program reaches them with `use` like any module (`use { std.io }`). Their
//! interfaces are supplied here; their functions are implemented by the runtime, and
//! lowering turns each call into the matching IR intrinsic.

use crate::project::ModulePath;
use crate::resolve::{Callable, Globals, Signature};
use crate::types::Type;

/// The module path of standard input and output.
pub const IO_MODULE: &str = "std.io";

/// `std.io`'s functions: `print(s: string)` writes `s` to standard output, and
/// `println(s: string)` writes it followed by a newline.
pub const IO_FUNCTIONS: [&str; 2] = ["print", "println"];

/// The interfaces of the compiler-provided modules, by path.
pub(crate) fn interfaces() -> Vec<(ModulePath, Globals)> {
    let mut io = Globals::default();
    io.module = IO_MODULE.to_string();
    for name in IO_FUNCTIONS {
        let signature = Signature {
            item: Callable::Func {
                module: IO_MODULE.to_string(),
                name: name.to_string(),
            },
            type_params: Vec::new(),
            receiver: None,
            ty: Type::Func {
                return_type: Box::new(Type::Unit),
                param_types: vec![Type::Primitive("string".to_string())],
            },
        };
        io.funcs.insert(name.to_string(), signature);
    }
    vec![(ModulePath::parse(IO_MODULE), io)]
}
