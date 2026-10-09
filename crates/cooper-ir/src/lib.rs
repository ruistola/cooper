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

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt;

mod decision;
mod ir;
mod mono;

pub use ir::*;
pub use mono::{monomorphize, MonoError};

use decision::{bool_patterns, compile_match};

use cooper_frontend::ast::{
    AssignOp, BinaryOp, Block, Expr, ExprKind, FuncDecl, Pattern, PatternKind, Stmt, StmtKind,
    TypeExpr, TypedIdent, UnaryOp,
};
use cooper_frontend::diag::Span;
use cooper_frontend::resolve::{
    receiver_pattern_params, resolve_type, underlying_struct_name, Callable, Globals, Signature,
};
use cooper_frontend::typecheck::{
    decode_number_literal, decode_string_literal, ItemRef, LiteralValue, Typed,
};
use cooper_frontend::{stdlib, CheckedProject};
use cooper_frontend::types::{is_numeric_name, Type, TypeDefs, INDEX_INT};

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
    /// A numeric or string literal's spelling did not decode to a value. Like `UnresolvedType`,
    /// this is unreachable after a clean type check (the checker rejects out-of-range
    /// literals) and signals an internal inconsistency.
    MalformedLiteral(Span),
    /// A syntactic form not yet handled by lowering.
    Unsupported { span: Span, what: &'static str },
    /// An `extern func` whose C symbol another declaration gives a different signature.
    ConflictingExtern { span: Span, name: String },
}

impl LowerError {
    /// The source span the error refers to.
    pub fn span(&self) -> Span {
        match self {
            LowerError::MissingType(span)
            | LowerError::UnresolvedType(span)
            | LowerError::MalformedLiteral(span)
            | LowerError::Unsupported { span, .. }
            | LowerError::ConflictingExtern { span, .. } => *span,
        }
    }
}

impl fmt::Display for LowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LowerError::MissingType(_) => write!(f, "internal error: expression has no type"),
            LowerError::UnresolvedType(_) => write!(f, "internal error: type did not resolve"),
            LowerError::MalformedLiteral(_) => write!(f, "internal error: literal did not decode"),
            LowerError::Unsupported { what, .. } => write!(f, "{what} is not supported yet"),
            LowerError::ConflictingExtern { name, .. } => {
                write!(f, "extern function `{name}` is declared elsewhere with a different signature")
            }
        }
    }
}

impl std::error::Error for LowerError {}

/// The type map keyed by span that the type checker hands off.
pub type TypeTable = HashMap<Span, Type>;

/// A lowered, monomorphized program: every function instance it runs, none generic,
/// and the definitions of the struct and sum types their types name (a [`Type`]
/// names a declaration by identity only, so a backend reads layouts here).
#[derive(Debug, Clone)]
pub struct Program {
    pub functions: Vec<Function>,
    /// The C functions the program declares, once per symbol.
    pub externs: Vec<Extern>,
    pub defs: TypeDefs,
}

/// Why a program could not be lowered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramError {
    /// A file failed to lower; `file` names it, since spans index into one file.
    Lower { file: String, error: LowerError },
    Mono(MonoError),
}

impl ProgramError {
    /// The source file and span the error refers to.
    pub fn location(&self) -> (&str, Span) {
        match self {
            ProgramError::Lower { file, error } => (file, error.span()),
            ProgramError::Mono(error) => error.location(),
        }
    }
}

impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProgramError::Lower { error, .. } => write!(f, "{error}"),
            ProgramError::Mono(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProgramError {}

/// Lower every file of a checked project and monomorphize the whole: the IR a
/// backend consumes.
pub fn lower_program(project: &CheckedProject) -> Result<Program, ProgramError> {
    let mut functions = Vec::new();
    let mut externs: Vec<Extern> = Vec::new();
    for file in &project.files {
        let error = |error| ProgramError::Lower {
            file: file.name.clone(),
            error,
        };
        functions.extend(lower_module(&file.name, &file.decls, &file.globals, &file.typed).map_err(error)?);
        for (declared, span) in file_externs(&file.decls, &file.globals) {
            match externs.iter().find(|e| e.name == declared.name) {
                Some(existing) if !existing.same_signature(&declared) => {
                    return Err(error(LowerError::ConflictingExtern {
                        span,
                        name: declared.name,
                    }))
                }
                Some(_) => {}
                None => externs.push(declared),
            }
        }
    }
    Ok(Program {
        functions: monomorphize(&functions).map_err(ProgramError::Mono)?,
        externs,
        defs: project.defs.clone(),
    })
}

/// The C functions a file's top-level declarations declare, with their signatures.
fn file_externs(decls: &[Stmt], globals: &Globals) -> Vec<(Extern, Span)> {
    decls
        .iter()
        .filter_map(|decl| {
            let StmtKind::ExternFunc { name, .. } = &decl.kind else {
                return None;
            };
            let Type::Func { return_type, param_types } = &globals.lookup_func(name)?.ty else {
                return None;
            };
            let declared = Extern {
                name: name.clone(),
                params: param_types.clone(),
                return_type: (**return_type).clone(),
            };
            Some((declared, decl.span))
        })
        .collect()
}

/// Lower the type-checked top-level declarations of the source file `file` into typed
/// functions.
/// Struct and sum-type declarations carry no runnable body and are skipped; every
/// other top-level form that is not a function is rejected as unsupported.
pub fn lower_module(
    file: &str,
    decls: &[Stmt],
    globals: &Globals,
    checked: &Typed,
) -> Result<Vec<Function>, LowerError> {
    let mut functions = Vec::new();
    for decl in decls {
        match &decl.kind {
            StmtKind::FuncDecl(func) => {
                functions.extend(lower_function(file, func, globals, checked)?);
            }
            StmtKind::StructDecl { .. } | StmtKind::OneofDecl { .. } | StmtKind::ExternFunc { .. } => {}
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

/// Lower a single type-checked function declaration, followed by every function
/// literal and nested function lifted out of its body.
fn lower_function(
    file: &str,
    func: &FuncDecl,
    globals: &Globals,
    checked: &Typed,
) -> Result<Vec<Function>, LowerError> {
    let mut type_params: HashSet<String> = func.type_params.iter().cloned().collect();
    if let Some(receiver) = &func.receiver {
        type_params.extend(receiver_pattern_params(&receiver.ty));
    }
    let signature = signature_of(func, globals).ok_or(LowerError::UnresolvedType(func.span))?;
    let lower = Lower {
        types: &checked.types,
        refs: &checked.refs,
        captures: &checked.captures,
        globals,
        type_params,
        lifting: Some(Lifting {
            file: file.to_string(),
            parent: signature.item.clone(),
            binders: signature.type_params.clone(),
            next: Cell::new(0),
            lifted: RefCell::new(Vec::new()),
        }),
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
    let mut functions = vec![Function {
        item: signature.item.clone(),
        file: file.to_string(),
        receiver,
        env: None,
        type_params: signature.type_params.clone(),
        type_args: Vec::new(),
        params,
        return_type,
        body,
        span: func.span,
    }];
    functions.extend(lower.lifting.expect("set above").lifted.into_inner());
    Ok(functions)
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
        captures: &checked.captures,
        globals,
        type_params: HashSet::new(),
        lifting: None,
    }
    .expr(expr)
}

/// The resolved-context that lowering threads through a function body: the type
/// and reference tables to read from, the globals and type parameters a signature
/// annotation resolves against.
struct Lower<'a> {
    types: &'a TypeTable,
    refs: &'a HashMap<Span, ItemRef>,
    captures: &'a HashMap<Span, Vec<(String, Type)>>,
    globals: &'a Globals,
    type_params: HashSet<String>,
    /// Where function literals are lifted to; absent when lowering a lone expression.
    lifting: Option<Lifting>,
}

/// The function literals and nested functions lifted out of one top-level declaration.
struct Lifting {
    file: String,
    parent: Callable,
    /// The declaration's binders, which every lifted function inherits.
    binders: Vec<String>,
    /// The index the next lifted function takes.
    next: Cell<u32>,
    lifted: RefCell<Vec<Function>>,
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
        let mut out = Vec::with_capacity(stmts.len());
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::FuncDecl(func) => out.extend(self.nested_func(func)?),
                _ => out.push(self.stmt(stmt)?),
            }
        }
        Ok(out)
    }

    /// A nested function: its variable, bound first (to an unassigned function value)
    /// so the closure stored into it can capture it and recurse.
    fn nested_func(&self, func: &FuncDecl) -> Result<[IrStmt; 2], LowerError> {
        let params: Vec<_> = func.params.iter().map(|p| (p.name.as_str(), p.span)).collect();
        let closure = self.closure(&params, &func.body, func.span)?;
        let ty = closure.ty.clone();
        let var = |kind| IrExpr {
            kind,
            ty: ty.clone(),
            span: func.span,
        };
        let assign = var(IrExprKind::Assign {
            op: AssignOp::Assign,
            target: Box::new(var(IrExprKind::Var(func.name.clone()))),
            value: Box::new(closure),
        });
        Ok([
            IrStmt {
                kind: IrStmtKind::Var {
                    name: func.name.clone(),
                    ty: ty.clone(),
                    init: None,
                },
                span: func.span,
            },
            IrStmt {
                kind: IrStmtKind::Expr(assign),
                span: func.span,
            },
        ])
    }

    /// Lift a function literal or nested function out as a function of its own,
    /// returning the closure that creates its function value. Its signature is the
    /// function type the checker gave it, inferred parts included.
    fn closure(&self, params: &[(&str, Span)], body: &[Stmt], span: Span) -> Result<IrExpr, LowerError> {
        let Some(lifting) = &self.lifting else {
            return unsupported(span, "a function literal outside a function");
        };
        // Numbered before the body, so literals nested in it number after this one.
        let index = lifting.next.get();
        lifting.next.set(index + 1);
        let ty = self.types.get(&span).cloned().ok_or(LowerError::MissingType(span))?;
        let Type::Func { return_type, param_types } = &ty else {
            unreachable!("a function literal has a function type");
        };
        let params = params
            .iter()
            .zip(param_types)
            .map(|(&(name, span), ty)| Param {
                name: name.to_string(),
                ty: ty.clone(),
                span,
            })
            .collect();
        let return_type = (**return_type).clone();
        let captured = self.captures.get(&span).ok_or(LowerError::MissingType(span))?;
        let env = captured
            .iter()
            .map(|(name, ty)| Param {
                name: name.clone(),
                ty: ty.clone(),
                span,
            })
            .collect();
        let body = self.block(body)?;
        self.lift_at(index, params, return_type, body, env, ty, span)
    }

    /// Lift a function with the given signature, body, and environment out of the
    /// declaration being lowered, returning the closure that creates its value.
    fn lift(
        &self,
        params: Vec<Param>,
        return_type: Type,
        body: Vec<IrStmt>,
        env: Vec<Param>,
        ty: Type,
        span: Span,
    ) -> Result<IrExpr, LowerError> {
        let Some(lifting) = &self.lifting else {
            return unsupported(span, "a function value outside a function");
        };
        let index = lifting.next.get();
        lifting.next.set(index + 1);
        self.lift_at(index, params, return_type, body, env, ty, span)
    }

    /// [`Self::lift`] at an index already taken.
    #[allow(clippy::too_many_arguments)]
    fn lift_at(
        &self,
        index: u32,
        params: Vec<Param>,
        return_type: Type,
        body: Vec<IrStmt>,
        env: Vec<Param>,
        ty: Type,
        span: Span,
    ) -> Result<IrExpr, LowerError> {
        let lifting = self.lifting.as_ref().expect("checked by the caller");
        let item = Callable::Closure {
            parent: Box::new(lifting.parent.clone()),
            index,
        };
        let captures = env.iter().map(|p| p.name.clone()).collect();
        lifting.lifted.borrow_mut().push(Function {
            item: item.clone(),
            file: lifting.file.clone(),
            receiver: None,
            env: Some(env),
            type_params: lifting.binders.clone(),
            type_args: Vec::new(),
            params,
            return_type,
            body,
            span,
        });
        Ok(IrExpr {
            kind: IrExprKind::Closure {
                item,
                type_args: lifting.binders.iter().map(|b| Type::TypeParam(b.clone())).collect(),
                captures,
            },
            ty,
            span,
        })
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
                                ty: Type::Primitive(INDEX_INT.to_string()),
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
            StmtKind::FuncDecl(func) => IrStmtKind::Block(self.nested_func(func)?.into()),
            StmtKind::StructDecl { .. } | StmtKind::OneofDecl { .. } => {
                return unsupported_stmt(stmt.span, "local type declaration");
            }
            StmtKind::ExternFunc { .. } => unreachable!("the checker rejects a local extern"),
        };
        Ok(IrStmt {
            kind,
            span: stmt.span,
        })
    }

    fn expr(&self, expr: &Expr) -> Result<IrExpr, LowerError> {
        match &expr.kind {
            ExprKind::Group(inner) => return self.expr(inner),
            ExprKind::Func { params, body, .. } => {
                let params: Vec<_> = params.iter().map(|p| (p.name.as_str(), p.span)).collect();
                return self.closure(&params, body, expr.span);
            }
            _ => {}
        }
        let ty = self
            .types
            .get(&expr.span)
            .cloned()
            .ok_or(LowerError::MissingType(expr.span))?;
        if matches!(ty, Type::Infer(_)) {
            unreachable!("inference variables are resolved before lowering");
        }
        // A reference to a function or method names its declaration; a module prefix
        // carries no runtime value and drops away.
        if let Some(reference) = self.refs.get(&expr.span) {
            if let Some(op) = standard_intrinsic(&reference.item) {
                return self.wrapper(ty, expr.span, |args| IrExprKind::Intrinsic { op, args });
            }
            if matches!(reference.item, Callable::Extern { .. }) {
                let item = reference.item.clone();
                let callee_ty = ty.clone();
                return self.wrapper(ty, expr.span, |args| IrExprKind::Call {
                    callee: Box::new(IrExpr {
                        kind: IrExprKind::FuncRef {
                            item,
                            type_args: Vec::new(),
                        },
                        ty: callee_ty,
                        span: expr.span,
                    }),
                    args,
                });
            }
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
            ExprKind::Str(text) => IrExprKind::Str(
                decode_string_literal(text).map_err(|_| LowerError::MalformedLiteral(expr.span))?,
            ),
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
            ExprKind::Slice {
                array,
                start,
                end,
                inclusive,
            } => self.slice(array, start.as_deref(), end.as_deref(), *inclusive, expr.span)?,
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
            ExprKind::Chain { operands, ops } => {
                let operands = self.each(operands)?;
                return Ok(chain(operands, ops, &ty, expr.span));
            }
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
            ExprKind::Group(_) | ExprKind::Func { .. } => unreachable!("lowered above"),
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
        let stored = match op.binary() {
            None => value,
            Some(binary) => {
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
                        op: binary,
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
            if is_numeric_name(name) || name == "string" {
                let operand = self.expr(&args[0])?;
                if stdlib::is_c_string(&operand.ty) {
                    return Ok(IrExprKind::Intrinsic {
                        op: Intrinsic::FromCString,
                        args: vec![operand],
                    });
                }
                return Ok(IrExprKind::Convert(Box::new(operand)));
            }
        }
        // A callee of struct type is a type name: `CString(s)`.
        if self.types.get(&strip_group(callee).span).is_some_and(stdlib::is_c_string) {
            return Ok(IrExprKind::Intrinsic {
                op: Intrinsic::ToCString,
                args: self.each(args)?,
            });
        }
        if let Some(op) = self.standard_function(callee) {
            return Ok(IrExprKind::Intrinsic {
                op,
                args: self.each(args)?,
            });
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
        // A C function is called directly, not through a function value.
        if let Some(reference) = self.refs.get(&strip_group(callee).span) {
            if matches!(reference.item, Callable::Extern { .. }) {
                let callee = IrExpr {
                    kind: IrExprKind::FuncRef {
                        item: reference.item.clone(),
                        type_args: Vec::new(),
                    },
                    ty: self.types.get(&callee.span).cloned().ok_or(LowerError::MissingType(callee.span))?,
                    span: callee.span,
                };
                return Ok(IrExprKind::Call {
                    callee: Box::new(callee),
                    args: self.each(args)?,
                });
            }
        }
        Ok(IrExprKind::Call {
            callee: Box::new(self.expr(callee)?),
            args: self.each(args)?,
        })
    }

    /// The intrinsic a call to a compiler-provided standard library function lowers to,
    /// if `callee` names one.
    fn standard_function(&self, callee: &Expr) -> Option<Intrinsic> {
        standard_intrinsic(&self.refs.get(&strip_group(callee).span)?.item)
    }

    /// A function the backend only calls directly (a standard library function or a C
    /// function) used as a value of function type `ty`: a closure of a lifted function
    /// passing its parameters to the call `call` builds, as if the program had written
    /// `func(s: string) { io.println(s) }`.
    fn wrapper(
        &self,
        ty: Type,
        span: Span,
        call: impl FnOnce(Vec<IrExpr>) -> IrExprKind,
    ) -> Result<IrExpr, LowerError> {
        let Type::Func { return_type, param_types } = &ty else {
            unreachable!("a function reference has a function type");
        };
        // Named with a `.`, which no source identifier contains.
        let params: Vec<Param> = param_types
            .iter()
            .enumerate()
            .map(|(index, ty)| Param {
                name: format!("arg.{index}"),
                ty: ty.clone(),
                span,
            })
            .collect();
        let args = params
            .iter()
            .map(|p| IrExpr {
                kind: IrExprKind::Var(p.name.clone()),
                ty: p.ty.clone(),
                span,
            })
            .collect();
        let call = IrExpr {
            kind: call(args),
            ty: (**return_type).clone(),
            span,
        };
        let body = vec![IrStmt {
            kind: IrStmtKind::Return(Some(call)),
            span,
        }];
        self.lift(params, (**return_type).clone(), body, Vec::new(), ty, span)
    }

    /// Lower a slice `array[start..end]` to the slice intrinsic. An omitted start is
    /// position 0; an omitted end is the array's end.
    fn slice(
        &self,
        array: &Expr,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        span: Span,
    ) -> Result<IrExprKind, LowerError> {
        let start = match start {
            Some(start) => self.expr(start)?,
            None => IrExpr {
                kind: IrExprKind::Int(0),
                ty: Type::Primitive(INDEX_INT.to_string()),
                span,
            },
        };
        let mut args = vec![self.expr(array)?, start];
        if let Some(end) = end {
            args.push(self.expr(end)?);
        }
        Ok(IrExprKind::Intrinsic {
            op: Intrinsic::ArraySlice { inclusive },
            args,
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
            "copy" => Intrinsic::ArrayCopy,
            "reserve" => Intrinsic::ArrayReserve,
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

/// Desugar a comparison chain into nested value blocks,
/// `{ t0 := a; { t1 := b; t0 < t1 and t1 < c } }`: each operand but the last is bound
/// to a temporary just before the pair that first reads it, so every operand is
/// evaluated at most once and in order, and `and` stops at the first false pair.
/// Temporaries are named with a `.`, which no source identifier contains.
fn chain(mut operands: Vec<IrExpr>, ops: &[BinaryOp], boolean: &Type, span: Span) -> IrExpr {
    // The checker unified every operand to one type.
    let operand_ty = operands[0].ty.clone();
    let name = |i: usize| format!("chain.{}.{i}", span.start);
    let temp = |i: usize| IrExpr {
        kind: IrExprKind::Var(name(i)),
        ty: operand_ty.clone(),
        span,
    };
    let block = |i: usize, operand: IrExpr, result: IrExpr| IrExpr {
        kind: IrExprKind::Block {
            stmts: vec![IrStmt {
                span: operand.span,
                kind: IrStmtKind::Var {
                    name: name(i),
                    ty: operand_ty.clone(),
                    init: Some(operand),
                },
            }],
            result: Box::new(result),
        },
        ty: boolean.clone(),
        span,
    };
    let binary = |op: BinaryOp, lhs: IrExpr, rhs: IrExpr| IrExpr {
        kind: IrExprKind::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        ty: boolean.clone(),
        span,
    };
    let last = operands.pop().expect("a chain has operands");
    let n = ops.len();
    let mut rest = binary(ops[n - 1], temp(n - 1), last);
    for i in (0..n - 1).rev() {
        let operand = operands.pop().expect("one operand per operator");
        let pair = binary(ops[i], temp(i), temp(i + 1));
        rest = block(i + 1, operand, binary(BinaryOp::And, pair, rest));
    }
    let first = operands.pop().expect("one operand per operator");
    block(0, first, rest)
}

/// The intrinsic a compiler-provided standard library function lowers to, if `item`
/// is one.
fn standard_intrinsic(item: &Callable) -> Option<Intrinsic> {
    let Callable::Func { module, name } = item else {
        return None;
    };
    Some(match (module.as_str(), name.as_str()) {
        (stdlib::IO_MODULE, "print") => Intrinsic::Print { newline: false },
        (stdlib::IO_MODULE, "println") => Intrinsic::Print { newline: true },
        (stdlib::FFI_MODULE, "copyBytes") => Intrinsic::CopyBytes,
        (module, name) if module.starts_with("std.") => {
            unreachable!("`{module}.{name}` is not a standard function")
        }
        _ => return None,
    })
}

fn unsupported(span: Span, what: &'static str) -> Result<IrExpr, LowerError> {
    Err(LowerError::Unsupported { span, what })
}

fn unsupported_stmt(span: Span, what: &'static str) -> Result<IrStmt, LowerError> {
    Err(LowerError::Unsupported { span, what })
}

#[cfg(test)]
mod tests;
