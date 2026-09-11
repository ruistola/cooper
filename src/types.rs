use std::collections::HashMap;
use std::fmt;

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
    Struct {
        name: String,
        members: HashMap<String, Type>,
        type_params: Vec<String>,
        type_args: Vec<Type>,
    },
    Oneof {
        name: String,
        variants: HashMap<String, Vec<Type>>,
        variant_order: Vec<String>,
        type_params: Vec<String>,
        type_args: Vec<Type>,
    },
    /// A reference to a bound type parameter, e.g. `T` inside `struct Box T`.
    /// Substitution replaces it with a concrete argument at instantiation.
    TypeParam(String),
}

impl Type {
    /// Structural equality with the Cooper compatibility rules: a pointer is equal
    /// to a matching-element pointer and to the untyped `nil`; `nil` is equal to
    /// itself and to any pointer; a nominal struct or sum type is equal by name and
    /// type arguments only. Templates are never compared directly against
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
            (
                Type::Struct {
                    name: na,
                    type_args: aa,
                    ..
                },
                Type::Struct {
                    name: nb,
                    type_args: ab,
                    ..
                },
            ) => na == nb && aa.len() == ab.len() && aa.iter().zip(ab).all(|(x, y)| x.equals(y)),
            (
                Type::Oneof {
                    name: na,
                    type_args: aa,
                    ..
                },
                Type::Oneof {
                    name: nb,
                    type_args: ab,
                    ..
                },
            ) => na == nb && aa.len() == ab.len() && aa.iter().zip(ab).all(|(x, y)| x.equals(y)),
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
            Type::Struct {
                name,
                members,
                type_params,
                type_args,
            } => {
                if type_args.is_empty() {
                    return self.clone();
                }
                Type::Struct {
                    name: name.clone(),
                    members: members
                        .iter()
                        .map(|(k, v)| (k.clone(), v.substitute(subst)))
                        .collect(),
                    type_params: type_params.clone(),
                    type_args: type_args.iter().map(|a| a.substitute(subst)).collect(),
                }
            }
            Type::Oneof {
                name,
                variants,
                variant_order,
                type_params,
                type_args,
            } => {
                if type_args.is_empty() {
                    return self.clone();
                }
                Type::Oneof {
                    name: name.clone(),
                    variants: variants
                        .iter()
                        .map(|(k, slots)| {
                            (k.clone(), slots.iter().map(|s| s.substitute(subst)).collect())
                        })
                        .collect(),
                    variant_order: variant_order.clone(),
                    type_params: type_params.clone(),
                    type_args: type_args.iter().map(|a| a.substitute(subst)).collect(),
                }
            }
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
        _ => template.equals(concrete),
    }
}

pub fn is_unit(t: &Type) -> bool {
    matches!(t, Type::Unit)
}

pub fn is_primitive(t: &Type, name: &str) -> bool {
    matches!(t, Type::Primitive(n) if n == name)
}

pub fn is_numeric(t: &Type) -> bool {
    matches!(t, Type::Primitive(n) if matches!(n.as_str(), "i8" | "i32" | "i64" | "f32" | "f64"))
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Unknown => write!(f, "<unknown>"),
            Type::Unit => write!(f, "()"),
            Type::Primitive(name) => write!(f, "{}", name),
            Type::Array(elem) => write!(f, "{}[]", elem),
            Type::Pointer(elem) => write!(f, "{}^", elem),
            Type::Nil => write!(f, "nil"),
            Type::Tuple(elems) => {
                let parts: Vec<String> = elems.iter().map(|e| e.to_string()).collect();
                write!(f, "({})", parts.join(", "))
            }
            Type::Func {
                return_type,
                param_types,
            } => {
                let params: Vec<String> = param_types.iter().map(|p| p.to_string()).collect();
                write!(f, "func({}):{}", params.join(","), return_type)
            }
            Type::Struct {
                name, type_args, ..
            }
            | Type::Oneof {
                name, type_args, ..
            } => {
                if type_args.is_empty() {
                    write!(f, "{}", name)
                } else {
                    let args: Vec<String> = type_args.iter().map(|a| a.to_string()).collect();
                    write!(f, "{} {}", name, args.join(" "))
                }
            }
            Type::TypeParam(name) => write!(f, "{}", name),
        }
    }
}
