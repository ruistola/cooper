use std::collections::{HashMap, HashSet};
use std::fmt;

/// The identity of a declared struct or sum type: the dotted path of its defining
/// module and its declared name. Two declarations are the same type exactly when
/// their identities match, however an import spells them locally.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeId {
    pub module: String,
    pub name: String,
}

/// A struct declaration's body: its type parameters and member types, the latter
/// written in terms of those parameters.
#[derive(Debug, Clone, Default)]
pub struct StructDef {
    pub type_params: Vec<String>,
    pub members: HashMap<String, Type>,
}

/// A sum type declaration's body: its type parameters and its variants' payload
/// types, with the variants' declaration order.
#[derive(Debug, Clone, Default)]
pub struct OneofDef {
    pub type_params: Vec<String>,
    pub variants: HashMap<String, Vec<Type>>,
    pub variant_order: Vec<String>,
}

/// Every struct and sum type definition known to the compilation, keyed by
/// identity. A [`Type`] names a declaration by [`TypeId`] only; its members and
/// variants are read here, substituted by the type's arguments.
#[derive(Debug, Clone, Default)]
pub struct TypeDefs {
    pub structs: HashMap<TypeId, StructDef>,
    pub oneofs: HashMap<TypeId, OneofDef>,
}

impl TypeDefs {
    /// The type parameters of the struct or sum type `ty` names, or none for any
    /// other type.
    pub fn type_params(&self, ty: &Type) -> &[String] {
        match ty {
            Type::Struct { id, .. } => self.structs.get(id).map_or(&[], |d| &d.type_params),
            Type::Oneof { id, .. } => self.oneofs.get(id).map_or(&[], |d| &d.type_params),
            _ => &[],
        }
    }

    /// The members of struct type `ty`, substituted by its type arguments (a
    /// template's members keep their type parameters). `None` when `ty` is no struct.
    pub fn struct_members(&self, ty: &Type) -> Option<HashMap<String, Type>> {
        let Type::Struct { id, type_args } = ty else {
            return None;
        };
        let def = self.structs.get(id)?;
        let subst = binding(&def.type_params, type_args);
        Some(
            def.members
                .iter()
                .map(|(name, t)| (name.clone(), t.substitute(&subst)))
                .collect(),
        )
    }

    /// The type of member `name` of struct type `ty`, substituted by its arguments.
    pub fn member(&self, ty: &Type, name: &str) -> Option<Type> {
        let Type::Struct { id, type_args } = ty else {
            return None;
        };
        let def = self.structs.get(id)?;
        let member = def.members.get(name)?;
        Some(member.substitute(&binding(&def.type_params, type_args)))
    }

    /// The payload of `variant` of sum type `ty`, substituted by its arguments.
    pub fn payload(&self, ty: &Type, variant: &str) -> Option<Vec<Type>> {
        let Type::Oneof { id, type_args } = ty else {
            return None;
        };
        let def = self.oneofs.get(id)?;
        let subst = binding(&def.type_params, type_args);
        let slots = def.variants.get(variant)?;
        Some(slots.iter().map(|t| t.substitute(&subst)).collect())
    }

    /// The variant names of sum type `ty` in declaration order, or none for any
    /// other type.
    pub fn variant_order(&self, ty: &Type) -> &[String] {
        match ty {
            Type::Oneof { id, .. } => self.oneofs.get(id).map_or(&[], |d| &d.variant_order),
            _ => &[],
        }
    }
}

/// Pair declared type parameters with type arguments as a substitution. A template
/// (no arguments) yields an empty substitution, leaving its parameters in place.
fn binding(params: &[String], args: &[Type]) -> HashMap<String, Type> {
    params.iter().cloned().zip(args.iter().cloned()).collect()
}

/// A resolved type in the Cooper language. A closed `enum` replaces the Go `Type`
/// interface, so every `match` over it is checked for exhaustiveness at compile
/// time — the class of "forgot a variant" bugs becomes a build error.
#[derive(Debug, Clone)]
pub enum Type {
    /// Placeholder for a binding whose type the checker infers later (walrus and
    /// tuple-destructuring bindings). It satisfies identifier-existence checks
    /// during the resolve pass and never participates in a real equality check.
    Unknown,
    Unit,
    Primitive(String),
    Array(Box<Type>),
    Pointer(Box<Type>),
    /// The type of the `nil` literal, compatible with any pointer type.
    Nil,
    Tuple(Vec<Type>),
    Func {
        return_type: Box<Type>,
        param_types: Vec<Type>,
    },
    /// A declared struct, by identity. Its members live in the [`TypeDefs`] table,
    /// so a struct may refer to itself (through a pointer or array). An empty
    /// `type_args` on a generic struct denotes its uninstantiated template.
    Struct {
        id: TypeId,
        type_args: Vec<Type>,
    },
    /// A declared sum type, by identity; its variants live in the [`TypeDefs`] table.
    Oneof {
        id: TypeId,
        type_args: Vec<Type>,
    },
    /// A reference to a bound type parameter, e.g. `T` inside `struct Box T`.
    /// Substitution replaces it with a concrete argument at instantiation.
    TypeParam(String),
    /// A partial or complete reference to a `use`-bound module, by its local
    /// spelling (`std.io` → `["std", "io"]`). It is an intermediate produced while
    /// navigating a qualified path: further `.member` access resolves it to the
    /// member's type. It is never the type of a value.
    Module(Vec<String>),
}

impl Type {
    /// Structural equality with the Cooper compatibility rules: a pointer is equal
    /// to a matching-element pointer and to the untyped `nil`; `nil` is equal to
    /// itself and to any pointer; a nominal struct or sum type is equal by identity
    /// and type arguments only. Templates are never compared directly against
    /// instantiations: substitution eliminates every `TypeParam` first.
    pub fn equals(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Unknown, Type::Unknown) => true,
            (Type::Unit, Type::Unit) => true,
            (Type::Primitive(a), Type::Primitive(b)) => a == b,
            (Type::Array(a), Type::Array(b)) => a.equals(b),
            (Type::Pointer(a), Type::Pointer(b)) => a.equals(b),
            (Type::Pointer(_), Type::Nil) => true,
            (Type::Nil, Type::Nil) => true,
            (Type::Nil, Type::Pointer(_)) => true,
            (Type::Tuple(a), Type::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y))
            }
            (
                Type::Func {
                    return_type: ra,
                    param_types: pa,
                },
                Type::Func {
                    return_type: rb,
                    param_types: pb,
                },
            ) => ra.equals(rb) && pa.len() == pb.len() && pa.iter().zip(pb).all(|(x, y)| x.equals(y)),
            (Type::Struct { id: ia, type_args: aa }, Type::Struct { id: ib, type_args: ab })
            | (Type::Oneof { id: ia, type_args: aa }, Type::Oneof { id: ib, type_args: ab }) => {
                ia == ib && aa.len() == ab.len() && aa.iter().zip(ab).all(|(x, y)| x.equals(y))
            }
            (Type::TypeParam(a), Type::TypeParam(b)) => a == b,
            _ => false,
        }
    }

    /// Replace every `TypeParam` in `self` according to `subst` (parameter name to
    /// concrete argument), returning the resulting type. Types with no type
    /// parameters are returned structurally unchanged.
    pub fn substitute(&self, subst: &HashMap<String, Type>) -> Type {
        match self {
            Type::TypeParam(name) => subst.get(name).cloned().unwrap_or_else(|| self.clone()),
            Type::Array(elem) => Type::Array(Box::new(elem.substitute(subst))),
            Type::Pointer(elem) => Type::Pointer(Box::new(elem.substitute(subst))),
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| e.substitute(subst)).collect())
            }
            Type::Func {
                return_type,
                param_types,
            } => Type::Func {
                return_type: Box::new(return_type.substitute(subst)),
                param_types: param_types.iter().map(|p| p.substitute(subst)).collect(),
            },
            Type::Struct { id, type_args } => Type::Struct {
                id: id.clone(),
                type_args: type_args.iter().map(|a| a.substitute(subst)).collect(),
            },
            Type::Oneof { id, type_args } => Type::Oneof {
                id: id.clone(),
                type_args: type_args.iter().map(|a| a.substitute(subst)).collect(),
            },
            _ => self.clone(),
        }
    }
}

/// Match a template type (which may contain `TypeParam` placeholders) against a
/// concrete type, recording each parameter's inferred binding in `subst`. Returns
/// false on a structural mismatch or a parameter bound inconsistently to two
/// different types. Concrete-vs-concrete positions are compared with `equals`.
pub fn unify(template: &Type, concrete: &Type, subst: &mut HashMap<String, Type>) -> bool {
    match template {
        Type::TypeParam(name) => {
            if let Some(bound) = subst.get(name) {
                return bound.equals(concrete);
            }
            subst.insert(name.clone(), concrete.clone());
            true
        }
        Type::Array(te) => match concrete {
            Type::Array(ce) => unify(te, ce, subst),
            _ => false,
        },
        Type::Pointer(te) => match concrete {
            Type::Pointer(ce) => unify(te, ce, subst),
            // `nil` fits any pointer slot but determines none of its parameters.
            Type::Nil => true,
            _ => false,
        },
        Type::Tuple(telems) => match concrete {
            Type::Tuple(celems) if telems.len() == celems.len() => telems
                .iter()
                .zip(celems)
                .all(|(t, c)| unify(t, c, subst)),
            _ => false,
        },
        Type::Func {
            return_type: tr,
            param_types: tp,
        } => match concrete {
            Type::Func {
                return_type: cr,
                param_types: cp,
            } if tp.len() == cp.len() => {
                tp.iter().zip(cp).all(|(t, c)| unify(t, c, subst)) && unify(tr, cr, subst)
            }
            _ => false,
        },
        Type::Struct { id: ti, type_args: ta } | Type::Oneof { id: ti, type_args: ta } => {
            match concrete {
                Type::Struct { id: ci, type_args: ca } | Type::Oneof { id: ci, type_args: ca }
                    if std::mem::discriminant(template) == std::mem::discriminant(concrete)
                        && ti == ci
                        && ta.len() == ca.len() =>
                {
                    ta.iter().zip(ca).all(|(t, c)| unify(t, c, subst))
                }
                _ => false,
            }
        }
        _ => template.equals(concrete),
    }
}

/// The signed integer types, narrowest to widest.
pub const SIGNED_INTS: [&str; 4] = ["i8", "i16", "i32", "i64"];
/// The unsigned integer types, narrowest to widest.
pub const UNSIGNED_INTS: [&str; 4] = ["u8", "u16", "u32", "u64"];
/// The floating-point types, narrowest to widest.
pub const FLOATS: [&str; 2] = ["f32", "f64"];
/// The non-numeric primitives.
pub const OTHER_PRIMITIVES: [&str; 2] = ["bool", "string"];

/// The type a bare integer literal takes with no contextual type to guide it.
pub const DEFAULT_INT: &str = "i32";
/// The type a bare floating-point literal takes with no contextual type to guide it.
pub const DEFAULT_FLOAT: &str = "f32";

pub fn is_integer_name(name: &str) -> bool {
    SIGNED_INTS.contains(&name) || UNSIGNED_INTS.contains(&name)
}

pub fn is_float_name(name: &str) -> bool {
    FLOATS.contains(&name)
}

pub fn is_numeric_name(name: &str) -> bool {
    is_integer_name(name) || is_float_name(name)
}

/// Whether `name` denotes a built-in primitive type recognised without declaration.
pub fn is_primitive_name(name: &str) -> bool {
    is_numeric_name(name) || OTHER_PRIMITIVES.contains(&name)
}

pub fn is_unit(t: &Type) -> bool {
    matches!(t, Type::Unit)
}

pub fn is_primitive(t: &Type, name: &str) -> bool {
    matches!(t, Type::Primitive(n) if n == name)
}

pub fn is_numeric(t: &Type) -> bool {
    matches!(t, Type::Primitive(n) if is_numeric_name(n))
}

pub fn is_integer(t: &Type) -> bool {
    matches!(t, Type::Primitive(n) if is_integer_name(n))
}

pub fn is_float(t: &Type) -> bool {
    matches!(t, Type::Primitive(n) if is_float_name(n))
}

/// The first component of `t` that makes it non-comparable, or `None` when `==` is
/// defined on `t`. Equality is structural, shallow, and field-wise: primitives by
/// value (`string` by content, floats by IEEE), pointers by address, and tuples,
/// structs, and sum types (tag, then payload) when every component is comparable.
/// Functions are excluded (closure identity has no meaning), as are dynamic arrays
/// (a view's identity and its contents are both plausible readings) and unconstrained
/// type parameters. The same set defines which types are hashable.
pub(crate) fn incomparable_part(t: &Type, defs: &TypeDefs) -> Option<Type> {
    match t {
        Type::Unknown | Type::Unit | Type::Primitive(_) | Type::Pointer(_) | Type::Nil => None,
        Type::Array(_) | Type::Func { .. } | Type::TypeParam(_) | Type::Module(_) => {
            Some(t.clone())
        }
        Type::Tuple(elems) => elems.iter().find_map(|e| incomparable_part(e, defs)),
        Type::Struct { .. } => {
            let members = defs.struct_members(t)?;
            let mut names: Vec<&String> = members.keys().collect();
            names.sort();
            names.into_iter().find_map(|n| incomparable_part(&members[n], defs))
        }
        Type::Oneof { .. } => defs
            .variant_order(t)
            .iter()
            .flat_map(|v| defs.payload(t, v).unwrap_or_default())
            .find_map(|slot| incomparable_part(&slot, defs)),
    }
}

/// Render `a` and `b` for a diagnostic that sets them side by side. Distinct
/// declarations sharing a name are qualified by their module path (`a.Node` versus
/// `b.Node`) so the two never read alike; everything else renders as [`Display`].
///
/// [`Display`]: fmt::Display
pub fn display_pair(a: &Type, b: &Type) -> (String, String) {
    let mut ids = Vec::new();
    a.collect_ids(&mut ids);
    b.collect_ids(&mut ids);
    let mut by_name: HashMap<&str, &TypeId> = HashMap::new();
    let mut qualify = HashSet::new();
    for id in ids {
        if let Some(seen) = by_name.insert(&id.name, id) {
            if seen != id {
                qualify.insert(id.name.clone());
            }
        }
    }
    let render = |ty: &Type| Rendered { ty, qualify: &qualify }.to_string();
    (render(a), render(b))
}

impl Type {
    /// Every declaration identity `self` mentions, including within type arguments.
    fn collect_ids<'a>(&'a self, out: &mut Vec<&'a TypeId>) {
        match self {
            Type::Array(elem) | Type::Pointer(elem) => elem.collect_ids(out),
            Type::Tuple(elems) => elems.iter().for_each(|e| e.collect_ids(out)),
            Type::Func {
                return_type,
                param_types,
            } => {
                param_types.iter().for_each(|p| p.collect_ids(out));
                return_type.collect_ids(out);
            }
            Type::Struct { id, type_args } | Type::Oneof { id, type_args } => {
                out.push(id);
                type_args.iter().for_each(|a| a.collect_ids(out));
            }
            Type::Unknown
            | Type::Unit
            | Type::Primitive(_)
            | Type::Nil
            | Type::TypeParam(_)
            | Type::Module(_) => {}
        }
    }
}

/// A type rendered with the declarations named in `qualify` spelled with their
/// module path.
struct Rendered<'a> {
    ty: &'a Type,
    qualify: &'a HashSet<String>,
}

impl fmt::Display for Rendered<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = |ty| Rendered { ty, qualify: self.qualify };
        match self.ty {
            Type::Unknown => write!(f, "<unknown>"),
            Type::Unit => write!(f, "()"),
            Type::Primitive(name) => write!(f, "{}", name),
            Type::Array(elem) => write!(f, "{}[]", r(elem)),
            Type::Pointer(elem) => write!(f, "{}^", r(elem)),
            Type::Nil => write!(f, "nil"),
            Type::Tuple(elems) => {
                let parts: Vec<String> = elems.iter().map(|e| r(e).to_string()).collect();
                write!(f, "({})", parts.join(", "))
            }
            Type::Func {
                return_type,
                param_types,
            } => {
                let params: Vec<String> = param_types.iter().map(|p| r(p).to_string()).collect();
                write!(f, "func({}):{}", params.join(","), r(return_type))
            }
            Type::Struct { id, type_args } | Type::Oneof { id, type_args } => {
                let name = if self.qualify.contains(&id.name) {
                    format!("{}.{}", id.module, id.name)
                } else {
                    id.name.clone()
                };
                if type_args.is_empty() {
                    write!(f, "{}", name)
                } else {
                    let args: Vec<String> = type_args.iter().map(|a| r(a).to_string()).collect();
                    write!(f, "{} {}", name, args.join(" "))
                }
            }
            Type::TypeParam(name) => write!(f, "{}", name),
            Type::Module(path) => write!(f, "module {}", path.join(".")),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Rendered {
            ty: self,
            qualify: &HashSet::new(),
        }
        .fmt(f)
    }
}
