//! Calls, callable references, conversions, and sum-type variants.

use super::*;

impl<'g> TypeChecker<'g> {
    /// Whether `name` can only be a variant of some sum type: no variable, function,
    /// or type of that name is in scope, but a sum type has such a variant.
    pub(super) fn is_bare_variant(&self, name: &str) -> bool {
        self.lookup_var(name).is_none()
            && self.globals.lookup_func(name).is_none()
            && self.globals.lookup_struct(name).is_none()
            && self.globals.lookup_oneof(name).is_none()
            && self.globals.defs.oneofs.values().any(|def| def.variants.contains_key(name))
    }

    pub(super) fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Option<Type> {
        let expected = self.expected.take();
        // A call whose callee is a variant access (`Maybe.Some(5)`) is sum-type
        // construction, not an ordinary call: its type arguments are inferred by
        // unifying the payload slots against the arguments.
        if let ExprKind::Field { target, name } = &callee.kind {
            if let Some(oneof @ Type::Oneof { .. }) = self.check_expr(target) {
                return self.check_variant_construction(&oneof, name, args, callee.span);
            }
        }
        // A bare constructor records its payload now and resolves its sum type later.
        if let ExprKind::Ident(name) = &callee.kind {
            if self.is_bare_variant(name) {
                let subject = self.bare_variant_subject(expected);
                let payload = self.variant_arguments(args)?;
                return Some(self.defer_variant(subject, name, Some(payload), true, callee.span));
            }
        }
        // A call whose callee names a numeric type (`i64(x)`, `f32(n)`) is an explicit
        // constructor-style conversion, as is `string(x)`, which formats a number or a
        // bool as text. The result is the named type.
        if let ExprKind::Ident(name) = &callee.kind {
            if is_numeric_name(name) || name == "string" {
                return self.check_conversion(name, args, span);
            }
        }
        self.expected = None;
        let callee_type = self.check_expr(callee)?;
        let deferred = matches!(callee_type, Type::Infer(_));
        let callee_type = if deferred {
            let signature = Type::Func {
                return_type: Box::new(self.infer.fresh(InferKind::General)),
                param_types: args.iter().map(|_| self.infer.fresh(InferKind::General)).collect(),
            };
            if self.infer.unify(&callee_type, &signature).is_err() {
                let shown = self.shown_type(&callee_type);
                self.err(span, format!("cannot call non-function value of type {shown}"));
                return None;
            }
            self.infer.resolve(&signature)
        } else {
            callee_type
        };
        let Type::Func {
            return_type,
            param_types,
        } = &callee_type
        else {
            self.err(span, format!("cannot call non-function value of type {callee_type}"));
            return None;
        };
        if args.len() != param_types.len() {
            self.err(
                span,
                format!(
                    "wrong number of arguments, expected {}, found {}",
                    param_types.len(),
                    args.len()
                ),
            );
            return None;
        }
        for (i, (arg, param)) in args.iter().zip(param_types).enumerate() {
            self.expected = Some(param.clone());
            let arg_type = self.check_expr(arg)?;
            if let Err(conflict) = self.infer.unify(param, &arg_type) {
                let (param_shown, arg_type_shown) = self.shown_pair(&conflict.left, &conflict.right);
                self.err(
                    arg.span,
                    format!("argument {} type mismatch: expected {param_shown}, found {arg_type_shown}", i + 1),
                );
                return None;
            }
        }
        if deferred {
            let checked = args.iter().map(|arg| {
                (self.types.get(&arg.span).expect("a call argument is typed").clone(), arg.span)
            }).collect();
            let mut head = callee;
            while let ExprKind::Group(inner) = &head.kind {
                head = inner;
            }
            self.body.calls.insert(head.span, DeferredCall { span, args: checked });
        }
        Some((**return_type).clone())
    }

    /// Instantiate a reference at `span` to the function or method `sig`, with one
    /// fresh variable per binder. Matching a method's receiver against the concrete
    /// `receiver` fixes its receiver-pattern variables.
    /// Uses of the reference constrain those variables by equality.
    /// The body's finalization records the resolved reference. Returns its function
    /// type, written in terms of any variables still unsolved.
    pub(super) fn reference(
        &mut self,
        span: Span,
        sig: &Signature,
        label: &str,
        receiver: Option<&Type>,
    ) -> Option<Type> {
        let type_args: Vec<_> = sig.type_params.iter()
            .map(|param| (param.clone(), self.infer.fresh(InferKind::General)))
            .collect();
        let subst = type_args.iter().cloned().collect();
        if let (Some(template), Some(actual)) = (&sig.receiver, receiver) {
            let _ = self.infer.unify(&template.substitute(&subst), actual);
        }
        let ty = self.infer.resolve(&sig.ty.substitute(&subst));
        let pending = PendingRef {
            span,
            label: label.to_string(),
            item: sig.item.clone(),
            type_args,
        };
        self.body.references.insert(span, pending);
        Some(ty)
    }

    /// Record `pending` with every type argument resolved, or report the first
    /// binder whose argument still contains an unbound inference variable.
    pub(super) fn finish_reference(&mut self, pending: &PendingRef, reported: &mut HashSet<InferId>) {
        let mut unresolved = false;
        for (binder, arg) in &pending.type_args {
            let mut ids = Vec::new();
            self.infer.unbound_ids(arg, &mut ids);
            if !ids.is_empty() {
                unresolved = true;
            }
            if ids.iter().any(|id| !reported.contains(id)) {
                for (_, arg) in &pending.type_args {
                    self.infer.unbound_ids(arg, &mut ids);
                }
                reported.extend(ids);
                self.err(
                    pending.span,
                    format!(
                        "cannot infer type argument {binder} for {}; annotate the expected type",
                        pending.label
                    ),
                );
                return;
            }
        }
        if unresolved {
            return;
        }
        let type_args = pending.type_args.iter()
            .map(|(_, arg)| self.infer.resolve(arg))
            .collect();
        self.refs.insert(
            pending.span,
            ItemRef {
                item: pending.item.clone(),
                type_args,
            },
        );
    }

    /// Type check an explicit conversion `T(value)`: to a numeric type from any numeric
    /// type, or to `string` from any numeric type or `bool`. The result is `T`.
    fn check_conversion(&mut self, name: &str, args: &[Expr], span: Span) -> Option<Type> {
        if args.len() != 1 {
            self.err(
                span,
                format!("conversion to {name} takes exactly one argument, found {}", args.len()),
            );
            return None;
        }
        self.expected = None;
        let arg_type = self.check_expr(&args[0])?;
        let kind = if name == "string" { PredicateKind::Formattable } else { PredicateKind::Numeric };
        self.predicate(kind, arg_type, args[0].span, PredicateSite::Conversion(name.to_string()));
        Some(Type::Primitive(name.to_string()))
    }

    pub(super) fn solve_variant(
        &mut self, subject: &Type, name: &str, result: &Type,
        payload: Option<&[(Type, Span)]>, bare: bool, span: Span,
    ) {
        let slots = self.globals.defs.payload(subject, name);
        let Type::Oneof { id, .. } = subject else {
            self.err(span, format!("cannot infer sum type for bare variant {name}; qualify it as Type.{name} or add a type annotation"));
            return;
        };
        let Some(slots) = slots else {
            let message = if bare {
                format!("cannot infer sum type for bare variant {name}; qualify it as Type.{name} or add a type annotation")
            } else {
                format!("{name} is not a variant of sum type {}", id.name)
            };
            self.err(span, message);
            return;
        };
        let actual = if let Some(args) = payload {
            if args.len() != slots.len() {
                self.err(span, format!("variant {}.{name} expects {} argument(s), got {}", id.name, slots.len(), args.len()));
                return;
            }
            for (i, ((arg, arg_span), slot)) in args.iter().zip(&slots).enumerate() {
                if self.infer.unify(slot, arg).is_err() {
                    let (expected, found) = self.shown_pair(slot, arg);
                    self.err(*arg_span, format!(
                        "argument {} to variant {}.{name} type mismatch: expected {expected}, found {found}", i + 1, id.name
                    ));
                    return;
                }
            }
            subject.clone()
        } else if slots.is_empty() {
            subject.clone()
        } else {
            Type::Func { return_type: Box::new(subject.clone()), param_types: slots }
        };
        if payload.is_none() && !self.check_deferred_call(span, &actual) {
            return;
        }
        if self.infer.unify(result, &actual).is_err() {
            let (expected, found) = self.shown_pair(result, &actual);
            self.err(span, format!("variant type mismatch: expected {expected}, found {found}"));
        }
    }

    fn variant_subject(&mut self, oneof: &Type) -> Type {
        let Type::Oneof { id, type_args } = oneof else {
            unreachable!("a qualified variant has a sum type");
        };
        if !type_args.is_empty() {
            return oneof.clone();
        }
        let params = self.globals.defs.type_params(oneof).to_vec();
        let subst = self.fresh_subst(&params);
        Type::Oneof {
            id: id.clone(),
            type_args: params.iter().map(|param| subst[param].clone()).collect(),
        }
    }

    pub(super) fn bare_variant_subject(&mut self, expected: Option<Type>) -> Type {
        match expected.map(|ty| self.infer.resolve(&ty)) {
            Some(ty @ (Type::Oneof { .. } | Type::Infer(_))) => ty,
            Some(Type::Func { return_type, .. }) => *return_type,
            _ => self.infer.fresh(InferKind::General),
        }
    }

    pub(super) fn defer_variant(
        &mut self, subject: Type, name: &str, payload: Option<Vec<(Type, Span)>>,
        bare: bool, span: Span,
    ) -> Type {
        let result = if payload.is_some() { subject.clone() } else { self.infer.fresh(InferKind::General) };
        self.body.obligations.push(Obligation {
            subject,
            span,
            kind: ObligationKind::Variant { name: name.to_string(), result: result.clone(), payload, bare },
        });
        result
    }

    fn variant_arguments(&mut self, args: &[Expr]) -> Option<Vec<(Type, Span)>> {
        let mut payload = Vec::with_capacity(args.len());
        for arg in args {
            self.expected = None;
            payload.push((self.check_expr(arg)?, arg.span));
        }
        Some(payload)
    }

    /// Type check a bare variant access `Oneof.Variant`. A payload-free variant is a
    /// complete value of the sum type; a variant with a payload yields its
    /// constructor as a function type.
    pub(super) fn check_variant_access(&mut self, oneof: &Type, variant: &str, span: Span) -> Option<Type> {
        let subject = self.variant_subject(oneof);
        Some(self.defer_variant(subject, variant, None, false, span))
    }

    /// Type check a variant construction `Oneof.Variant(args)`, inferring a generic
    /// sum type's arguments by unifying each payload slot against its argument.
    fn check_variant_construction(
        &mut self,
        oneof: &Type,
        variant: &str,
        args: &[Expr],
        span: Span,
    ) -> Option<Type> {
        let subject = self.variant_subject(oneof);
        let payload = self.variant_arguments(args)?;
        Some(self.defer_variant(subject, variant, Some(payload), false, span))
    }
}
