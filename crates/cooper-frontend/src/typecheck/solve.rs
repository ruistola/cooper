//! Constraint solving: obligations, predicates, and body completion.

use super::*;

impl<'g> TypeChecker<'g> {
    fn mark_obligation(&mut self, obligation: &Obligation) {
        let mut ids = Vec::new();
        self.infer.unbound_ids(&obligation.subject, &mut ids);
        match &obligation.kind {
            ObligationKind::Field { result, .. } => self.infer.unbound_ids(result, &mut ids),
            ObligationKind::Variant { result, payload, .. } => {
                self.infer.unbound_ids(result, &mut ids);
                if let Some(payload) = payload {
                    for (ty, _) in payload {
                        self.infer.unbound_ids(ty, &mut ids);
                    }
                }
            }
            ObligationKind::Pattern { binders, .. } => {
                for ty in binders.values() {
                    self.infer.unbound_ids(ty, &mut ids);
                }
            }
            ObligationKind::Exhaustive(_) => {}
        }
        self.body.reported.extend(ids);
    }

    fn report_obligation(&mut self, obligation: &Obligation) {
        let message = match &obligation.kind {
            ObligationKind::Variant { name, bare: true, .. } => format!(
                "cannot infer sum type for bare variant {name}; qualify it as Type.{name} or add a type annotation"
            ),
            _ => "cannot infer the type of this value; add a type annotation".to_string(),
        };
        self.err(obligation.span, message);
        self.mark_obligation(obligation);
    }

    pub(super) fn check_deferred_call(&mut self, span: Span, actual: &Type) -> bool {
        let Some(call) = self.body.calls.get(&span) else { return true };
        let call_span = call.span;
        let args = call.args.clone();
        let actual = self.infer.resolve(actual);
        let Type::Func { param_types, .. } = &actual else {
            let shown = self.shown_type(&actual);
            if self.infer.is_bound(&shown) {
                self.err(call_span, format!("cannot call non-function value of type {shown}"));
                return false;
            }
            return true;
        };
        if args.len() != param_types.len() {
            self.err(call_span, format!(
                "wrong number of arguments, expected {}, found {}", param_types.len(), args.len()
            ));
            return false;
        }
        for (i, ((arg, arg_span), param)) in args.iter().zip(param_types).enumerate() {
            if self.infer.unify(param, arg).is_err() {
                let (expected, found) = self.shown_pair(param, arg);
                self.err(*arg_span, format!("argument {} type mismatch: expected {expected}, found {found}", i + 1));
                return false;
            }
        }
        true
    }

    fn try_obligation(&mut self, obligation: &Obligation) -> bool {
        if let ObligationKind::Variant { result, payload: None, .. } = &obligation.kind {
            if !self.infer.is_bound(&obligation.subject) {
                let expected = match self.infer.resolve(result) {
                    sum @ Type::Oneof { .. } => Some(sum),
                    Type::Func { return_type, .. } => Some(*return_type),
                    _ => None,
                };
                if let Some(expected) = expected {
                    let _ = self.infer.unify(&obligation.subject, &expected);
                }
            }
        }
        let subject = self.infer.resolve(&obligation.subject);
        let effective = match &subject {
            Type::Pointer(elem) if matches!(obligation.kind, ObligationKind::Field { .. }) => elem.as_ref(),
            _ => &subject,
        };
        if !self.infer.is_bound(effective) {
            return false;
        }
        let errors = self.diags.len();
        match &obligation.kind {
            ObligationKind::Field { name, result, target_span } => {
                if let Some(actual) = self.resolve_field(subject, name, obligation.span, *target_span) {
                    if self.check_deferred_call(obligation.span, &actual)
                        && self.infer.unify(result, &actual).is_err()
                    {
                        let (expected, found) = self.shown_pair(result, &actual);
                        self.err(obligation.span, format!("member type mismatch: expected {expected}, found {found}"));
                    }
                }
            }
            ObligationKind::Variant { name, result, payload, bare } => {
                self.solve_variant(&subject, name, result, payload.as_deref(), *bare, obligation.span);
            }
            ObligationKind::Pattern { pattern, binders } => {
                self.scopes.push(HashMap::new());
                self.check_pattern(pattern, &subject);
                let actual = self.scopes.pop().expect("a pattern has a binding scope");
                let mut names: Vec<_> = binders.keys().collect();
                names.sort();
                for name in names {
                    let inferred = &binders[name];
                    if let Some(ty) = actual.get(name) {
                        if self.infer.unify(inferred, ty).is_err() {
                            let (expected, found) = self.shown_pair(inferred, ty);
                            self.err(pattern.span, format!("pattern binding {name} type mismatch: expected {expected}, found {found}"));
                        }
                    }
                }
            }
            ObligationKind::Exhaustive(coverage) => {
                if self.match_type(subject.clone(), obligation.span).is_some() {
                    self.check_match_exhaustiveness(&subject, coverage, obligation.span);
                }
            }
        }
        if self.diags.len() != errors {
            self.mark_obligation(obligation);
        }
        true
    }

    fn solve_obligations(&mut self) {
        loop {
            let pending = std::mem::take(&mut self.body.obligations);
            let mut progress = false;
            for obligation in pending {
                if self.try_obligation(&obligation) {
                    progress = true;
                } else {
                    self.body.obligations.push(obligation);
                }
            }
            if !progress {
                break;
            }
        }
    }

    /// Complete the current body's constraints before its types reach lowering.
    pub(super) fn finish_body(&mut self) {
        self.solve_obligations();
        for literal in &self.body.literals {
            self.infer.default_literal(&literal.ty);
        }
        self.solve_obligations();
        let pending = std::mem::take(&mut self.body.obligations);
        for obligation in pending {
            self.report_obligation(&obligation);
        }
        let mut body = std::mem::take(&mut self.body);
        for predicate in body.predicates {
            self.check_predicate(predicate);
        }
        for literal in body.literals {
            if !literal_is_float(&literal.text) {
                if let Type::Primitive(name) = self.infer.resolve(&literal.ty) {
                    if is_integer_name(&name) {
                        self.check_integer_range(&literal.text, &name, literal.negated, literal.span);
                    }
                }
            }
        }
        let mut reported = HashSet::new();
        for id in body.reported {
            let mut ids = Vec::new();
            self.infer.unbound_ids(&Type::Infer(id), &mut ids);
            reported.extend(ids);
        }
        let mut references: Vec<_> = body.references.into_values().collect();
        references.sort_by_key(|reference| reference.span.start);
        for reference in references {
            self.finish_reference(&reference, &mut reported);
        }
        for captures in self.captures.values_mut() {
            for (_, ty) in captures {
                *ty = self.infer.resolve(ty);
            }
        }
        body.spans.sort_by_key(|span| (span.start, span.end));
        body.spans.dedup();
        for span in body.spans {
            let Some(ty) = self.types.get(&span).map(|ty| self.infer.resolve(ty)) else {
                continue;
            };
            let mut ids = Vec::new();
            self.infer.unbound_ids(&ty, &mut ids);
            let mut unreported = false;
            for id in ids {
                unreported |= reported.insert(id);
            }
            if unreported {
                self.err(span, "cannot infer the type of this value; add a type annotation");
            }
            self.types.insert(span, ty);
        }
    }

    fn check_predicate(&mut self, predicate: Predicate) {
        let ty = self.infer.resolve(&predicate.ty);
        if !self.infer.is_resolved(&ty) {
            return;
        }
        let valid = match predicate.kind {
            PredicateKind::Numeric => is_numeric(&ty),
            PredicateKind::Addable => is_numeric(&ty) || is_primitive(&ty, "string"),
            PredicateKind::Integer => is_integer(&ty),
            PredicateKind::Logical => is_integer(&ty) || is_primitive(&ty, "bool"),
            PredicateKind::Comparable => incomparable_part(&ty, &self.globals.defs).is_none(),
            PredicateKind::Formattable => !matches!(ty, Type::Module(_)),
        };
        if !valid {
            let message = match predicate.kind {
                PredicateKind::Comparable => {
                    let part = incomparable_part(&ty, &self.globals.defs)
                        .expect("a failed comparable predicate has an incomparable component");
                    let reason = if part.equals(&ty) {
                        String::new()
                    } else {
                        format!(" (it contains {part})")
                    };
                    format!("values of type {ty} are not comparable{reason}")
                }
                _ => match predicate.site {
                    PredicateSite::Binary(op, other) => {
                        let (left, right) = self.shown_pair(&ty, &other);
                        format!("invalid operands for {}: {left} and {right}", op.symbol())
                    }
                    PredicateSite::Unary(op) => format!("invalid operand for {}: {ty}", op.symbol()),
                    PredicateSite::Assignment(op, other) => {
                        let (left, right) = self.shown_pair(&ty, &other);
                        format!("invalid operands for {}: {left} and {right}", op.symbol())
                    }
                    PredicateSite::Conversion(name) => {
                        if name == "string" {
                            "a module is not a value and cannot be converted to string".to_string()
                        } else {
                            format!("cannot convert value of type {ty} to {name}; source must be numeric")
                        }
                    }
                    PredicateSite::Index => format!("index must be an integer, found {ty}"),
                    PredicateSite::ShiftCount => format!("shift count must be an integer, found {ty}"),
                    PredicateSite::Range => format!("range bounds must be an integer type, found {ty}"),
                    PredicateSite::IntegerPattern { .. } => {
                        format!("integer pattern cannot match a value of type {ty}")
                    }
                },
            };
            self.err(predicate.span, message);
            return;
        }
        if let PredicateSite::IntegerPattern { negative, magnitude } = predicate.site {
            if decode_number_literal(&magnitude).is_none() {
                self.err(predicate.span, format!("integer literal {magnitude} is out of range"));
            } else if negative {
                if let Type::Primitive(name) = ty {
                    if !is_signed_int(&name) {
                        self.err(predicate.span, format!("negative pattern -{magnitude} cannot match unsigned {name}"));
                    }
                }
            }
        }
    }
}
