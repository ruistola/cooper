//! Per-body type variables and structural unification.

use std::fmt;

use crate::types::{is_float, is_numeric, InferId, Type, DEFAULT_FLOAT, DEFAULT_INT};

/// The types an unbound variable may take. Literal classes intersect when merged:
/// an integer literal can take a float type, but a float literal cannot take an integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum InferKind {
    General,
    IntLiteral,
    FloatLiteral,
}

impl InferKind {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::General, kind) | (kind, Self::General) => kind,
            (Self::FloatLiteral, _) | (_, Self::FloatLiteral) => Self::FloatLiteral,
            (Self::IntLiteral, Self::IntLiteral) => Self::IntLiteral,
        }
    }

    fn accepts(self, ty: &Type) -> bool {
        match self {
            Self::General => true,
            Self::IntLiteral => is_numeric(ty),
            Self::FloatLiteral => is_float(ty),
        }
    }
}

impl InferId {
    // The low two bits carry a display hint; the remaining bits index the table.
    // Resolution refreshes the hint from the representative's current class.
    fn new(index: usize, kind: InferKind) -> Self {
        let index = u32::try_from(index).expect("too many inference variables");
        assert!(index <= u32::MAX >> 2, "too many inference variables");
        Self((index << 2) | kind as u32)
    }

    fn index(self) -> usize {
        (self.0 >> 2) as usize
    }

    fn kind(self) -> InferKind {
        match self.0 & 3 {
            0 => InferKind::General,
            1 => InferKind::IntLiteral,
            2 => InferKind::FloatLiteral,
            _ => unreachable!("an inference variable has a valid display class"),
        }
    }
}

impl fmt::Display for InferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind() {
            InferKind::General => "_",
            InferKind::IntLiteral => "{integer}",
            InferKind::FloatLiteral => "{float}",
        })
    }
}

#[derive(Debug, Clone)]
struct Entry {
    parent: usize,
    rank: u8,
    kind: InferKind,
    binding: Option<Type>,
}

/// The resolved operands of a failed equality. The caller supplies the diagnostic
/// wording and source span; failure can leave earlier components unified.
#[derive(Debug)]
pub(crate) struct Conflict {
    pub(crate) left: Box<Type>,
    pub(crate) right: Box<Type>,
}

/// A union-find table with structural bindings on representatives. Variables belong
/// to this table only; declared type parameters are rigid, not inference variables.
#[derive(Debug, Clone, Default)]
pub(crate) struct InferTable {
    entries: Vec<Entry>,
}

impl InferTable {
    pub(crate) fn fresh(&mut self, kind: InferKind) -> Type {
        let index = self.entries.len();
        let id = InferId::new(index, kind);
        self.entries.push(Entry {
            parent: index,
            rank: 0,
            kind,
            binding: None,
        });
        Type::Infer(id)
    }

    /// Equate two types, following variable bindings and checking recursive types.
    pub(crate) fn unify(&mut self, left: &Type, right: &Type) -> Result<(), Conflict> {
        self.unify_inner(left, right).map_err(|()| Conflict {
            left: Box::new(self.resolve(left)),
            right: Box::new(self.resolve(right)),
        })
    }

    /// Resolve variables throughout a type, retaining canonical unbound variables.
    pub(crate) fn resolve(&self, ty: &Type) -> Type {
        match self.shallow(ty) {
            Type::Array(elem) => Type::Array(Box::new(self.resolve(&elem))),
            Type::Pointer(elem) => Type::Pointer(Box::new(self.resolve(&elem))),
            Type::Tuple(elems) => Type::Tuple(self.resolve_all(&elems)),
            Type::Func { return_type, param_types } => Type::Func {
                return_type: Box::new(self.resolve(&return_type)),
                param_types: self.resolve_all(&param_types),
            },
            Type::Struct { id, type_args } => Type::Struct {
                id,
                type_args: self.resolve_all(&type_args),
            },
            Type::Oneof { id, type_args } => Type::Oneof {
                id,
                type_args: self.resolve_all(&type_args),
            },
            leaf => leaf,
        }
    }

    fn resolve_all(&self, types: &[Type]) -> Vec<Type> {
        types.iter().map(|ty| self.resolve(ty)).collect()
    }

    pub(crate) fn default_literals(&mut self) {
        for index in 0..self.entries.len() {
            let id = InferId::new(index, self.entries[index].kind);
            self.default_literal(&Type::Infer(id));
        }
    }

    /// Default the representative of `ty`, leaving unrelated variables untouched.
    /// Aliases and already-bound variables require no action.
    pub(crate) fn default_literal(&mut self, ty: &Type) {
        let Type::Infer(id) = ty else { return };
        let index = self.root_mut(id.index());
        let entry = &mut self.entries[index];
        if entry.binding.is_some() {
            return;
        }
        let name = match entry.kind {
            InferKind::General => return,
            InferKind::IntLiteral => DEFAULT_INT,
            InferKind::FloatLiteral => DEFAULT_FLOAT,
        };
        entry.binding = Some(Type::Primitive(name.to_string()));
    }

    /// Collect canonical unbound variables throughout a type.
    pub(crate) fn unbound_ids(&self, ty: &Type, out: &mut Vec<InferId>) {
        match self.shallow(ty) {
            Type::Infer(id) => out.push(id),
            Type::Array(elem) | Type::Pointer(elem) => self.unbound_ids(&elem, out),
            Type::Tuple(elems) => {
                for elem in elems {
                    self.unbound_ids(&elem, out);
                }
            }
            Type::Func { return_type, param_types } => {
                for param in param_types {
                    self.unbound_ids(&param, out);
                }
                self.unbound_ids(&return_type, out);
            }
            Type::Struct { type_args, .. } | Type::Oneof { type_args, .. } => {
                for arg in type_args {
                    self.unbound_ids(&arg, out);
                }
            }
            _ => {}
        }
    }

    /// Whether the outer type is known, even if its components contain variables.
    pub(crate) fn is_bound(&self, ty: &Type) -> bool {
        !matches!(self.shallow(ty), Type::Infer(_))
    }

    /// Whether every inference variable in a type has a binding. Declared type
    /// parameters are resolved types, even though they are not concrete.
    pub(crate) fn is_resolved(&self, ty: &Type) -> bool {
        match ty {
            Type::Infer(_) => self.is_bound(ty) && self.is_resolved(&self.shallow(ty)),
            Type::Array(elem) | Type::Pointer(elem) => self.is_resolved(elem),
            Type::Tuple(elems) => elems.iter().all(|ty| self.is_resolved(ty)),
            Type::Func { return_type, param_types } => {
                self.is_resolved(return_type)
                    && param_types.iter().all(|ty| self.is_resolved(ty))
            }
            Type::Struct { type_args, .. } | Type::Oneof { type_args, .. } => {
                type_args.iter().all(|ty| self.is_resolved(ty))
            }
            _ => true,
        }
    }

    fn root(&self, mut index: usize) -> usize {
        while self.entries[index].parent != index {
            index = self.entries[index].parent;
        }
        index
    }

    fn root_mut(&mut self, index: usize) -> usize {
        let parent = self.entries[index].parent;
        if parent == index {
            return index;
        }
        let root = self.root_mut(parent);
        self.entries[index].parent = root;
        root
    }

    fn unify_inner(&mut self, left: &Type, right: &Type) -> Result<(), ()> {
        let left = self.shallow(left);
        let right = self.shallow(right);
        match (&left, &right) {
            (Type::Infer(a), Type::Infer(b)) => {
                self.merge(*a, *b);
                Ok(())
            }
            (Type::Infer(id), Type::Nil) | (Type::Nil, Type::Infer(id)) => {
                if self.entries[id.index()].kind != InferKind::General {
                    return Err(());
                }
                // Nil determines the pointer shape, but not the pointee type.
                let elem = self.fresh(InferKind::General);
                self.bind(*id, &Type::Pointer(Box::new(elem)))
            }
            (Type::Infer(id), ty) | (ty, Type::Infer(id)) => self.bind(*id, ty),
            (Type::Array(a), Type::Array(b)) | (Type::Pointer(a), Type::Pointer(b)) => {
                self.unify_inner(a, b)
            }
            (Type::Pointer(_), Type::Nil) | (Type::Nil, Type::Pointer(_)) => Ok(()),
            (Type::Tuple(a), Type::Tuple(b)) => self.unify_all(a, b),
            (
                Type::Func { return_type: a, param_types: pa },
                Type::Func { return_type: b, param_types: pb },
            ) => {
                self.unify_all(pa, pb)?;
                self.unify_inner(a, b)
            }
            (Type::Struct { id: a, type_args: aa }, Type::Struct { id: b, type_args: ab })
            | (Type::Oneof { id: a, type_args: aa }, Type::Oneof { id: b, type_args: ab })
                if a == b => self.unify_all(aa, ab),
            _ if left.equals(&right) => Ok(()),
            _ => Err(()),
        }
    }

    fn unify_all(&mut self, left: &[Type], right: &[Type]) -> Result<(), ()> {
        if left.len() != right.len() {
            return Err(());
        }
        for (a, b) in left.iter().zip(right) {
            self.unify_inner(a, b)?;
        }
        Ok(())
    }

    fn merge(&mut self, left: InferId, right: InferId) {
        let mut a = self.root_mut(left.index());
        let mut b = self.root_mut(right.index());
        if a == b {
            return;
        }
        if self.entries[a].rank < self.entries[b].rank {
            std::mem::swap(&mut a, &mut b);
        }
        let kind = self.entries[a].kind.merge(self.entries[b].kind);
        self.entries[b].parent = a;
        self.entries[a].kind = kind;
        if self.entries[a].rank == self.entries[b].rank {
            self.entries[a].rank += 1;
        }
    }

    fn bind(&mut self, id: InferId, ty: &Type) -> Result<(), ()> {
        let index = self.root_mut(id.index());
        if !self.entries[index].kind.accepts(ty) || self.occurs(index, ty) {
            return Err(());
        }
        let binding = self.pointer_nils(ty);
        self.entries[index].binding = Some(binding);
        Ok(())
    }

    /// Nil in a variable's binding denotes a pointer with an unknown pointee, also
    /// inside composites. It cannot become a concrete generic type argument.
    fn pointer_nils(&mut self, ty: &Type) -> Type {
        match ty {
            Type::Nil => Type::Pointer(Box::new(self.fresh(InferKind::General))),
            Type::Array(elem) => Type::Array(Box::new(self.pointer_nils(elem))),
            Type::Pointer(elem) => Type::Pointer(Box::new(self.pointer_nils(elem))),
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|ty| self.pointer_nils(ty)).collect()),
            Type::Func { return_type, param_types } => Type::Func {
                return_type: Box::new(self.pointer_nils(return_type)),
                param_types: param_types.iter().map(|ty| self.pointer_nils(ty)).collect(),
            },
            Type::Struct { id, type_args } => Type::Struct {
                id: id.clone(),
                type_args: type_args.iter().map(|ty| self.pointer_nils(ty)).collect(),
            },
            Type::Oneof { id, type_args } => Type::Oneof {
                id: id.clone(),
                type_args: type_args.iter().map(|ty| self.pointer_nils(ty)).collect(),
            },
            _ => ty.clone(),
        }
    }

    fn occurs(&self, index: usize, ty: &Type) -> bool {
        match self.shallow(ty) {
            Type::Infer(id) => self.root(id.index()) == index,
            Type::Array(elem) | Type::Pointer(elem) => self.occurs(index, &elem),
            Type::Tuple(elems) => elems.iter().any(|ty| self.occurs(index, ty)),
            Type::Func { return_type, param_types } => {
                self.occurs(index, &return_type)
                    || param_types.iter().any(|ty| self.occurs(index, ty))
            }
            Type::Struct { type_args, .. } | Type::Oneof { type_args, .. } => {
                type_args.iter().any(|ty| self.occurs(index, ty))
            }
            _ => false,
        }
    }

    fn shallow(&self, ty: &Type) -> Type {
        let Type::Infer(id) = ty else {
            return ty.clone();
        };
        let index = self.root(id.index());
        let entry = &self.entries[index];
        match &entry.binding {
            Some(binding) => self.shallow(binding),
            None => Type::Infer(InferId::new(index, entry.kind)),
        }
    }
}

#[cfg(test)]
mod tests;
