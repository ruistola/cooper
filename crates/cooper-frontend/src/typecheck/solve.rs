//! Constraint solving: obligations, predicates, and body completion.

use super::*;
use super::literals::integer_range_error;
use crate::types::TypeDefs;

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
                    sum @ Type::Union { .. } => Some(sum),
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
                if let Some(actual) = self.resolve_field(subject, name, obligation.span, *target_span, false) {
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
                    if let Some(Binding { ty, .. }) = actual.get(name) {
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
        // A precision asks for a float, as may a generic callee, so an integer literal it
        // takes is one.
        let precisions = (self.body.predicates.iter())
            .filter(|predicate| matches!((&predicate.kind, &predicate.site), (PredicateKind::Float, PredicateSite::Spec(_))))
            .map(|predicate| predicate.ty.clone());
        let floats: Vec<Type> = precisions.chain(self.body.floats.iter().cloned()).collect();
        for ty in floats {
            if self.infer.is_int_literal(&ty) {
                let float = self.infer.fresh(InferKind::FloatLiteral);
                let _ = self.infer.unify(&ty, &float);
            }
        }
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
            let float = literal_is_float(&literal.text);
            match self.infer.resolve(&literal.ty) {
                Type::Primitive(name) if !float && is_integer_name(&name) => {
                    self.check_integer_range(&literal.text, &name, literal.negated, literal.span);
                }
                Type::TypeParam(param) => {
                    let kind = if float {
                        PredicateKind::Float
                    } else {
                        PredicateKind::Literal { text: literal.text.clone(), negated: literal.negated }
                    };
                    let sign = if literal.negated { "-" } else { "" };
                    let text = &literal.text;
                    if !self.require(&[param.clone()], &kind, literal.span, |param| format!("uses the literal {sign}{text} as a {param}")) {
                        self.err(literal.span, format!("a literal cannot take the type {param}, a type parameter of a nested function"));
                    }
                }
                _ => {}
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
        references.extend(body.dispatches);
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
        let mut nested = false;
        if let Some(Ok(params)) = params_needing(&predicate.kind, &ty, &self.globals.defs) {
            nested = true;
            if self.require(&params, &predicate.kind, predicate.span, |param| predicate.site.describe(param)) {
                if let PredicateSite::IntegerPattern { negative, magnitude } = &predicate.site {
                    let kind = PredicateKind::Literal { text: magnitude.clone(), negated: *negative };
                    self.require(&params, &kind, predicate.span, |param| predicate.site.describe(param));
                }
                return;
            }
        }
        if !holds(&predicate.kind, &ty, &self.globals.defs) {
            let message = match predicate.kind {
                PredicateKind::Comparable => unmet(&predicate.kind, &ty, &self.globals.defs),
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
                            let sources = if name == "u64" { "numeric or a pointer" } else { "numeric" };
                            format!("cannot convert value of type {ty} to {name}; source must be {sources}")
                        }
                    }
                    PredicateSite::Index => format!("index must be an integer, found {ty}"),
                    PredicateSite::Spec(what) => {
                        let wants = match predicate.kind {
                            PredicateKind::Integer => "an integer",
                            PredicateKind::Float => "a float",
                            _ => "a number",
                        };
                        format!("{what} in a format spec applies to {wants}, found {ty}")
                    }
                    PredicateSite::ShiftCount => format!("shift count must be an integer, found {ty}"),
                    PredicateSite::Range => format!("range bounds must be an integer type, found {ty}"),
                    PredicateSite::IntegerPattern { .. } => {
                        format!("integer pattern cannot match a value of type {ty}")
                    }
                },
            };
            let mut diag = Diagnostic::error(predicate.span, message);
            if nested {
                diag = diag.with_note("only a module-level function or method can require this of its type parameters");
            }
            self.diags.push(diag);
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

impl TypeChecker<'_> {
    /// Record that the declaration being checked needs `kind` of each of `params`, if they are
    /// all its own type parameters; whether they were. Those of a nested generic function
    /// cannot carry requirements, since nothing checks its instantiations against them.
    fn require(&mut self, params: &[String], kind: &PredicateKind, span: Span, origin: impl Fn(&str) -> String) -> bool {
        let Some((item, own)) = &self.generic else { return false };
        if !params.iter().all(|param| own.contains(param)) {
            return false;
        }
        for param in params {
            let requirement = Requirement { param: param.clone(), kind: kind.clone(), origin: origin(param) };
            self.requirements.push((item.clone(), requirement, span));
        }
        true
    }
}

impl PredicateSite {
    /// What the operation at this site does with the type parameter `param`, for a
    /// requirement's origin.
    fn describe(&self, param: &str) -> String {
        match self {
            PredicateSite::Binary(op, _) => format!("uses {} on {param}", op.symbol()),
            PredicateSite::Unary(op) => format!("uses {} on {param}", op.symbol()),
            PredicateSite::Assignment(op, _) => format!("uses {} on {param}", op.symbol()),
            PredicateSite::Conversion(name) => format!("converts a {param} to {name}"),
            PredicateSite::Index => format!("indexes with a {param}"),
            PredicateSite::ShiftCount => format!("shifts by a {param}"),
            PredicateSite::Range => format!("uses a {param} as a range bound"),
            PredicateSite::IntegerPattern { .. } => format!("matches a {param} against an integer pattern"),
            PredicateSite::Spec(what) => format!("formats a {param} with {what}"),
        }
    }
}

/// Whether `ty`, a type without inference variables, meets `kind` whatever its type parameters are.
pub(super) fn holds(kind: &PredicateKind, ty: &Type, defs: &TypeDefs) -> bool {
    match kind {
        PredicateKind::Numeric => is_numeric(ty),
        PredicateKind::Ordered => is_numeric(ty) || is_primitive(ty, "rune"),
        PredicateKind::Addable => is_numeric(ty) || is_primitive(ty, "string"),
        PredicateKind::Integer => is_integer(ty),
        PredicateKind::Logical => is_integer(ty) || is_primitive(ty, "bool"),
        PredicateKind::Comparable => incomparable_part(ty, defs).is_none(),
        PredicateKind::Formattable => !matches!(ty, Type::Module(_)),
        PredicateKind::Float => crate::types::is_float(ty),
        PredicateKind::Address => is_numeric(ty) || is_primitive(ty, "rune") || matches!(ty, Type::Pointer(_) | Type::Nil),
        PredicateKind::Literal { text, negated } => match ty {
            Type::Primitive(name) if is_integer_name(name) => integer_range_error(text, name, *negated).is_none(),
            _ => is_numeric(ty),
        },
    }
}

/// Why `ty` does not meet `kind`, in terms of what can be done with its values.
pub(super) fn unmet(kind: &PredicateKind, ty: &Type, defs: &TypeDefs) -> String {
    match kind {
        PredicateKind::Comparable => {
            let part = incomparable_part(ty, defs).expect("a type that is not comparable has an incomparable component");
            let reason = if part.equals(ty) { String::new() } else { format!(" (it contains {part})") };
            format!("values of type {ty} are not comparable{reason}")
        }
        PredicateKind::Numeric => format!("{ty} is not a numeric type"),
        PredicateKind::Ordered => format!("values of type {ty} cannot be ordered"),
        PredicateKind::Addable => format!("values of type {ty} cannot be added"),
        PredicateKind::Integer => format!("{ty} is not an integer type"),
        PredicateKind::Logical => format!("{ty} is neither bool nor an integer type"),
        PredicateKind::Formattable => format!("values of type {ty} cannot be formatted"),
        PredicateKind::Float => format!("{ty} is not a float type"),
        PredicateKind::Address => format!("{ty} is neither a number nor a pointer"),
        PredicateKind::Literal { text, negated } => match ty {
            Type::Primitive(name) if is_integer_name(name) => {
                integer_range_error(text, name, *negated).expect("an unmet literal is out of range")
            }
            _ => format!("{ty} is not a numeric type, so it cannot take the literal {}{text}", if *negated { "-" } else { "" }),
        },
    }
}

/// The type parameters whose meeting `kind` would make `ty` meet it, when `ty` has type
/// parameters: `Ok` with them, or `Err` when no choice of them could. `None` when `ty` has none.
pub(super) fn params_needing(kind: &PredicateKind, ty: &Type, defs: &TypeDefs) -> Option<Result<Vec<String>, ()>> {
    if let Type::TypeParam(name) = ty {
        return Some(Ok(vec![name.clone()]));
    }
    let mut params = Vec::new();
    let incomparable = crate::types::incomparable_given(ty, defs, &mut params);
    if params.is_empty() {
        return None;
    }
    // A component type is comparable when its parts are; no other predicate holds of one.
    Some(match kind {
        PredicateKind::Comparable if incomparable.is_none() => Ok(params),
        _ => Err(()),
    })
}
