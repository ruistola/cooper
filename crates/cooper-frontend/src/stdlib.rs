//! The standard library modules the compiler provides.
//!
//! A program reaches them with `use` like any module (`use { std.io }`). Their
//! interfaces are supplied here; their functions are implemented by the runtime, and
//! lowering turns each call into the matching IR intrinsic.
//!
//! * `std.io`: `print(s: string)` writes `s` to standard output, and `println(s: string)`
//!   writes it followed by a newline.
//! * `std.fmt`: `format(template: string): string`, the text of a template whose
//!   positional holes `{}` its further arguments fill (`format("X is {}", x)`). `print`
//!   and `println` take such arguments too.
//! * `std.ffi`: `CString`, a NUL-terminated C string (`struct CString { data: u8^ }`),
//!   converted from a string as `CString(s)` and back as `string(c)`, and
//!   `copyBytes(p: u8^, n: i64): u8[]`, which copies C bytes into an array.

use crate::project::ModulePath;
use crate::resolve::{Callable, Globals, Signature};
use crate::types::{StructDef, Type, TypeId};

/// The module path of standard input and output.
pub const IO_MODULE: &str = "std.io";
/// The module path of text formatting.
pub const FMT_MODULE: &str = "std.fmt";
/// The module path of the C interop helpers.
pub const FFI_MODULE: &str = "std.ffi";
/// The C string type `std.ffi` declares.
pub const C_STRING: &str = "CString";

/// The interfaces of the compiler-provided modules, by path.
pub(crate) fn interfaces() -> Vec<(ModulePath, Globals)> {
    let string = || Type::Primitive("string".to_string());
    let byte_ptr = || Type::Pointer(Box::new(Type::Primitive("u8".to_string())));

    let mut io = module(IO_MODULE);
    for name in ["print", "println"] {
        function(&mut io, name, vec![string()], Type::Unit);
    }

    let mut fmt = module(FMT_MODULE);
    function(&mut fmt, "format", vec![string()], string());

    let mut ffi = module(FFI_MODULE);
    let id = TypeId {
        module: FFI_MODULE.to_string(),
        name: C_STRING.to_string(),
    };
    ffi.structs.insert(C_STRING.to_string(), id.clone());
    ffi.defs.structs.insert(
        id,
        StructDef {
            type_params: Vec::new(),
            members: vec![("data".to_string(), byte_ptr())],
        },
    );
    let bytes = Type::Array(Box::new(Type::Primitive("u8".to_string())));
    function(&mut ffi, "copyBytes", vec![byte_ptr(), crate::builtins::index_type()], bytes);

    vec![
        (ModulePath::parse(IO_MODULE), io),
        (ModulePath::parse(FMT_MODULE), fmt),
        (ModulePath::parse(FFI_MODULE), ffi),
    ]
}

/// Whether `item` takes a template, whose positional holes its further arguments fill:
/// `std.fmt.format`, `std.io.print`, and `std.io.println`.
pub fn takes_template(item: &Callable) -> bool {
    matches!(item, Callable::Func { module, name }
        if (module == FMT_MODULE && name == "format") || (module == IO_MODULE && (name == "print" || name == "println")))
}

/// Whether `ty` is `std.ffi`'s `CString`, which converts from and to `string`.
pub fn is_c_string(ty: &Type) -> bool {
    matches!(ty, Type::Struct { id, .. } if id.module == FFI_MODULE && id.name == C_STRING)
}

fn module(path: &str) -> Globals {
    let mut globals = Globals::default();
    globals.module = path.to_string();
    globals
}

fn function(globals: &mut Globals, name: &str, params: Vec<Type>, ret: Type) {
    let signature = Signature {
        item: Callable::Func {
            module: globals.module.clone(),
            name: name.to_string(),
        },
        type_params: Vec::new(),
        receiver: None,
        ty: Type::Func {
            return_type: Box::new(ret),
            param_types: params,
        },
    };
    globals.funcs.insert(name.to_string(), signature);
}
