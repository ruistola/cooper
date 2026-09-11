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
use crate::types::{is_numeric, is_primitive, is_unit, unify, Type};

/// Type check `module` against `globals`, returning any diagnostics.
pub fn check(module: &[Stmt], globals: &Globals) -> Vec<Diagnostic> {
    let mut tc = TypeChecker::new(globals);
    tc.scopes.push(HashMap::new());
    for stmt in module {
        tc.check_stmt(stmt);
    }
    tc.diags
}

struct TypeChecker<'g> {
    globals: &'g Globals,
    diags: Vec<Diagnostic>,
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
}

impl<'g> TypeChecker<'g> {
    fn new(globals: &'g Globals) -> Self {
        TypeChecker {
            globals,
            diags: Vec::new(),
            scopes: Vec::new(),
            current_return: None,
            type_params: HashSet::new(),
            expected: None,
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
            StmtKind::StructDecl { .. } | StmtKind::OneofDecl { .. } | StmtKind::Use(_) => {}
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
            StmtKind::For {
                init,
                cond,
                iter,
                body,
            } => {
                self.check_stmt(init);
                let cond_type = self.check_expr(cond);
                if !matches!(cond_type, Some(t) if is_primitive(&t, "bool")) {
                    self.err(cond.span, "for-statement condition does not evaluate to a boolean type");
                }
                self.check_expr(iter);
                self.scopes.push(HashMap::new());
                for s in body {
                    self.check_stmt(s);
                }
                self.scopes.pop();
            }
            StmtKind::Return(expr) => self.check_return(expr.as_ref(), stmt.span),
            StmtKind::Expression(expr) => {
                self.check_expr(expr);
            }
        }
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
                    self.err(
                        span,
                        format!(
                            "type mismatch: variable {name} declared as {declared} but initialized with {value}"
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
            self.err(
                expr.span,
                format!("return type mismatch: expected {return_type}, found {expr_type}"),
            );
        }
    }

    // --- match ---

    fn check_match_stmt(&mut self, scrutinee: &Expr, arms: &[StmtArm]) {
        let Some(oneof) = self.check_scrutinee(scrutinee) else {
            return;
        };
        let (name, variants, order) = oneof_parts(&oneof);
        let mut covered = HashSet::new();
        let mut has_wildcard = false;
        for arm in arms {
            self.scopes.push(HashMap::new());
            match self.check_arm_pattern(&name, &variants, &arm.pattern) {
                ArmCover::Wildcard => has_wildcard = true,
                ArmCover::Variant(v) => {
                    covered.insert(v);
                }
                ArmCover::None => {}
            }
            self.check_stmt(&arm.body);
            self.scopes.pop();
        }
        self.check_exhaustiveness(&name, &order, &covered, has_wildcard, scrutinee.span);
    }

    fn check_match_expr(&mut self, scrutinee: &Expr, arms: &[ExprArm]) -> Option<Type> {
        let oneof = self.check_scrutinee(scrutinee)?;
        let (name, variants, order) = oneof_parts(&oneof);
        let mut covered = HashSet::new();
        let mut has_wildcard = false;
        let mut match_type: Option<Type> = None;
        for arm in arms {
            self.scopes.push(HashMap::new());
            match self.check_arm_pattern(&name, &variants, &arm.pattern) {
                ArmCover::Wildcard => has_wildcard = true,
                ArmCover::Variant(v) => {
                    covered.insert(v);
                }
                ArmCover::None => {}
            }
            let arm_type = self.check_expr(&arm.body);
            self.scopes.pop();
            if let Some(arm_type) = arm_type {
                match &match_type {
                    None => match_type = Some(arm_type),
                    Some(prev) if !prev.equals(&arm_type) => self.err(
                        arm.body.span,
                        format!("match arms have mismatched types: {prev} and {arm_type}"),
                    ),
                    _ => {}
                }
            }
        }
        self.check_exhaustiveness(&name, &order, &covered, has_wildcard, scrutinee.span);
        Some(match_type.unwrap_or(Type::Unit))
    }

    /// Check a match scrutinee and require it to be a sum type, returning that type.
    fn check_scrutinee(&mut self, scrutinee: &Expr) -> Option<Type> {
        let ty = self.check_expr(scrutinee)?;
        if matches!(ty, Type::Oneof { .. }) {
            Some(ty)
        } else {
            self.err(
                scrutinee.span,
                format!("cannot match on non-sum-type value of type {ty}"),
            );
            None
        }
    }

    /// Validate an arm pattern against the scrutinee's sum type and bind its
    /// payload slots into the current (arm) scope.
    fn check_arm_pattern(
        &mut self,
        oneof_name: &str,
        variants: &HashMap<String, Vec<Type>>,
        pattern: &Pattern,
    ) -> ArmCover {
        match &pattern.kind {
            PatternKind::Wildcard => ArmCover::Wildcard,
            PatternKind::Variant {
                type_name,
                variant,
                binders,
            } => {
                if type_name != oneof_name {
                    self.err(
                        pattern.span,
                        format!(
                            "pattern names sum type {type_name} but the scrutinee has type {oneof_name}"
                        ),
                    );
                    return ArmCover::None;
                }
                let Some(payload) = variants.get(variant).cloned() else {
                    self.err(
                        pattern.span,
                        format!("{variant} is not a variant of sum type {oneof_name}"),
                    );
                    return ArmCover::None;
                };
                if binders.len() != payload.len() {
                    self.err(
                        pattern.span,
                        format!(
                            "variant pattern {oneof_name}.{variant} binds {} slot(s) but the variant has {}",
                            binders.len(),
                            payload.len()
                        ),
                    );
                    return ArmCover::Variant(variant.clone());
                }
                for (binder, slot) in binders.iter().zip(payload) {
                    if binder != "_" {
                        self.define_var(binder, slot);
                    }
                }
                ArmCover::Variant(variant.clone())
            }
        }
    }

    fn check_exhaustiveness(
        &mut self,
        name: &str,
        order: &[String],
        covered: &HashSet<String>,
        has_wildcard: bool,
        span: Span,
    ) {
        if has_wildcard {
            return;
        }
        let missing: Vec<&str> = order
            .iter()
            .filter(|v| !covered.contains(*v))
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

    // --- expressions ---

    fn check_expr(&mut self, expr: &Expr) -> Option<Type> {
        // expectedType is a head-only hint: capture it for this expression and
        // clear it so it never leaks into sub-expressions. Only the dispatches that
        // can act on it restore it before recursing.
        let expected = self.expected.take();
        match &expr.kind {
            ExprKind::Number(_) => Some(Type::Primitive("i32".to_string())),
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
            ExprKind::Ident(name) => self.check_ident(name, expr.span),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, expr.span),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, expr.span),
            ExprKind::Group(inner) => self.check_expr(inner),
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
            return Some(t.clone());
        }
        if let Some(t) = self.globals.lookup_oneof(name) {
            return Some(t.clone());
        }
        if let Some(t) = self.globals.lookup_func(name) {
            return Some(t.clone());
        }
        self.err(span, format!("undefined variable: {name}"));
        None
    }

    fn check_binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, span: Span) -> Option<Type> {
        let left = self.check_expr(lhs)?;
        let right = self.check_expr(rhs)?;
        match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                if is_numeric(&left) && is_numeric(&right) {
                    return Some(left);
                }
                if op == BinaryOp::Add
                    && is_primitive(&left, "string")
                    && is_primitive(&right, "string")
                {
                    return Some(Type::Primitive("string".to_string()));
                }
                self.err(
                    span,
                    format!("invalid operands for {}: {left} and {right}", op.symbol()),
                );
                None
            }
            BinaryOp::Eq | BinaryOp::Ne => {
                if !left.equals(&right) {
                    self.err(span, format!("cannot compare {left} and {right}"));
                    return None;
                }
                Some(Type::Primitive("bool".to_string()))
            }
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                if is_numeric(&left) && is_numeric(&right) {
                    return Some(Type::Primitive("bool".to_string()));
                }
                self.err(
                    span,
                    format!("invalid operands for {}: {left} and {right}", op.symbol()),
                );
                None
            }
            BinaryOp::And | BinaryOp::Or => {
                if is_primitive(&left, "bool") && is_primitive(&right, "bool") {
                    return Some(Type::Primitive("bool".to_string()));
                }
                self.err(
                    span,
                    format!("invalid operands for {}: {left} and {right}", op.symbol()),
                );
                None
            }
        }
    }

    fn check_unary(&mut self, op: UnaryOp, operand: &Expr, span: Span) -> Option<Type> {
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
        for (i, (arg, expected)) in args.iter().zip(param_types).enumerate() {
            let arg_type = self.check_expr(arg)?;
            if !expected.equals(&arg_type) {
                self.err(
                    arg.span,
                    format!("argument {} type mismatch: expected {expected}, found {arg_type}", i + 1),
                );
                return None;
            }
        }
        Some((**return_type).clone())
    }

    /// Type check a bare variant access `Oneof.Variant`. A payload-free variant is a
    /// complete value of the sum type; a variant with a payload yields its
    /// constructor as a function type.
    fn check_variant_access(&mut self, oneof: &Type, variant: &str, span: Span) -> Option<Type> {
        let expected = self.expected.take();
        let (name, variants, type_params) = match oneof {
            Type::Oneof {
                name,
                variants,
                type_params,
                ..
            } => (name.clone(), variants, type_params.clone()),
            _ => unreachable!(),
        };
        let Some(payload) = variants.get(variant).cloned() else {
            self.err(span, format!("{variant} is not a variant of sum type {name}"));
            return None;
        };
        if payload.is_empty() {
            if !type_params.is_empty() {
                let mut subst = HashMap::new();
                if !seed_subst_from_expected(oneof, expected.as_ref(), &mut subst) {
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
        let (name, variants, type_params) = match oneof {
            Type::Oneof {
                name,
                variants,
                type_params,
                ..
            } => (name.clone(), variants.clone(), type_params.clone()),
            _ => unreachable!(),
        };
        let Some(payload) = variants.get(variant).cloned() else {
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
        seed_subst_from_expected(oneof, expected.as_ref(), &mut subst);
        for (i, (arg, slot)) in args.iter().zip(&payload).enumerate() {
            let arg_type = self.check_expr(arg)?;
            if !unify(slot, &arg_type, &mut subst) {
                self.err(
                    arg.span,
                    format!(
                        "argument {} to variant {name}.{variant} type mismatch: expected {}, found {arg_type}",
                        i + 1,
                        slot.substitute(&subst)
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
        let Type::Oneof {
            name,
            variants,
            variant_order,
            type_params,
            ..
        } = oneof
        else {
            unreachable!()
        };
        let mut type_args = Vec::with_capacity(type_params.len());
        for param in type_params {
            let Some(arg) = subst.get(param) else {
                self.err(
                    span,
                    format!("cannot infer type argument {param} for sum type {name}"),
                );
                return None;
            };
            type_args.push(arg.clone());
        }
        Some(Type::Oneof {
            name: name.clone(),
            variants: variants
                .iter()
                .map(|(k, slots)| (k.clone(), slots.iter().map(|s| s.substitute(subst)).collect()))
                .collect(),
            variant_order: variant_order.clone(),
            type_params: type_params.clone(),
            type_args,
        })
    }

    fn check_struct_literal(&mut self, target: &Expr, members: &[MemberInit]) -> Option<Type> {
        let target_type = self.check_expr(target)?;
        let Type::Struct {
            name,
            members: struct_members,
            type_params,
            type_args,
        } = &target_type
        else {
            self.err(
                target.span,
                format!("expression of type {target_type} cannot be used as a struct"),
            );
            return None;
        };
        let struct_members = struct_members.clone();
        let name = name.clone();
        // A generic struct is constructed against its template; each member
        // assignment constrains the type arguments through unification.
        let is_generic = !type_params.is_empty() && type_args.is_empty();
        let type_params = type_params.clone();
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
            let Some(value_type) = self.check_expr(&member.value) else {
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
                self.err(
                    member.value.span,
                    format!(
                        "cannot assign {value_type} to {member_type} of struct member {}",
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
            return self.instantiate_struct(&name, &struct_members, &type_params, &subst, target.span);
        }
        Some(target_type)
    }

    fn instantiate_struct(
        &mut self,
        name: &str,
        members: &HashMap<String, Type>,
        type_params: &[String],
        subst: &HashMap<String, Type>,
        span: Span,
    ) -> Option<Type> {
        let mut type_args = Vec::with_capacity(type_params.len());
        for param in type_params {
            let Some(arg) = subst.get(param) else {
                self.err(
                    span,
                    format!("cannot infer type argument {param} for generic struct {name}"),
                );
                return None;
            };
            type_args.push(arg.clone());
        }
        Some(Type::Struct {
            name: name.to_string(),
            members: members
                .iter()
                .map(|(k, v)| (k.clone(), v.substitute(subst)))
                .collect(),
            type_params: type_params.to_vec(),
            type_args,
        })
    }

    fn check_field(&mut self, target: &Expr, field: &str, span: Span) -> Option<Type> {
        let expected = self.expected.take();
        let mut target_type = self.check_expr(target)?;
        // A member access whose base is a sum type names a variant constructor.
        if matches!(target_type, Type::Oneof { .. }) {
            self.expected = expected;
            return self.check_variant_access(&target_type, field, span);
        }
        // Auto-deref: `p.field` works transparently through a pointer-to-struct.
        if let Type::Pointer(elem) = target_type {
            target_type = *elem;
        }
        let Type::Struct { name, members, .. } = &target_type else {
            self.err(
                target.span,
                format!("expression of type {target_type} cannot be used as a struct"),
            );
            return None;
        };
        if let Some(member_type) = members.get(field) {
            return Some(member_type.clone());
        }
        // Not a data field: fall back to a method on this struct type. A bound
        // method has its receiver stripped, so it is usable wherever a matching
        // function type is expected.
        if let Some(method_type) = self.globals.lookup_method(name, field) {
            return Some(method_type.clone());
        }
        self.err(span, format!("{field} is not a member of struct {name}"));
        None
    }

    fn check_index(&mut self, array: &Expr, index: &Expr) -> Option<Type> {
        let index_type = self.check_expr(index)?;
        if !is_numeric(&index_type) {
            self.err(
                index.span,
                "array index expression does not result in a numeric type",
            );
            return None;
        }
        let array_type = self.check_expr(array)?;
        let Type::Array(elem) = &array_type else {
            self.err(array.span, format!("cannot index non-array type {array_type}"));
            return None;
        };
        Some((**elem).clone())
    }

    fn check_address_of(&mut self, operand: &Expr, span: Span) -> Option<Type> {
        if !self.is_addressable(operand) {
            self.err(span, "cannot take the address of a non-addressable expression");
            return None;
        }
        let operand_type = self.check_expr(operand)?;
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
    /// Temporaries (literals, call results, arithmetic) are not addressable.
    fn is_addressable(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Ident(name) => self.lookup_var(name).is_some(),
            ExprKind::Field { .. } | ExprKind::Index { .. } | ExprKind::Deref(_) => true,
            ExprKind::Group(inner) => self.is_addressable(inner),
            _ => false,
        }
    }

    fn check_assign(&mut self, op: AssignOp, target: &Expr, value: &Expr) -> Option<Type> {
        let target_type = self.check_expr(target)?;
        let value_type = self.check_expr(value)?;
        match op {
            AssignOp::Assign => {
                if !target_type.equals(&value_type) {
                    self.err(
                        target.span,
                        format!("cannot assign {value_type} to {target_type}"),
                    );
                }
            }
            AssignOp::Add => {
                let numeric = is_numeric(&target_type) && is_numeric(&value_type);
                let strings = is_primitive(&target_type, "string") && is_primitive(&value_type, "string");
                if !numeric && !strings {
                    self.err(
                        target.span,
                        format!("invalid operands for {}: {target_type} and {value_type}", op.symbol()),
                    );
                }
            }
            AssignOp::Sub | AssignOp::Mul | AssignOp::Div => {
                if !(is_numeric(&target_type) && is_numeric(&value_type)) {
                    self.err(
                        target.span,
                        format!("invalid operands for {}: {target_type} and {value_type}", op.symbol()),
                    );
                }
            }
        }
        Some(target_type)
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
                    self.err(
                        value.span,
                        format!("type mismatch: {name} declared as {declared_elem} but bound to {elem_type}"),
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
            self.err(
                then.span.to(els.span),
                format!("if-expression branches have mismatched types: {then_type} and {else_type}"),
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

/// The coverage a single match arm contributes.
enum ArmCover {
    Wildcard,
    Variant(String),
    None,
}

/// Extract the name, variant table, and declaration order from a sum type.
fn oneof_parts(oneof: &Type) -> (String, HashMap<String, Vec<Type>>, Vec<String>) {
    match oneof {
        Type::Oneof {
            name,
            variants,
            variant_order,
            ..
        } => (name.clone(), variants.clone(), variant_order.clone()),
        _ => unreachable!("check_scrutinee guarantees a sum type"),
    }
}

/// Fill `subst` with type arguments taken from an expected sum-type instantiation,
/// letting a generic variant construction obtain the arguments its payload cannot
/// determine. Returns true when `expected` is a matching instantiation of the same
/// generic sum type carrying concrete type arguments.
fn seed_subst_from_expected(
    oneof: &Type,
    expected: Option<&Type>,
    subst: &mut HashMap<String, Type>,
) -> bool {
    let (Type::Oneof { name, type_params, .. }, Some(Type::Oneof { name: exp_name, type_args, .. })) =
        (oneof, expected)
    else {
        return false;
    };
    if name != exp_name || type_args.len() != type_params.len() {
        return false;
    }
    for (param, arg) in type_params.iter().zip(type_args) {
        subst.insert(param.clone(), arg.clone());
    }
    true
}
