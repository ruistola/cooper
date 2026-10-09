//! Type checking: the second semantic pass.
//!
//! Walking function and module bodies against the [`Globals`] table produced by
//! [`crate::resolve`], this pass assigns a [`Type`] to every expression and reports
//! mismatches. It keeps its own stack of block-scoped variable bindings; structs,
//! sum types, functions, and methods live in the global table. Undefined-variable
//! detection falls out naturally from identifier lookup here.
//!
//! This module holds the checker's state, statements, and core expressions. Its
//! submodules extend `TypeChecker` by responsibility: `solve` (obligations,
//! predicates, body completion), `patterns` (`match` and exhaustiveness), `literals`
//! (numeric literals), `calls` (calls, references, variants), and `access` (struct
//! construction, members, elements, assignment).

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::infer::{InferKind, InferTable};
use crate::resolve::{self, Callable, Globals, Signature};
use crate::types::{
    display_pair, incomparable_part, integer_bits, is_integer, is_integer_name,
    is_numeric, is_numeric_name, is_primitive, is_unit, InferId, Type, TypeId, SIGNED_INTS,
};

mod access;
mod calls;
mod literals;
mod patterns;
mod solve;

pub use literals::{decode_number_literal, decode_string_literal, LiteralValue};
use literals::{bare_number_text, is_signed_int, literal_is_float};
use patterns::Coverage;

/// The result of type checking one file: its diagnostics, the type attributed to
/// every expression, and the declaration every function or method reference names,
/// each keyed by span. The span tables are the lowering pass's handoff — `cooper-ir`
/// reads them so no type or name need be recomputed downstream. Spans are unique per
/// expression within a file (they are disjoint byte ranges), so the key is exact.
pub struct Typed {
    pub diags: Vec<Diagnostic>,
    pub types: HashMap<Span, Type>,
    pub refs: HashMap<Span, ItemRef>,
    /// The variables each function literal or nested function captures, keyed by its
    /// span: every enclosing function's local its body (or a function nested in it)
    /// names, with its type, in order of first use.
    pub captures: HashMap<Span, Vec<(String, Type)>>,
}

/// A function literal or nested function whose body is being checked.
struct Closure {
    span: Span,
    /// The index of the literal's own outermost scope: a variable found below it is
    /// captured.
    base: usize,
    captures: Vec<(String, Type)>,
    /// Whether a `return` in the body carries a value.
    returns_value: bool,
}

/// A reference to a function or method declaration, instantiated: the declaration's
/// identity and one type argument per binder of its [`Signature`], in order. A
/// non-generic declaration takes none. Inside a generic body the arguments may name
/// the enclosing declaration's own type parameters.
#[derive(Debug, Clone)]
pub struct ItemRef {
    pub item: Callable,
    pub type_args: Vec<Type>,
}

/// A function or method reference awaiting the body's resolved type arguments.
struct PendingRef {
    span: Span,
    /// How diagnostics name the declaration (`function id`, `method map`).
    label: String,
    item: Callable,
    /// Each binder's name and the inference variable for its type argument.
    type_args: Vec<(String, Type)>,
}

#[derive(Clone, Copy)]
enum PredicateKind {
    Numeric,
    Addable,
    Integer,
    /// `bool` or an integer: the operand of `and`, `or`, `xor`, and `!`.
    Logical,
    Comparable,
    Formattable,
}

enum PredicateSite {
    Binary(BinaryOp, Type),
    Unary(UnaryOp),
    Assignment(AssignOp, Type),
    Conversion(String),
    Index,
    ShiftCount,
    Range,
    IntegerPattern { negative: bool, magnitude: String },
}

struct Predicate {
    kind: PredicateKind,
    ty: Type,
    span: Span,
    site: PredicateSite,
}

struct Literal {
    ty: Type,
    span: Span,
    text: String,
    negated: bool,
}

enum ObligationKind {
    Field { name: String, result: Type, target_span: Span },
    Variant { name: String, result: Type, payload: Option<Vec<(Type, Span)>>, bare: bool },
    Pattern { pattern: Pattern, binders: HashMap<String, Type> },
    Exhaustive(Coverage),
}

struct Obligation {
    subject: Type,
    span: Span,
    kind: ObligationKind,
}

struct DeferredCall {
    span: Span,
    args: Vec<(Type, Span)>,
}

#[derive(Default)]
struct BodyConstraints {
    predicates: Vec<Predicate>,
    literals: Vec<Literal>,
    references: HashMap<Span, PendingRef>,
    obligations: Vec<Obligation>,
    calls: HashMap<Span, DeferredCall>,
    reported: HashSet<InferId>,
    spans: Vec<Span>,
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
    tc.finish_body();
    Typed {
        diags: tc.diags,
        types: tc.types,
        refs: tc.refs,
        captures: tc.captures,
    }
}

struct TypeChecker<'g> {
    globals: &'g Globals,
    /// Bound module interfaces, keyed by local spelling (`["std", "io"]`).
    modules: &'g HashMap<Vec<String>, Globals>,
    diags: Vec<Diagnostic>,
    /// Every expression's attributed type, keyed by span: the lowering handoff.
    types: HashMap<Span, Type>,
    /// Expressions that failed to check, so a second check reports nothing again.
    failed: HashSet<Span>,
    /// Block-scoped variable bindings, innermost scope last.
    scopes: Vec<HashMap<String, Type>>,
    /// The return type of the function whose body is being checked, if any.
    current_return: Option<Type>,
    /// The type parameters in scope for the current function's local annotations.
    type_params: HashSet<String>,
    /// A head-only subject hint for bare variants, supplied by declarations,
    /// returns, and call parameters. Other inference uses equality constraints.
    expected: Option<Type>,
    /// How many loop bodies enclose the statement being checked, so `break` and
    /// `continue` outside any loop can be rejected.
    loop_depth: u32,
    /// Every function and method reference, keyed by span: the lowering handoff.
    refs: HashMap<Span, ItemRef>,
    /// The function literals enclosing the expression being checked, innermost last.
    closures: Vec<Closure>,
    /// Every checked closure's captures, keyed by span: the lowering handoff.
    captures: HashMap<Span, Vec<(String, Type)>>,
    /// The variables and bindings used to instantiate generic references.
    infer: InferTable,
    body: BodyConstraints,
}

impl<'g> TypeChecker<'g> {
    fn new(globals: &'g Globals, modules: &'g HashMap<Vec<String>, Globals>) -> Self {
        TypeChecker {
            globals,
            modules,
            diags: Vec::new(),
            types: HashMap::new(),
            failed: HashSet::new(),
            scopes: Vec::new(),
            current_return: None,
            type_params: HashSet::new(),
            expected: None,
            loop_depth: 0,
            refs: HashMap::new(),
            closures: Vec::new(),
            captures: HashMap::new(),
            infer: InferTable::default(),
            body: BodyConstraints::default(),
        }
    }

    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(span, msg.into()));
    }

    /// Types in an error use literal defaults without binding the live variables.
    fn shown_pair(&self, left: &Type, right: &Type) -> (String, String) {
        let mut view = self.infer.clone();
        view.default_literals();
        display_pair(&view.resolve(left), &view.resolve(right))
    }

    fn shown_type(&self, ty: &Type) -> Type {
        let mut view = self.infer.clone();
        view.default_literals();
        view.resolve(ty)
    }

    fn predicate(&mut self, kind: PredicateKind, ty: Type, span: Span, site: PredicateSite) {
        self.body.predicates.push(Predicate { kind, ty, span, site });
    }

    fn fresh_subst(&mut self, params: &[String]) -> HashMap<String, Type> {
        params.iter()
            .map(|param| (param.clone(), self.infer.fresh(InferKind::General)))
            .collect()
    }

    fn infer_binding(&mut self, ty: &Type) -> Type {
        let bound = self.infer.fresh(InferKind::General);
        self.infer.unify(&bound, ty).expect("a fresh binding fits its initializer");
        bound
    }

    fn array_element(&mut self, ty: &Type) -> Option<Type> {
        let elem = self.infer.fresh(InferKind::General);
        self.infer.unify(ty, &Type::Array(Box::new(elem.clone()))).ok()?;
        Some(elem)
    }

    // --- variable scope stack ---

    fn define_var(&mut self, name: &str, ty: Type) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }

    fn lookup_var(&self, name: &str) -> Option<&Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    /// Look up the variable `name` as a use: one bound in an enclosing function's
    /// body (not at module level) outside a closure being checked is captured by that
    /// closure, and by every closure between it and the use.
    fn use_var(&mut self, name: &str) -> Option<Type> {
        let (depth, ty) = self
            .scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(depth, scope)| scope.get(name).map(|ty| (depth, ty.clone())))?;
        if depth > 0 {
            for closure in self.closures.iter_mut().filter(|c| c.base > depth) {
                if !closure.captures.iter().any(|(captured, _)| captured == name) {
                    closure.captures.push((name.to_string(), ty.clone()));
                }
            }
        }
        Some(ty)
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
            StmtKind::FuncDecl(func) if self.current_return.is_some() => self.check_nested_func(func),
            StmtKind::FuncDecl(func) => self.check_func_decl(func),
            StmtKind::If { cond, then, els } => {
                let cond_type = self.check_expr(cond);
                if !cond_type.is_some_and(|ty| self.infer.unify(&ty, &Type::Primitive("bool".to_string())).is_ok()) {
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
                if !cond_type.is_some_and(|ty| self.infer.unify(&ty, &Type::Primitive("bool".to_string())).is_ok()) {
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
                let end_type = self.check_expr(end);
                let elem = match (start_type, end_type) {
                    (Some(s), Some(e)) => {
                        if self.infer.unify(&s, &e).is_err() {
                            let (s, e) = self.shown_pair(&s, &e);
                            self.err(end.span, format!("range bounds must share one type: {s} and {e}"));
                            None
                        } else {
                            self.predicate(PredicateKind::Integer, s.clone(), start.span, PredicateSite::Range);
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
                Some(array) => match self.array_element(&array) {
                    Some(elem) => match bindings.len() {
                        1 => Some(vec![elem]),
                        2 => Some(vec![crate::builtins::index_type(), elem]),
                        _ => {
                            self.err(span, "an array binds either one loop variable or an (index, value) pair");
                            None
                        }
                    },
                    None => {
                        let shown = self.shown_type(&array);
                        self.err(iterable.span, format!("cannot iterate over type {shown}"));
                        None
                    }
                },
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
                if self.infer.unify(declared, value).is_err() {
                    let (declared_shown, value_shown) = self.shown_pair(declared, value);
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
            (None, Some(value)) => self.infer_binding(value),
            (None, None) => Type::Unknown,
        };
        self.define_var(name, bound);
    }

    fn check_func_decl(&mut self, func: &FuncDecl) {
        let saved_body = std::mem::take(&mut self.body);
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
        self.finish_body();
        self.body = saved_body;
        self.current_return = saved_return;
        self.scopes.pop();
        self.type_params = saved_params;
    }

    /// A function declared in a body: a variable bound to a function literal, in
    /// scope from its declaration on and within its own body, so it may recurse.
    fn check_nested_func(&mut self, func: &FuncDecl) {
        if func.receiver.is_some() {
            self.err(func.span, "a method must be declared at module level");
            return;
        }
        if !func.type_params.is_empty() {
            self.err(func.span, "a nested function cannot be generic yet");
            return;
        }
        let params: Vec<_> = func.params.iter().map(|p| (p.name.as_str(), Some(&p.ty), p.span)).collect();
        self.check_closure(&params, &func.return_type, &func.body, func.span, Some(&func.name));
    }

    /// Check a function literal or nested function, returning its function type. A
    /// literal's omitted parameter type is an inference variable, and its omitted
    /// return type one that its `return` values fix, or unit when it has none; a nested
    /// function (`name`) is annotated like any declaration. The body shares the
    /// enclosing body's inference, so a captured variable's type may be fixed by a use
    /// on either side. `return` leaves the literal, and a loop around the literal is
    /// not one `break` can leave.
    fn check_closure(
        &mut self,
        params: &[(&str, Option<&TypeExpr>, Span)],
        return_type: &Option<TypeExpr>,
        body: &[Stmt],
        span: Span,
        name: Option<&str>,
    ) -> Option<Type> {
        let mut bound = HashMap::new();
        let mut param_types = Vec::with_capacity(params.len());
        for &(param, annotation, param_span) in params {
            let ty = match annotation {
                Some(annotation) => {
                    let declared = self.resolve_local_type(annotation)?;
                    self.infer_binding(&declared)
                }
                None => self.infer.fresh(InferKind::General),
            };
            // Lowering reads every parameter's type here, annotated or inferred.
            self.body.spans.push(param_span);
            self.types.insert(param_span, ty.clone());
            bound.insert(param.to_string(), ty.clone());
            param_types.push(ty);
        }
        let inferred_return = return_type.is_none() && name.is_none();
        let ret = match return_type {
            Some(ty) => self.resolve_local_type(ty)?,
            None if inferred_return => self.infer.fresh(InferKind::General),
            None => Type::Unit,
        };
        let func_type = Type::Func {
            return_type: Box::new(ret.clone()),
            param_types,
        };
        if let Some(name) = name {
            self.define_var(name, func_type.clone());
            self.body.spans.push(span);
            self.types.insert(span, func_type.clone());
        }
        self.closures.push(Closure {
            span,
            base: self.scopes.len(),
            captures: Vec::new(),
            returns_value: false,
        });
        self.scopes.push(bound);
        let saved_return = self.current_return.replace(ret.clone());
        let saved_loops = std::mem::replace(&mut self.loop_depth, 0);
        for stmt in body {
            self.check_stmt(stmt);
        }
        self.loop_depth = saved_loops;
        self.current_return = saved_return;
        self.scopes.pop();
        let closure = self.closures.pop().expect("pushed above");
        if inferred_return && !closure.returns_value {
            self.infer.unify(&ret, &Type::Unit).expect("a fresh variable unifies");
        }
        self.captures.insert(closure.span, closure.captures);
        Some(func_type)
    }

    fn check_return(&mut self, expr: Option<&Expr>, span: Span) {
        let Some(return_type) = self.current_return.clone() else {
            self.err(span, "return statement outside of function");
            return;
        };
        let Some(expr) = expr else {
            if self.infer.unify(&return_type, &Type::Unit).is_err() {
                let shown = self.shown_type(&return_type);
                self.err(span, format!("expected function to return {shown}"));
            }
            return;
        };
        if let Some(closure) = self.closures.last_mut() {
            closure.returns_value = true;
        }
        let is_unit_return = is_unit(&self.infer.resolve(&return_type));
        self.expected = Some(return_type.clone());
        let Some(expr_type) = self.check_expr(expr) else {
            return;
        };
        // A unit function may return a unit-typed value (`return ()`, or a call
        // to another unit function), but no other value.
        if self.infer.unify(&return_type, &expr_type).is_err() {
            let (return_type_shown, expr_type_shown) = self.shown_pair(&return_type, &expr_type);
            let message = if is_unit_return {
                format!("cannot return a value of type {expr_type_shown} from a function returning unit")
            } else {
                format!("return type mismatch: expected {return_type_shown}, found {expr_type_shown}")
            };
            self.err(expr.span, message);
        }
    }

    // --- expressions ---

    fn check_expr(&mut self, expr: &Expr) -> Option<Type> {
        if let Some(ty) = self.types.get(&expr.span).cloned() {
            self.expected = None;
            return Some(self.infer.resolve(&ty));
        }
        if self.failed.contains(&expr.span) {
            self.expected = None;
            return None;
        }
        let ty = self.check_expr_kind(expr).map(|ty| self.infer.resolve(&ty));
        match &ty {
            Some(ty) => {
                self.body.spans.push(expr.span);
                self.types.insert(expr.span, ty.clone());
            }
            None => {
                self.failed.insert(expr.span);
            }
        }
        ty
    }

    fn check_expr_kind(&mut self, expr: &Expr) -> Option<Type> {
        // Bare variants consume a head-only subject hint. Other expressions get
        // their types from equality constraints, not expected-type threading.
        let expected = self.expected.take();
        match &expr.kind {
            ExprKind::Number(text) => Some(self.number_type(text, expr.span, false)),
            ExprKind::Str(text) => {
                if let Err(escape) = decode_string_literal(text) {
                    self.err(expr.span, format!("unknown escape sequence `{escape}` in string literal"));
                }
                Some(Type::Primitive("string".to_string()))
            }
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
            ExprKind::Array(elems) => self.check_array_literal(elems),
            ExprKind::Ident(name) => {
                // Restore the hint so a bare payload-free variant can see it, but
                // clear it afterwards so an ordinary identifier never lets it leak.
                self.expected = expected;
                let t = self.check_ident(name, expr.span);
                self.expected = None;
                t
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.check_binary(*op, lhs, rhs, expr.span)
            }
            ExprKind::Chain { operands, ops } => self.check_chain(operands, ops),
            ExprKind::Range { .. } => {
                self.err(expr.span, "a range may only appear as a for-loop iterable");
                None
            }
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, expr.span),
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
            ExprKind::Field { target, name } => self.check_field(target, name, expr.span),
            ExprKind::Index { array, index } => self.check_index(array, index),
            ExprKind::Slice {
                array, start, end, ..
            } => self.check_slice(array, start.as_deref(), end.as_deref()),
            ExprKind::AddressOf(operand) => self.check_address_of(operand, expr.span),
            ExprKind::Deref(operand) => self.check_deref(operand),
            ExprKind::Assign { op, target, value } => self.check_assign(*op, target, value),
            ExprKind::Let { name, value } => {
                let value_type = self.check_expr(value)?;
                let bound = self.infer_binding(&value_type);
                self.define_var(name, bound.clone());
                Some(bound)
            }
            ExprKind::LetTuple { names, ty, value } => self.check_let_tuple(names, ty, value),
            ExprKind::Match { scrutinee, arms } => self.check_match_expr(scrutinee, arms),
            ExprKind::If { cond, then, els } => self.check_if_expr(cond, then, els),
            ExprKind::Block(block) => self.check_block_expr(block),
            ExprKind::Func {
                params,
                return_type,
                body,
            } => {
                let params: Vec<_> = params.iter().map(|p| (p.name.as_str(), p.ty.as_ref(), p.span)).collect();
                self.check_closure(&params, return_type, body, expr.span, None)
            }
        }
    }

    fn check_ident(&mut self, name: &str, span: Span) -> Option<Type> {
        if let Some(t) = self.use_var(name) {
            return Some(t);
        }
        if let Some(t) = self.globals.lookup_struct(name) {
            return Some(t);
        }
        if let Some(t) = self.globals.lookup_oneof(name) {
            return Some(t);
        }
        if let Some(sig) = self.globals.lookup_func(name) {
            let sig = sig.clone();
            return self.reference(span, &sig, &format!("function {name}"), None);
        }
        // A bare name that begins some `use`-bound module's spelling is a module
        // reference, navigated further by qualified field access.
        if self.module_is_prefix(std::slice::from_ref(&name.to_string())) {
            return Some(Type::Module(vec![name.to_string()]));
        }
        // A bare variant takes its subject from context or from a later use.
        if self.is_bare_variant(name) {
            let expected = self.expected.take();
            let subject = self.bare_variant_subject(expected);
            return Some(self.defer_variant(subject, name, None, true, span));
        }
        self.err(span, format!("undefined variable: {name}"));
        None
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
    fn check_module_access(
        &mut self,
        prefix: &[String],
        field: &str,
        span: Span,
    ) -> Option<Type> {
        let mut candidate = prefix.to_vec();
        candidate.push(field.to_string());
        if self.module_is_prefix(&candidate) {
            return Some(Type::Module(candidate));
        }
        if let Some(interface) = self.modules.get(prefix) {
            if let Some(ty) = interface.lookup_struct(field).or_else(|| interface.lookup_oneof(field)) {
                return Some(ty);
            }
            if let Some(sig) = interface.lookup_func(field) {
                let sig = sig.clone();
                let label = format!("function {}.{field}", prefix.join("."));
                return self.reference(span, &sig, &label, None);
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
    ) -> Option<Type> {
        let left = self.check_expr(lhs)?;
        let right = self.check_expr(rhs)?;
        if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            // The count only says how far to shift: any integer type, independent of
            // the shifted value's.
            self.predicate(PredicateKind::Integer, left.clone(), lhs.span, PredicateSite::Binary(op, right.clone()));
            self.predicate(PredicateKind::Integer, right, rhs.span, PredicateSite::ShiftCount);
            return Some(left);
        }
        self.check_operands(op, left, right, span)
    }

    /// Apply binary operator `op`'s type rule to operands of types `left` and
    /// `right`, which must be one type. `and`, `or`, and `xor` take two `bool`s or two
    /// integers, decided like any predicate once the body's constraints are solved.
    /// A comparison is `bool` even when its operands are invalid, so the error does not
    /// cascade into the comparison's use.
    fn check_operands(&mut self, op: BinaryOp, left: Type, right: Type, span: Span) -> Option<Type> {
        let boolean = Type::Primitive("bool".to_string());
        let comparison = matches!(
            op,
            BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
        );
        if self.infer.unify(&left, &right).is_err() {
            let (left, right) = self.shown_pair(&left, &right);
            let message = if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
                format!("cannot compare {left} and {right}")
            } else {
                format!("invalid operands for {}: {left} and {right}", op.symbol())
            };
            self.err(span, message);
            return comparison.then_some(boolean);
        }
        let kind = match op {
            BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => PredicateKind::Logical,
            BinaryOp::Shl | BinaryOp::Shr => unreachable!("shifts are checked by check_binary"),
            BinaryOp::Add => PredicateKind::Addable,
            BinaryOp::Eq | BinaryOp::Ne => PredicateKind::Comparable,
            BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => PredicateKind::Numeric,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => PredicateKind::Numeric,
        };
        self.predicate(kind, left.clone(), span, PredicateSite::Binary(op, right));
        Some(if comparison { boolean } else { left })
    }

    /// A comparison chain relates each adjacent pair of operands, each checked once.
    fn check_chain(&mut self, operands: &[Expr], ops: &[BinaryOp]) -> Option<Type> {
        let types: Vec<Option<Type>> = operands.iter().map(|e| self.check_expr(e)).collect();
        for (i, op) in ops.iter().enumerate() {
            if let (Some(left), Some(right)) = (&types[i], &types[i + 1]) {
                let span = operands[i].span.to(operands[i + 1].span);
                self.check_operands(*op, left.clone(), right.clone(), span);
            }
        }
        Some(Type::Primitive("bool".to_string()))
    }

    /// Every array element constrains one shared variable, including elements
    /// whose own types are supplied by later uses or deferred obligations.
    fn check_array_literal(&mut self, elems: &[Expr]) -> Option<Type> {
        let elem_type = self.infer.fresh(InferKind::General);
        for elem in elems {
            let Some(actual) = self.check_expr(elem) else {
                continue;
            };
            if self.infer.unify(&elem_type, &actual).is_err() {
                let (elem_type_shown, actual_shown) = self.shown_pair(&elem_type, &actual);
                self.err(
                    elem.span,
                    format!("array element type mismatch: expected {elem_type_shown}, found {actual_shown}"),
                );
            }
        }
        Some(Type::Array(Box::new(elem_type)))
    }

    fn check_expr_expecting(&mut self, expr: &Expr, expected: Option<Type>) -> Option<Type> {
        self.expected = expected;
        self.check_expr(expr)
    }

    fn check_unary(&mut self, op: UnaryOp, operand: &Expr, span: Span) -> Option<Type> {
        // A sign directly over a numeric literal is part of the literal for the
        // purpose of range checking: `-128` must be validated against a type's
        // minimum, not rejected as the out-of-range positive `128`. Intercept it
        // before the operand is checked on its own so the bound is applied once.
        if matches!(op, UnaryOp::Neg | UnaryOp::Pos) {
            if let Some(text) = bare_number_text(operand) {
                let ty = self.number_type(text, span, op == UnaryOp::Neg);
                // The literal under the sign is never checked on its own, so it takes
                // the signed literal's type here (through any grouping), for lowering.
                let mut inner = operand;
                loop {
                    self.body.spans.push(inner.span);
                    self.types.insert(inner.span, ty.clone());
                    match &inner.kind {
                        ExprKind::Group(group) => inner = group,
                        _ => break,
                    }
                }
                return Some(ty);
            }
        }
        let operand_type = self.check_expr(operand)?;
        // `!` is logical on `bool` and bitwise on integers, like `and`/`or`/`xor`.
        let kind = match op {
            UnaryOp::Neg | UnaryOp::Pos => PredicateKind::Numeric,
            UnaryOp::Not => PredicateKind::Logical,
        };
        self.predicate(kind, operand_type.clone(), span, PredicateSite::Unary(op));
        Some(operand_type)
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
        if !self.infer.is_bound(&rhs_type) {
            let tuple = Type::Tuple(names.iter().map(|_| self.infer.fresh(InferKind::General)).collect());
            let _ = self.infer.unify(&rhs_type, &tuple);
        }
        let rhs_type = self.infer.resolve(&rhs_type);
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
        if let Some(declared) = &declared {
            if declared.len() != names.len() {
                self.err(value.span, format!(
                    "destructuring pattern binds {} names but the annotation has {} elements",
                    names.len(), declared.len()
                ));
                return None;
            }
        }
        for (i, name) in names.iter().enumerate() {
            let elem_type = elem_types[i].clone();
            if let Some(declared) = &declared {
                let declared_elem = &declared[i];
                if self.infer.unify(declared_elem, &elem_type).is_err() {
                    let (declared_elem_shown, elem_type_shown) = self.shown_pair(declared_elem, &elem_type);
                    self.err(
                        value.span,
                        format!("type mismatch: {name} declared as {declared_elem_shown} but bound to {elem_type_shown}"),
                    );
                }
                self.define_var(name, declared_elem.clone());
            } else {
                let bound = self.infer_binding(&elem_type);
                self.define_var(name, bound);
            }
        }
        Some(rhs_type)
    }

    fn check_if_expr(&mut self, cond: &Expr, then: &Expr, els: &Expr) -> Option<Type> {
        let cond_type = self.check_expr(cond);
        if !cond_type.is_some_and(|ty| self.infer.unify(&ty, &Type::Primitive("bool".to_string())).is_ok()) {
            self.err(cond.span, "if-expression condition does not evaluate to a boolean type");
        }
        let then_type = self.check_expr(then)?;
        let else_type = self.check_expr(els)?;
        if self.infer.unify(&then_type, &else_type).is_err() {
            let (then_type_shown, else_type_shown) = self.shown_pair(&then_type, &else_type);
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
