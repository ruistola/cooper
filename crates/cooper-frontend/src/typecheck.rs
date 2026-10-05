//! Type checking: the second semantic pass.
//!
//! Walking function and module bodies against the [`Globals`] table produced by
//! [`crate::resolve`], this pass assigns a [`Type`] to every expression and reports
//! mismatches. It keeps its own stack of block-scoped variable bindings; structs,
//! sum types, functions, and methods live in the global table. Undefined-variable
//! detection falls out naturally from identifier lookup here.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::resolve::{self, Globals};
use crate::types::{
    display_pair, incomparable_part, is_float_name, is_integer, is_integer_name, is_numeric,
    is_numeric_name, is_primitive, is_unit, unify, Type, TypeDefs, TypeId, DEFAULT_FLOAT,
    DEFAULT_INT, SIGNED_INTS,
};

/// The result of type checking one file: its diagnostics, and the type attributed to
/// every expression keyed by span. The span table is the lowering pass's handoff —
/// `cooper-ir` reads it so no type need be recomputed downstream. Spans are unique
/// per expression within a file (they are disjoint byte ranges), so the key is exact.
pub struct Typed {
    pub diags: Vec<Diagnostic>,
    pub types: HashMap<Span, Type>,
}

/// Type check `module` against `globals`. `modules` maps each `use`-bound module's
/// local spelling to its interface, so qualified accesses (`other.func()`) resolve
/// against the depended-on module.
pub fn check(
    module: &[Stmt],
    globals: &Globals,
    modules: &HashMap<Vec<String>, Globals>,
) -> Typed {
    let mut tc = TypeChecker::new(globals, modules);
    tc.scopes.push(HashMap::new());
    for stmt in module {
        tc.check_stmt(stmt);
    }
    Typed {
        diags: tc.diags,
        types: tc.types,
    }
}

struct TypeChecker<'g> {
    globals: &'g Globals,
    /// Bound module interfaces, keyed by local spelling (`["std", "io"]`).
    modules: &'g HashMap<Vec<String>, Globals>,
    diags: Vec<Diagnostic>,
    /// Every expression's attributed type, keyed by span: the lowering handoff.
    types: HashMap<Span, Type>,
    /// Block-scoped variable bindings, innermost scope last.
    scopes: Vec<HashMap<String, Type>>,
    /// The return type of the function whose body is being checked, if any.
    current_return: Option<Type>,
    /// The type parameters in scope for the current function's local annotations.
    type_params: HashSet<String>,
    /// The type an expression is being checked against when known from immediate
    /// context (an annotated initializer or a `return` value). It supplies the type
    /// arguments a generic variant construction cannot infer from its payload
    /// alone. It is a head-only hint: consumed before checking sub-expressions.
    expected: Option<Type>,
    /// How many loop bodies enclose the statement being checked, so `break` and
    /// `continue` outside any loop can be rejected.
    loop_depth: u32,
}

impl<'g> TypeChecker<'g> {
    fn new(globals: &'g Globals, modules: &'g HashMap<Vec<String>, Globals>) -> Self {
        TypeChecker {
            globals,
            modules,
            diags: Vec::new(),
            types: HashMap::new(),
            scopes: Vec::new(),
            current_return: None,
            type_params: HashSet::new(),
            expected: None,
            loop_depth: 0,
        }
    }

    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(span, msg.into()));
    }

    // --- variable scope stack ---

    fn define_var(&mut self, name: &str, ty: Type) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn lookup_var(&self, name: &str) -> Option<&Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    fn resolve_local_type(&mut self, type_expr: &TypeExpr) -> Option<Type> {
        resolve::resolve_type(type_expr, &self.type_params, self.globals, &mut self.diags)
    }

    // --- statements ---

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Block(stmts) => {
                self.scopes.push(HashMap::new());
                for s in stmts {
                    self.check_stmt(s);
                }
                self.scopes.pop();
            }
            StmtKind::VarDecl { name, ty, init } => self.check_var_decl(name, ty, init, stmt.span),
            StmtKind::StructDecl { .. } | StmtKind::OneofDecl { .. } => {}
            StmtKind::FuncDecl(func) => self.check_func_decl(func),
            StmtKind::If { cond, then, els } => {
                let cond_type = self.check_expr(cond);
                if !matches!(cond_type, Some(t) if is_primitive(&t, "bool")) {
                    self.err(cond.span, "if-statement condition does not evaluate to a boolean type");
                }
                self.check_stmt(then);
                if let Some(els) = els {
                    self.check_stmt(els);
                }
            }
            StmtKind::Match { scrutinee, arms } => self.check_match_stmt(scrutinee, arms),
            StmtKind::ForIn {
                bindings,
                iterable,
                body,
            } => self.check_for_in(bindings, iterable, body, stmt.span),
            StmtKind::While { cond, body, .. } => {
                let cond_type = self.check_expr(cond);
                if !matches!(cond_type, Some(t) if is_primitive(&t, "bool")) {
                    self.err(cond.span, "loop condition does not evaluate to a boolean type");
                }
                self.scopes.push(HashMap::new());
                self.loop_depth += 1;
                for s in body {
                    self.check_stmt(s);
                }
                self.loop_depth -= 1;
                self.scopes.pop();
            }
            StmtKind::Break | StmtKind::Continue => {
                if self.loop_depth == 0 {
                    let kw = if matches!(stmt.kind, StmtKind::Break) { "break" } else { "continue" };
                    self.err(stmt.span, format!("'{kw}' outside of a loop"));
                }
            }
            StmtKind::Return(expr) => self.check_return(expr.as_ref(), stmt.span),
            StmtKind::Expression(expr) => {
                self.check_expr(expr);
            }
        }
    }

    /// Determine the element type(s) a `for … in …` loop binds and check its body
    /// against them. Ranges bind a single integer element; arrays bind either the
    /// element alone or an `(index, element)` pair. Any other iterable is an error.
    fn check_for_in(&mut self, bindings: &[String], iterable: &Expr, body: &[Stmt], span: Span) {
        let elems: Option<Vec<Type>> = match &iterable.kind {
            ExprKind::Range { start, end, .. } => {
                let start_type = self.check_expr(start);
                self.expected = start_type.clone();
                let end_type = self.check_expr(end);
                let elem = match (start_type, end_type) {
                    (Some(s), Some(e)) => {
                        if !is_integer(&s) {
                            self.err(start.span, format!("range bounds must be an integer type, found {s}"));
                            None
                        } else if !s.equals(&e) {
                            self.err(end.span, format!("range bounds must share one type: {s} and {e}"));
                            None
                        } else {
                            Some(s)
                        }
                    }
                    _ => None,
                };
                if bindings.len() != 1 {
                    self.err(span, "a range binds a single loop variable");
                    None
                } else {
                    elem.map(|e| vec![e])
                }
            }
            _ => match self.check_expr(iterable) {
                Some(Type::Array(elem)) => match bindings.len() {
                    1 => Some(vec![*elem]),
                    2 => Some(vec![Type::Primitive(DEFAULT_INT.to_string()), *elem]),
                    _ => {
                        self.err(span, "an array binds either one loop variable or an (index, value) pair");
                        None
                    }
                },
                Some(other) => {
                    self.err(iterable.span, format!("cannot iterate over type {other}"));
                    None
                }
                None => None,
            },
        };

        self.scopes.push(HashMap::new());
        if let Some(elems) = elems {
            for (name, ty) in bindings.iter().zip(elems) {
                self.define_var(name, ty);
            }
        }
        self.loop_depth += 1;
        for s in body {
            self.check_stmt(s);
        }
        self.loop_depth -= 1;
        self.scopes.pop();
    }

    fn check_var_decl(
        &mut self,
        name: &str,
        ty: &Option<TypeExpr>,
        init: &Option<Expr>,
        span: Span,
    ) {
        let declared = match ty {
            Some(ty) => match self.resolve_local_type(ty) {
                Some(t) => Some(t),
                None => return,
            },
            None => None,
        };
        let value_type = match init {
            Some(init) => {
                self.expected = declared.clone();
                self.check_expr(init)
            }
            None => None,
        };
        let bound = match (&declared, &value_type) {
            (Some(declared), Some(value)) => {
                if !declared.equals(value) {
                    let (declared_shown, value_shown) = display_pair(declared, value);
                    self.err(
                        span,
                        format!(
                            "type mismatch: variable {name} declared as {declared_shown} but initialized with {value_shown}"
                        ),
                    );
                }
                declared.clone()
            }
            (Some(declared), None) => declared.clone(),
            (None, Some(value)) => value.clone(),
            (None, None) => Type::Unknown,
        };
        self.define_var(name, bound);
    }

    fn check_func_decl(&mut self, func: &FuncDecl) {
        let mut type_params: HashSet<String> = func.type_params.iter().cloned().collect();
        if let Some(receiver) = &func.receiver {
            type_params.extend(resolve::receiver_pattern_params(&receiver.ty));
        }
        let saved_params = std::mem::replace(&mut self.type_params, type_params);

        let return_type = match &func.return_type {
            Some(rt) => self.resolve_local_type(rt).unwrap_or(Type::Unit),
            None => Type::Unit,
        };

        self.scopes.push(HashMap::new());
        if let Some(receiver) = &func.receiver {
            if let Some(t) = self.resolve_local_type(&receiver.ty) {
                self.define_var(&receiver.name, t);
            }
        }
        for param in &func.params {
            if let Some(t) = self.resolve_local_type(&param.ty) {
                self.define_var(&param.name, t);
            }
        }

        let saved_return = self.current_return.replace(return_type);
        for stmt in &func.body {
            self.check_stmt(stmt);
        }
        self.current_return = saved_return;
        self.scopes.pop();
        self.type_params = saved_params;
    }

    fn check_return(&mut self, expr: Option<&Expr>, span: Span) {
        let Some(return_type) = self.current_return.clone() else {
            self.err(span, "return statement outside of function");
            return;
        };
        let is_unit_return = is_unit(&return_type);
        let Some(expr) = expr else {
            if !is_unit_return {
                self.err(span, format!("expected function to return {return_type}"));
            }
            return;
        };
        self.expected = Some(return_type.clone());
        let Some(expr_type) = self.check_expr(expr) else {
            return;
        };
        if is_unit_return {
            self.err(
                expr.span,
                "cannot return a value from a function with no declared return type",
            );
        } else if !expr_type.equals(&return_type) {
            let (return_type_shown, expr_type_shown) = display_pair(&return_type, &expr_type);
            self.err(
                expr.span,
                format!("return type mismatch: expected {return_type_shown}, found {expr_type_shown}"),
            );
        }
    }

    // --- match ---

    fn check_match_stmt(&mut self, scrutinee: &Expr, arms: &[StmtArm]) {
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

    fn check_match_expr(&mut self, scrutinee: &Expr, arms: &[ExprArm]) -> Option<Type> {
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
                    Some(prev) if !prev.equals(&arm_type) => {
                        let (prev_shown, arm_shown) = display_pair(prev, &arm_type);
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
        match &ty {
            Type::Oneof { .. } | Type::Struct { .. } | Type::Tuple(_) => Some(ty),
            Type::Primitive(n) if n == "bool" || is_integer_name(n) => Some(ty),
            _ => {
                self.err(
                    scrutinee.span,
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
    fn check_pattern(&mut self, pattern: &Pattern, ty: &Type) {
        match &pattern.kind {
            PatternKind::Wildcard => {}
            PatternKind::Binding(name) => self.define_var(name, ty.clone()),
            PatternKind::Bool(_) => {
                if !is_primitive(ty, "bool") {
                    self.err(
                        pattern.span,
                        format!("boolean pattern cannot match a value of type {ty}"),
                    );
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
            match members.get(&field.name) {
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
        let Type::Primitive(int_name) = ty else {
            self.err(
                span,
                format!("integer pattern cannot match a value of type {ty}"),
            );
            return;
        };
        if !is_integer_name(int_name) {
            self.err(
                span,
                format!("integer pattern cannot match a value of type {ty}"),
            );
            return;
        }
        if decode_number_literal(magnitude).is_none() {
            self.err(span, format!("integer literal {magnitude} is out of range"));
            return;
        }
        if negative && !is_signed_int(int_name) {
            self.err(
                span,
                format!("negative pattern -{magnitude} cannot match unsigned {int_name}"),
            );
        }
    }

    fn check_match_exhaustiveness(&mut self, ty: &Type, coverage: &Coverage, span: Span) {
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

    // --- expressions ---

    fn check_expr(&mut self, expr: &Expr) -> Option<Type> {
        let ty = self.check_expr_kind(expr);
        if let Some(ty) = &ty {
            self.types.insert(expr.span, ty.clone());
        }
        ty
    }

    fn check_expr_kind(&mut self, expr: &Expr) -> Option<Type> {
        // expectedType is a head-only hint: capture it for this expression and
        // clear it so it never leaks into sub-expressions. Only the dispatches that
        // can act on it restore it before recursing.
        let expected = self.expected.take();
        match &expr.kind {
            ExprKind::Number(text) => Some(self.number_type(text, expr.span, expected, false)),
            ExprKind::Str(_) => Some(Type::Primitive("string".to_string())),
            ExprKind::Bool(_) => Some(Type::Primitive("bool".to_string())),
            ExprKind::Nil => Some(Type::Nil),
            ExprKind::Unit => Some(Type::Unit),
            ExprKind::Tuple(elems) => {
                let mut elem_types = Vec::with_capacity(elems.len());
                for elem in elems {
                    elem_types.push(self.check_expr(elem)?);
                }
                Some(Type::Tuple(elem_types))
            }
            ExprKind::Array(elems) => self.check_array_literal(elems, expr.span, expected),
            ExprKind::Ident(name) => {
                // Restore the hint so a bare payload-free variant can see it, but
                // clear it afterwards so an ordinary identifier never lets it leak.
                self.expected = expected;
                let t = self.check_ident(name, expr.span);
                self.expected = None;
                t
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.check_binary(*op, lhs, rhs, expr.span, expected)
            }
            ExprKind::Range { .. } => {
                self.err(expr.span, "a range may only appear as a for-loop iterable");
                None
            }
            ExprKind::Unary { op, operand } => {
                // A sign carried over a numeric literal is transparent to contextual
                // typing, so restore the hint for `-1` to adopt an expected width.
                self.expected = expected;
                self.check_unary(*op, operand, expr.span)
            }
            ExprKind::Group(inner) => {
                self.expected = expected;
                self.check_expr(inner)
            }
            ExprKind::Call { callee, args } => {
                self.expected = expected;
                self.check_call(callee, args, expr.span)
            }
            ExprKind::StructLiteral { target, members } => {
                self.check_struct_literal(target, members)
            }
            ExprKind::Field { target, name } => {
                self.expected = expected;
                self.check_field(target, name, expr.span)
            }
            ExprKind::Index { array, index } => self.check_index(array, index),
            ExprKind::AddressOf(operand) => self.check_address_of(operand, expr.span),
            ExprKind::Deref(operand) => self.check_deref(operand),
            ExprKind::Assign { op, target, value } => self.check_assign(*op, target, value),
            ExprKind::Let { name, value } => {
                let value_type = self.check_expr(value)?;
                self.define_var(name, value_type.clone());
                Some(value_type)
            }
            ExprKind::LetTuple { names, ty, value } => self.check_let_tuple(names, ty, value),
            ExprKind::Match { scrutinee, arms } => self.check_match_expr(scrutinee, arms),
            ExprKind::If { cond, then, els } => self.check_if_expr(cond, then, els),
            ExprKind::Block(block) => self.check_block_expr(block),
        }
    }

    fn check_ident(&mut self, name: &str, span: Span) -> Option<Type> {
        if let Some(t) = self.lookup_var(name) {
            return Some(t.clone());
        }
        if let Some(t) = self.globals.lookup_struct(name) {
            return Some(t);
        }
        if let Some(t) = self.globals.lookup_oneof(name) {
            return Some(t);
        }
        if let Some(t) = self.globals.lookup_func(name) {
            return Some(t.clone());
        }
        // A bare name that begins some `use`-bound module's spelling is a module
        // reference, navigated further by qualified field access.
        if self.module_is_prefix(std::slice::from_ref(&name.to_string())) {
            return Some(Type::Module(vec![name.to_string()]));
        }
        // A bare name may be a variant of the expected sum type (`None` where a
        // `Maybe` is wanted). `check_variant_access` consumes `self.expected`.
        if let Some(oneof) = self.expected_variant_oneof(name) {
            return self.check_variant_access(&oneof, name, span);
        }
        if self.any_oneof_with_variant(name).is_some() {
            self.err(
                span,
                format!(
                    "cannot infer sum type for bare variant {name}; qualify it as Type.{name} or add a type annotation"
                ),
            );
            return None;
        }
        self.err(span, format!("undefined variable: {name}"));
        None
    }

    /// The generic template of the currently expected sum type, but only when that
    /// type names `variant` among its variants. This is the hook that lets a bare
    /// variant resolve against context; `self.expected` is left intact for the
    /// caller to consume when seeding type arguments.
    fn expected_variant_oneof(&self, variant: &str) -> Option<Type> {
        let Some(Type::Oneof { id, .. }) = &self.expected else {
            return None;
        };
        let template = Type::Oneof {
            id: id.clone(),
            type_args: Vec::new(),
        };
        self.globals.defs.payload(&template, variant).is_some().then_some(template)
    }

    /// The name of any declared sum type carrying `variant`, used only to phrase a
    /// targeted error when a bare variant appears with no expected type to fix it.
    fn any_oneof_with_variant(&self, variant: &str) -> Option<String> {
        self.globals.oneofs.iter().find_map(|(name, id)| {
            let def = self.globals.defs.oneofs.get(id)?;
            def.variants.contains_key(variant).then(|| name.clone())
        })
    }

    /// Whether `segs` names a `use`-bound module or the leading segments of one, so
    /// that a partial spelling (`std` of `std.io`) is still recognised as a module
    /// reference to be navigated further.
    fn module_is_prefix(&self, segs: &[String]) -> bool {
        self.modules.keys().any(|key| key.starts_with(segs))
    }

    /// Resolve a qualified access `prefix.field` where `prefix` is a module reference.
    /// Extending the spelling toward a bound module yields a deeper module reference;
    /// once it names a complete module, `field` selects one of that module's exported
    /// items.
    fn check_module_access(&mut self, prefix: &[String], field: &str, span: Span) -> Option<Type> {
        let mut candidate = prefix.to_vec();
        candidate.push(field.to_string());
        if self.module_is_prefix(&candidate) {
            return Some(Type::Module(candidate));
        }
        if let Some(interface) = self.modules.get(prefix) {
            if let Some(ty) = interface
                .lookup_struct(field)
                .or_else(|| interface.lookup_oneof(field))
                .or_else(|| interface.lookup_func(field).cloned())
            {
                return Some(ty);
            }
            self.err(
                span,
                format!("module `{}` exports no item named `{field}`", prefix.join(".")),
            );
            return None;
        }
        self.err(
            span,
            format!("`{field}` is not a member of module `{}`", prefix.join(".")),
        );
        None
    }

    fn check_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        expected: Option<Type>,
    ) -> Option<Type> {
        match op {
            BinaryOp::And | BinaryOp::Or => {
                let left = self.check_expr(lhs)?;
                let right = self.check_expr(rhs)?;
                if is_primitive(&left, "bool") && is_primitive(&right, "bool") {
                    return Some(Type::Primitive("bool".to_string()));
                }
                let (left_shown, right_shown) = display_pair(&left, &right);
                self.err(
                    span,
                    format!("invalid operands for {}: {left_shown} and {right_shown}", op.symbol()),
                );
                None
            }
            // Every other operator relates two numbers of one type. Operands are
            // checked so a bare literal adopts the other side's width — `x + 1` with
            // `x: i64` types `1` as `i64` — and the arithmetic result carries the
            // shared type, while the comparisons yield `bool`.
            _ => {
                let (left, right) = self.check_numeric_operands(lhs, rhs, expected)?;
                match op {
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                        if is_numeric(&left) && left.equals(&right) {
                            return Some(left);
                        }
                        if op == BinaryOp::Add
                            && is_primitive(&left, "string")
                            && is_primitive(&right, "string")
                        {
                            return Some(Type::Primitive("string".to_string()));
                        }
                        let (left_shown, right_shown) = display_pair(&left, &right);
                        self.err(
                            span,
                            format!("invalid operands for {}: {left_shown} and {right_shown}", op.symbol()),
                        );
                        None
                    }
                    BinaryOp::Eq | BinaryOp::Ne => {
                        if !left.equals(&right) {
                            let (left_shown, right_shown) = display_pair(&left, &right);
                            self.err(span, format!("cannot compare {left_shown} and {right_shown}"));
                            return None;
                        }
                        if let Some(part) = incomparable_part(&left, &self.globals.defs) {
                            let reason = if part.equals(&left) {
                                String::new()
                            } else {
                                format!(" (it contains {part})")
                            };
                            self.err(span, format!("values of type {left} are not comparable{reason}"));
                            return None;
                        }
                        Some(Type::Primitive("bool".to_string()))
                    }
                    BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                        if is_numeric(&left) && left.equals(&right) {
                            return Some(Type::Primitive("bool".to_string()));
                        }
                        let (left_shown, right_shown) = display_pair(&left, &right);
                        self.err(
                            span,
                            format!("invalid operands for {}: {left_shown} and {right_shown}", op.symbol()),
                        );
                        None
                    }
                    BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
                }
            }
        }
    }

    /// Check an array literal, whose elements must all share one type. An expected
    /// `T[]` fixes the element type and guides each element (so `[1, 2]` against
    /// `u8[]` types its literals `u8`). Without one, the element type comes from the
    /// first element that is neither a bare numeric literal nor `nil` — `[1, x]` with
    /// `x: i64` is `i64[]`, and `[nil, p]` takes `p`'s pointer type. An all-literal
    /// list takes its first float literal, so `[1, 2.5]` is a float array, and
    /// otherwise its first element. An empty literal has nothing to infer from and needs an
    /// expected type.
    fn check_array_literal(
        &mut self,
        elems: &[Expr],
        span: Span,
        expected: Option<Type>,
    ) -> Option<Type> {
        let (elem_type, anchor) = match expected {
            Some(Type::Array(elem)) => (*elem, None),
            _ => {
                let Some(anchor) = elems
                    .iter()
                    .position(|e| !is_numeric_literal(e) && !matches!(e.kind, ExprKind::Nil))
                    .or_else(|| elems.iter().position(is_float_literal))
                    .or(if elems.is_empty() { None } else { Some(0) })
                else {
                    self.err(
                        span,
                        "cannot infer the element type of an empty array literal; annotate the binding",
                    );
                    return None;
                };
                let elem_type = self.check_expr(&elems[anchor])?;
                if matches!(elem_type, Type::Nil) {
                    self.err(
                        span,
                        "cannot infer the element type of an array literal of only nil; annotate the binding",
                    );
                    return None;
                }
                (elem_type, Some(anchor))
            }
        };
        for (i, elem) in elems.iter().enumerate() {
            if Some(i) == anchor {
                continue;
            }
            let Some(actual) = self.check_expr_expecting(elem, Some(elem_type.clone())) else {
                continue;
            };
            if !actual.equals(&elem_type) {
                let (elem_type_shown, actual_shown) = display_pair(&elem_type, &actual);
                self.err(
                    elem.span,
                    format!("array element type mismatch: expected {elem_type_shown}, found {actual_shown}"),
                );
            }
        }
        Some(Type::Array(Box::new(elem_type)))
    }

    /// Check the two operands of a numeric or comparison operator, letting a bare
    /// numeric literal borrow its width from the opposite, non-literal operand so
    /// `x + 1` needs no suffix. When neither or both sides is a literal, the enclosing
    /// `expected` hint guides both equally.
    fn check_numeric_operands(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
        expected: Option<Type>,
    ) -> Option<(Type, Type)> {
        let lhs_lit = is_numeric_literal(lhs);
        let rhs_lit = is_numeric_literal(rhs);
        if lhs_lit && !rhs_lit {
            let right = self.check_expr_expecting(rhs, expected.clone())?;
            let hint = if is_numeric(&right) { Some(right.clone()) } else { expected };
            let left = self.check_expr_expecting(lhs, hint)?;
            Some((left, right))
        } else if rhs_lit && !lhs_lit {
            let left = self.check_expr_expecting(lhs, expected.clone())?;
            let hint = if is_numeric(&left) { Some(left.clone()) } else { expected };
            let right = self.check_expr_expecting(rhs, hint)?;
            Some((left, right))
        } else {
            let left = self.check_expr_expecting(lhs, expected.clone())?;
            let right = self.check_expr_expecting(rhs, expected)?;
            Some((left, right))
        }
    }

    fn check_expr_expecting(&mut self, expr: &Expr, expected: Option<Type>) -> Option<Type> {
        self.expected = expected;
        self.check_expr(expr)
    }

    /// The type of a numeric literal, range-checked when it resolves to an integer.
    /// `negated` folds a leading sign into the literal so a negative value is checked
    /// against the type's minimum rather than its maximum.
    fn number_type(&mut self, text: &str, span: Span, expected: Option<Type>, negated: bool) -> Type {
        let ty = number_literal_type(text, expected.as_ref());
        if let Type::Primitive(name) = &ty {
            if is_integer_name(name) {
                self.check_integer_range(text, name, negated, span);
            }
        }
        ty
    }

    /// Report a diagnostic if the integer literal `text`, optionally `negated`, does
    /// not fit the range of integer type `name`. Values are staged through 128-bit
    /// arithmetic, wide enough to hold any 64-bit-or-narrower bound exactly.
    fn check_integer_range(&mut self, text: &str, name: &str, negated: bool, span: Span) {
        let Some(mag) = parse_literal_magnitude(text) else {
            self.err(span, format!("integer literal `{text}` is too large to represent"));
            return;
        };
        let bits = integer_bits(name);
        if name.starts_with('u') {
            if negated && mag != 0 {
                self.err(span, format!("cannot negate a literal of unsigned type {name}"));
                return;
            }
            let max = (1u128 << bits) - 1;
            if mag > max {
                self.err(span, format!("integer literal {mag} is out of range for {name} (0..={max})"));
            }
        } else if negated {
            let min_magnitude = 1u128 << (bits - 1);
            if mag > min_magnitude {
                self.err(span, format!("integer literal -{mag} is out of range for {name} (min -{min_magnitude})"));
            }
        } else {
            let max = (1u128 << (bits - 1)) - 1;
            if mag > max {
                self.err(span, format!("integer literal {mag} is out of range for {name} (max {max})"));
            }
        }
    }

    fn check_unary(&mut self, op: UnaryOp, operand: &Expr, span: Span) -> Option<Type> {
        // A sign directly over a numeric literal is part of the literal for the
        // purpose of range checking: `-128` must be validated against a type's
        // minimum, not rejected as the out-of-range positive `128`. Intercept it
        // before the operand is checked on its own so the bound is applied once.
        if matches!(op, UnaryOp::Neg | UnaryOp::Pos) {
            if let Some(text) = bare_number_text(operand) {
                let expected = self.expected.take();
                let ty = self.number_type(text, span, expected, op == UnaryOp::Neg);
                return Some(ty);
            }
        }
        let operand_type = self.check_expr(operand)?;
        match op {
            UnaryOp::Neg | UnaryOp::Pos => {
                if is_numeric(&operand_type) {
                    return Some(operand_type);
                }
                self.err(
                    span,
                    format!("invalid operand for {}: {operand_type}", op.symbol()),
                );
                None
            }
            UnaryOp::Not => {
                if is_primitive(&operand_type, "bool") {
                    return Some(Type::Primitive("bool".to_string()));
                }
                self.err(
                    span,
                    format!("invalid operand for {}: {operand_type}", op.symbol()),
                );
                None
            }
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Option<Type> {
        // A call whose callee is a variant access (`Maybe.Some(5)`) is sum-type
        // construction, not an ordinary call: its type arguments are inferred by
        // unifying the payload slots against the arguments.
        if let ExprKind::Field { target, name } = &callee.kind {
            let expected = self.expected.take();
            if let Some(oneof @ Type::Oneof { .. }) = self.check_expr(target) {
                self.expected = expected;
                return self.check_variant_construction(&oneof, name, args, callee.span);
            }
        }
        // A bare callee (`Ok(5)`) is variant construction when the expected type is a
        // sum type owning that variant; otherwise, if it merely matches some variant,
        // there is no context to pick a type and that is a hard error.
        if let ExprKind::Ident(name) = &callee.kind {
            if let Some(oneof) = self.expected_variant_oneof(name) {
                return self.check_variant_construction(&oneof, name, args, callee.span);
            }
            if self.lookup_var(name).is_none()
                && self.globals.lookup_func(name).is_none()
                && self.any_oneof_with_variant(name).is_some()
            {
                self.err(
                    callee.span,
                    format!(
                        "cannot infer sum type for bare variant {name}; qualify it as Type.{name} or add a type annotation"
                    ),
                );
                return None;
            }
        }
        // A call whose callee names a numeric type (`i64(x)`, `f32(n)`) is an explicit
        // constructor-style conversion. Any numeric source type is accepted; the result
        // is the named type.
        if let ExprKind::Ident(name) = &callee.kind {
            if is_numeric_name(name) {
                return self.check_numeric_conversion(name, args, span);
            }
        }
        let callee_type = self.check_expr(callee)?;
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
            if !param.equals(&arg_type) {
                let (param_shown, arg_type_shown) = display_pair(param, &arg_type);
                self.err(
                    arg.span,
                    format!("argument {} type mismatch: expected {param_shown}, found {arg_type_shown}", i + 1),
                );
                return None;
            }
        }
        Some((**return_type).clone())
    }

    /// Type check an explicit numeric conversion `Type(value)`. The argument may have
    /// any numeric type; the result is the target primitive type.
    fn check_numeric_conversion(&mut self, name: &str, args: &[Expr], span: Span) -> Option<Type> {
        if args.len() != 1 {
            self.err(
                span,
                format!("conversion to {name} takes exactly one argument, found {}", args.len()),
            );
            return None;
        }
        self.expected = None;
        let arg_type = self.check_expr(&args[0])?;
        if !is_numeric(&arg_type) {
            self.err(
                args[0].span,
                format!("cannot convert value of type {arg_type} to {name}; source must be numeric"),
            );
            return None;
        }
        Some(Type::Primitive(name.to_string()))
    }

    /// Type check a bare variant access `Oneof.Variant`. A payload-free variant is a
    /// complete value of the sum type; a variant with a payload yields its
    /// constructor as a function type.
    fn check_variant_access(&mut self, oneof: &Type, variant: &str, span: Span) -> Option<Type> {
        let expected = self.expected.take();
        let Type::Oneof { id, .. } = oneof else {
            unreachable!("a variant access is on a sum type")
        };
        let name = id.name.clone();
        let type_params = self.globals.defs.type_params(oneof).to_vec();
        let Some(payload) = self.globals.defs.payload(oneof, variant) else {
            self.err(span, format!("{variant} is not a variant of sum type {name}"));
            return None;
        };
        if payload.is_empty() {
            if !type_params.is_empty() {
                let mut subst = HashMap::new();
                if !seed_subst_from_expected(oneof, expected.as_ref(), &self.globals.defs, &mut subst) {
                    self.err(
                        span,
                        format!(
                            "cannot infer type arguments for payload-free variant {name}.{variant}; an explicit annotation is required"
                        ),
                    );
                    return None;
                }
                return self.instantiate_oneof(oneof, &subst, span);
            }
            return Some(oneof.clone());
        }
        Some(Type::Func {
            return_type: Box::new(oneof.clone()),
            param_types: payload,
        })
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
        let expected = self.expected.take();
        let Type::Oneof { id, .. } = oneof else {
            unreachable!("a variant construction is on a sum type")
        };
        let name = id.name.clone();
        let type_params = self.globals.defs.type_params(oneof).to_vec();
        let Some(payload) = self.globals.defs.payload(oneof, variant) else {
            self.err(span, format!("{variant} is not a variant of sum type {name}"));
            return None;
        };
        if args.len() != payload.len() {
            self.err(
                span,
                format!(
                    "variant {name}.{variant} expects {} argument(s), got {}",
                    payload.len(),
                    args.len()
                ),
            );
            return None;
        }
        let mut subst = HashMap::new();
        seed_subst_from_expected(oneof, expected.as_ref(), &self.globals.defs, &mut subst);
        for (i, (arg, slot)) in args.iter().zip(&payload).enumerate() {
            let arg_type = self.check_expr_expecting(arg, Some(slot.substitute(&subst)))?;
            if !unify(slot, &arg_type, &mut subst) {
                let (slot_shown, arg_shown) = display_pair(&slot.substitute(&subst), &arg_type);
                self.err(
                    arg.span,
                    format!(
                        "argument {} to variant {name}.{variant} type mismatch: expected {slot_shown}, found {arg_shown}",
                        i + 1,
                    ),
                );
                return None;
            }
        }
        if type_params.is_empty() {
            return Some(oneof.clone());
        }
        self.instantiate_oneof(oneof, &subst, span)
    }

    /// Build a concrete instantiation of a generic sum type from an inferred
    /// substitution, requiring every type parameter to be determined.
    fn instantiate_oneof(
        &mut self,
        oneof: &Type,
        subst: &HashMap<String, Type>,
        span: Span,
    ) -> Option<Type> {
        let Type::Oneof { id, .. } = oneof else {
            unreachable!("only a sum type is instantiated here")
        };
        let mut type_args = Vec::new();
        for param in self.globals.defs.type_params(oneof) {
            let Some(arg) = subst.get(param) else {
                self.err(
                    span,
                    format!("cannot infer type argument {param} for sum type {}", id.name),
                );
                return None;
            };
            type_args.push(arg.clone());
        }
        Some(Type::Oneof {
            id: id.clone(),
            type_args,
        })
    }

    fn check_struct_literal(&mut self, target: &Expr, members: &[MemberInit]) -> Option<Type> {
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
        let struct_members = self.globals.defs.struct_members(&target_type).unwrap_or_default();
        // A generic struct is constructed against its template; each member
        // assignment constrains the type arguments through unification.
        let type_params = self.globals.defs.type_params(&target_type).to_vec();
        let is_generic = !type_params.is_empty() && type_args.is_empty();
        let mut subst = HashMap::new();
        let mut assigned: HashSet<String> = HashSet::new();
        for member in members {
            let Some(member_type) = struct_members.get(&member.name) else {
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
            if is_generic {
                if !unify(member_type, &value_type, &mut subst) {
                    self.err(
                        member.value.span,
                        format!(
                            "cannot assign {value_type} to member {} of generic struct {name}",
                            member.name
                        ),
                    );
                }
            } else if !member_type.equals(&value_type) {
                let (value_type_shown, member_type_shown) = display_pair(&value_type, member_type);
                self.err(
                    member.value.span,
                    format!(
                        "cannot assign {value_type_shown} to {member_type_shown} of struct member {}",
                        member.name
                    ),
                );
            }
        }
        for member_name in struct_members.keys() {
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

    fn check_field(&mut self, target: &Expr, field: &str, span: Span) -> Option<Type> {
        let expected = self.expected.take();
        let mut target_type = self.check_expr(target)?;
        // A member access whose base is a module reference selects an exported item
        // (or a deeper module), distinct from struct-field and variant access.
        if let Type::Module(prefix) = &target_type {
            let prefix = prefix.clone();
            self.expected = expected;
            return self.check_module_access(&prefix, field, span);
        }
        // A member access whose base is a sum type names a variant constructor.
        if matches!(target_type, Type::Oneof { .. }) {
            self.expected = expected;
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
                target.span,
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
        if let Some(method_type) = self.globals.lookup_method(id, field) {
            return Some(method_type.clone());
        }
        self.err(span, format!("{field} is not a member of struct {name}"));
        None
    }

    /// Index syntax is a built-in-array privilege: `a[i]` reads and `a[i] = v` writes an
    /// array element at a `u64` index. User-defined collections are not indexable and
    /// expose ordinary methods (`at`, `set`, …) instead; array method-call forms
    /// (`xs.length()`, `xs.push(v)`) go through the builtins registry via `check_field`.
    fn check_index(&mut self, array: &Expr, index: &Expr) -> Option<Type> {
        let array_type = self.check_expr(array)?;
        // Index syntax is a built-in-array privilege: `a[i]` is the blessed read
        // `a.get(i)`, with a `u64` index. User-defined collections are not indexable —
        // they expose ordinary methods (`at`, `set`, …), visibly userspace.
        let Type::Array(elem) = &array_type else {
            self.err(array.span, format!("type {array_type} cannot be indexed"));
            return None;
        };
        let element = (**elem).clone();
        let want = crate::builtins::index_type();
        self.expected = Some(want.clone());
        let index_type = self.check_expr(index)?;
        if !index_type.equals(&want) {
            self.err(index.span, format!("index must be {want}, found {index_type}"));
            return None;
        }
        Some(element)
    }

    fn check_address_of(&mut self, operand: &Expr, span: Span) -> Option<Type> {
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

    fn check_deref(&mut self, operand: &Expr) -> Option<Type> {
        let operand_type = self.check_expr(operand)?;
        let Type::Pointer(elem) = &operand_type else {
            self.err(
                operand.span,
                format!("cannot dereference non-pointer type {operand_type}"),
            );
            return None;
        };
        Some((**elem).clone())
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

    fn check_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr) -> Option<Type> {
        // `a[i] = v` writes an array element through the built-in index sugar; only a
        // built-in array is index-assignable.
        if let ExprKind::Index { array, index } = &target.kind {
            return self.check_index_assign(op, array, index, value, target.span);
        }
        let target_type = self.check_expr(target)?;
        let value_type = self.check_expr(value)?;
        self.check_assign_op(op, &target_type, &value_type, target.span);
        Some(target_type)
    }

    /// Check an index assignment `a[i] <op>= v`: the receiver must be a built-in array,
    /// the index is a `u64`, and any compound operator relates the element and the value.
    fn check_index_assign(
        &mut self,
        op: AssignOp,
        array: &Expr,
        index: &Expr,
        value: &Expr,
        span: Span,
    ) -> Option<Type> {
        let recv = self.check_expr(array)?;
        let Type::Array(elem) = &recv else {
            self.err(span, format!("type {recv} cannot be assigned by index"));
            return None;
        };
        let element = (**elem).clone();
        let want = crate::builtins::index_type();
        self.expected = Some(want.clone());
        let index_type = self.check_expr(index)?;
        if !index_type.equals(&want) {
            self.err(index.span, format!("index must be {want}, found {index_type}"));
            return None;
        }
        self.expected = Some(element.clone());
        let value_type = self.check_expr(value)?;
        self.check_assign_op(op, &element, &value_type, span);
        Some(element)
    }

    /// Apply an assignment operator's type rule, reporting a diagnostic on mismatch: a
    /// plain assignment requires equal types, a compound `+=` two numbers or two
    /// strings, and the other compound operators two numbers.
    fn check_assign_op(&mut self, op: AssignOp, target_type: &Type, value_type: &Type, span: Span) {
        match op {
            AssignOp::Assign => {
                if !target_type.equals(value_type) {
                    let (value_type_shown, target_type_shown) = display_pair(value_type, target_type);
                    self.err(span, format!("cannot assign {value_type_shown} to {target_type_shown}"));
                }
            }
            AssignOp::Add => {
                let numeric = is_numeric(target_type) && is_numeric(value_type);
                let strings = is_primitive(target_type, "string") && is_primitive(value_type, "string");
                if !numeric && !strings {
                    let (target_type_shown, value_type_shown) = display_pair(target_type, value_type);
                    self.err(
                        span,
                        format!("invalid operands for {}: {target_type_shown} and {value_type_shown}", op.symbol()),
                    );
                }
            }
            AssignOp::Sub | AssignOp::Mul | AssignOp::Div => {
                if !(is_numeric(target_type) && is_numeric(value_type)) {
                    let (target_type_shown, value_type_shown) = display_pair(target_type, value_type);
                    self.err(
                        span,
                        format!("invalid operands for {}: {target_type_shown} and {value_type_shown}", op.symbol()),
                    );
                }
            }
        }
    }

    fn check_let_tuple(
        &mut self,
        names: &[String],
        ty: &Option<TypeExpr>,
        value: &Expr,
    ) -> Option<Type> {
        let declared = match ty {
            Some(ty) => match self.resolve_local_type(ty) {
                Some(Type::Tuple(elems)) => Some(elems),
                Some(other) => {
                    self.err(ty.span, format!("cannot destructure into non-tuple type {other}"));
                    return None;
                }
                None => return None,
            },
            None => None,
        };
        let rhs_type = self.check_expr(value)?;
        let Type::Tuple(elem_types) = &rhs_type else {
            self.err(
                value.span,
                format!("cannot destructure non-tuple value of type {rhs_type}"),
            );
            return None;
        };
        if elem_types.len() != names.len() {
            self.err(
                value.span,
                format!(
                    "destructuring pattern binds {} names but value has {} elements",
                    names.len(),
                    elem_types.len()
                ),
            );
            return None;
        }
        for (i, name) in names.iter().enumerate() {
            let elem_type = elem_types[i].clone();
            if let Some(declared) = &declared {
                let declared_elem = &declared[i];
                if !declared_elem.equals(&elem_type) {
                    let (declared_elem_shown, elem_type_shown) = display_pair(declared_elem, &elem_type);
                    self.err(
                        value.span,
                        format!("type mismatch: {name} declared as {declared_elem_shown} but bound to {elem_type_shown}"),
                    );
                }
                self.define_var(name, declared_elem.clone());
            } else {
                self.define_var(name, elem_type);
            }
        }
        Some(rhs_type)
    }

    fn check_if_expr(&mut self, cond: &Expr, then: &Expr, els: &Expr) -> Option<Type> {
        let cond_type = self.check_expr(cond);
        if !matches!(cond_type, Some(t) if is_primitive(&t, "bool")) {
            self.err(cond.span, "if-expression condition does not evaluate to a boolean type");
        }
        let then_type = self.check_expr(then)?;
        let else_type = self.check_expr(els)?;
        if !then_type.equals(&else_type) {
            let (then_type_shown, else_type_shown) = display_pair(&then_type, &else_type);
            self.err(
                then.span.to(els.span),
                format!("if-expression branches have mismatched types: {then_type_shown} and {else_type_shown}"),
            );
            return None;
        }
        Some(then_type)
    }

    fn check_block_expr(&mut self, block: &Block) -> Option<Type> {
        self.scopes.push(HashMap::new());
        for stmt in &block.statements {
            self.check_stmt(stmt);
        }
        let result = self.check_expr(&block.result);
        self.scopes.pop();
        result
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
#[derive(Default)]
struct Coverage {
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

/// Whether an integer type is signed (admits negative literal patterns).
fn is_signed_int(name: &str) -> bool {
    SIGNED_INTS.contains(&name)
}

/// The type of a numeric literal. A literal's lexical form fixes its class — a `.`
/// or exponent (outside a hex/binary prefix) makes it floating-point — and an
/// `expected` type of the matching class fixes its width: an integer literal adopts
/// any expected numeric type (so `f32 = 5` holds), a float literal only an expected
/// float type. Absent a usable hint, the class default (`i32` or `f32`) applies.
fn number_literal_type(text: &str, expected: Option<&Type>) -> Type {
    let is_float = literal_is_float(text);
    if let Some(Type::Primitive(name)) = expected {
        let adopts = if is_float {
            is_float_name(name)
        } else {
            is_numeric_name(name)
        };
        if adopts {
            return Type::Primitive(name.clone());
        }
    }
    let default = if is_float { DEFAULT_FLOAT } else { DEFAULT_INT };
    Type::Primitive(default.to_string())
}

/// Whether a numeric literal is floating-point. Hexadecimal and binary literals are
/// always integers; a decimal literal is floating-point when it carries a fractional
/// point or a decimal exponent.
fn literal_is_float(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("0x") || lower.starts_with("0b") {
        return false;
    }
    lower.contains('.') || lower.contains('e')
}

/// Whether `expr` is a bare numeric literal, seen through a grouping or a leading
/// sign — the shapes whose width may be borrowed from the opposite operand of a
/// binary operator.
fn is_numeric_literal(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Number(_) => true,
        ExprKind::Group(inner) => is_numeric_literal(inner),
        ExprKind::Unary {
            op: UnaryOp::Neg | UnaryOp::Pos,
            operand,
        } => is_numeric_literal(operand),
        _ => false,
    }
}

/// The bare integer magnitude of a numeric literal, ignoring digit separators and
/// respecting a hexadecimal or binary prefix. `None` when the value overflows 128
/// bits — beyond any Cooper integer type, so already out of range.
fn parse_literal_magnitude(text: &str) -> Option<u128> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let lower = cleaned.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        u128::from_str_radix(hex, 16).ok()
    } else if let Some(bin) = lower.strip_prefix("0b") {
        u128::from_str_radix(bin, 2).ok()
    } else {
        cleaned.parse::<u128>().ok()
    }
}

/// A numeric literal's decoded value: a non-negative integer magnitude (any sign is
/// a surrounding unary operator, not part of the literal) or a floating-point value.
/// The literal's resolved type, carried on the IR node, fixes its width and signedness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LiteralValue {
    Int(u128),
    Float(f64),
}

/// Decode a numeric literal's source spelling into its value, classifying it by the
/// same lexical rule the checker applies (a fractional point or decimal exponent
/// makes it floating-point). `None` only for text the checker would already have
/// rejected — an integer magnitude beyond 128 bits or an unparsable float — so a
/// clean check guarantees `Some`. Shared with lowering so literal spelling is
/// interpreted in exactly one place.
pub fn decode_number_literal(text: &str) -> Option<LiteralValue> {
    if literal_is_float(text) {
        let cleaned: String = text.chars().filter(|c| *c != '_').collect();
        cleaned.parse::<f64>().ok().map(LiteralValue::Float)
    } else {
        parse_literal_magnitude(text).map(LiteralValue::Int)
    }
}

/// The bit width of an integer type by name.
fn integer_bits(name: &str) -> u32 {
    match name {
        "i8" | "u8" => 8,
        "i16" | "u16" => 16,
        "i32" | "u32" => 32,
        "i64" | "u64" => 64,
        _ => unreachable!("not an integer type: {name}"),
    }
}

/// The literal text of a bare numeric literal seen through groupings, or `None` for
/// any other expression — the shape a leading sign folds into for range checking.
/// Whether `expr` is a floating-point numeric literal, seeing through grouping and a
/// leading sign.
fn is_float_literal(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Number(text) => literal_is_float(text),
        ExprKind::Group(inner) => is_float_literal(inner),
        ExprKind::Unary {
            op: UnaryOp::Neg | UnaryOp::Pos,
            operand,
        } => is_float_literal(operand),
        _ => false,
    }
}

fn bare_number_text(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Number(text) => Some(text),
        ExprKind::Group(inner) => bare_number_text(inner),
        _ => None,
    }
}

/// Fill `subst` with type arguments taken from an expected sum-type instantiation,
/// letting a generic variant construction obtain the arguments its payload cannot
/// determine. Returns true when `expected` is a matching instantiation of the same
/// generic sum type carrying concrete type arguments.
fn seed_subst_from_expected(
    oneof: &Type,
    expected: Option<&Type>,
    defs: &TypeDefs,
    subst: &mut HashMap<String, Type>,
) -> bool {
    let (Type::Oneof { id, .. }, Some(Type::Oneof { id: exp_id, type_args })) = (oneof, expected)
    else {
        return false;
    };
    let type_params = defs.type_params(oneof);
    if id != exp_id || type_args.len() != type_params.len() {
        return false;
    }
    for (param, arg) in type_params.iter().zip(type_args) {
        subst.insert(param.clone(), arg.clone());
    }
    true
}
