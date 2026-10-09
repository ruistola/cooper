//! The C boundary: how `extern` C functions are declared and called.
//!
//! An integer, a float, `bool`, or a pointer passes to C directly, with the extension
//! attribute the C ABI gives small integers and `bool`. So does a struct of one member
//! that passes as an integer, `bool`, or pointer (such as `std.ffi.CString`), which every
//! supported 64-bit ABI passes like that member.
//!
//! Every other struct passes through a C wrapper the backend generates and the C
//! compiler builds with the program, so the C compiler, not this backend, implements the
//! target's rules for structs by value. The wrapper takes each struct argument, and the
//! struct result, by pointer, and makes the by-value call:
//!
//! ```c
//! void cooper_shim_name(cooper_t_P *ret, const cooper_t_P *a0, int32_t a1) {
//!     *ret = name(*a0, a1);
//! }
//! ```
//!
//! Each struct gets a C definition with its members in order, which lays it out as the
//! backend does.

use std::fmt::Write;

use cooper_frontend::types::Type;
use cooper_ir::Extern;

use crate::layout::Scalar;
use crate::{mangle, Module};

/// How one parameter or result crosses to C.
pub(crate) enum Passing {
    /// As the LLVM scalar `llvm` (with C's extension attribute, if any), extracted from
    /// any one-member structs around it.
    Direct {
        llvm: String,
        extension: Option<&'static str>,
    },
    /// By pointer to a wrapper.
    Indirect,
}

impl Module<'_> {
    /// How a value of type `ty` crosses to C.
    pub(crate) fn passing(&mut self, ty: &Type) -> Result<Passing, String> {
        let mut scalar = ty.clone();
        let mut depth = 0;
        while let Type::Struct { .. } = scalar {
            match self.defs.struct_members(&scalar).as_deref() {
                Some([(_, member)]) => scalar = member.clone(),
                _ => return Ok(Passing::Indirect),
            }
            depth += 1;
        }
        let llvm = self.llvm_type(&scalar)?.ok_or_else(|| format!("passing {ty} to C"))?;
        // A one-member struct holding a float is passed in an integer register on some
        // targets (Windows x64), unlike the float itself.
        let wrapped_float = depth > 0 && matches!(Scalar::of(&scalar), Some(Scalar::Float { .. }));
        if wrapped_float {
            return Ok(Passing::Indirect);
        }
        // C widens a small integer or `bool` passed on its own, but not a struct member.
        let extension = match Scalar::of(&scalar) {
            _ if depth > 0 => None,
            Some(Scalar::Bool) => Some("zeroext"),
            Some(Scalar::Int { bits: 8 | 16, signed: true }) => Some("signext"),
            Some(Scalar::Int { bits: 8 | 16, signed: false }) => Some("zeroext"),
            _ => None,
        };
        Ok(Passing::Direct { llvm, extension })
    }

    /// Whether a call to `declared` goes through its C wrapper.
    pub(crate) fn needs_shim(&mut self, params: &[Type], return_type: &Type) -> Result<bool, String> {
        for ty in params.iter().chain(std::iter::once(return_type)) {
            if !matches!(ty, Type::Unit) && matches!(self.passing(ty)?, Passing::Indirect) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Declare the C function `declared`, or define its wrapper and declare that.
    pub(crate) fn declare_extern(&mut self, declared: &Extern) -> Result<(), String> {
        let shim = self.needs_shim(&declared.params, &declared.return_type)?;
        let mut params = Vec::new();
        let mut ret = "void".to_string();
        if shim && !matches!(declared.return_type, Type::Unit) {
            params.push("ptr".to_string());
        } else if !matches!(declared.return_type, Type::Unit) {
            ret = self.c_return(&declared.return_type)?;
        }
        for ty in &declared.params {
            params.push(match self.passing(ty)? {
                Passing::Indirect => "ptr".to_string(),
                Passing::Direct { .. } => self.c_param(ty)?,
            });
        }
        let symbol = if shim { shim_symbol(&declared.name) } else { declared.name.clone() };
        self.declare(&format!("declare {ret} @{symbol}({})", params.join(", ")));
        if shim {
            self.define_shim(declared)?;
        }
        Ok(())
    }

    /// The LLVM type of `ty` as a direct C parameter, with its attribute (`i8 signext`).
    pub(crate) fn c_param(&mut self, ty: &Type) -> Result<String, String> {
        let Passing::Direct { llvm, extension, .. } = self.passing(ty)? else {
            unreachable!("only a direct value has an LLVM parameter type");
        };
        Ok(format!("{llvm}{}", extension.map(|e| format!(" {e}")).unwrap_or_default()))
    }

    /// The LLVM type of `ty` as a direct C result, after its attribute (`signext i8`).
    pub(crate) fn c_return(&mut self, ty: &Type) -> Result<String, String> {
        let Passing::Direct { llvm, extension, .. } = self.passing(ty)? else {
            unreachable!("only a direct value has an LLVM return type");
        };
        Ok(format!("{}{llvm}", extension.map(|e| format!("{e} ")).unwrap_or_default()))
    }

    /// Define the C wrapper of `declared`, which takes its struct arguments and result
    /// by pointer.
    fn define_shim(&mut self, declared: &Extern) -> Result<(), String> {
        let ret = self.c_name(&declared.return_type)?;
        let params = declared
            .params
            .iter()
            .map(|ty| self.c_name(ty))
            .collect::<Result<Vec<_>, _>>()?;
        let mut shim_params = Vec::new();
        let mut args = Vec::new();
        let has_result = !matches!(declared.return_type, Type::Unit);
        if has_result {
            shim_params.push(format!("{ret} *ret"));
        }
        for (index, (ty, c)) in declared.params.iter().zip(&params).enumerate() {
            if matches!(self.passing(ty)?, Passing::Indirect) {
                shim_params.push(format!("const {c} *a{index}"));
                args.push(format!("*a{index}"));
            } else {
                shim_params.push(format!("{c} a{index}"));
                args.push(format!("a{index}"));
            }
        }
        let name = &declared.name;
        let call = format!("{name}({})", args.join(", "));
        let body = if has_result { format!("*ret = {call};") } else { format!("{call};") };
        let shim = shim_symbol(name);
        let declared_params = if params.is_empty() { "void".to_string() } else { params.join(", ") };
        let shim_params = if shim_params.is_empty() { "void".to_string() } else { shim_params.join(", ") };
        writeln!(
            self.shims,
            "\n{ret} {name}({declared_params});\nvoid {shim}({shim_params}) {{\n    {body}\n}}"
        )
        .expect("writing to a String cannot fail");
        Ok(())
    }

    /// The C spelling of type `ty`, defining the C struct types it names.
    fn c_name(&mut self, ty: &Type) -> Result<String, String> {
        Ok(match ty {
            Type::Unit => "void".to_string(),
            Type::Pointer(_) | Type::Nil => "void *".to_string(),
            Type::Struct { .. } => {
                let name = format!("cooper_t_{}", mangle::type_name(ty));
                if !self.c_types.iter().any(|(n, _)| *n == name) {
                    let members = self.defs.struct_members(ty).ok_or_else(|| format!("passing {ty} to C"))?;
                    let mut fields = String::new();
                    for (index, (_, member)) in members.iter().enumerate() {
                        let member = self.c_name(member)?;
                        writeln!(fields, "    {member} m{index};").expect("writing to a String cannot fail");
                    }
                    self.c_types.push((name.clone(), format!("typedef struct {{\n{fields}}} {name};")));
                }
                name
            }
            _ => match Scalar::of(ty) {
                Some(Scalar::Bool) => "_Bool".to_string(),
                Some(Scalar::Int { bits, signed }) => format!("{}int{bits}_t", if signed { "" } else { "u" }),
                Some(Scalar::Float { bits: 32 }) => "float".to_string(),
                Some(Scalar::Float { .. }) => "double".to_string(),
                None => return Err(format!("passing {ty} to C")),
            },
        })
    }

    /// The C source of every wrapper, or nothing when no call needs one.
    pub(crate) fn c_source(&self) -> String {
        if self.shims.is_empty() {
            return String::new();
        }
        let mut out = String::from(
            "/* Generated by the Cooper compiler: wrappers for C functions taking or\n   \
             returning structs by value. */\n#include <stdint.h>\n\n\
             /* A declaration here may differ from a library's own in struct names only. */\n\
             #pragma clang diagnostic ignored \"-Wincompatible-library-redeclaration\"\n\n",
        );
        self.c_types.iter().for_each(|(_, def)| {
            out.push_str(def);
            out.push('\n');
        });
        out.push_str(&self.shims);
        out
    }
}

/// The symbol of the C wrapper of the C function `name`.
pub(crate) fn shim_symbol(name: &str) -> String {
    format!("cooper_shim_{name}")
}
