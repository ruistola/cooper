//! Data layout: the LLVM type of every Cooper value type.
//!
//! Scalars map to LLVM integer and float types, pointers to the opaque `ptr`, tuples to
//! literal struct types, and each struct instantiation to a named struct type
//! (`%QN4mainE5Point`, its mangled type name) whose fields follow declaration order. A
//! unit field occupies no space, as the empty struct `{}`; a unit *value* has no LLVM
//! value at all.

use cooper_frontend::types::{integer_bits, is_float_name, is_integer_name, Type};

use crate::{mangle, Module};

/// How a scalar type computes: it selects the LLVM type and instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scalar {
    Bool,
    Int { bits: u32, signed: bool },
    Float { bits: u32 },
}

impl Scalar {
    pub(crate) fn of(ty: &Type) -> Option<Scalar> {
        let Type::Primitive(name) = ty else {
            return None;
        };
        if name == "bool" {
            Some(Scalar::Bool)
        } else if is_integer_name(name) {
            Some(Scalar::Int {
                bits: integer_bits(name),
                signed: name.starts_with('i'),
            })
        } else if is_float_name(name) {
            Some(Scalar::Float {
                bits: if name == "f32" { 32 } else { 64 },
            })
        } else {
            None
        }
    }

    pub(crate) fn llvm(self) -> String {
        match self {
            Scalar::Bool => "i1".to_string(),
            Scalar::Int { bits, .. } => format!("i{bits}"),
            Scalar::Float { bits: 32 } => "float".to_string(),
            Scalar::Float { .. } => "double".to_string(),
        }
    }
}

/// A description of a type code generation cannot lay out yet.
pub(crate) type Unsupported = String;

impl Module<'_> {
    /// The LLVM type of a value of type `ty`, or `None` for unit. Defines the named
    /// struct types it mentions on first use.
    pub(crate) fn llvm_type(&mut self, ty: &Type) -> Result<Option<String>, Unsupported> {
        let llvm = match ty {
            Type::Unit => return Ok(None),
            Type::Primitive(_) => Scalar::of(ty)
                .ok_or_else(|| format!("values of type {ty}"))?
                .llvm(),
            Type::Pointer(_) | Type::Nil => "ptr".to_string(),
            Type::Tuple(elems) => {
                let fields = elems
                    .iter()
                    .map(|e| self.field_type(e))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("{{ {} }}", fields.join(", "))
            }
            Type::Struct { .. } => self.struct_type(ty)?,
            _ => return Err(format!("values of type {ty}")),
        };
        Ok(Some(llvm))
    }

    /// The LLVM type of an aggregate field of type `ty`: unit is the empty struct.
    pub(crate) fn field_type(&mut self, ty: &Type) -> Result<String, Unsupported> {
        Ok(self.llvm_type(ty)?.unwrap_or_else(|| "{}".to_string()))
    }

    /// The named LLVM struct type of struct instantiation `ty`, defined on first use.
    fn struct_type(&mut self, ty: &Type) -> Result<String, Unsupported> {
        let name = format!("%{}", mangle::type_name(ty));
        if self.types.contains_key(&name) {
            return Ok(name);
        }
        let members = self
            .defs
            .struct_members(ty)
            .ok_or_else(|| format!("values of type {ty}"))?;
        // Reserve the name first: a member may refer back to this struct through a
        // pointer, which needs no definition, but the lookup must terminate.
        self.types.insert(name.clone(), String::new());
        let fields = members
            .iter()
            .map(|(_, member)| self.field_type(member))
            .collect::<Result<Vec<_>, _>>()?;
        self.types
            .insert(name.clone(), format!("{name} = type {{ {} }}", fields.join(", ")));
        Ok(name)
    }

    /// An upper bound on the size in bytes of a value of type `ty`, counting every
    /// field as at least eight bytes so that alignment padding is always covered.
    pub(crate) fn size_bound(&self, ty: &Type) -> usize {
        match ty {
            Type::Unit => 0,
            Type::Tuple(elems) => elems.iter().map(|e| self.size_bound(e).max(8)).sum(),
            Type::Struct { .. } => self
                .defs
                .struct_members(ty)
                .unwrap_or_default()
                .iter()
                .map(|(_, member)| self.size_bound(member).max(8))
                .sum(),
            _ => 8,
        }
    }

    /// The position of field `name` in struct type `ty`'s layout, with its type.
    pub(crate) fn field(&self, ty: &Type, name: &str) -> (usize, Type) {
        self.defs
            .struct_members(ty)
            .expect("a field access is on a defined struct")
            .into_iter()
            .enumerate()
            .find(|(_, (member, _))| member == name)
            .map(|(index, (_, member_ty))| (index, member_ty))
            .expect("a checked field access names a member")
    }
}
