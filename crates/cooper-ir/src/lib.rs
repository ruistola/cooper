//! The Cooper typed lowering IR.
//!
//! This crate is the seam between the frontend and a backend: it lowers the frontend's
//! AST into a typed, span-carrying tree where every node knows its resolved [`Type`],
//! so no name resolution or type inference happens downstream. Lowering consumes the
//! `span → Type` table the type checker hands off (see [`cooper_frontend::typecheck`]),
//! rather than recomputing any type.
//!
//! Runtime-touching operations are modelled here as abstract, typed intrinsics, never
//! as concrete runtime calls or a committed ABI; a backend chooses how those intrinsics
//! become allocations, barriers, and safepoints. This keeps the IR neutral about
//! garbage collection, concurrency, and how a runtime is delivered.

use std::collections::{HashMap, HashSet};

use cooper_frontend::ast::{
    AssignOp, BinaryOp, Block, Expr, ExprKind, FuncDecl, Pattern, PatternKind, Stmt, StmtKind,
    TypeExpr, TypedIdent, UnaryOp,
};
use cooper_frontend::diag::Span;
use cooper_frontend::resolve::{
    receiver_pattern_params, resolve_type, underlying_struct_name, Callable, Globals, Signature,
};
use cooper_frontend::typecheck::{decode_number_literal, ItemRef, LiteralValue, Typed};
use cooper_frontend::types::{is_numeric_name, Type, TypeDefs, DEFAULT_INT};

/// A lowered function or method: the declaration it lowers, its signature, its body
/// as typed statements, and the span it was lowered from. A method carries its
/// receiver as the first bound place. `type_params` lists every binder of the
/// declaration's signature — its own, then those a generic receiver introduces — in
/// the order a reference supplies type arguments. A generic function as lowered is a
/// template whose types name those binders; [`monomorphize`] stamps out instances,
/// each with `type_args` holding one concrete argument per binder (empty for a
/// non-generic function, and for a template).
#[derive(Debug, Clone)]
pub struct Function {
    pub item: Callable,
    pub receiver: Option<Param>,
    pub type_params: Vec<String>,
    pub type_args: Vec<Type>,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub body: Vec<IrStmt>,
    pub span: Span,
}

/// A bound place in a signature: a name, its resolved type, and its source span.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

/// A typed statement.
#[derive(Debug, Clone)]
pub struct IrStmt {
    pub kind: IrStmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrStmtKind {
    /// A local binding; `ty` is the resolved type the name takes.
    Var {
        name: String,
        ty: Type,
        init: Option<IrExpr>,
    },
    Expr(IrExpr),
    Return(Option<IrExpr>),
    While {
        cond: IrExpr,
        body: Vec<IrStmt>,
        post_test: bool,
        until: bool,
    },
    /// Iteration over an integer range. `var` takes successive values of type `ty`
    /// from `start` up to `end` (inclusive when `inclusive`).
    ForRange {
        var: String,
        ty: Type,
        start: Box<IrExpr>,
        end: Box<IrExpr>,
        inclusive: bool,
        body: Vec<IrStmt>,
    },
    /// Iteration over the elements of an array. `elem` takes each element in turn;
    /// when `index` is present it takes the element's position. The loop drives from
    /// the array's length (the `ArrayLength` intrinsic) and reads each element
    /// (`ArrayGet`), but carries the array and binders abstractly so the backend
    /// chooses the lowering.
    ForEach {
        array: Box<IrExpr>,
        index: Option<Binder>,
        elem: Binder,
        body: Vec<IrStmt>,
    },
    Block(Vec<IrStmt>),
    Break,
    Continue,
    /// A `match` run for effect. The scrutinee is evaluated once, then `tree` tests
    /// it to select an `action` (a statement body); `actions` is indexed by the
    /// tree's leaves. A plain-boolean `if` desugars into this form. The checker
    /// verified the arms exhaust the scrutinee, so a reached `Decision::Fail` is
    /// impossible.
    Match {
        scrutinee: IrExpr,
        actions: Vec<IrStmt>,
        tree: Decision,
    },
}

/// A lowered decision tree: the nested tests a `match` compiles to, selecting a
/// leaf action. Built by the usefulness/matrix algorithm so each scrutinee position
/// is tested at most once along any path.
#[derive(Debug, Clone)]
pub enum Decision {
    /// Run the action at `action`, having bound each name in `bindings` to the
    /// subvalue its access reaches.
    Leaf {
        bindings: Vec<MatchBinding>,
        action: usize,
    },
    /// Test the subvalue at `access` (of type `ty`) against each case in turn;
    /// `default` is taken when no case matches (absent when the cases are
    /// exhaustive over the type).
    Switch {
        access: Access,
        ty: Type,
        cases: Vec<Case>,
        default: Option<Box<Decision>>,
    },
    /// No arm matched — unreachable after the checker's exhaustiveness check.
    Fail,
}

/// One arm of a [`Decision::Switch`]: the constructor tested and the subtree taken
/// when it matches.
#[derive(Debug, Clone)]
pub struct Case {
    pub test: Test,
    pub tree: Decision,
}

/// A constructor a [`Switch`](Decision::Switch) discriminates on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Test {
    /// A sum-type variant, selected by name.
    Variant(String),
    /// A boolean constant.
    Bool(bool),
    /// An integer constant: its decoded magnitude and whether it is negated.
    Int { value: u128, negative: bool },
}

/// A name bound by a matched pattern: the subvalue it names (via `access`) and that
/// subvalue's resolved type.
#[derive(Debug, Clone)]
pub struct MatchBinding {
    pub name: String,
    pub access: Access,
    pub ty: Type,
}

/// A path from the `match` scrutinee to a subvalue a decision tree tests or binds.
#[derive(Debug, Clone)]
pub enum Access {
    /// The scrutinee itself.
    Root,
    /// A named field of a struct subvalue.
    Field { parent: Box<Access>, name: String },
    /// A positional component of a tuple subvalue.
    Elem { parent: Box<Access>, index: usize },
    /// A positional payload slot of a sum-type variant subvalue.
    Payload {
        parent: Box<Access>,
        variant: String,
        index: usize,
    },
}

/// A lowered pattern: its shape, the type of the value it matches at this position
/// (so bindings and sub-patterns carry a resolved type for the backend), and the
/// span it was lowered from.
#[derive(Debug, Clone)]
pub struct IrPattern {
    pub kind: IrPatternKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrPatternKind {
    /// Matches anything, binds nothing.
    Wildcard,
    /// Matches anything and binds the whole value to a name (its type is on the node).
    Binding(String),
    /// A sum-type variant, binding its payload slots positionally. A binder named
    /// `_` discards its slot.
    Variant {
        variant: String,
        binders: Vec<Binder>,
    },
    /// A boolean literal pattern matching one constant of a `bool` scrutinee.
    Bool(bool),
    /// An integer literal pattern: the decoded magnitude and whether it is negated.
    /// The node's type fixes its width and signedness.
    Int {
        value: u128,
        negative: bool,
    },
    /// A tuple pattern, matching a tuple value component-wise.
    Tuple(Vec<IrPattern>),
    /// A struct pattern, matching the listed fields; unlisted fields are wildcards.
    Struct {
        name: String,
        fields: Vec<(String, IrPattern)>,
    },
}

/// A payload slot bound by a variant pattern: the name introduced and the slot's
/// resolved type.
#[derive(Debug, Clone)]
pub struct Binder {
    pub name: String,
    pub ty: Type,
}

/// A typed expression: a shape, the resolved type it evaluates to, and the source
/// span it was lowered from.
#[derive(Debug, Clone)]
pub struct IrExpr {
    pub kind: IrExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrExprKind {
    /// An integer literal's decoded magnitude; its resolved type (width and
    /// signedness) is on the node, and any sign is a surrounding `Unary` operator.
    Int(u128),
    /// A floating-point literal's decoded value; its width is on the node.
    Float(f64),
    Bool(bool),
    Str(String),
    Nil,
    Unit,
    /// A reference to a bound variable or parameter.
    Var(String),
    /// A reference to a free function, instantiated at `type_args` (one per binder of
    /// its signature; none when it is not generic).
    FuncRef { item: Callable, type_args: Vec<Type> },
    /// A method bound to its receiver value, instantiated at `type_args`. Calling it
    /// passes `receiver` as the method's receiver.
    Method {
        receiver: Box<IrExpr>,
        item: Callable,
        type_args: Vec<Type>,
    },
    Tuple(Vec<IrExpr>),
    Unary {
        op: UnaryOp,
        operand: Box<IrExpr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<IrExpr>,
        rhs: Box<IrExpr>,
    },
    Call {
        callee: Box<IrExpr>,
        args: Vec<IrExpr>,
    },
    Field {
        target: Box<IrExpr>,
        name: String,
    },
    /// A call to a runtime-touching built-in operation, modelled abstractly rather
    /// than as a concrete runtime call. Arguments are fixed by `op`: the receiver
    /// array first, then an index, then a value (see [`Intrinsic`]).
    Intrinsic {
        op: Intrinsic,
        args: Vec<IrExpr>,
    },
    /// Postfix `^` load through a pointer.
    Deref(Box<IrExpr>),
    /// Prefix `&`, taking the address of an addressable place.
    AddressOf(Box<IrExpr>),
    Assign {
        op: AssignOp,
        target: Box<IrExpr>,
        value: Box<IrExpr>,
    },
    /// Walrus binding `name := value`, evaluating to the bound value.
    Let {
        name: String,
        value: Box<IrExpr>,
    },
    /// A value block: statements run for effect, then a trailing result expression.
    Block {
        stmts: Vec<IrStmt>,
        result: Box<IrExpr>,
    },
    /// Construction of a nominal struct value. The struct's name and type arguments
    /// are on the node's resolved type; members are carried in source order, each a
    /// field name paired with its lowered value.
    StructLiteral {
        name: String,
        members: Vec<(String, IrExpr)>,
    },
    /// A `match` producing a value: the scrutinee is evaluated once, then `tree`
    /// selects one of `actions` (each an expression of the node's resolved type). A
    /// plain-boolean `if` expression desugars into this form.
    Match {
        scrutinee: Box<IrExpr>,
        actions: Vec<IrExpr>,
        tree: Box<Decision>,
    },
    /// Construction of a sum-type value: a variant name and its payload arguments
    /// (empty for a payload-free variant). The owning sum type and its type arguments
    /// are on the node's resolved type.
    Variant {
        variant: String,
        args: Vec<IrExpr>,
    },
    /// A constructor-style numeric conversion `T(x)`. The source value is the operand;
    /// the destination type is the node's resolved type.
    Convert(Box<IrExpr>),
    /// Tuple-destructuring binding `(a, b) := value`: each name is bound to the
    /// matching component of the tuple `value`, and the whole expression evaluates to
    /// that tuple (the node's resolved type).
    LetTuple {
        bindings: Vec<Binder>,
        value: Box<IrExpr>,
    },
}

/// A runtime-touching built-in operation on a blessed collection, modelled as an
/// abstract typed intrinsic a backend lowers behind the runtime boundary. Each
/// variant fixes its argument order; a receiver array, where there is one, is first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intrinsic {
    /// A fresh array holding the given elements in order (an `[a, b, c]` literal).
    /// Args: the elements; the array type is on the node.
    ArrayLiteral,
    /// `length(): u64` — the element count of an array. Args: `[array]`.
    ArrayLength,
    /// `get(i): T` — the element at an index (the `a[i]` read). Args: `[array, index]`.
    ArrayGet,
    /// `set(i, v): unit` — store an element at an index (the `a[i] = v` write).
    /// Args: `[array, index, value]`.
    ArraySet,
    /// `push(v): T[]` — append, yielding the (possibly reallocated) array.
    /// Args: `[array, value]`.
    ArrayPush,
    /// Interior pointer to an element (the `&a[i]` address-of). Args: `[array, index]`.
    ArrayElementPtr,
}

/// Why lowering could not proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// The type checker attributed no type to this span — the expression did not
    /// check, so it cannot be lowered.
    MissingType(Span),
    /// A type annotation in a signature or binding failed to resolve. Lowering runs
    /// after a clean type check, so this signals an internal inconsistency rather
    /// than user error.
    UnresolvedType(Span),
    /// A numeric literal's spelling did not decode to a value. Like `UnresolvedType`,
    /// this is unreachable after a clean type check (the checker rejects out-of-range
    /// literals) and signals an internal inconsistency.
    MalformedLiteral(Span),
    /// A syntactic form not yet handled by lowering.
    Unsupported { span: Span, what: &'static str },
}

/// The type map keyed by span that the type checker hands off.
pub type TypeTable = HashMap<Span, Type>;

/// Lower a type-checked module's top-level declarations into typed functions.
/// Struct and sum-type declarations carry no runnable body and are skipped; every
/// other top-level form that is not a function is rejected as unsupported.
pub fn lower_module(
    decls: &[Stmt],
    globals: &Globals,
    checked: &Typed,
) -> Result<Vec<Function>, LowerError> {
    let mut functions = Vec::new();
    for decl in decls {
        match &decl.kind {
            StmtKind::FuncDecl(func) => {
                functions.push(lower_function(func, globals, checked)?);
            }
            StmtKind::StructDecl { .. } | StmtKind::OneofDecl { .. } => {}
            StmtKind::VarDecl { .. } => {
                return Err(LowerError::Unsupported {
                    span: decl.span,
                    what: "module-level variable",
                });
            }
            _ => {
                return Err(LowerError::Unsupported {
                    span: decl.span,
                    what: "top-level statement",
                });
            }
        }
    }
    Ok(functions)
}

/// Lower a single type-checked function declaration.
fn lower_function(
    func: &FuncDecl,
    globals: &Globals,
    checked: &Typed,
) -> Result<Function, LowerError> {
    let mut type_params: HashSet<String> = func.type_params.iter().cloned().collect();
    if let Some(receiver) = &func.receiver {
        type_params.extend(receiver_pattern_params(&receiver.ty));
    }
    let signature = signature_of(func, globals).ok_or(LowerError::UnresolvedType(func.span))?;
    let lower = Lower {
        types: &checked.types,
        refs: &checked.refs,
        globals,
        type_params,
    };
    let receiver = func
        .receiver
        .as_ref()
        .map(|r| lower.param(r))
        .transpose()?;
    let params = func.params.iter().map(|p| lower.param(p)).collect::<Result<_, _>>()?;
    let return_type = match &func.return_type {
        Some(ty) => lower.resolve(ty)?,
        None => Type::Unit,
    };
    let body = lower.block(&func.body)?;
    Ok(Function {
        item: signature.item.clone(),
        receiver,
        type_params: signature.type_params.clone(),
        type_args: Vec::new(),
        params,
        return_type,
        body,
        span: func.span,
    })
}

/// The resolved signature of a function or method declaration: a free function by
/// name, a method through its receiver struct.
fn signature_of<'g>(func: &FuncDecl, globals: &'g Globals) -> Option<&'g Signature> {
    match &func.receiver {
        None => globals.lookup_func(&func.name),
        Some(receiver) => {
            let id = globals.structs.get(underlying_struct_name(&receiver.ty)?)?;
            globals.lookup_method(id, &func.name)
        }
    }
}

/// Lower one type-checked expression into the typed IR, reading each subexpression's
/// type and each callee's declaration from the checker's tables. Grouping parentheses
/// carry no semantics and are collapsed.
pub fn lower_expr(expr: &Expr, globals: &Globals, checked: &Typed) -> Result<IrExpr, LowerError> {
    Lower {
        types: &checked.types,
        refs: &checked.refs,
        globals,
        type_params: HashSet::new(),
    }
    .expr(expr)
}

/// The resolved-context that lowering threads through a function body: the type
/// and reference tables to read from, the globals and type parameters a signature
/// annotation resolves against.
struct Lower<'a> {
    types: &'a TypeTable,
    refs: &'a HashMap<Span, ItemRef>,
    globals: &'a Globals,
    type_params: HashSet<String>,
}

impl Lower<'_> {
    fn param(&self, ident: &TypedIdent) -> Result<Param, LowerError> {
        Ok(Param {
            name: ident.name.clone(),
            ty: self.resolve(&ident.ty)?,
            span: ident.span,
        })
    }

    /// Resolve a signature or binding annotation. Lowering runs after a clean type
    /// check, so an unresolved annotation is an internal inconsistency.
    fn resolve(&self, ty: &TypeExpr) -> Result<Type, LowerError> {
        let mut diags = Vec::new();
        resolve_type(ty, &self.type_params, self.globals, &mut diags)
            .ok_or(LowerError::UnresolvedType(ty.span))
    }

    fn block(&self, stmts: &[Stmt]) -> Result<Vec<IrStmt>, LowerError> {
        stmts.iter().map(|s| self.stmt(s)).collect()
    }

    fn stmt(&self, stmt: &Stmt) -> Result<IrStmt, LowerError> {
        let kind = match &stmt.kind {
            StmtKind::Block(stmts) => IrStmtKind::Block(self.block(stmts)?),
            StmtKind::Expression(expr) => IrStmtKind::Expr(self.expr(expr)?),
            StmtKind::VarDecl { name, ty, init } => {
                let init = init.as_ref().map(|e| self.expr(e)).transpose()?;
                let resolved = match (ty, &init) {
                    (Some(annot), _) => self.resolve(annot)?,
                    (None, Some(value)) => value.ty.clone(),
                    (None, None) => {
                        return Err(LowerError::MissingType(stmt.span));
                    }
                };
                IrStmtKind::Var {
                    name: name.clone(),
                    ty: resolved,
                    init,
                }
            }
            StmtKind::Return(value) => {
                IrStmtKind::Return(value.as_ref().map(|e| self.expr(e)).transpose()?)
            }
            StmtKind::If { cond, then, els } => {
                let scrutinee = self.expr(cond)?;
                let then = self.stmt(then)?;
                let els = match els {
                    Some(s) => self.stmt(s)?,
                    None => IrStmt {
                        kind: IrStmtKind::Block(Vec::new()),
                        span: stmt.span,
                    },
                };
                let patterns = bool_patterns(&scrutinee.ty, scrutinee.span);
                let tree = compile_match(&self.globals.defs, &scrutinee.ty, &patterns);
                IrStmtKind::Match {
                    scrutinee,
                    actions: vec![then, els],
                    tree,
                }
            }
            StmtKind::While {
                cond,
                body,
                post_test,
                until,
            } => IrStmtKind::While {
                cond: self.expr(cond)?,
                body: self.block(body)?,
                post_test: *post_test,
                until: *until,
            },
            StmtKind::Break => IrStmtKind::Break,
            StmtKind::Continue => IrStmtKind::Continue,
            StmtKind::Match { scrutinee, arms } => {
                let scrutinee = self.expr(scrutinee)?;
                let patterns = arms
                    .iter()
                    .map(|arm| self.pattern(&scrutinee.ty, &arm.pattern))
                    .collect::<Result<Vec<_>, LowerError>>()?;
                let actions = arms
                    .iter()
                    .map(|arm| self.stmt(&arm.body))
                    .collect::<Result<Vec<_>, LowerError>>()?;
                let tree = compile_match(&self.globals.defs, &scrutinee.ty, &patterns);
                IrStmtKind::Match {
                    scrutinee,
                    actions,
                    tree,
                }
            }
            StmtKind::ForIn {
                bindings,
                iterable,
                body,
            } => match &iterable.kind {
                ExprKind::Range {
                    start,
                    end,
                    inclusive,
                } if bindings.len() == 1 => {
                    let start = self.expr(start)?;
                    let end = self.expr(end)?;
                    IrStmtKind::ForRange {
                        var: bindings[0].clone(),
                        ty: start.ty.clone(),
                        start: Box::new(start),
                        end: Box::new(end),
                        inclusive: *inclusive,
                        body: self.block(body)?,
                    }
                }
                // Array iteration drives from the backing's length and reads each
                // element; it binds either `[elem]` or `[index, elem]`.
                _ => {
                    let array = self.expr(iterable)?;
                    let Type::Array(elem_ty) = &array.ty else {
                        unreachable!("a checked for-in iterates a range or an array");
                    };
                    let elem_ty = (**elem_ty).clone();
                    let (index, elem_name) = match bindings.as_slice() {
                        [elem] => (None, elem),
                        [idx, elem] => (
                            Some(Binder {
                                name: idx.clone(),
                                ty: Type::Primitive(DEFAULT_INT.to_string()),
                            }),
                            elem,
                        ),
                        _ => unreachable!("a checked array iteration binds one or two names"),
                    };
                    IrStmtKind::ForEach {
                        array: Box::new(array),
                        index,
                        elem: Binder {
                            name: elem_name.clone(),
                            ty: elem_ty,
                        },
                        body: self.block(body)?,
                    }
                }
            },
            StmtKind::FuncDecl(_) => return unsupported_stmt(stmt.span, "nested function"),
            StmtKind::StructDecl { .. } | StmtKind::OneofDecl { .. } => {
                return unsupported_stmt(stmt.span, "local type declaration");
            }
        };
        Ok(IrStmt {
            kind,
            span: stmt.span,
        })
    }

    fn expr(&self, expr: &Expr) -> Result<IrExpr, LowerError> {
        if let ExprKind::Group(inner) = &expr.kind {
            return self.expr(inner);
        }
        let ty = self
            .types
            .get(&expr.span)
            .cloned()
            .ok_or(LowerError::MissingType(expr.span))?;
        // A reference to a function or method names its declaration; a module prefix
        // carries no runtime value and drops away.
        if let Some(reference) = self.refs.get(&expr.span) {
            let item = reference.item.clone();
            let type_args = reference.type_args.clone();
            let kind = match (&expr.kind, &item) {
                (ExprKind::Field { target, .. }, Callable::Method { .. }) => IrExprKind::Method {
                    receiver: Box::new(self.expr(target)?),
                    item,
                    type_args,
                },
                _ => IrExprKind::FuncRef { item, type_args },
            };
            return Ok(IrExpr {
                kind,
                ty,
                span: expr.span,
            });
        }
        let kind = match &expr.kind {
            ExprKind::Number(text) => match decode_number_literal(text) {
                Some(LiteralValue::Int(v)) => IrExprKind::Int(v),
                Some(LiteralValue::Float(v)) => IrExprKind::Float(v),
                None => return Err(LowerError::MalformedLiteral(expr.span)),
            },
            ExprKind::Bool(b) => IrExprKind::Bool(*b),
            ExprKind::Str(s) => IrExprKind::Str(s.clone()),
            ExprKind::Nil => IrExprKind::Nil,
            ExprKind::Unit => IrExprKind::Unit,
            ExprKind::Ident(name) => {
                if variant_named(&self.globals.defs, name, &ty) {
                    // A bare payload-free variant inferred from the expected type.
                    IrExprKind::Variant {
                        variant: name.clone(),
                        args: Vec::new(),
                    }
                } else {
                    IrExprKind::Var(name.clone())
                }
            }
            ExprKind::Tuple(elems) => IrExprKind::Tuple(self.each(elems)?),
            ExprKind::Array(elems) => IrExprKind::Intrinsic {
                op: Intrinsic::ArrayLiteral,
                args: self.each(elems)?,
            },
            ExprKind::Unary { op, operand } => IrExprKind::Unary {
                op: *op,
                operand: Box::new(self.expr(operand)?),
            },
            ExprKind::Binary { op, lhs, rhs } => IrExprKind::Binary {
                op: *op,
                lhs: Box::new(self.expr(lhs)?),
                rhs: Box::new(self.expr(rhs)?),
            },
            ExprKind::Call { callee, args } => self.lower_call(callee, args, &ty)?,
            ExprKind::Field { target, name } => match self.types.get(&target.span) {
                // `Oneof.Variant` reads as a payload-free variant value, not a
                // field access on a runtime target.
                Some(Type::Oneof { .. }) => IrExprKind::Variant {
                    variant: name.clone(),
                    args: Vec::new(),
                },
                // A module-qualified function is a recorded reference (above); no
                // other module item is a runtime value yet.
                Some(Type::Module(_)) => return unsupported(expr.span, "module-qualified value"),
                _ => IrExprKind::Field {
                    target: Box::new(self.expr(target)?),
                    name: name.clone(),
                },
            },
            ExprKind::Index { array, index } => IrExprKind::Intrinsic {
                op: Intrinsic::ArrayGet,
                args: vec![self.expr(array)?, self.expr(index)?],
            },
            ExprKind::Deref(operand) => IrExprKind::Deref(Box::new(self.expr(operand)?)),
            ExprKind::AddressOf(operand) => match &strip_group(operand).kind {
                // `&a[i]` is an interior pointer into the array's storage, an
                // abstract intrinsic rather than an address of an ordinary place.
                ExprKind::Index { array, index } => IrExprKind::Intrinsic {
                    op: Intrinsic::ArrayElementPtr,
                    args: vec![self.expr(array)?, self.expr(index)?],
                },
                _ => IrExprKind::AddressOf(Box::new(self.expr(operand)?)),
            },
            ExprKind::Assign { op, target, value } => {
                if let ExprKind::Index { array, index } = &strip_group(target).kind {
                    self.lower_index_assign(*op, array, index, value)?
                } else {
                    IrExprKind::Assign {
                        op: *op,
                        target: Box::new(self.expr(target)?),
                        value: Box::new(self.expr(value)?),
                    }
                }
            }
            ExprKind::Let { name, value } => IrExprKind::Let {
                name: name.clone(),
                value: Box::new(self.expr(value)?),
            },
            ExprKind::If { cond, then, els } => {
                let scrutinee = self.expr(cond)?;
                let then = self.expr(then)?;
                let els = self.expr(els)?;
                let patterns = bool_patterns(&scrutinee.ty, scrutinee.span);
                let tree = compile_match(&self.globals.defs, &scrutinee.ty, &patterns);
                IrExprKind::Match {
                    scrutinee: Box::new(scrutinee),
                    actions: vec![then, els],
                    tree: Box::new(tree),
                }
            }
            ExprKind::Block(block) => self.value_block(block)?,
            ExprKind::StructLiteral { members, .. } => {
                let Type::Struct { id, .. } = &ty else {
                    return Err(LowerError::MissingType(expr.span));
                };
                IrExprKind::StructLiteral {
                    name: id.name.clone(),
                    members: members
                        .iter()
                        .map(|m| Ok((m.name.clone(), self.expr(&m.value)?)))
                        .collect::<Result<Vec<_>, LowerError>>()?,
                }
            }
            ExprKind::Group(_) => unreachable!("grouping collapsed above"),
            ExprKind::Range { .. } => return unsupported(expr.span, "range"),
            ExprKind::Match { scrutinee, arms } => {
                let scrutinee = self.expr(scrutinee)?;
                let patterns = arms
                    .iter()
                    .map(|arm| self.pattern(&scrutinee.ty, &arm.pattern))
                    .collect::<Result<Vec<_>, LowerError>>()?;
                let actions = arms
                    .iter()
                    .map(|arm| self.expr(&arm.body))
                    .collect::<Result<Vec<_>, LowerError>>()?;
                let tree = compile_match(&self.globals.defs, &scrutinee.ty, &patterns);
                IrExprKind::Match {
                    scrutinee: Box::new(scrutinee),
                    actions,
                    tree: Box::new(tree),
                }
            }
            ExprKind::LetTuple { names, value, .. } => {
                let value = self.expr(value)?;
                let Type::Tuple(elems) = &value.ty else {
                    unreachable!("a checked tuple destructuring binds a tuple value");
                };
                let bindings = names
                    .iter()
                    .zip(elems)
                    .map(|(name, ty)| Binder {
                        name: name.clone(),
                        ty: ty.clone(),
                    })
                    .collect();
                IrExprKind::LetTuple {
                    bindings,
                    value: Box::new(value),
                }
            }
        };
        Ok(IrExpr {
            kind,
            ty,
            span: expr.span,
        })
    }

    fn value_block(&self, block: &Block) -> Result<IrExprKind, LowerError> {
        Ok(IrExprKind::Block {
            stmts: self.block(&block.statements)?,
            result: Box::new(self.expr(&block.result)?),
        })
    }

    /// Lower a match-arm pattern against the type of the value it matches. A variant
    /// pattern reads each bound slot's type from the sum type's (already instantiated)
    /// variant payloads; tuple and struct patterns recurse into their components;
    /// literal patterns carry their decoded constant. The type checker validated the
    /// pattern, so every sub-pattern's type is known and consistent.
    fn pattern(&self, ty: &Type, pattern: &Pattern) -> Result<IrPattern, LowerError> {
        let kind = match &pattern.kind {
            PatternKind::Wildcard => IrPatternKind::Wildcard,
            PatternKind::Binding(name) => IrPatternKind::Binding(name.clone()),
            PatternKind::Bool(b) => IrPatternKind::Bool(*b),
            PatternKind::Int { negative, magnitude } => {
                let Some(LiteralValue::Int(value)) = decode_number_literal(magnitude) else {
                    unreachable!("a checked integer pattern decodes to an in-range magnitude");
                };
                IrPatternKind::Int {
                    value,
                    negative: *negative,
                }
            }
            PatternKind::Variant { variant, binders, .. } => {
                let payload = self
                    .globals
                    .defs
                    .payload(ty, variant)
                    .expect("a checked variant pattern names a variant of its sum type");
                let binders = binders
                    .iter()
                    .zip(payload)
                    .map(|(name, ty)| Binder {
                        name: name.clone(),
                        ty: ty.clone(),
                    })
                    .collect();
                IrPatternKind::Variant {
                    variant: variant.clone(),
                    binders,
                }
            }
            PatternKind::Tuple(elems) => {
                let Type::Tuple(types) = ty else {
                    unreachable!("a checked tuple pattern matches a tuple type");
                };
                let elems = elems
                    .iter()
                    .zip(types)
                    .map(|(sub, elem_ty)| self.pattern(elem_ty, sub))
                    .collect::<Result<_, _>>()?;
                IrPatternKind::Tuple(elems)
            }
            PatternKind::Struct { name, fields } => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        let member_ty = self
                            .globals
                            .defs
                            .member(ty, &field.name)
                            .expect("a checked struct pattern names existing fields");
                        Ok((field.name.clone(), self.pattern(&member_ty, &field.pattern)?))
                    })
                    .collect::<Result<_, _>>()?;
                IrPatternKind::Struct {
                    name: name.clone(),
                    fields,
                }
            }
        };
        Ok(IrPattern {
            kind,
            ty: ty.clone(),
            span: pattern.span,
        })
    }

    fn each(&self, exprs: &[Expr]) -> Result<Vec<IrExpr>, LowerError> {
        exprs.iter().map(|e| self.expr(e)).collect()
    }

    /// Lower an indexed assignment `a[i] <op>= v` to an `ArraySet` intrinsic. A plain
    /// `=` stores the value directly; a compound `+=`/`-=`/`*=`/`/=` reads the current
    /// element via `ArrayGet` and stores the combined result, since indexed places
    /// must route through the get/set pair rather than a mutable binding.
    fn lower_index_assign(
        &self,
        op: AssignOp,
        array: &Expr,
        index: &Expr,
        value: &Expr,
    ) -> Result<IrExprKind, LowerError> {
        let array = self.expr(array)?;
        let index = self.expr(index)?;
        let value = self.expr(value)?;
        let elem_ty = match &array.ty {
            Type::Array(elem) => (**elem).clone(),
            _ => unreachable!("a checked indexed assignment targets an array"),
        };
        let stored = match op {
            AssignOp::Assign => value,
            _ => {
                let value_span = value.span;
                let current = IrExpr {
                    kind: IrExprKind::Intrinsic {
                        op: Intrinsic::ArrayGet,
                        args: vec![array.clone(), index.clone()],
                    },
                    ty: elem_ty.clone(),
                    span: value_span,
                };
                IrExpr {
                    kind: IrExprKind::Binary {
                        op: compound_binop(op),
                        lhs: Box::new(current),
                        rhs: Box::new(value),
                    },
                    ty: elem_ty,
                    span: value_span,
                }
            }
        };
        Ok(IrExprKind::Intrinsic {
            op: Intrinsic::ArraySet,
            args: vec![array, index, stored],
        })
    }

    /// Classify a call: a constructor-style numeric conversion `T(x)`, a sum-type
    /// variant construction, or an ordinary function/method call.
    fn lower_call(
        &self,
        callee: &Expr,
        args: &[Expr],
        result: &Type,
    ) -> Result<IrExprKind, LowerError> {
        if let ExprKind::Ident(name) = &callee.kind {
            if is_numeric_name(name) {
                return Ok(IrExprKind::Convert(Box::new(self.expr(&args[0])?)));
            }
        }
        if let Some(variant) = self.variant_callee(callee, result) {
            return Ok(IrExprKind::Variant {
                variant,
                args: self.each(args)?,
            });
        }
        if let Some(intrinsic) = self.array_method(callee, args)? {
            return Ok(intrinsic);
        }
        Ok(IrExprKind::Call {
            callee: Box::new(self.expr(callee)?),
            args: self.each(args)?,
        })
    }

    /// Lower a blessed array method call `a.m(..)` to its intrinsic, when the callee
    /// is a field access whose target is a built-in array. Arrays carry no vtable, so
    /// these never become ordinary calls.
    fn array_method(
        &self,
        callee: &Expr,
        args: &[Expr],
    ) -> Result<Option<IrExprKind>, LowerError> {
        let ExprKind::Field { target, name } = &callee.kind else {
            return Ok(None);
        };
        if !matches!(self.types.get(&target.span), Some(Type::Array(_))) {
            return Ok(None);
        }
        let op = match name.as_str() {
            "length" => Intrinsic::ArrayLength,
            "get" => Intrinsic::ArrayGet,
            "set" => Intrinsic::ArraySet,
            "push" => Intrinsic::ArrayPush,
            _ => unreachable!("a checked array method names a blessed method"),
        };
        let mut lowered = vec![self.expr(target)?];
        for arg in args {
            lowered.push(self.expr(arg)?);
        }
        Ok(Some(IrExprKind::Intrinsic { op, args: lowered }))
    }

    /// The variant name a call constructs, if the callee denotes a sum-type variant
    /// rather than a function: a qualified `Oneof.Variant(..)` (target typed as a sum
    /// type) or a bare `Variant(..)` whose result type is a sum type owning that
    /// variant and whose name is not a known function.
    fn variant_callee(&self, callee: &Expr, result: &Type) -> Option<String> {
        match &callee.kind {
            ExprKind::Field { target, name } => {
                matches!(self.types.get(&target.span), Some(Type::Oneof { .. }))
                    .then(|| name.clone())
            }
            ExprKind::Ident(name) => (variant_named(&self.globals.defs, name, result)
                && self.globals.lookup_func(name).is_none())
            .then(|| name.clone()),
            _ => None,
        }
    }
}

/// Whether `name` is a variant of sum type `ty`. Used to tell a bare variant value
/// apart from an ordinary variable of some sum type.
fn variant_named(defs: &TypeDefs, name: &str, ty: &Type) -> bool {
    defs.payload(ty, name).is_some()
}

/// Peel parenthesised groupings to reach the expression they wrap, so a target like
/// `&(a[i])` is classified by its inner shape.
fn strip_group(expr: &Expr) -> &Expr {
    let mut current = expr;
    while let ExprKind::Group(inner) = &current.kind {
        current = inner;
    }
    current
}

/// The binary operator a compound assignment applies. `Assign` has no operator and
/// is handled before this is reached.
fn compound_binop(op: AssignOp) -> BinaryOp {
    match op {
        AssignOp::Add => BinaryOp::Add,
        AssignOp::Sub => BinaryOp::Sub,
        AssignOp::Mul => BinaryOp::Mul,
        AssignOp::Div => BinaryOp::Div,
        AssignOp::Assign => unreachable!("plain assignment has no binary operator"),
    }
}

/// The two boolean patterns an `if` desugars to: `true` (the `then` action) then
/// `false` (the `else` action), in that order.
fn bool_patterns(ty: &Type, span: Span) -> Vec<IrPattern> {
    vec![
        IrPattern {
            kind: IrPatternKind::Bool(true),
            ty: ty.clone(),
            span,
        },
        IrPattern {
            kind: IrPatternKind::Bool(false),
            ty: ty.clone(),
            span,
        },
    ]
}

/// A scrutinee position the matrix compiler discriminates on: where it is reached
/// from the scrutinee and the type of value found there.
#[derive(Clone)]
struct Occurrence {
    access: Access,
    ty: Type,
}

/// One pattern-matrix row: the patterns left to test (aligned with the current
/// occurrences), the bindings accumulated as columns were consumed, and the action
/// to run when the row matches.
#[derive(Clone)]
struct Row {
    columns: Vec<IrPattern>,
    bindings: Vec<MatchBinding>,
    action: usize,
}

/// Compile a match's arm patterns (in source order, the row index being the action
/// index) over a scrutinee of `ty` into a decision tree.
fn compile_match(defs: &TypeDefs, ty: &Type, patterns: &[IrPattern]) -> Decision {
    let occurrences = vec![Occurrence {
        access: Access::Root,
        ty: ty.clone(),
    }];
    let rows = patterns
        .iter()
        .enumerate()
        .map(|(action, pattern)| Row {
            columns: vec![pattern.clone()],
            bindings: Vec::new(),
            action,
        })
        .collect();
    compile(defs, occurrences, rows)
}

/// The matrix algorithm: the first row whose columns are all irrefutable wins;
/// otherwise test the leftmost column the top row cares about, deconstructing a
/// single-constructor type in place or branching on a `Switch`.
fn compile(defs: &TypeDefs, occurrences: Vec<Occurrence>, mut rows: Vec<Row>) -> Decision {
    let Some(first) = rows.first() else {
        return Decision::Fail;
    };
    if first.columns.iter().all(ir_is_irrefutable) {
        let mut row = rows.swap_remove(0);
        for (occ, pat) in occurrences.iter().zip(&row.columns) {
            collect_bindings(defs, pat, &occ.access, &occ.ty, &mut row.bindings);
        }
        return Decision::Leaf {
            bindings: row.bindings,
            action: row.action,
        };
    }
    let col = first
        .columns
        .iter()
        .position(|p| !ir_is_irrefutable(p))
        .expect("an all-irrefutable row was handled above");
    match &occurrences[col].ty {
        Type::Tuple(_) | Type::Struct { .. } => deconstruct(defs, col, occurrences, rows),
        _ => switch(defs, col, occurrences, rows),
    }
}

/// Expand a single-constructor (tuple or struct) column into its fields: the column
/// is replaced by one occurrence per field, and every row's pattern there by its
/// sub-patterns (missing struct fields and irrefutable patterns become wildcards).
fn deconstruct(defs: &TypeDefs, col: usize, occurrences: Vec<Occurrence>, rows: Vec<Row>) -> Decision {
    let subs = sub_fields(defs, &occurrences[col].access, &occurrences[col].ty);
    let mut new_occurrences = occurrences.clone();
    new_occurrences.splice(col..=col, subs.clone());
    let new_rows = rows
        .into_iter()
        .map(|mut row| {
            let pat = row.columns.remove(col);
            let subpats = deconstruct_pattern(defs, pat, &occurrences[col], &subs, &mut row.bindings);
            row.columns.splice(col..col, subpats);
            row
        })
        .collect();
    compile(defs, new_occurrences, new_rows)
}

/// The sub-patterns a tuple/struct column pattern contributes, aligned with `subs`.
/// A binding over the whole value is recorded before it is dropped to wildcards.
fn deconstruct_pattern(
    defs: &TypeDefs,
    pat: IrPattern,
    occ: &Occurrence,
    subs: &[Occurrence],
    bindings: &mut Vec<MatchBinding>,
) -> Vec<IrPattern> {
    let span = pat.span;
    match pat.kind {
        IrPatternKind::Wildcard => subs.iter().map(|s| ir_wildcard(s.ty.clone(), span)).collect(),
        IrPatternKind::Binding(name) => {
            bindings.push(MatchBinding {
                name,
                access: occ.access.clone(),
                ty: occ.ty.clone(),
            });
            subs.iter().map(|s| ir_wildcard(s.ty.clone(), span)).collect()
        }
        IrPatternKind::Tuple(ps) => ps,
        IrPatternKind::Struct { fields, .. } => {
            sorted_members(defs, &occ.ty)
                .into_iter()
                .map(|(name, ty)| {
                    fields
                        .iter()
                        .find(|(f, _)| *f == name)
                        .map(|(_, p)| p.clone())
                        .unwrap_or_else(|| ir_wildcard(ty, span))
                })
                .collect()
        }
        _ => unreachable!("deconstruct handles only tuple/struct columns"),
    }
}

/// Branch on a column of a switchable type (`bool`, integer, or sum type): a case
/// per head constructor, plus a default from the irrefutable rows when the cases do
/// not exhaust the type.
fn switch(defs: &TypeDefs, col: usize, occurrences: Vec<Occurrence>, rows: Vec<Row>) -> Decision {
    let access = occurrences[col].access.clone();
    let ty = occurrences[col].ty.clone();
    let tests = head_tests(&rows, col);
    let mut cases = Vec::new();
    for test in &tests {
        let subs = test_sub_occurrences(defs, &access, &ty, test);
        let mut case_occurrences = occurrences.clone();
        case_occurrences.splice(col..=col, subs.clone());
        let case_rows = rows
            .iter()
            .filter_map(|row| specialize_row(row, col, test, &access, &ty, &subs))
            .collect();
        cases.push(Case {
            test: test.clone(),
            tree: compile(defs, case_occurrences, case_rows),
        });
    }
    let default = if tests_exhaustive(defs, &ty, &tests) {
        None
    } else {
        let mut default_occurrences = occurrences.clone();
        default_occurrences.remove(col);
        let default_rows = rows
            .iter()
            .filter(|row| ir_is_irrefutable(&row.columns[col]))
            .map(|row| default_row(row, col, &access, &ty))
            .collect();
        Some(Box::new(compile(defs, default_occurrences, default_rows)))
    };
    Decision::Switch {
        access,
        ty,
        cases,
        default,
    }
}

/// Specialize a row against the case constructor `test`: a matching constructor
/// row contributes its sub-patterns; an irrefutable row falls through as wildcards
/// (recording any binding); any other constructor drops out.
fn specialize_row(
    row: &Row,
    col: usize,
    test: &Test,
    access: &Access,
    ty: &Type,
    subs: &[Occurrence],
) -> Option<Row> {
    let span = row.columns[col].span;
    if ir_is_irrefutable(&row.columns[col]) {
        let mut r = row.clone();
        let removed = r.columns.remove(col);
        if let IrPatternKind::Binding(name) = removed.kind {
            r.bindings.push(MatchBinding {
                name,
                access: access.clone(),
                ty: ty.clone(),
            });
        }
        let wilds: Vec<_> = subs.iter().map(|s| ir_wildcard(s.ty.clone(), span)).collect();
        r.columns.splice(col..col, wilds);
        return Some(r);
    }
    if pattern_test(&row.columns[col]).as_ref() != Some(test) {
        return None;
    }
    let mut r = row.clone();
    let removed = r.columns.remove(col);
    let subpats = match removed.kind {
        IrPatternKind::Variant { binders, .. } => binders
            .into_iter()
            .map(|b| {
                let kind = if b.name == "_" {
                    IrPatternKind::Wildcard
                } else {
                    IrPatternKind::Binding(b.name)
                };
                IrPattern {
                    kind,
                    ty: b.ty,
                    span,
                }
            })
            .collect(),
        IrPatternKind::Bool(_) | IrPatternKind::Int { .. } => Vec::new(),
        _ => unreachable!("a refutable switch column is a variant or literal"),
    };
    r.columns.splice(col..col, subpats);
    Some(r)
}

/// A default-branch row: the tested (irrefutable) column is dropped, recording any
/// whole-value binding it introduced.
fn default_row(row: &Row, col: usize, access: &Access, ty: &Type) -> Row {
    let mut r = row.clone();
    let removed = r.columns.remove(col);
    if let IrPatternKind::Binding(name) = removed.kind {
        r.bindings.push(MatchBinding {
            name,
            access: access.clone(),
            ty: ty.clone(),
        });
    }
    r
}

/// The head constructors appearing in column `col`, in order of first appearance.
fn head_tests(rows: &[Row], col: usize) -> Vec<Test> {
    let mut tests = Vec::new();
    for row in rows {
        if let Some(test) = pattern_test(&row.columns[col]) {
            if !tests.contains(&test) {
                tests.push(test);
            }
        }
    }
    tests
}

/// The constructor a pattern tests, or `None` for an irrefutable pattern.
fn pattern_test(pat: &IrPattern) -> Option<Test> {
    match &pat.kind {
        IrPatternKind::Variant { variant, .. } => Some(Test::Variant(variant.clone())),
        IrPatternKind::Bool(b) => Some(Test::Bool(*b)),
        IrPatternKind::Int { value, negative } => Some(Test::Int {
            value: *value,
            negative: *negative,
        }),
        _ => None,
    }
}

/// The occurrences a matched constructor introduces: a variant exposes its payload
/// slots; `bool` and integer constructors expose nothing.
fn test_sub_occurrences(defs: &TypeDefs, access: &Access, ty: &Type, test: &Test) -> Vec<Occurrence> {
    match test {
        Test::Variant(variant) => {
            let payload = defs
                .payload(ty, variant)
                .expect("a variant test names a variant of its sum type");
            payload
                .iter()
                .enumerate()
                .map(|(index, slot)| Occurrence {
                    access: Access::Payload {
                        parent: Box::new(access.clone()),
                        variant: variant.clone(),
                        index,
                    },
                    ty: slot.clone(),
                })
                .collect()
        }
        Test::Bool(_) | Test::Int { .. } => Vec::new(),
    }
}

/// Whether the tested constructors cover the type: both booleans, or every variant
/// of a sum type. Integers are never exhausted by literals, so they always need a
/// default (the checker requires an irrefutable catch-all there).
fn tests_exhaustive(defs: &TypeDefs, ty: &Type, tests: &[Test]) -> bool {
    match ty {
        Type::Primitive(name) if name == "bool" => {
            tests.contains(&Test::Bool(true)) && tests.contains(&Test::Bool(false))
        }
        Type::Oneof { .. } => defs
            .variant_order(ty)
            .iter()
            .all(|v| tests.contains(&Test::Variant(v.clone()))),
        _ => false,
    }
}

/// The occurrences a tuple/struct value exposes, in a deterministic order (tuple
/// position, struct fields sorted by name).
fn sub_fields(defs: &TypeDefs, access: &Access, ty: &Type) -> Vec<Occurrence> {
    match ty {
        Type::Tuple(elems) => elems
            .iter()
            .enumerate()
            .map(|(index, elem)| Occurrence {
                access: Access::Elem {
                    parent: Box::new(access.clone()),
                    index,
                },
                ty: elem.clone(),
            })
            .collect(),
        Type::Struct { .. } => sorted_members(defs, ty)
            .into_iter()
            .map(|(name, ty)| Occurrence {
                access: Access::Field {
                    parent: Box::new(access.clone()),
                    name,
                },
                ty,
            })
            .collect(),
        _ => unreachable!("sub_fields runs on tuple/struct occurrences only"),
    }
}

/// A struct's members as `(name, type)` pairs sorted by name, giving deconstruction
/// a deterministic field order independent of the members map's iteration order.
fn sorted_members(defs: &TypeDefs, ty: &Type) -> Vec<(String, Type)> {
    let mut pairs: Vec<_> = defs
        .struct_members(ty)
        .expect("a struct occurrence names a defined struct")
        .into_iter()
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

/// Whether a pattern matches every value of its type (so it never forces a test).
fn ir_is_irrefutable(pat: &IrPattern) -> bool {
    match &pat.kind {
        IrPatternKind::Wildcard | IrPatternKind::Binding(_) => true,
        IrPatternKind::Tuple(ps) => ps.iter().all(ir_is_irrefutable),
        IrPatternKind::Struct { fields, .. } => fields.iter().all(|(_, p)| ir_is_irrefutable(p)),
        _ => false,
    }
}

/// Collect the names an irrefutable pattern binds, each with the access and type of
/// the subvalue it reaches, descending through tuple and struct patterns.
fn collect_bindings(defs: &TypeDefs, pat: &IrPattern, access: &Access, ty: &Type, out: &mut Vec<MatchBinding>) {
    match &pat.kind {
        IrPatternKind::Wildcard => {}
        IrPatternKind::Binding(name) => out.push(MatchBinding {
            name: name.clone(),
            access: access.clone(),
            ty: ty.clone(),
        }),
        IrPatternKind::Tuple(ps) => {
            let Type::Tuple(elems) = ty else {
                unreachable!("a tuple pattern matches a tuple type");
            };
            for (index, (sub, elem_ty)) in ps.iter().zip(elems).enumerate() {
                collect_bindings(
                    defs,
                    sub,
                    &Access::Elem {
                        parent: Box::new(access.clone()),
                        index,
                    },
                    elem_ty,
                    out,
                );
            }
        }
        IrPatternKind::Struct { fields, .. } => {
            for (name, sub) in fields {
                let member_ty = &defs
                    .member(ty, name)
                    .expect("a checked struct pattern names existing fields");
                collect_bindings(
                    defs,
                    sub,
                    &Access::Field {
                        parent: Box::new(access.clone()),
                        name: name.clone(),
                    },
                    member_ty,
                    out,
                );
            }
        }
        _ => unreachable!("collect_bindings runs on irrefutable patterns only"),
    }
}

/// A wildcard pattern of a given type and span.
fn ir_wildcard(ty: Type, span: Span) -> IrPattern {
    IrPattern {
        kind: IrPatternKind::Wildcard,
        ty,
        span,
    }
}

fn unsupported(span: Span, what: &'static str) -> Result<IrExpr, LowerError> {
    Err(LowerError::Unsupported { span, what })
}

fn unsupported_stmt(span: Span, what: &'static str) -> Result<IrStmt, LowerError> {
    Err(LowerError::Unsupported { span, what })
}

#[cfg(test)]
mod tests;
