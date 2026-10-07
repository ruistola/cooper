//! Data layout: the LLVM type of every Cooper value type.
//!
//! Scalars map to LLVM integer and float types, pointers to the opaque `ptr`, tuples to
//! literal struct types, and each struct instantiation to a named struct type
//! (`%QN4mainE5Point`, its mangled type name) whose fields follow declaration order. A
//! function value is a `{ code, env }` pointer pair, a string a `{ data, length }` pair,
//! and an array a `{ data, length, capacity }` triple whose copies share elements. A
//! sum type instantiation is a named struct of an `i32` tag (the variant's declaration
//! index) and a payload area of 8-byte words sized for its largest variant; a variant's
//! payload is read and written there as a literal struct of its slot types. A unit field
//! occupies no space, as the empty struct `{}`; a unit *value* has no LLVM value at all.

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

/// The LLVM type of a string: its UTF-8 bytes and their count. Strings are immutable.
pub(crate) const STRING: &str = "{ ptr, i64 }";

/// The LLVM type of an array: its element storage, length, and capacity. Copies of an
/// array share its elements.
pub(crate) const ARRAY: &str = "{ ptr, i64, i64 }";

/// The LLVM type of a function value: a code pointer and an environment pointer.
pub(crate) const FUNC_VALUE: &str = "{ ptr, ptr }";

/// A description of a type code generation cannot lay out yet.
pub(crate) type Unsupported = String;

impl Module<'_> {
    /// The LLVM type of a value of type `ty`, or `None` for unit. Defines the named
    /// struct types it mentions on first use.
    pub(crate) fn llvm_type(&mut self, ty: &Type) -> Result<Option<String>, Unsupported> {
        let llvm = match ty {
            Type::Unit => return Ok(None),
            Type::Primitive(name) if name == "string" => STRING.to_string(),
            Type::Array(_) => ARRAY.to_string(),
            Type::Primitive(_) => Scalar::of(ty)
                .ok_or_else(|| format!("values of type {ty}"))?
                .llvm(),
            Type::Pointer(_) | Type::Nil => "ptr".to_string(),
            // A function value: its code, and the environment passed as the hidden first
            // argument of every call through it.
            Type::Func { .. } => FUNC_VALUE.to_string(),
            Type::Tuple(elems) => {
                let fields = elems
                    .iter()
                    .map(|e| self.field_type(e))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("{{ {} }}", fields.join(", "))
            }
            Type::Struct { .. } => self.struct_type(ty)?,
            Type::Oneof { .. } => self.oneof_type(ty)?,
            Type::Infer(_) => unreachable!("inference variables are resolved before code generation"),
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

    /// The named LLVM struct type of sum type instantiation `ty`, defined on first use:
    /// a tag and a payload area large enough for every variant.
    fn oneof_type(&mut self, ty: &Type) -> Result<String, Unsupported> {
        let name = format!("%{}", mangle::type_name(ty));
        if self.types.contains_key(&name) {
            return Ok(name);
        }
        self.types.insert(name.clone(), String::new());
        let mut words = 0;
        for variant in self.defs.variant_order(ty).to_vec() {
            let slots = self.defs.payload(ty, &variant).unwrap_or_default();
            // Defining the slot types up front reports any unsupported one here.
            self.payload_type(&slots)?;
            words = words.max(self.size_bound(&Type::Tuple(slots)).div_ceil(8));
        }
        self.types
            .insert(name.clone(), format!("{name} = type {{ i32, [{words} x i64] }}"));
        Ok(name)
    }

    /// The literal struct type a variant payload with `slots` is stored as.
    pub(crate) fn payload_type(&mut self, slots: &[Type]) -> Result<String, Unsupported> {
        let fields = slots
            .iter()
            .map(|slot| self.field_type(slot))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(format!("{{ {} }}", fields.join(", ")))
    }

    /// The tag and payload slot types of `variant` of sum type `ty`.
    pub(crate) fn variant(&self, ty: &Type, variant: &str) -> (usize, Vec<Type>) {
        let tag = self
            .defs
            .variant_order(ty)
            .iter()
            .position(|v| v == variant)
            .expect("a checked variant belongs to its sum type");
        let slots = self.defs.payload(ty, variant).expect("a checked variant has a payload");
        (tag, slots)
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
            Type::Oneof { .. } => {
                let largest = self
                    .defs
                    .variant_order(ty)
                    .iter()
                    .map(|v| {
                        let slots = self.defs.payload(ty, v).unwrap_or_default();
                        self.size_bound(&Type::Tuple(slots))
                    })
                    .max()
                    .unwrap_or(0);
                8 + largest.div_ceil(8) * 8
            }
            Type::Func { .. } => 16,
            Type::Primitive(name) if name == "string" => 16,
            Type::Array(_) => 24,
            Type::Infer(_) => unreachable!("inference variables are resolved before code generation"),
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
