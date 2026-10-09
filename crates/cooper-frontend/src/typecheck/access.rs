//! Struct construction, member and element access, and assignment.

use super::*;

impl<'g> TypeChecker<'g> {
    pub(super) fn check_struct_literal(&mut self, target: &Expr, members: &[MemberInit]) -> Option<Type> {
        let target_type = self.check_expr(target)?;
        let Type::Struct { id, type_args } = &target_type else {
            self.err(
                target.span,
                format!("expression of type {target_type} cannot be used as a struct"),
            );
            return None;
        };
        let id = id.clone();
        let name = id.name.clone();
        // A generic struct's members constrain fresh variables for its type arguments.
        let type_params = self.globals.defs.type_params(&target_type).to_vec();
        let is_generic = !type_params.is_empty() && type_args.is_empty();
        let subst = if is_generic { self.fresh_subst(&type_params) } else { HashMap::new() };
        let struct_members: Vec<_> = self.globals.defs.struct_members(&target_type)
            .unwrap_or_default().into_iter()
            .map(|(name, ty)| (name, ty.substitute(&subst)))
            .collect();
        let mut assigned: HashSet<String> = HashSet::new();
        for member in members {
            let Some((_, member_type)) = struct_members.iter().find(|(name, _)| name == &member.name)
            else {
                self.err(
                    member.span,
                    format!("{} is not a member of struct {name}", member.name),
                );
                continue;
            };
            if !assigned.insert(member.name.clone()) {
                self.err(
                    member.span,
                    format!("struct member {} assigned multiple times", member.name),
                );
                continue;
            }
            let Some(value_type) = self.check_expr_expecting(&member.value, Some(member_type.clone()))
            else {
                continue;
            };
            if self.infer.unify(member_type, &value_type).is_err() {
                let (value_type_shown, member_type_shown) = self.shown_pair(&value_type, member_type);
                let message = if is_generic {
                    format!("cannot assign {value_type_shown} to member {} of generic struct {name}", member.name)
                } else {
                    format!("cannot assign {value_type_shown} to {member_type_shown} of struct member {}", member.name)
                };
                self.err(member.value.span, message);
            }
        }
        for (member_name, _) in &struct_members {
            if !assigned.contains(member_name) {
                self.err(
                    target.span,
                    format!("struct member {member_name} is not assigned a value"),
                );
            }
        }
        if is_generic {
            return self.instantiate_struct(&id, &type_params, &subst, target.span);
        }
        Some(target_type)
    }

    fn instantiate_struct(
        &mut self,
        id: &TypeId,
        type_params: &[String],
        subst: &HashMap<String, Type>,
        span: Span,
    ) -> Option<Type> {
        let mut type_args = Vec::with_capacity(type_params.len());
        for param in type_params {
            let Some(arg) = subst.get(param) else {
                self.err(
                    span,
                    format!("cannot infer type argument {param} for generic struct {}", id.name),
                );
                return None;
            };
            type_args.push(arg.clone());
        }
        Some(Type::Struct {
            id: id.clone(),
            type_args,
        })
    }

    pub(super) fn check_field(&mut self, target: &Expr, field: &str, span: Span) -> Option<Type> {
        let target_type = self.check_expr(target)?;
        let effective = match &target_type {
            Type::Pointer(elem) => elem.as_ref(),
            _ => &target_type,
        };
        if !self.infer.is_bound(effective) {
            let result = self.infer.fresh(InferKind::General);
            self.body.obligations.push(Obligation {
                subject: target_type,
                span,
                kind: ObligationKind::Field { name: field.to_string(), result: result.clone(), target_span: target.span },
            });
            return Some(result);
        }
        self.resolve_field(target_type, field, span, target.span)
    }

    pub(super) fn resolve_field(
        &mut self, mut target_type: Type, field: &str, span: Span,
        target_span: Span,
    ) -> Option<Type> {
        // A member access whose base is a module reference selects an exported item
        // (or a deeper module), distinct from struct-field and variant access.
        if let Type::Module(prefix) = &target_type {
            let prefix = prefix.clone();
            return self.check_module_access(&prefix, field, span);
        }
        // A member access whose base is a sum type names a variant constructor.
        if matches!(target_type, Type::Oneof { .. }) {
            return self.check_variant_access(&target_type, field, span);
        }
        // Auto-deref: `p.field` works transparently through a pointer-to-struct.
        if let Type::Pointer(elem) = target_type {
            target_type = *elem;
        }
        // An array exposes a compiler-blessed method set (`xs.length()`, the `get`/
        // `set` that index syntax binds to) rather than data fields; these resolve by
        // name against the builtin registry.
        if let Type::Array(elem) = &target_type {
            if let Some(method) = crate::builtins::array_method(field, elem) {
                return Some(method);
            }
            self.err(span, format!("{field} is not a method of array type {target_type}"));
            return None;
        }
        let Type::Struct { id, .. } = &target_type else {
            self.err(
                target_span,
                format!("expression of type {target_type} cannot be used as a struct"),
            );
            return None;
        };
        let name = &id.name;
        if let Some(member_type) = self.globals.defs.member(&target_type, field) {
            return Some(member_type);
        }
        // Not a data field: fall back to a method on this struct type. A bound
        // method has its receiver stripped, so it is usable wherever a matching
        // function type is expected.
        if let Some(sig) = self.globals.lookup_method(id, field) {
            let sig = sig.clone();
            let label = format!("method {field}");
            return self.reference(span, &sig, &label, Some(&target_type));
        }
        self.err(span, format!("{field} is not a member of struct {name}"));
        None
    }

    /// Index syntax is a built-in-array privilege: `a[i]` reads and `a[i] = v` writes an
    /// array element at an integer index. User-defined collections are not indexable and
    /// expose ordinary methods (`at`, `set`, …) instead; array method-call forms
    /// (`xs.length()`, `xs.push(v)`) go through the builtins registry via `check_field`.
    pub(super) fn check_index(&mut self, array: &Expr, index: &Expr) -> Option<Type> {
        let array_type = self.check_expr(array)?;
        // Index syntax is a built-in-array privilege: `a[i]` reads like `a.get(i)` but
        // takes an index of any integer type. User-defined collections are not
        // indexable — they expose ordinary methods (`at`, `set`, …), visibly userspace.
        let Some(element) = self.array_element(&array_type) else {
            let shown = self.shown_type(&array_type);
            self.err(array.span, format!("type {shown} cannot be indexed"));
            return None;
        };
        self.check_index_value(index)?;
        Some(element)
    }

    /// Index and slice bounds require an integer type; literal widths follow their uses.
    fn check_index_value(&mut self, index: &Expr) -> Option<Type> {
        let index_type = self.check_expr(index)?;
        self.predicate(PredicateKind::Integer, index_type.clone(), index.span, PredicateSite::Index);
        Some(index_type)
    }

    /// Check a slice `array[start..end]`: the array must be a built-in array and the
    /// bounds integers. A slice is an array of the same type sharing the elements.
    pub(super) fn check_slice(&mut self, array: &Expr, start: Option<&Expr>, end: Option<&Expr>) -> Option<Type> {
        let array_type = self.check_expr(array)?;
        if self.array_element(&array_type).is_none() {
            let shown = self.shown_type(&array_type);
            self.err(array.span, format!("type {shown} cannot be sliced"));
            return None;
        }
        for bound in start.into_iter().chain(end) {
            self.check_index_value(bound)?;
        }
        Some(array_type)
    }

    pub(super) fn check_address_of(&mut self, operand: &Expr, span: Span) -> Option<Type> {
        // A slice already shares its array's elements, so there is nothing to address.
        if let ExprKind::Slice { .. } = &operand.kind {
            self.err(
                span,
                "a slice already shares its array's elements; write `a[lo..hi]`, or `a[lo..hi].copy()` for a copy",
            );
            return None;
        }
        // Type-check the operand first so addressability can consult its type (the
        // receiver of an index must be a built-in array). A well-typed operand is
        // re-probed without emitting further diagnostics.
        let operand_type = self.check_expr(operand)?;
        if !self.is_addressable(operand) {
            self.err(span, "cannot take the address of a non-addressable expression");
            return None;
        }
        Some(Type::Pointer(Box::new(operand_type)))
    }

    pub(super) fn check_deref(&mut self, operand: &Expr) -> Option<Type> {
        let operand_type = self.check_expr(operand)?;
        let elem = self.infer.fresh(InferKind::General);
        if self.infer.unify(&operand_type, &Type::Pointer(Box::new(elem.clone()))).is_err() {
            let shown = self.shown_type(&operand_type);
            self.err(operand.span, format!("cannot dereference non-pointer type {shown}"));
            return None;
        }
        Some(elem)
    }

    /// Whether an expression denotes a storage location whose address can be taken.
    /// Temporaries (literals, call results, arithmetic) are not addressable. Indexing is
    /// a place only on a built-in array, whose element lives in the backing buffer.
    fn is_addressable(&mut self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Ident(name) => self.lookup_var(name).is_some(),
            ExprKind::Field { .. } | ExprKind::Deref(_) => true,
            ExprKind::Index { array, .. } => matches!(self.check_expr(array), Some(Type::Array(_))),
            ExprKind::Group(inner) => self.is_addressable(inner),
            _ => false,
        }
    }

    pub(super) fn check_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr) -> Option<Type> {
        // `a[i] = v` writes an array element through the built-in index sugar; only a
        // built-in array is index-assignable.
        if let ExprKind::Index { array, index } = &target.kind {
            return self.check_index_assign(op, array, index, value, target.span);
        }
        let target_type = self.check_expr(target)?;
        // The hint supplies a bare variant's subject; assignment equality constrains
        // the value's type, including literal variables.
        self.expected = Some(target_type.clone());
        let value_type = self.check_expr(value)?;
        self.check_assign_op(op, &target_type, &value_type, target.span);
        Some(target_type)
    }

    /// Check an index assignment `a[i] <op>= v`: the receiver must be a built-in array,
    /// the index is an integer, and a compound operator relates the element and value.
    fn check_index_assign(
        &mut self,
        op: AssignOp,
        array: &Expr,
        index: &Expr,
        value: &Expr,
        span: Span,
    ) -> Option<Type> {
        let recv = self.check_expr(array)?;
        let Some(element) = self.array_element(&recv) else {
            let shown = self.shown_type(&recv);
            self.err(span, format!("type {shown} cannot be assigned by index"));
            return None;
        };
        self.check_index_value(index)?;
        self.expected = Some(element.clone());
        let value_type = self.check_expr(value)?;
        self.check_assign_op(op, &element, &value_type, span);
        Some(element)
    }

    /// Apply an assignment operator's type rule, reporting a diagnostic on mismatch. A
    /// plain assignment requires equal types. A compound operator follows its binary
    /// operator: two values of one numeric type, or two strings for `+=`.
    fn check_assign_op(&mut self, op: AssignOp, target_type: &Type, value_type: &Type, span: Span) {
        if self.infer.unify(target_type, value_type).is_err() {
            let (value_shown, target_shown) = self.shown_pair(value_type, target_type);
            let message = match op {
                AssignOp::Assign => format!("cannot assign {value_shown} to {target_shown}"),
                _ => format!("invalid operands for {}: {target_shown} and {value_shown}", op.symbol()),
            };
            self.err(span, message);
            return;
        }
        let kind = match op {
            AssignOp::Assign => return,
            AssignOp::Add => PredicateKind::Addable,
            AssignOp::Sub | AssignOp::Mul | AssignOp::Div | AssignOp::Rem => PredicateKind::Numeric,
        };
        self.predicate(kind, target_type.clone(), span, PredicateSite::Assignment(op, value_type.clone()));
    }
}
