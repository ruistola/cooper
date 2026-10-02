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
use cooper_frontend::resolve::{receiver_pattern_params, resolve_type, Globals};
use cooper_frontend::typecheck::{decode_number_literal, LiteralValue};
use cooper_frontend::types::{is_numeric_name, Type};

/// A lowered function or method: its signature, its body as typed statements, and
/// the span it was lowered from. A method carries its receiver as the first bound
/// place; type parameters list both the function's own binders and those a generic
/// receiver introduces.
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub receiver: Option<Param>,
    pub type_params: Vec<String>,
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
    /// A reference to a bound variable, parameter, or named global.
    Var(String),
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
    Index {
        array: Box<IrExpr>,
        index: Box<IrExpr>,
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
    types: &TypeTable,
) -> Result<Vec<Function>, LowerError> {
    let mut functions = Vec::new();
    for decl in decls {
        match &decl.kind {
            StmtKind::FuncDecl(func) => {
                functions.push(lower_function(func, globals, types)?);
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
    types: &TypeTable,
) -> Result<Function, LowerError> {
    let mut type_params: HashSet<String> = func.type_params.iter().cloned().collect();
    if let Some(receiver) = &func.receiver {
        type_params.extend(receiver_pattern_params(&receiver.ty));
    }
    let lower = Lower {
        types,
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
        name: func.name.clone(),
        receiver,
        type_params: func.type_params.clone(),
        params,
        return_type,
        body,
        span: func.span,
    })
}

/// Lower one type-checked expression into the typed IR, reading each subexpression's
/// type from `types`. Grouping parentheses carry no semantics and are collapsed.
pub fn lower_expr(
    expr: &Expr,
    globals: &Globals,
    types: &TypeTable,
) -> Result<IrExpr, LowerError> {
    Lower {
        types,
        globals,
        type_params: HashSet::new(),
    }
    .expr(expr)
}

/// The resolved-context that lowering threads through a function body: the type
/// table to read from, the globals and type parameters a signature annotation
/// resolves against.
struct Lower<'a> {
    types: &'a TypeTable,
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
                let tree = compile_match(&scrutinee.ty, &patterns);
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
                let tree = compile_match(&scrutinee.ty, &patterns);
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
                // Array iteration drives from the backing's length, an array builtin
                // the frontend does not yet model.
                _ => return unsupported_stmt(stmt.span, "array iteration"),
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
                if variant_named(name, &ty) {
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
                // `module.name` is a qualified reference to an imported item; the
                // module prefix carries no runtime value, so it resolves to the bare
                // name the item is known by.
                Some(Type::Module(_)) => IrExprKind::Var(name.clone()),
                _ => IrExprKind::Field {
                    target: Box::new(self.expr(target)?),
                    name: name.clone(),
                },
            },
            ExprKind::Index { array, index } => IrExprKind::Index {
                array: Box::new(self.expr(array)?),
                index: Box::new(self.expr(index)?),
            },
            ExprKind::Deref(operand) => IrExprKind::Deref(Box::new(self.expr(operand)?)),
            ExprKind::AddressOf(operand) => IrExprKind::AddressOf(Box::new(self.expr(operand)?)),
            ExprKind::Assign { op, target, value } => IrExprKind::Assign {
                op: *op,
                target: Box::new(self.expr(target)?),
                value: Box::new(self.expr(value)?),
            },
            ExprKind::Let { name, value } => IrExprKind::Let {
                name: name.clone(),
                value: Box::new(self.expr(value)?),
            },
            ExprKind::If { cond, then, els } => {
                let scrutinee = self.expr(cond)?;
                let then = self.expr(then)?;
                let els = self.expr(els)?;
                let patterns = bool_patterns(&scrutinee.ty, scrutinee.span);
                let tree = compile_match(&scrutinee.ty, &patterns);
                IrExprKind::Match {
                    scrutinee: Box::new(scrutinee),
                    actions: vec![then, els],
                    tree: Box::new(tree),
                }
            }
            ExprKind::Block(block) => self.value_block(block)?,
            ExprKind::StructLiteral { members, .. } => {
                let Type::Struct { name, .. } = &ty else {
                    return Err(LowerError::MissingType(expr.span));
                };
                IrExprKind::StructLiteral {
                    name: name.clone(),
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
                let tree = compile_match(&scrutinee.ty, &patterns);
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
                let Type::Oneof { variants, .. } = ty else {
                    unreachable!("a checked variant pattern matches a sum type");
                };
                let payload = variants
                    .get(variant)
                    .expect("a checked pattern names an existing variant");
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
                let Type::Struct { members, .. } = ty else {
                    unreachable!("a checked struct pattern matches a struct type");
                };
                let fields = fields
                    .iter()
                    .map(|field| {
                        let member_ty = members
                            .get(&field.name)
                            .expect("a checked struct pattern names existing fields");
                        Ok((field.name.clone(), self.pattern(member_ty, &field.pattern)?))
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
        Ok(IrExprKind::Call {
            callee: Box::new(self.expr(callee)?),
            args: self.each(args)?,
        })
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
            ExprKind::Ident(name) => (variant_named(name, result)
                && self.globals.lookup_func(name).is_none())
            .then(|| name.clone()),
            _ => None,
        }
    }
}

/// Whether `name` is a variant of sum type `ty`. Used to tell a bare variant value
/// apart from an ordinary variable of some sum type.
fn variant_named(name: &str, ty: &Type) -> bool {
    matches!(ty, Type::Oneof { variants, .. } if variants.contains_key(name))
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
fn compile_match(ty: &Type, patterns: &[IrPattern]) -> Decision {
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
    compile(occurrences, rows)
}

/// The matrix algorithm: the first row whose columns are all irrefutable wins;
/// otherwise test the leftmost column the top row cares about, deconstructing a
/// single-constructor type in place or branching on a `Switch`.
fn compile(occurrences: Vec<Occurrence>, mut rows: Vec<Row>) -> Decision {
    let Some(first) = rows.first() else {
        return Decision::Fail;
    };
    if first.columns.iter().all(ir_is_irrefutable) {
        let mut row = rows.swap_remove(0);
        for (occ, pat) in occurrences.iter().zip(&row.columns) {
            collect_bindings(pat, &occ.access, &occ.ty, &mut row.bindings);
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
        Type::Tuple(_) | Type::Struct { .. } => deconstruct(col, occurrences, rows),
        _ => switch(col, occurrences, rows),
    }
}

/// Expand a single-constructor (tuple or struct) column into its fields: the column
/// is replaced by one occurrence per field, and every row's pattern there by its
/// sub-patterns (missing struct fields and irrefutable patterns become wildcards).
fn deconstruct(col: usize, occurrences: Vec<Occurrence>, rows: Vec<Row>) -> Decision {
    let subs = sub_fields(&occurrences[col].access, &occurrences[col].ty);
    let mut new_occurrences = occurrences.clone();
    new_occurrences.splice(col..=col, subs.clone());
    let new_rows = rows
        .into_iter()
        .map(|mut row| {
            let pat = row.columns.remove(col);
            let subpats = deconstruct_pattern(pat, &occurrences[col], &subs, &mut row.bindings);
            row.columns.splice(col..col, subpats);
            row
        })
        .collect();
    compile(new_occurrences, new_rows)
}

/// The sub-patterns a tuple/struct column pattern contributes, aligned with `subs`.
/// A binding over the whole value is recorded before it is dropped to wildcards.
fn deconstruct_pattern(
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
            let Type::Struct { members, .. } = &occ.ty else {
                unreachable!("a struct pattern matches a struct occurrence");
            };
            sorted_members(members)
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
fn switch(col: usize, occurrences: Vec<Occurrence>, rows: Vec<Row>) -> Decision {
    let access = occurrences[col].access.clone();
    let ty = occurrences[col].ty.clone();
    let tests = head_tests(&rows, col);
    let mut cases = Vec::new();
    for test in &tests {
        let subs = test_sub_occurrences(&access, &ty, test);
        let mut case_occurrences = occurrences.clone();
        case_occurrences.splice(col..=col, subs.clone());
        let case_rows = rows
            .iter()
            .filter_map(|row| specialize_row(row, col, test, &access, &ty, &subs))
            .collect();
        cases.push(Case {
            test: test.clone(),
            tree: compile(case_occurrences, case_rows),
        });
    }
    let default = if tests_exhaustive(&ty, &tests) {
        None
    } else {
        let mut default_occurrences = occurrences.clone();
        default_occurrences.remove(col);
        let default_rows = rows
            .iter()
            .filter(|row| ir_is_irrefutable(&row.columns[col]))
            .map(|row| default_row(row, col, &access, &ty))
            .collect();
        Some(Box::new(compile(default_occurrences, default_rows)))
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
fn test_sub_occurrences(access: &Access, ty: &Type, test: &Test) -> Vec<Occurrence> {
    match test {
        Test::Variant(variant) => {
            let Type::Oneof { variants, .. } = ty else {
                unreachable!("a variant test discriminates a sum type");
            };
            let payload = variants
                .get(variant)
                .expect("a variant test names an existing variant");
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
fn tests_exhaustive(ty: &Type, tests: &[Test]) -> bool {
    match ty {
        Type::Primitive(name) if name == "bool" => {
            tests.contains(&Test::Bool(true)) && tests.contains(&Test::Bool(false))
        }
        Type::Oneof { variant_order, .. } => variant_order
            .iter()
            .all(|v| tests.contains(&Test::Variant(v.clone()))),
        _ => false,
    }
}

/// The occurrences a tuple/struct value exposes, in a deterministic order (tuple
/// position, struct fields sorted by name).
fn sub_fields(access: &Access, ty: &Type) -> Vec<Occurrence> {
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
        Type::Struct { members, .. } => sorted_members(members)
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
fn sorted_members(members: &HashMap<String, Type>) -> Vec<(String, Type)> {
    let mut pairs: Vec<_> = members
        .iter()
        .map(|(name, ty)| (name.clone(), ty.clone()))
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
fn collect_bindings(pat: &IrPattern, access: &Access, ty: &Type, out: &mut Vec<MatchBinding>) {
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
            let Type::Struct { members, .. } = ty else {
                unreachable!("a struct pattern matches a struct type");
            };
            for (name, sub) in fields {
                let member_ty = members
                    .get(name)
                    .expect("a checked struct pattern names existing fields");
                collect_bindings(
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
mod tests {
    use super::*;
    use cooper_frontend::resolve::resolve_into;
    use cooper_frontend::{lexer, parser, typecheck};

    /// Run the frontend over a snippet and return its declarations, the resolved
    /// globals, and the type table the checker hands off.
    fn check(source: &str) -> (Vec<Stmt>, Globals, TypeTable) {
        let tokens = lexer::tokenize(source).expect("snippet lexes");
        let parsed = parser::parse(tokens);
        assert!(parsed.errors.is_empty(), "snippet parses: {:?}", parsed.errors);
        let (globals, diags) = resolve_into(Globals::default(), &parsed.decls);
        assert!(diags.is_empty(), "snippet resolves: {diags:?}");
        let checked = typecheck::check(&parsed.decls, &globals, &Default::default());
        assert!(checked.diags.is_empty(), "snippet checks: {:?}", checked.diags);
        (parsed.decls, globals, checked.types)
    }

    /// The expression returned by the sole function's trailing `return`.
    fn return_expr(decls: &[Stmt]) -> &Expr {
        let StmtKind::FuncDecl(func) = &decls[0].kind else {
            panic!("expected a function declaration");
        };
        for stmt in &func.body {
            if let StmtKind::Return(Some(expr)) = &stmt.kind {
                return expr;
            }
        }
        panic!("expected a return statement");
    }

    fn is_primitive(ty: &Type, name: &str) -> bool {
        matches!(ty, Type::Primitive(n) if n == name)
    }

    /// The switch case testing a given sum-type variant.
    fn case_for<'a>(cases: &'a [Case], variant: &str) -> &'a Case {
        cases
            .iter()
            .find(|c| matches!(&c.test, Test::Variant(v) if v == variant))
            .expect("a case for the variant")
    }

    #[test]
    fn lowers_a_typed_arithmetic_expression() {
        let (decls, globals, types) = check("func add(a: i32, b: i32): i32 { return a + b }");
        let ir = lower_expr(return_expr(&decls), &globals, &types).expect("lowers");

        assert!(is_primitive(&ir.ty, "i32"), "result type carried: {:?}", ir.ty);
        let IrExprKind::Binary { op, lhs, rhs } = &ir.kind else {
            panic!("expected a binary node, got {:?}", ir.kind);
        };
        assert_eq!(*op, BinaryOp::Add);
        assert!(matches!(&lhs.kind, IrExprKind::Var(n) if n == "a"));
        assert!(is_primitive(&lhs.ty, "i32"));
        assert!(matches!(&rhs.kind, IrExprKind::Var(n) if n == "b"));
        assert!(is_primitive(&rhs.ty, "i32"));
    }

    #[test]
    fn grouping_is_collapsed_and_literals_keep_their_inferred_width() {
        let (decls, globals, types) = check("func f(x: i64): i64 { return (x + 1) }");
        let ir = lower_expr(return_expr(&decls), &globals, &types).expect("lowers");

        // The parenthesised group is gone; the binary is the root.
        let IrExprKind::Binary { rhs, .. } = &ir.kind else {
            panic!("expected a binary node, got {:?}", ir.kind);
        };
        // The bare literal `1` adopted the i64 width from `x`.
        assert!(matches!(&rhs.kind, IrExprKind::Int(1)));
        assert!(is_primitive(&rhs.ty, "i64"), "literal width: {:?}", rhs.ty);
    }

    #[test]
    fn lowers_a_function_signature_and_a_conditional_body() {
        let (decls, globals, types) = check(
            "func max(a: i32, b: i32): i32 {\n\
             \tif a > b then { return a } else { return b }\n\
             \treturn a\n\
             }",
        );
        let functions = lower_module(&decls, &globals, &types).expect("module lowers");

        assert_eq!(functions.len(), 1);
        let max = &functions[0];
        assert_eq!(max.name, "max");
        assert!(max.receiver.is_none());
        assert_eq!(max.params.len(), 2);
        assert_eq!(max.params[0].name, "a");
        assert!(is_primitive(&max.params[0].ty, "i32"));
        assert!(is_primitive(&max.return_type, "i32"));

        // The body is an `if` statement (desugared to a boolean `match`) followed
        // by a trailing return. The match tests the condition and selects between a
        // `then` action and an `else` action.
        let IrStmtKind::Match {
            scrutinee,
            actions,
            tree,
        } = &max.body[0].kind
        else {
            panic!("expected a match, got {:?}", max.body[0].kind);
        };
        assert!(is_primitive(&scrutinee.ty, "bool"));
        assert_eq!(actions.len(), 2);
        assert!(matches!(actions[0].kind, IrStmtKind::Block(_)));
        assert!(matches!(actions[1].kind, IrStmtKind::Block(_)));
        // The tree switches on the boolean and is exhaustive (no default).
        let Decision::Switch { cases, default, .. } = tree else {
            panic!("expected a switch, got {tree:?}");
        };
        assert_eq!(cases.len(), 2);
        assert!(default.is_none());
        assert!(matches!(max.body[1].kind, IrStmtKind::Return(Some(_))));
    }

    #[test]
    fn lowers_a_local_binding_taking_its_type_from_the_initializer() {
        let (decls, globals, types) =
            check("func f(n: i32): i32 { x := n + 1\n\treturn x }");
        let functions = lower_module(&decls, &globals, &types).expect("module lowers");

        // `x := n + 1` lowers to an expression statement holding the walrus binding,
        // carrying the i32 type flowing from the initializer.
        let IrStmtKind::Expr(expr) = &functions[0].body[0].kind else {
            panic!("expected an expression statement, got {:?}", functions[0].body[0].kind);
        };
        let IrExprKind::Let { name, value } = &expr.kind else {
            panic!("expected a walrus binding, got {:?}", expr.kind);
        };
        assert_eq!(name, "x");
        assert!(is_primitive(&value.ty, "i32"));
    }

    #[test]
    fn numeric_literals_are_decoded_to_their_values() {
        // A hexadecimal integer with separators and a float both decode exactly.
        let (decls, globals, types) =
            check("func f(): u16 { return 0xFF_FF }");
        let ir = lower_expr(return_expr(&decls), &globals, &types).expect("lowers");
        assert!(matches!(ir.kind, IrExprKind::Int(0xFFFF)));
        assert!(is_primitive(&ir.ty, "u16"));

        let (decls, globals, types) = check("func g(): f64 { return 1.5 }");
        let ir = lower_expr(return_expr(&decls), &globals, &types).expect("lowers");
        let IrExprKind::Float(v) = ir.kind else {
            panic!("expected a float literal, got {:?}", ir.kind);
        };
        assert_eq!(v, 1.5);
        assert!(is_primitive(&ir.ty, "f64"));
    }

    #[test]
    fn lowers_a_struct_literal_with_its_members_in_source_order() {
        let (decls, globals, types) = check(
            "struct Point { x: i32, y: i32 }\n\
             func origin(): Point { return Point { x: 1, y: 2 } }",
        );
        // The function whose body returns the literal is the second declaration.
        let StmtKind::FuncDecl(func) = &decls[1].kind else {
            panic!("expected the origin function");
        };
        let StmtKind::Return(Some(expr)) = &func.body[0].kind else {
            panic!("expected a return");
        };
        let ir = lower_expr(expr, &globals, &types).expect("lowers");

        let IrExprKind::StructLiteral { name, members } = &ir.kind else {
            panic!("expected a struct literal, got {:?}", ir.kind);
        };
        assert_eq!(name, "Point");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].0, "x");
        assert!(is_primitive(&members[0].1.ty, "i32"));
        assert_eq!(members[1].0, "y");
    }

    #[test]
    fn lowers_a_match_expression_binding_variant_payloads() {
        let (decls, globals, types) = check(
            "oneof Shape { Circle(i32), Rect(i32, i32) }\n\
             func area(s: Shape): i32 {\n\
             \ta := match s with {\n\
             \t\tShape.Circle(r) => { r }\n\
             \t\tShape.Rect(w, h) => { w }\n\
             \t}\n\
             \treturn a\n\
             }",
        );
        // The match is the value of the `a := …` binding in the second declaration.
        let StmtKind::FuncDecl(func) = &decls[1].kind else {
            panic!("expected the area function");
        };
        let StmtKind::Expression(Expr { kind: ExprKind::Let { value, .. }, .. }) =
            &func.body[0].kind
        else {
            panic!("expected a walrus binding");
        };
        let ir = lower_expr(value, &globals, &types).expect("lowers");

        let IrExprKind::Match {
            scrutinee,
            actions,
            tree,
        } = &ir.kind
        else {
            panic!("expected a match, got {:?}", ir.kind);
        };
        assert!(matches!(&scrutinee.kind, IrExprKind::Var(n) if n == "s"));
        assert!(is_primitive(&ir.ty, "i32"), "match result type: {:?}", ir.ty);
        assert_eq!(actions.len(), 2);

        // The tree switches on the sum type, exhaustively (no default), with a case
        // per variant whose leaf binds its payload slots by access.
        let Decision::Switch { cases, default, .. } = tree.as_ref() else {
            panic!("expected a switch, got {tree:?}");
        };
        assert!(default.is_none(), "all variants covered");
        assert_eq!(cases.len(), 2);

        let circle = case_for(cases, "Circle");
        let Decision::Leaf { bindings, action } = &circle.tree else {
            panic!("expected a leaf");
        };
        assert_eq!(*action, 0);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].name, "r");
        assert!(is_primitive(&bindings[0].ty, "i32"));
        assert!(matches!(
            &bindings[0].access,
            Access::Payload { variant, index: 0, .. } if variant == "Circle"
        ));

        let rect = case_for(cases, "Rect");
        let Decision::Leaf { bindings, action } = &rect.tree else {
            panic!("expected a leaf");
        };
        assert_eq!(*action, 1);
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[1].name, "h");
        assert!(matches!(
            &bindings[1].access,
            Access::Payload { variant, index: 1, .. } if variant == "Rect"
        ));
    }

    #[test]
    fn generic_variant_binders_lower_with_instantiated_slot_types() {
        // A scrutinee of an instantiated generic sum type binds payload slots at
        // their substituted types, not the sum type's parameters.
        let (decls, globals, types) = check(
            "oneof Maybe T { Some(T), None }\n\
             func unwrap(m: Maybe i32): i32 {\n\
             \ta := match m with {\n\
             \t\tMaybe.Some(x) => { x }\n\
             \t\tMaybe.None => { 0 }\n\
             \t}\n\
             \treturn a\n\
             }",
        );
        let StmtKind::FuncDecl(func) = &decls[1].kind else {
            panic!("expected the unwrap function");
        };
        let StmtKind::Expression(Expr { kind: ExprKind::Let { value, .. }, .. }) =
            &func.body[0].kind
        else {
            panic!("expected a walrus binding");
        };
        let ir = lower_expr(value, &globals, &types).expect("lowers");
        let IrExprKind::Match { tree, .. } = &ir.kind else {
            panic!("expected a match");
        };
        let Decision::Switch { cases, .. } = tree.as_ref() else {
            panic!("expected a switch");
        };
        let some = case_for(cases, "Some");
        let Decision::Leaf { bindings, .. } = &some.tree else {
            panic!("expected a leaf");
        };
        assert_eq!(bindings[0].name, "x");
        assert!(
            is_primitive(&bindings[0].ty, "i32"),
            "payload slot instantiated to i32, got {:?}",
            bindings[0].ty
        );
    }

    #[test]
    fn lowers_a_tuple_match_into_a_nested_decision_tree() {
        // A tuple scrutinee is deconstructed into its components; a literal in the
        // first position becomes a switch on that component, and the irrefutable
        // catch-all supplies the default (binding both components by access).
        let (decls, globals, types) = check(
            "func classify(p: (i32, i32)): i32 {\n\
             \treturn match p with {\n\
             \t\t(0, y) => { y }\n\
             \t\t(x, y) => { x }\n\
             \t}\n\
             }",
        );
        let ir = lower_expr(return_expr(&decls), &globals, &types).expect("lowers");
        let IrExprKind::Match { actions, tree, .. } = &ir.kind else {
            panic!("expected a match, got {:?}", ir.kind);
        };
        assert_eq!(actions.len(), 2);

        // The root tuple is deconstructed: the switch tests the first component.
        let Decision::Switch {
            access,
            ty,
            cases,
            default,
        } = tree.as_ref()
        else {
            panic!("expected a switch, got {tree:?}");
        };
        assert!(matches!(access, Access::Elem { index: 0, .. }));
        assert!(is_primitive(ty, "i32"));
        assert_eq!(cases.len(), 1);
        assert_eq!(
            cases[0].test,
            Test::Int {
                value: 0,
                negative: false
            }
        );

        // Matching `0` leaves the first arm, binding `y` to the second component.
        let Decision::Leaf { bindings, action } = &cases[0].tree else {
            panic!("expected a leaf under the matched literal");
        };
        assert_eq!(*action, 0);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].name, "y");
        assert!(matches!(&bindings[0].access, Access::Elem { index: 1, .. }));

        // Any other first component falls through to the catch-all, binding both.
        let Some(default) = default else {
            panic!("an integer switch needs a default");
        };
        let Decision::Leaf { bindings, action } = default.as_ref() else {
            panic!("expected a leaf default");
        };
        assert_eq!(*action, 1);
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].name, "x");
        assert!(matches!(&bindings[0].access, Access::Elem { index: 0, .. }));
        assert_eq!(bindings[1].name, "y");
    }

    #[test]
    fn lowers_variant_construction_and_payload_free_access() {
        let (decls, globals, types) = check(
            "oneof Maybe T { Some(T), None }\n\
             func some(): Maybe i32 { return Maybe.Some(5) }\n\
             func none(): Maybe i32 { return Maybe.None }",
        );
        let variant_return = |idx: usize| {
            let StmtKind::FuncDecl(func) = &decls[idx].kind else {
                panic!("expected a function at {idx}");
            };
            let StmtKind::Return(Some(expr)) = &func.body[0].kind else {
                panic!("expected a return");
            };
            lower_expr(expr, &globals, &types).expect("lowers")
        };

        // `Maybe.Some(5)` is a payloaded construction carrying the instantiated type.
        let some = variant_return(1);
        let IrExprKind::Variant { variant, args } = &some.kind else {
            panic!("expected a variant, got {:?}", some.kind);
        };
        assert_eq!(variant, "Some");
        assert_eq!(args.len(), 1);
        assert!(is_primitive(&args[0].ty, "i32"));
        assert!(matches!(&some.ty, Type::Oneof { name, .. } if name == "Maybe"));

        // `Maybe.None` is a payload-free variant value, not a field access.
        let none = variant_return(2);
        let IrExprKind::Variant { variant, args } = &none.kind else {
            panic!("expected a variant, got {:?}", none.kind);
        };
        assert_eq!(variant, "None");
        assert!(args.is_empty());
    }

    #[test]
    fn numeric_conversions_lower_to_a_convert_node() {
        let (decls, globals, types) = check("func widen(x: i32): i64 { return i64(x) }");
        let ir = lower_expr(return_expr(&decls), &globals, &types).expect("lowers");
        let IrExprKind::Convert(value) = &ir.kind else {
            panic!("expected a conversion, got {:?}", ir.kind);
        };
        assert!(is_primitive(&ir.ty, "i64"), "destination type: {:?}", ir.ty);
        assert!(is_primitive(&value.ty, "i32"), "source type: {:?}", value.ty);
        assert!(matches!(&value.kind, IrExprKind::Var(n) if n == "x"));
    }

    #[test]
    fn array_iteration_is_reported_not_panicked() {
        let (decls, globals, types) = check(
            "func sum(xs: i32[]): i32 {\n\
             \ttotal := 0\n\
             \tfor x in xs do { total += x }\n\
             \treturn total\n\
             }",
        );
        let err = lower_module(&decls, &globals, &types).expect_err("array iteration is unsupported");
        assert!(matches!(err, LowerError::Unsupported { what: "array iteration", .. }));
    }

    #[test]
    fn lowers_a_range_for_loop_with_a_typed_counter() {
        let (decls, globals, types) = check(
            "func count(): i32 {\n\
             \ttotal := 0\n\
             \tfor i in 0..10 do { total += i }\n\
             \treturn total\n\
             }",
        );
        let functions = lower_module(&decls, &globals, &types).expect("module lowers");
        let IrStmtKind::ForRange {
            var, ty, inclusive, body, ..
        } = &functions[0].body[1].kind
        else {
            panic!("expected a range loop, got {:?}", functions[0].body[1].kind);
        };
        assert_eq!(var, "i");
        assert!(is_primitive(ty, "i32"), "counter type: {ty:?}");
        assert!(!inclusive);
        assert_eq!(body.len(), 1);
    }

    #[test]
    fn module_qualified_reference_drops_its_prefix() {
        // A dependency exports a function; the main module reaches it qualified as
        // `dep.helper(..)`. The module prefix carries no runtime value, so the call's
        // callee lowers to the bare name the function is known by.
        let dep_tokens = lexer::tokenize("func helper(): i32 { return 1 }").expect("dep lexes");
        let dep = parser::parse(dep_tokens);
        assert!(dep.errors.is_empty(), "dep parses: {:?}", dep.errors);
        let (dep_globals, dep_diags) = resolve_into(Globals::default(), &dep.decls);
        assert!(dep_diags.is_empty(), "dep resolves: {dep_diags:?}");

        let main_tokens =
            lexer::tokenize("func main(): i32 { return dep.helper() }").expect("main lexes");
        let main = parser::parse(main_tokens);
        assert!(main.errors.is_empty(), "main parses: {:?}", main.errors);
        let (globals, diags) = resolve_into(Globals::default(), &main.decls);
        assert!(diags.is_empty(), "main resolves: {diags:?}");

        let modules = HashMap::from([(vec!["dep".to_string()], dep_globals)]);
        let checked = typecheck::check(&main.decls, &globals, &modules);
        assert!(checked.diags.is_empty(), "main checks: {:?}", checked.diags);

        let ir = lower_expr(return_expr(&main.decls), &globals, &checked.types).expect("lowers");
        let IrExprKind::Call { callee, args } = &ir.kind else {
            panic!("expected a call, got {:?}", ir.kind);
        };
        assert!(args.is_empty());
        assert!(
            matches!(&callee.kind, IrExprKind::Var(n) if n == "helper"),
            "module prefix dropped to a bare callee, got {:?}",
            callee.kind
        );
    }

    #[test]
    fn lowers_tuple_destructuring_into_typed_bindings() {
        let (decls, globals, types) = check(
            "func f(): i32 {\n\
             \tp := (1, 2)\n\
             \t(a, b) := p\n\
             \treturn a + b\n\
             }",
        );
        let StmtKind::FuncDecl(func) = &decls[0].kind else {
            panic!("expected a function");
        };
        let StmtKind::Expression(expr) = &func.body[1].kind else {
            panic!("expected the destructuring statement");
        };
        let ir = lower_expr(expr, &globals, &types).expect("lowers");
        let IrExprKind::LetTuple { bindings, value } = &ir.kind else {
            panic!("expected a tuple destructuring, got {:?}", ir.kind);
        };
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].name, "a");
        assert!(is_primitive(&bindings[0].ty, "i32"));
        assert_eq!(bindings[1].name, "b");
        assert!(matches!(&value.kind, IrExprKind::Var(n) if n == "p"));
    }
}
