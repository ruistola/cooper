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
    If {
        cond: IrExpr,
        then: Box<IrStmt>,
        els: Option<Box<IrStmt>>,
    },
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
    /// A `match` run for effect: each arm's body is a statement. Arms are exhaustive
    /// over the scrutinee's sum type (the checker verified coverage).
    Match {
        scrutinee: IrExpr,
        arms: Vec<MatchArm<IrStmt>>,
    },
}

/// One `match` arm: a pattern and a body, generic over whether that body is a
/// statement (`match` statement) or an expression (`match` expression).
#[derive(Debug, Clone)]
pub struct MatchArm<B> {
    pub pattern: IrPattern,
    pub body: B,
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
    If {
        cond: Box<IrExpr>,
        then: Box<IrExpr>,
        els: Box<IrExpr>,
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
    /// A `match` producing a value: every arm's body is an expression of the node's
    /// resolved type.
    Match {
        scrutinee: Box<IrExpr>,
        arms: Vec<MatchArm<IrExpr>>,
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
            StmtKind::If { cond, then, els } => IrStmtKind::If {
                cond: self.expr(cond)?,
                then: Box::new(self.stmt(then)?),
                els: els.as_ref().map(|s| self.stmt(s)).transpose()?.map(Box::new),
            },
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
                let arms = arms
                    .iter()
                    .map(|arm| {
                        Ok(MatchArm {
                            pattern: self.pattern(&scrutinee.ty, &arm.pattern)?,
                            body: self.stmt(&arm.body)?,
                        })
                    })
                    .collect::<Result<_, LowerError>>()?;
                IrStmtKind::Match { scrutinee, arms }
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
            ExprKind::If { cond, then, els } => IrExprKind::If {
                cond: Box::new(self.expr(cond)?),
                then: Box::new(self.expr(then)?),
                els: Box::new(self.expr(els)?),
            },
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
                let arms = arms
                    .iter()
                    .map(|arm| {
                        Ok(MatchArm {
                            pattern: self.pattern(&scrutinee.ty, &arm.pattern)?,
                            body: self.expr(&arm.body)?,
                        })
                    })
                    .collect::<Result<_, LowerError>>()?;
                IrExprKind::Match {
                    scrutinee: Box::new(scrutinee),
                    arms,
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

        // The body is an `if` statement followed by a trailing return.
        assert!(matches!(max.body[0].kind, IrStmtKind::If { .. }));
        let IrStmtKind::If { then, els, .. } = &max.body[0].kind else {
            unreachable!();
        };
        assert!(matches!(then.kind, IrStmtKind::Block(_)));
        assert!(els.is_some());
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

        let IrExprKind::Match { scrutinee, arms } = &ir.kind else {
            panic!("expected a match, got {:?}", ir.kind);
        };
        assert!(matches!(&scrutinee.kind, IrExprKind::Var(n) if n == "s"));
        assert!(is_primitive(&ir.ty, "i32"), "match result type: {:?}", ir.ty);
        assert_eq!(arms.len(), 2);

        let IrPatternKind::Variant { variant, binders } = &arms[0].pattern.kind else {
            panic!("expected a variant pattern");
        };
        assert_eq!(variant, "Circle");
        assert_eq!(binders.len(), 1);
        assert_eq!(binders[0].name, "r");
        assert!(is_primitive(&binders[0].ty, "i32"));

        let IrPatternKind::Variant { binders, .. } = &arms[1].pattern.kind else {
            panic!("expected a variant pattern");
        };
        assert_eq!(binders.len(), 2);
        assert_eq!(binders[1].name, "h");
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
        let IrExprKind::Match { arms, .. } = &ir.kind else {
            panic!("expected a match");
        };
        let IrPatternKind::Variant { binders, .. } = &arms[0].pattern.kind else {
            panic!("expected a variant pattern");
        };
        assert_eq!(binders[0].name, "x");
        assert!(
            is_primitive(&binders[0].ty, "i32"),
            "payload slot instantiated to i32, got {:?}",
            binders[0].ty
        );
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
