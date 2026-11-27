//! The standard library modules the compiler provides.
//!
//! A program reaches them with `use` like any module (`use { std.io }`).
//!
//! * Modules written in Cooper, embedded in the compiler (`std/`): `std.io` (`print` and
//!   `println`, over the C library's `write`), `std.testing` (assertions for `cooper test`), `std.fmt` (`format`, the text of a template
//!   whose positional holes `{}` its further arguments fill, and `list`, `quote`, and `fixed`), `std.strings`,
//!   `std.arrays`, and `std.runes` (the methods of `string`, the arrays, and `rune`), `std.math`
//!   (the checked conversions of the numeric types, functions such as `min` and `sqrt`, and
//!   constants; the compiler provides its [`FLOAT_FUNCTIONS`]), `std.threads`, `std.sync`, and `std.gc` (control of the garbage collector). A program that reaches one through its `use` declarations,
//!   directly or through another std module, has its source compiled with it; the others cost
//!   nothing. The compiler fills the holes of a template call (`io.println("x is {}", x)`).
//! * The prelude, in scope in every module without a `use`: the sum types `Maybe T`
//!   (`None`, `Some(T)`) and `Result T E` (`Ok(T)`, `Err(E)`), and `Thread`, the handle a
//!   `thread` expression gives (`struct Thread { id: i64 }`). A module's own declaration of
//!   one of these names shadows the prelude's.
//! * `std.ffi`, interface only (the runtime implements it): `CString`, a NUL-terminated C string (`struct CString { data: u8^ }`),
//!   converted from a string as `ffi.CString(s)` and back as `string(c)`, and
//!   `copyBytes(p: u8^, n: i64): u8[]`, which copies C bytes into an array.

use std::collections::HashSet;

use crate::ast::{TypeExpr, TypeExprKind};
use crate::project::{Module, ModulePath, Project, SourceFile};
use crate::resolve::{Callable, Globals, Signature};
use crate::types::{StructDef, Type, TypeId, UnionDef};

/// The module path of standard input and output.
pub const IO_MODULE: &str = "std.io";
/// The module path of text formatting.
pub const FMT_MODULE: &str = "std.fmt";
/// The module path of the numeric types' home.
pub const MATH_MODULE: &str = "std.math";

/// The functions of `std.math` the compiler provides, by name and number of parameters. Each is
/// `func name T (x: T, …): T` for a float type `T`, computed as the C library's function of the
/// same name computes it, or for `f32` the one with an `f` suffix.
pub const FLOAT_FUNCTIONS: [(&str, usize); 23] = [
    ("sqrt", 1), ("cbrt", 1), ("pow", 2), ("hypot", 2),
    ("exp", 1), ("exp2", 1), ("log", 1), ("log2", 1), ("log10", 1),
    ("sin", 1), ("cos", 1), ("tan", 1), ("asin", 1), ("acos", 1), ("atan", 1), ("atan2", 2),
    ("sinh", 1), ("cosh", 1), ("tanh", 1),
    ("floor", 1), ("ceil", 1), ("round", 1), ("trunc", 1),
];

/// The module path of the C interop helpers.
pub const FFI_MODULE: &str = "std.ffi";
/// The C string type `std.ffi` declares.
pub const C_STRING: &str = "CString";

/// The module the prelude's types belong to. It is not a module a program can `use`.
pub const PRELUDE_MODULE: &str = "std.prelude";

/// The handle of a thread, the type of a `thread` expression.
pub fn thread_type() -> Type {
    Type::Struct { id: TypeId { module: PRELUDE_MODULE.to_string(), name: "Thread".to_string() }, type_args: Vec::new() }
}

/// The declarations every module sees without a `use`: `Maybe`, `Result`, and `Thread`. The
/// first variant of each sum type is the one a zero value takes.
pub(crate) fn prelude() -> Globals {
    let param = |name: &str| Type::TypeParam(name.to_string());
    let mut globals = module(PRELUDE_MODULE);
    let mut union = |name: &str, params: &[&str], variants: Vec<(&str, Vec<Type>)>| {
        let id = TypeId {
            module: PRELUDE_MODULE.to_string(),
            name: name.to_string(),
        };
        globals.unions.insert(name.to_string(), id.clone());
        globals.defs.unions.insert(
            id,
            UnionDef {
                type_params: params.iter().map(|p| p.to_string()).collect(),
                variant_order: variants.iter().map(|(v, _)| v.to_string()).collect(),
                variants: variants.into_iter().map(|(v, slots)| (v.to_string(), slots)).collect(),
                ..Default::default()
            },
        );
    };
    union("Maybe", &["T"], vec![("None", vec![]), ("Some", vec![param("T")])]);
    union("Result", &["T", "E"], vec![("Ok", vec![param("T")]), ("Err", vec![param("E")])]);
    let Type::Struct { id, .. } = thread_type() else { unreachable!() };
    globals.structs.insert(id.name.clone(), id.clone());
    globals.defs.structs.insert(
        id,
        StructDef { type_params: Vec::new(), members: vec![("id".to_string(), Type::Primitive("i64".to_string()))], ..Default::default() },
    );
    globals
}

/// The standard library modules that declare the methods of built-in types: their homes. A home
/// module is part of every program, and sees the methods of the homes before it.
pub(crate) const HOMES: [&str; 4] = ["std.arrays", MATH_MODULE, "std.runes", "std.strings"];

/// The home module of the built-in type `name`, if it has methods.
pub(crate) fn home(name: &str) -> Option<&'static str> {
    match name {
        ARRAY => Some("std.arrays"),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" => Some(MATH_MODULE),
        "rune" => Some("std.runes"),
        "string" => Some("std.strings"),
        _ => None,
    }
}

/// The identity a method on the built-in type `ty` is declared under, if `ty` has a home.
pub fn builtin_owner(ty: &Type) -> Option<TypeId> {
    let name = match ty {
        Type::Primitive(name) => name.as_str(),
        Type::Array(_) => ARRAY,
        _ => return None,
    };
    home(name).map(|_| TypeId {
        module: PRELUDE_MODULE.to_string(),
        name: name.to_string(),
    })
}

/// The identity of the built-in type a method receiver `ty` names, through a pointer.
pub fn builtin_receiver_owner(ty: &TypeExpr) -> Option<TypeId> {
    match &ty.kind {
        TypeExprKind::Pointer(inner) => builtin_receiver_owner(inner),
        TypeExprKind::Named(name) => builtin_owner(&Type::Primitive(name.clone())),
        TypeExprKind::Array(_) => builtin_owner(&Type::Array(Box::new(Type::Unit))),
        _ => None,
    }
}

/// The name the methods of the dynamic array types are keyed under, whatever the element.
const ARRAY: &str = "array";

/// A standard library module written in Cooper: its path, and its files' names and sources.
type SourceModule = (&'static str, &'static [(&'static str, &'static str)]);

/// The standard library modules written in Cooper, one directory each under `std/`.
const SOURCE_MODULES: [SourceModule; 9] = [
    ("std.arrays", &[
        ("std/arrays/arrays.coop", include_str!("../std/arrays/arrays.coop")),
        ("std/arrays/internal.coop", include_str!("../std/arrays/internal.coop")),
    ]),
    ("std.strings", &[
        ("std/strings/strings.coop", include_str!("../std/strings/strings.coop")),
        ("std/strings/internal.coop", include_str!("../std/strings/internal.coop")),
    ]),
    ("std.runes", &[("std/runes/runes.coop", include_str!("../std/runes/runes.coop"))]),
    ("std.io", &[
        ("std/io/io.coop", include_str!("../std/io/io.coop")),
        ("std/io/internal.coop", include_str!("../std/io/internal.coop")),
    ]),
    ("std.fmt", &[("std/fmt/fmt.coop", include_str!("../std/fmt/fmt.coop"))]),
    ("std.threads", &[("std/threads/threads.coop", include_str!("../std/threads/threads.coop"))]),
    ("std.sync", &[
        ("std/sync/sync.coop", include_str!("../std/sync/sync.coop")),
        ("std/sync/internal.coop", include_str!("../std/sync/internal.coop")),
    ]),
    ("std.testing", &[("std/testing/testing.coop", include_str!("../std/testing/testing.coop"))]),
    ("std.gc", &[("std/gc/gc.coop", include_str!("../std/gc/gc.coop"))]),
];

/// The standard library module `path` as a module of a project, if it is one written in Cooper.
fn source_module(path: &str) -> Option<Module> {
    if path == MATH_MODULE {
        let files = vec![
            SourceFile::new("std/math/math.coop", include_str!("../std/math/math.coop")),
            SourceFile::new("std/math/conversions.coop", math_conversions()),
        ];
        return Some(Module::new(path, files));
    }
    let (path, files) = SOURCE_MODULES.iter().find(|(p, _)| *p == path)?;
    Some(Module::new(path, files.iter().map(|(name, source)| SourceFile::new(*name, *source)).collect()))
}

/// The source of `std.math`'s checked conversions, generated since there is one method for
/// each pair of types: from every integer and float type to every other integer type, and
/// from every integer type to `rune`. Each converts with `T(n)` and checks that the result is
/// the same number.
fn math_conversions() -> String {
    use crate::types::{integer_bits, FLOATS, SIGNED_INTS, UNSIGNED_INTS};
    use std::fmt::Write;
    let ints: Vec<&str> = SIGNED_INTS.iter().chain(&UNSIGNED_INTS).copied().collect();
    let method = |to: &str| format!("to{}{}", to[..1].to_uppercase(), &to[1..]);
    let mut out = String::new();
    for from in &ints {
        for to in ints.iter().filter(|to| *to != from) {
            // Converting back fails to restore `n` when it does not fit, except that a change
            // of signedness at one width restores it, so the signs are compared too.
            let _ = writeln!(
                out,
                "(n: {from}) func {}(): Maybe {to} {{\n    m := {to}(n)\n    \
                 return if {from}(m) == n and (m < 0) == (n < 0) then Some(m) else None\n}}\n",
                method(to)
            );
        }
        let _ = writeln!(
            out,
            "(n: {from}) func toRune(): Maybe rune {{\n    c := u64(n)\n    \
             return if n >= 0 and c <= 0x10FFFF and (c < 0xD800 or c > 0xDFFF) then Some(rune(n)) else None\n}}\n"
        );
    }
    for from in FLOATS {
        let mantissa = if from == "f32" { 24 } else { 53 };
        for to in &ints {
            let bits = integer_bits(to);
            // `T(f)` rounds toward zero, so the bounds admit the fractions that round into
            // range. Below a signed type's minimum, a float either exists one less than it,
            // or nothing lies between them.
            let (lower, upper) = if SIGNED_INTS.contains(to) {
                let min = -(1i128 << (bits - 1));
                let lower = if bits - 1 < mantissa { format!("f > {}.0", min - 1) } else { format!("f >= {min}.0") };
                (lower, 1i128 << (bits - 1))
            } else {
                ("f > -1.0".to_string(), 1i128 << bits)
            };
            let _ = writeln!(
                out,
                "(f: {from}) func {}(): Maybe {to} {{\n    \
                 return if {lower} and f < {upper}.0 then Some({to}(f)) else None\n}}\n",
                method(to)
            );
        }
    }
    out
}

/// `project` with the embedded standard library modules its `use` declarations reach, and
/// those they reach in turn, added as ordinary modules.
pub fn with_source_modules(project: &Project) -> Project {
    let mut project = project.clone();
    let mut present: HashSet<String> = project.modules.iter().map(|m| m.path.to_string()).collect();
    for home in HOMES {
        if present.contains(home) {
            continue;
        }
        if let Some(module) = source_module(home) {
            present.insert(home.to_string());
            project.modules.push(module);
        }
    }
    let mut pending: Vec<usize> = (0..project.modules.len()).collect();
    while let Some(index) = pending.pop() {
        let mut wanted = Vec::new();
        for file in &project.modules[index].files {
            let Ok(tokens) = crate::lexer::tokenize(&file.source) else { continue };
            for spec in crate::parser::parse(tokens).uses {
                // A binding names a module, or an item of one.
                let full = spec.path.join(".");
                let owner = spec.path[..spec.path.len().saturating_sub(1)].join(".");
                wanted.extend([full, owner]);
            }
        }
        for name in wanted {
            if present.contains(&name) {
                continue;
            }
            if let Some(module) = source_module(&name) {
                present.insert(name);
                project.modules.push(module);
                pending.push(project.modules.len() - 1);
            }
        }
    }
    project
}

/// The interfaces of the compiler-provided modules, by path.
pub(crate) fn interfaces() -> Vec<(ModulePath, Globals)> {
    let byte_ptr = || Type::Pointer(Box::new(Type::Primitive("u8".to_string())));

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
            ..Default::default()
        },
    );
    let bytes = Type::Array(Box::new(Type::Primitive("u8".to_string())));
    function(&mut ffi, "copyBytes", vec![byte_ptr(), crate::builtins::index_type()], bytes);

    vec![
        (ModulePath::parse(FFI_MODULE), ffi),
    ]
}

/// Add the functions the compiler provides to the module `globals` declares, if any.
pub(crate) fn provide(globals: &mut Globals) {
    if globals.module != MATH_MODULE {
        return;
    }
    let param = Type::TypeParam("T".to_string());
    for (name, arity) in FLOAT_FUNCTIONS {
        let requirement = crate::resolve::Requirement {
            param: "T".to_string(),
            kind: crate::types::PredicateKind::Float,
            origin: "takes f32 or f64".to_string(),
        };
        let signature = Signature {
            item: Callable::Func { module: MATH_MODULE.to_string(), name: name.to_string() },
            type_params: vec!["T".to_string()],
            receiver: None,
            ty: Type::Func { return_type: Box::new(param.clone()), param_types: vec![param.clone(); arity] },
            by_pointer: false,
            variadic: false,
            requirements: vec![requirement],
        };
        globals.funcs.insert(name.to_string(), signature);
    }
}

/// Whether `item` is `std.fmt.format`, which is the text of its template.
pub fn is_format(item: &Callable) -> bool {
    matches!(item, Callable::Func { module, name } if module == FMT_MODULE && name == "format")
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
        by_pointer: false,
        variadic: false,
        requirements: Vec::new(),
    };
    globals.funcs.insert(name.to_string(), signature);
}
