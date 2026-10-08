//! Match expressions and statements, patterns, and exhaustiveness.

use super::*;

impl<'g> TypeChecker<'g> {
    pub(super) fn check_match_stmt(&mut self, scrutinee: &Expr, arms: &[StmtArm]) {
        let Some(scrutinee_ty) = self.match_scrutinee(scrutinee) else {
            return;
        };
        let mut coverage = Coverage::default();
        for arm in arms {
            self.scopes.push(HashMap::new());
            self.check_pattern(&arm.pattern, &scrutinee_ty);
            coverage.record(arm_cover(&arm.pattern));
            self.check_stmt(&arm.body);
            self.scopes.pop();
        }
        self.check_match_exhaustiveness(&scrutinee_ty, &coverage, scrutinee.span);
    }

    pub(super) fn check_match_expr(&mut self, scrutinee: &Expr, arms: &[ExprArm]) -> Option<Type> {
        let scrutinee_ty = self.match_scrutinee(scrutinee)?;
        let mut coverage = Coverage::default();
        let mut match_type: Option<Type> = None;
        for arm in arms {
            self.scopes.push(HashMap::new());
            self.check_pattern(&arm.pattern, &scrutinee_ty);
            coverage.record(arm_cover(&arm.pattern));
            let arm_type = self.check_expr(&arm.body);
            self.scopes.pop();
            if let Some(arm_type) = arm_type {
                match &match_type {
                    None => match_type = Some(arm_type),
                    Some(prev) if self.infer.unify(prev, &arm_type).is_err() => {
                        let (prev_shown, arm_shown) = self.shown_pair(prev, &arm_type);
                        self.err(
                            arm.body.span,
                            format!("match arms have mismatched types: {prev_shown} and {arm_shown}"),
                        )
                    }
                    _ => {}
                }
            }
        }
        self.check_match_exhaustiveness(&scrutinee_ty, &coverage, scrutinee.span);
        Some(match_type.unwrap_or(Type::Unit))
    }

    /// Check a match scrutinee and require a matchable type: a sum type, a struct, a
    /// tuple, a boolean, or an integer. Returns that type for the arm patterns to be
    /// checked against.
    fn match_scrutinee(&mut self, scrutinee: &Expr) -> Option<Type> {
        let ty = self.check_expr(scrutinee)?;
        self.match_type(ty, scrutinee.span)
    }

    pub(super) fn match_type(&mut self, ty: Type, span: Span) -> Option<Type> {
        if !self.infer.is_bound(&ty) {
            return Some(ty);
        }
        match &ty {
            Type::Oneof { .. } | Type::Struct { .. } | Type::Tuple(_) => Some(ty),
            Type::Primitive(n) if n == "bool" || is_integer_name(n) => Some(ty),
            _ => {
                self.err(
                    span,
                    format!(
                        "cannot match on value of type {ty}; match supports sum types, structs, tuples, bool, and integers"
                    ),
                );
                None
            }
        }
    }

    /// Validate a pattern against the type of the value it matches, binding every
    /// name it introduces into the current (arm) scope. Recurses through tuple and
    /// struct patterns so a sub-pattern is checked against its component's type.
    pub(super) fn check_pattern(&mut self, pattern: &Pattern, ty: &Type) {
        let resolved = self.infer.resolve(ty);
        if !self.infer.is_bound(&resolved) {
            let mut binders = HashMap::new();
            self.fresh_pattern_binders(pattern, &mut binders);
            if let PatternKind::Binding(name) = &pattern.kind {
                if let Some(binder) = binders.get(name) {
                    let _ = self.infer.unify(binder, &resolved);
                }
            }
            self.body.obligations.push(Obligation {
                subject: resolved,
                span: pattern.span,
                kind: ObligationKind::Pattern { pattern: pattern.clone(), binders },
            });
            return;
        }
        let ty = &resolved;
        match &pattern.kind {
            PatternKind::Wildcard => {}
            PatternKind::Binding(name) => {
                let bound = self.infer_binding(ty);
                self.define_var(name, bound);
            }
            PatternKind::Bool(_) => {
                if self.infer.unify(ty, &Type::Primitive("bool".to_string())).is_err() {
                    let shown = self.shown_type(ty);
                    self.err(pattern.span, format!("boolean pattern cannot match a value of type {shown}"));
                }
            }
            PatternKind::Int { negative, magnitude } => {
                self.check_int_pattern(*negative, magnitude, ty, pattern.span)
            }
            PatternKind::Tuple(elems) => match ty {
                Type::Tuple(types) if types.len() == elems.len() => {
                    for (sub, elem_ty) in elems.iter().zip(types.clone()) {
                        self.check_pattern(sub, &elem_ty);
                    }
                }
                Type::Tuple(types) => self.err(
                    pattern.span,
                    format!(
                        "tuple pattern binds {} element(s) but the value has {}",
                        elems.len(),
                        types.len()
                    ),
                ),
                _ => self.err(
                    pattern.span,
                    format!("tuple pattern cannot match a value of type {ty}"),
                ),
            },
            PatternKind::Struct { name, fields } => {
                self.check_struct_pattern(name, fields, ty, pattern.span)
            }
            PatternKind::Variant {
                type_name,
                variant,
                binders,
            } => self.check_variant_pattern(type_name, variant, binders, ty, pattern.span),
        }
    }

    fn fresh_pattern_binders(&mut self, pattern: &Pattern, out: &mut HashMap<String, Type>) {
        let names: &[String] = match &pattern.kind {
            PatternKind::Binding(name) => std::slice::from_ref(name),
            PatternKind::Variant { binders, .. } => binders,
            PatternKind::Tuple(elems) => {
                for elem in elems {
                    self.fresh_pattern_binders(elem, out);
                }
                return;
            }
            PatternKind::Struct { fields, .. } => {
                for field in fields {
                    self.fresh_pattern_binders(&field.pattern, out);
                }
                return;
            }
            _ => return,
        };
        for name in names {
            if name != "_" {
                let ty = out.entry(name.clone())
                    .or_insert_with(|| self.infer.fresh(InferKind::General)).clone();
                self.define_var(name, ty);
            }
        }
    }

    fn check_struct_pattern(
        &mut self,
        name: &str,
        fields: &[FieldPattern],
        ty: &Type,
        span: Span,
    ) {
        let Type::Struct { id, .. } = ty else {
            self.err(
                span,
                format!("struct pattern cannot match a value of type {ty}"),
            );
            return;
        };
        let struct_name = id.name.clone();
        let members = self.globals.defs.struct_members(ty).unwrap_or_default();
        if self.globals.structs.get(name) != Some(id) {
            let named = self.globals.lookup_struct(name);
            let (name_shown, ty_shown) = match &named {
                Some(named) => display_pair(named, ty),
                None => (name.to_string(), ty.to_string()),
            };
            self.err(
                span,
                format!("pattern names struct {name_shown} but the scrutinee has type {ty_shown}"),
            );
            return;
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for field in fields {
            if !seen.insert(field.name.as_str()) {
                self.err(
                    field.span,
                    format!("field {} is matched more than once", field.name),
                );
                continue;
            }
            match members.iter().find(|(name, _)| name == &field.name).map(|(_, t)| t) {
                Some(member_ty) => {
                    let member_ty = member_ty.clone();
                    self.check_pattern(&field.pattern, &member_ty);
                }
                None => self.err(
                    field.span,
                    format!("{} is not a field of struct {struct_name}", field.name),
                ),
            }
        }
    }

    fn check_variant_pattern(
        &mut self,
        type_name: &Option<String>,
        variant: &str,
        binders: &[String],
        ty: &Type,
        span: Span,
    ) {
        let Type::Oneof { id, .. } = ty else {
            self.err(
                span,
                format!("variant pattern cannot match a value of type {ty}"),
            );
            return;
        };
        let oneof_name = id.name.clone();
        if let Some(type_name) = type_name {
            if self.globals.oneofs.get(type_name) != Some(id) {
                let named = self.globals.lookup_oneof(type_name);
                let (name_shown, ty_shown) = match &named {
                    Some(named) => display_pair(named, ty),
                    None => (type_name.to_string(), ty.to_string()),
                };
                self.err(
                    span,
                    format!(
                        "pattern names sum type {name_shown} but the scrutinee has type {ty_shown}"
                    ),
                );
                return;
            }
        }
        let Some(payload) = self.globals.defs.payload(ty, variant) else {
            self.err(
                span,
                format!("{variant} is not a variant of sum type {oneof_name}"),
            );
            return;
        };
        if binders.len() != payload.len() {
            self.err(
                span,
                format!(
                    "variant pattern {oneof_name}.{variant} binds {} slot(s) but the variant has {}",
                    binders.len(),
                    payload.len()
                ),
            );
            return;
        }
        for (binder, slot) in binders.iter().zip(payload) {
            if binder != "_" {
                self.define_var(binder, slot);
            }
        }
    }

    /// Validate an integer literal pattern, requiring it to fit the scrutinee's
    /// integer type.
    fn check_int_pattern(&mut self, negative: bool, magnitude: &str, ty: &Type, span: Span) {
        self.predicate(
            PredicateKind::Integer,
            ty.clone(),
            span,
            PredicateSite::IntegerPattern { negative, magnitude: magnitude.to_string() },
        );
    }

    pub(super) fn check_match_exhaustiveness(&mut self, ty: &Type, coverage: &Coverage, span: Span) {
        let resolved = self.infer.resolve(ty);
        if !self.infer.is_bound(&resolved) {
            self.body.obligations.push(Obligation {
                subject: resolved,
                span,
                kind: ObligationKind::Exhaustive(coverage.clone()),
            });
            return;
        }
        let ty = &resolved;
        if coverage.wildcard {
            return;
        }
        match ty {
            Type::Oneof { id, .. } => {
                let name = &id.name;
                let missing: Vec<&str> = self
                    .globals
                    .defs
                    .variant_order(ty)
                    .iter()
                    .filter(|v| !coverage.variants.contains(*v))
                    .map(String::as_str)
                    .collect();
                if !missing.is_empty() {
                    self.err(
                        span,
                        format!(
                            "non-exhaustive match on sum type {name}: missing variant(s) {missing:?}"
                        ),
                    );
                }
            }
            Type::Primitive(n) if n == "bool" => {
                let missing: Vec<&str> = [false, true]
                    .iter()
                    .filter(|b| !coverage.bools.contains(*b))
                    .map(|b| if *b { "true" } else { "false" })
                    .collect();
                if !missing.is_empty() {
                    self.err(
                        span,
                        format!("non-exhaustive match on bool: missing {missing:?}"),
                    );
                }
            }
            // Integer and structured (tuple/struct) scrutinees cannot be shown
            // exhaustive by enumeration here, so they require an irrefutable catch-all
            // arm (`_` or a binding).
            _ => self.err(
                span,
                format!(
                    "non-exhaustive match on {ty}: add a catch-all arm (`_` or a binding)"
                ),
            ),
        }
    }
}

/// The domain-specific coverage a single top-level match arm contributes.
enum ArmCover {
    /// An irrefutable arm (wildcard, binding, or an all-irrefutable tuple/struct):
    /// a catch-all that makes any scrutinee exhaustive.
    Wildcard,
    Variant(String),
    Bool(bool),
    /// A refutable arm that enumeration cannot credit toward exhaustiveness.
    Other,
}

/// The accumulated coverage across a match's arms, read for exhaustiveness.
#[derive(Clone, Default)]
pub(super) struct Coverage {
    variants: HashSet<String>,
    bools: HashSet<bool>,
    wildcard: bool,
}

impl Coverage {
    fn record(&mut self, cover: ArmCover) {
        match cover {
            ArmCover::Wildcard => self.wildcard = true,
            ArmCover::Variant(v) => {
                self.variants.insert(v);
            }
            ArmCover::Bool(b) => {
                self.bools.insert(b);
            }
            ArmCover::Other => {}
        }
    }
}

/// The coverage a top-level arm pattern contributes toward exhaustiveness. A tuple
/// or struct pattern counts as a catch-all only when it is wholly irrefutable.
fn arm_cover(pattern: &Pattern) -> ArmCover {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Binding(_) => ArmCover::Wildcard,
        PatternKind::Variant { variant, .. } => ArmCover::Variant(variant.clone()),
        PatternKind::Bool(b) => ArmCover::Bool(*b),
        PatternKind::Int { .. } => ArmCover::Other,
        PatternKind::Tuple(_) | PatternKind::Struct { .. } => {
            if is_irrefutable(pattern) {
                ArmCover::Wildcard
            } else {
                ArmCover::Other
            }
        }
    }
}

/// Whether a pattern matches every value of its type, binding names but never
/// rejecting. A struct pattern's unlisted fields are implicitly wildcards.
fn is_irrefutable(pattern: &Pattern) -> bool {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Binding(_) => true,
        PatternKind::Tuple(elems) => elems.iter().all(is_irrefutable),
        PatternKind::Struct { fields, .. } => {
            fields.iter().all(|f| is_irrefutable(&f.pattern))
        }
        PatternKind::Variant { .. } | PatternKind::Bool(_) | PatternKind::Int { .. } => false,
    }
}
