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
mod format;
mod init;
mod ir;
mod mono;

pub use ir::*;
pub use mono::{monomorphize, monomorphize_from, MonoError};

use decision::{bool_patterns, compile_match};

use cooper_frontend::ast::{
    AssignOp, BinaryOp, Block, Expr, ExprKind, FormatSpec, FuncDecl, Pattern, PatternKind, Stmt, StmtKind,
    StrPart,
    TypeExpr, TypeExprKind, TypedIdent, UnaryOp,
};
use cooper_frontend::diag::Span;
use cooper_frontend::resolve::{
    receiver_pattern_params, resolve_type, underlying_struct_name, Callable, Globals, Signature,
};
use cooper_frontend::typecheck::{
    decode_escapes, decode_number_literal, decode_string_literal, is_binder, ItemRef, LiteralValue, LocalRef, Typed, VarRef,
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
    /// A module-level variable whose initializer depends, directly or through the
    /// functions it calls, on itself or on a variable that depends on it.
    InitializationCycle { span: Span, name: String },
}

impl LowerError {
    /// The source span the error refers to.
    pub fn span(&self) -> Span {
        match self {
            LowerError::MissingType(span)
            | LowerError::UnresolvedType(span)
            | LowerError::MalformedLiteral(span)
            | LowerError::Unsupported { span, .. }
            | LowerError::ConflictingExtern { span, .. }
            | LowerError::InitializationCycle { span, .. } => *span,
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
            LowerError::InitializationCycle { name, .. } => {
                write!(f, "initialization cycle: the initializer of `{name}` depends on `{name}`")
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
    /// The module-level variables, each zero-initialized.
    pub globals: Vec<Global>,
    /// The initializer functions to run, in order, before `main`: each module's after
    /// those of the modules it depends on.
    pub initializers: Vec<Callable>,
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
    let mut variables = Vec::new();
    for file in &project.files {
        let error = |error| ProgramError::Lower {
            file: file.name.clone(),
            error,
        };
        functions.extend(lower_module(&file.name, &file.decls, &file.globals, &file.typed).map_err(error)?);
        variables.extend(file_variables(&file.name, &file.decls, &file.globals));
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
    let initializers = init::order(&variables, &functions)?;
    // The program is what `main` and the module initializers reach.
    let roots: Vec<Callable> = functions
        .iter()
        .map(|f| &f.item)
        .filter(|item| {
            matches!(item, Callable::Initializer { .. })
                || matches!(item, Callable::Func { module, name } if module == "main" && name == "main")
        })
        .cloned()
        .collect();
    let functions = monomorphize_from(&functions, &roots).map_err(ProgramError::Mono)?;
    Ok(Program {
        functions: format::expand_formatting(functions, &project.defs),
        externs,
        globals: variables.into_iter().map(|v| v.global).collect(),
        initializers,
        defs: project.defs.clone(),
    })
}

/// A module-level variable a file declares, with where it is declared.
struct Variable {
    global: Global,
    file: String,
    span: Span,
    /// Whether the declaration has an initializer; without one the variable keeps its
    /// zero value.
    initialized: bool,
}

/// The module-level variables a file's top-level declarations declare.
fn file_variables(file: &str, decls: &[Stmt], globals: &Globals) -> Vec<Variable> {
    decls
        .iter()
        .filter_map(|decl| {
            let StmtKind::VarDecl { name, init, .. } = &decl.kind else {
                return None;
            };
            let var = globals.vars.get(name)?;
            Some(Variable {
                global: Global {
                    module: var.module.clone(),
                    name: name.clone(),
                    ty: var.ty.clone(),
                },
                file: file.to_string(),
                span: decl.span,
                initialized: init.is_some(),
            })
        })
        .collect()
}

/// The C functions a file's top-level declarations declare, with their signatures.
fn file_externs(decls: &[Stmt], globals: &Globals) -> Vec<(Extern, Span)> {
    decls
        .iter()
        .filter_map(|decl| {
            let StmtKind::ExternFunc { name, .. } = &decl.kind else {
                return None;
            };
            let signature = globals.lookup_func(name)?;
            let Type::Func { return_type, param_types } = &signature.ty else {
                return None;
            };
            let declared = Extern {
                name: name.clone(),
                params: param_types.clone(),
                variadic: signature.variadic,
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
            StmtKind::VarDecl { init: None, .. } => {}
            StmtKind::VarDecl { name, init: Some(init), .. } => {
                functions.extend(lower_initializer(file, decl.span, name, init, globals, checked)?);
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

/// Lower the initializer of the module-level variable `name` into a function that
/// evaluates it and stores the result, followed by every function literal lifted out of
/// the expression.
fn lower_initializer(
    file: &str,
    span: Span,
    name: &str,
    init: &Expr,
    globals: &Globals,
    checked: &Typed,
) -> Result<Vec<Function>, LowerError> {
    let var = globals.vars.get(name).ok_or(LowerError::UnresolvedType(span))?;
    let item = Callable::Initializer {
        module: var.module.clone(),
        name: name.to_string(),
    };
    let lower = Lower {
        types: &checked.types,
        refs: &checked.refs,
        captures: &checked.captures,
        variables: &checked.variables,
        local_refs: &checked.local_refs,
        aliases: &checked.aliases,
        dispatch: &checked.dispatch,
        packed: &checked.packed,
        defers: RefCell::new(Vec::new()),
        globals,
        type_params: RefCell::new(HashSet::new()),
        lifting: Some(Lifting {
            file: file.to_string(),
            parent: item.clone(),
            binders: RefCell::new(Vec::new()),
            generics: RefCell::new(HashMap::new()),
            next: Cell::new(0),
            lifted: RefCell::new(Vec::new()),
        }),
    };
    let target = IrExpr {
        kind: IrExprKind::Global {
            module: var.module.clone(),
            name: name.to_string(),
        },
        ty: var.ty.clone(),
        span,
    };
    let store = IrExpr {
        kind: IrExprKind::Assign {
            op: AssignOp::Assign,
            target: Box::new(target),
            value: Box::new(lower.expr(init)?),
        },
        ty: var.ty.clone(),
        span,
    };
    let mut functions = vec![Function {
        item,
        file: file.to_string(),
        receiver: None,
        env: None,
        type_params: Vec::new(),
        type_args: Vec::new(),
        params: Vec::new(),
        return_type: Type::Unit,
        body: vec![IrStmt {
            kind: IrStmtKind::Expr(store),
            span,
        }],
        span,
    }];
    functions.extend(lower.lifting.expect("set above").lifted.into_inner());
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
        variables: &checked.variables,
        local_refs: &checked.local_refs,
        aliases: &checked.aliases,
        dispatch: &checked.dispatch,
        packed: &checked.packed,
        defers: RefCell::new(Vec::new()),
        globals,
        type_params: RefCell::new(type_params),
        lifting: Some(Lifting {
            file: file.to_string(),
            parent: signature.item.clone(),
            binders: RefCell::new(signature.type_params.clone()),
            generics: RefCell::new(HashMap::new()),
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
        Some(receiver) if matches!(&receiver.ty.kind, TypeExprKind::Named(name) if name == "string") => {
            let owner = cooper_frontend::stdlib::builtin_owner(&Type::Primitive("string".to_string()))?;
            globals.lookup_method(&owner, &func.name)
        }
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
        variables: &checked.variables,
        local_refs: &checked.local_refs,
        aliases: &checked.aliases,
        dispatch: &checked.dispatch,
        packed: &checked.packed,
        defers: RefCell::new(Vec::new()),
        globals,
        type_params: RefCell::new(HashSet::new()),
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
    variables: &'a HashMap<Span, VarRef>,
    local_refs: &'a HashMap<Span, LocalRef>,
    aliases: &'a HashMap<Span, String>,
    dispatch: &'a HashMap<(Span, usize), ItemRef>,
    packed: &'a HashMap<Span, usize>,
    /// The blocks being lowered, innermost last, with the deferred calls each has met.
    defers: RefCell<Vec<DeferScope>>,
    globals: &'a Globals,
    type_params: RefCell<HashSet<String>>,
    /// Where function literals are lifted to; absent when lowering a lone expression.
    lifting: Option<Lifting>,
}

/// A block being lowered: the calls of the statements it has deferred so far, in order,
/// and whether it is the body of a loop, which `break` and `continue` leave.
struct DeferScope {
    calls: Vec<IrStmt>,
    loop_body: bool,
}

/// The function literals and nested functions lifted out of one top-level declaration.
struct Lifting {
    file: String,
    parent: Callable,
    /// The binders in scope, which every lifted function inherits: the declaration's,
    /// then those of each generic nested function being lowered.
    binders: RefCell<Vec<String>>,
    /// Each generic nested function lowered so far, by the span of its declaration: its
    /// lifted function and the binders in scope where it is declared, which precede its own.
    generics: RefCell<HashMap<Span, (Callable, Vec<String>)>>,
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
        resolve_type(ty, &self.type_params.borrow(), self.globals, &mut diags)
            .ok_or(LowerError::UnresolvedType(ty.span))
    }

    fn block(&self, stmts: &[Stmt]) -> Result<Vec<IrStmt>, LowerError> {
        self.scope(false, stmts)
    }

    /// The body of a loop: a block that `break` and `continue` leave.
    fn loop_block(&self, stmts: &[Stmt]) -> Result<Vec<IrStmt>, LowerError> {
        self.scope(true, stmts)
    }

    /// Lower the statements of a block, then the calls it deferred, last deferred first,
    /// unless the block ends by leaving, which has run them already.
    fn scope(&self, loop_body: bool, stmts: &[Stmt]) -> Result<Vec<IrStmt>, LowerError> {
        let (mut out, calls) = self.deferring(loop_body, |this| this.statements(stmts))?;
        if !matches!(stmts.last().map(|s| &s.kind), Some(StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue)) {
            out.extend(calls);
        }
        Ok(out)
    }

    /// Run `lower` in a new block, returning its result and the calls the block deferred,
    /// in the order they run.
    fn deferring<T>(
        &self,
        loop_body: bool,
        lower: impl FnOnce(&Self) -> Result<T, LowerError>,
    ) -> Result<(T, Vec<IrStmt>), LowerError> {
        self.defers.borrow_mut().push(DeferScope { calls: Vec::new(), loop_body });
        let result = lower(self);
        let scope = self.defers.borrow_mut().pop().expect("pushed above");
        let mut calls = scope.calls;
        calls.reverse();
        Ok((result?, calls))
    }

    fn statements(&self, stmts: &[Stmt]) -> Result<Vec<IrStmt>, LowerError> {
        let mut out = Vec::with_capacity(stmts.len());
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::FuncDecl(func) => out.extend(self.nested_func(func)?),
                StmtKind::Defer(body) => out.push(self.defer(body, stmt.span)?),
                _ => out.push(self.stmt(stmt)?),
            }
        }
        Ok(out)
    }

    /// A deferred statement: its function value is made here, and called by every exit
    /// from the block that follows.
    fn defer(&self, body: &Stmt, span: Span) -> Result<IrStmt, LowerError> {
        let closure = self.closure(&[], std::slice::from_ref(body), span, &[])?;
        let Type::Func { return_type, .. } = &closure.ty else {
            unreachable!("a function literal has a function type");
        };
        let name = format!("defer.{}", span.start);
        let function = |ty: Type| IrExpr {
            kind: IrExprKind::Var(name.clone()),
            ty,
            span,
        };
        let call = IrExpr {
            ty: (**return_type).clone(),
            kind: IrExprKind::Call {
                callee: Box::new(function(closure.ty.clone())),
                args: Vec::new(),
            },
            span,
        };
        self.defers
            .borrow_mut()
            .last_mut()
            .expect("a deferred statement is in a block")
            .calls
            .push(IrStmt { kind: IrStmtKind::Expr(call), span });
        Ok(IrStmt {
            kind: IrStmtKind::Var {
                name,
                ty: closure.ty.clone(),
                init: Some(closure),
            },
            span,
        })
    }

    /// The calls that leaving the blocks of the current function down to, and with, the
    /// innermost loop body (or all of them, for a `return`) must run, innermost first.
    fn unwinding(&self, to_loop: bool) -> Vec<IrStmt> {
        let mut calls = Vec::new();
        for scope in self.defers.borrow().iter().rev() {
            calls.extend(scope.calls.iter().rev().cloned());
            if to_loop && scope.loop_body {
                break;
            }
        }
        calls
    }

    /// A `return`, `break`, or `continue` as one statement.
    fn leave(&self, jump: IrStmtKind, span: Span) -> IrStmt {
        let mut stmts = self.jump(jump, span);
        if stmts.len() == 1 {
            return stmts.remove(0);
        }
        IrStmt { kind: IrStmtKind::Block(stmts), span }
    }

    /// The statements for a `return`, `break`, or `continue` of `jump`: the returned value
    /// is evaluated first, then the deferred calls run, then the jump.
    fn jump(&self, jump: IrStmtKind, span: Span) -> Vec<IrStmt> {
        let calls = self.unwinding(!matches!(jump, IrStmtKind::Return(_)));
        if calls.is_empty() {
            return vec![IrStmt { kind: jump, span }];
        }
        let statement = |kind| IrStmt { kind, span };
        let mut out = Vec::new();
        let jump = match jump {
            IrStmtKind::Return(Some(value)) if matches!(value.ty, Type::Unit) => {
                out.push(statement(IrStmtKind::Expr(value)));
                IrStmtKind::Return(None)
            }
            IrStmtKind::Return(Some(value)) => {
                let name = format!("return.{}", span.start);
                let ty = value.ty.clone();
                let saved = IrExpr { kind: IrExprKind::Var(name.clone()), ty: ty.clone(), span };
                out.push(statement(IrStmtKind::Var { name, ty, init: Some(value) }));
                IrStmtKind::Return(Some(saved))
            }
            other => other,
        };
        out.extend(calls);
        out.push(statement(jump));
        out
    }

    /// A nested function: its variable, bound first (to an unassigned function value)
    /// so the closure stored into it can capture it and recurse.
    fn nested_func(&self, func: &FuncDecl) -> Result<[IrStmt; 2], LowerError> {
        let params: Vec<_> = func.params.iter().map(|p| (p.name.as_str(), p.span)).collect();
        let mut closure = self.closure(&params, &func.body, func.span, &func.type_params)?;
        // A generic function's variable holds only its environment; each use supplies
        // the code of its instantiation.
        if !func.type_params.is_empty() {
            let IrExprKind::Closure { captures, .. } = closure.kind else {
                unreachable!("lifting yields a closure");
            };
            closure.kind = IrExprKind::GenericClosure { captures };
        }
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
    fn closure(&self, params: &[(&str, Span)], body: &[Stmt], span: Span, own: &[String]) -> Result<IrExpr, LowerError> {
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
        // A generic function's own binders are in scope in its body and lifted with it.
        let outer = lifting.binders.borrow().clone();
        if !own.is_empty() {
            let item = Callable::Closure {
                parent: Box::new(lifting.parent.clone()),
                index,
            };
            lifting.generics.borrow_mut().insert(span, (item, outer.clone()));
            lifting.binders.borrow_mut().extend(own.iter().cloned());
            self.type_params.borrow_mut().extend(own.iter().cloned());
        }
        // A function's deferred calls belong to its own blocks.
        let enclosing = self.defers.replace(Vec::new());
        let body = self.block(body);
        self.defers.replace(enclosing);
        let body = body?;
        let lifted = self.lift_at(index, params, return_type, body, env, ty, span);
        lifting.binders.borrow_mut().truncate(outer.len());
        own.iter().for_each(|binder| {
            self.type_params.borrow_mut().remove(binder);
        });
        lifted
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
            type_params: lifting.binders.borrow().clone(),
            type_args: Vec::new(),
            params,
            return_type,
            body,
            span,
        });
        Ok(IrExpr {
            kind: IrExprKind::Closure {
                item,
                type_args: lifting.binders.borrow().iter().map(|b| Type::TypeParam(b.clone())).collect(),
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
                let value = value.as_ref().map(|e| self.expr(e)).transpose()?;
                return Ok(self.leave(IrStmtKind::Return(value), stmt.span));
            }
            StmtKind::If { cond, then, els } => {
                let mut declared = Vec::new();
                let scrutinee = self.condition(cond, &mut declared)?;
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
                let branch = IrStmtKind::Match {
                    scrutinee,
                    actions: vec![then, els],
                    tree,
                };
                if declared.is_empty() {
                    branch
                } else {
                    declared.push(IrStmt { kind: branch, span: stmt.span });
                    IrStmtKind::Block(declared)
                }
            }
            StmtKind::While {
                cond,
                body,
                post_test: false,
                until: false,
            } if has_test(cond) => {
                // The variables a test binds are fresh each pass, so the loop tests inside
                // its body and leaves when the test fails.
                let mut declared = Vec::new();
                let tested = self.condition(cond, &mut declared)?;
                let patterns = bool_patterns(&tested.ty, tested.span);
                let tree = compile_match(&self.globals.defs, &tested.ty, &patterns);
                let leave = IrStmt { kind: IrStmtKind::Break, span: stmt.span };
                let run = IrStmt { kind: IrStmtKind::Block(self.loop_block(body)?), span: stmt.span };
                declared.push(IrStmt {
                    kind: IrStmtKind::Match { scrutinee: tested, actions: vec![run, leave], tree },
                    span: stmt.span,
                });
                let always = IrExpr {
                    kind: IrExprKind::Bool(true),
                    ty: Type::Primitive("bool".to_string()),
                    span: cond.span,
                };
                IrStmtKind::While {
                    cond: always,
                    body: vec![IrStmt { kind: IrStmtKind::Block(declared), span: stmt.span }],
                    post_test: false,
                    until: false,
                }
            }
            StmtKind::While {
                cond,
                body,
                post_test,
                until,
            } => IrStmtKind::While {
                cond: self.expr(cond)?,
                body: self.loop_block(body)?,
                post_test: *post_test,
                until: *until,
            },
            StmtKind::Break => return Ok(self.leave(IrStmtKind::Break, stmt.span)),
            StmtKind::Continue => return Ok(self.leave(IrStmtKind::Continue, stmt.span)),
            StmtKind::Defer(_) => unreachable!("a deferred statement is lowered with its block"),
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
                        body: self.loop_block(body)?,
                    }
                }
                // Array iteration drives from the backing's length and reads each
                // element; it binds either `[elem]` or `[index, elem]`.
                _ => {
                    let array = self.expr(iterable)?;
                    let elem_ty = match &array.ty {
                        Type::Array(elem) | Type::StaticArray(elem, _) => (**elem).clone(),
                        // A string yields its runes.
                        ty if is_string(ty) => Type::Primitive("rune".to_string()),
                        _ => unreachable!("a checked for-in iterates a range, an array, or a string"),
                    };
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
                        body: self.loop_block(body)?,
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
                return self.closure(&params, body, expr.span, &[]);
            }
            ExprKind::Return(_) | ExprKind::Break | ExprKind::Continue => {
                let ty = self.types.get(&expr.span).cloned().ok_or(LowerError::MissingType(expr.span))?;
                let jump = match &expr.kind {
                    ExprKind::Return(value) => {
                        IrStmtKind::Return(value.as_ref().map(|v| self.expr(v)).transpose()?)
                    }
                    ExprKind::Break => IrStmtKind::Break,
                    _ => IrStmtKind::Continue,
                };
                return Ok(self.leaving(self.jump(jump, expr.span), ty, expr.span));
            }
            ExprKind::Is { .. } | ExprKind::Binary { op: BinaryOp::And, .. } => {
                let mut declared = Vec::new();
                let tested = self.condition(expr, &mut declared)?;
                return Ok(with_variables(declared, tested));
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
        if let Some(var) = self.variables.get(&expr.span) {
            return Ok(IrExpr {
                kind: IrExprKind::Global {
                    module: var.module.clone(),
                    name: var.name.clone(),
                },
                ty,
                span: expr.span,
            });
        }
        if let (Some(local), ExprKind::Ident(name), Some(lifting)) =
            (self.local_refs.get(&expr.span), &expr.kind, &self.lifting)
        {
            let (item, outer) = lifting.generics.borrow()[&local.decl].clone();
            let type_args = outer
                .into_iter()
                .map(Type::TypeParam)
                .chain(local.type_args.iter().cloned())
                .collect();
            let closure = IrExpr {
                kind: IrExprKind::Var(name.clone()),
                ty: ty.clone(),
                span: expr.span,
            };
            return Ok(IrExpr {
                kind: IrExprKind::Instantiate {
                    closure: Box::new(closure),
                    item,
                    type_args,
                },
                ty,
                span: expr.span,
            });
        }
        // A reference to a function or method names its declaration; a module prefix
        // carries no runtime value and drops away.
        if let Some(reference) = self.refs.get(&expr.span) {
            if let Some(op) = standard_intrinsic(&reference.item) {
                return self.wrapper(ty, expr.span, |args| IrExprKind::Intrinsic { op, args });
            }
            if stdlib::is_format(&reference.item) {
                return self.wrapper(ty, expr.span, |args| {
                    args.into_iter().next().expect("format takes its template").kind
                });
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
            ExprKind::Rune(rune) => IrExprKind::Int(u128::from(u32::from(*rune))),
            ExprKind::Str(text) => IrExprKind::Str(
                decode_string_literal(text).map_err(|_| LowerError::MalformedLiteral(expr.span))?,
            ),
            ExprKind::Interpolated(parts) => return self.interpolated(parts, &[], &ty, expr.span),
            ExprKind::Nil => IrExprKind::Nil,
            ExprKind::Unit => IrExprKind::Unit,
            ExprKind::Ident(name) => {
                if let Some(alias) = self.aliases.get(&expr.span) {
                    IrExprKind::Var(alias.clone())
                } else if variant_named(&self.globals.defs, name, &ty) {
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
            ExprKind::Field { .. } if self.dispatch.contains_key(&(expr.span, 0)) => {
                return unsupported(expr.span, "a method of a sum type used as a value");
            }
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
                let mut declared = Vec::new();
                let scrutinee = self.condition(cond, &mut declared)?;
                let then = self.expr(then)?;
                let els = self.expr(els)?;
                let patterns = bool_patterns(&scrutinee.ty, scrutinee.span);
                let tree = compile_match(&self.globals.defs, &scrutinee.ty, &patterns);
                let branch = IrExpr {
                    kind: IrExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        actions: vec![then, els],
                        tree: Box::new(tree),
                    },
                    ty: ty.clone(),
                    span: expr.span,
                };
                return Ok(with_variables(declared, branch));
            }
            ExprKind::Block(block) => self.value_block(block, &ty)?,
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
            ExprKind::Group(_)
            | ExprKind::Func { .. }
            | ExprKind::Is { .. }
            | ExprKind::Return(_)
            | ExprKind::Spread(_)
            | ExprKind::Break
            | ExprKind::Continue => unreachable!("lowered above"),
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

    /// Lower a condition, one that may test patterns with `is`. Each variable a test binds
    /// is a local named by [`is_binder`], pushed onto `declared` for the caller to bring into
    /// scope around the condition and what it guards, and assigned by the test.
    fn condition(&self, cond: &Expr, declared: &mut Vec<IrStmt>) -> Result<IrExpr, LowerError> {
        match &cond.kind {
            ExprKind::Group(inner) => self.condition(inner, declared),
            ExprKind::Binary { op: BinaryOp::And, lhs, rhs } => {
                let lhs = self.condition(lhs, declared)?;
                let rhs = self.condition(rhs, declared)?;
                let ty = self.types.get(&cond.span).cloned().ok_or(LowerError::MissingType(cond.span))?;
                Ok(IrExpr {
                    kind: IrExprKind::Binary {
                        op: BinaryOp::And,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    ty,
                    span: cond.span,
                })
            }
            ExprKind::Is { value, pattern } => {
                let scrutinee = self.expr(value)?;
                let pattern = self.pattern(&scrutinee.ty, pattern)?;
                let mut binders = Vec::new();
                pattern_binders(&pattern, &mut binders);
                let bool_ty = Type::Primitive("bool".to_string());
                let mut stores = Vec::new();
                for Binder { name, ty } in binders {
                    let alias = is_binder(&name, cond.span);
                    let place = |name: String| IrExpr {
                        kind: IrExprKind::Var(name),
                        ty: ty.clone(),
                        span: cond.span,
                    };
                    stores.push(IrStmt {
                        kind: IrStmtKind::Expr(IrExpr {
                            kind: IrExprKind::Assign {
                                op: AssignOp::Assign,
                                target: Box::new(place(alias.clone())),
                                value: Box::new(place(name)),
                            },
                            ty: ty.clone(),
                            span: cond.span,
                        }),
                        span: cond.span,
                    });
                    declared.push(IrStmt {
                        kind: IrStmtKind::Var { name: alias, ty, init: None },
                        span: cond.span,
                    });
                }
                let truth = |value| IrExpr {
                    kind: IrExprKind::Bool(value),
                    ty: bool_ty.clone(),
                    span: cond.span,
                };
                let yes = if stores.is_empty() {
                    truth(true)
                } else {
                    IrExpr {
                        kind: IrExprKind::Block { stmts: stores, result: Box::new(truth(true)) },
                        ty: bool_ty.clone(),
                        span: cond.span,
                    }
                };
                let other = IrPattern { kind: IrPatternKind::Wildcard, ty: scrutinee.ty.clone(), span: cond.span };
                let patterns = vec![pattern, other];
                let tree = compile_match(&self.globals.defs, &scrutinee.ty, &patterns);
                Ok(IrExpr {
                    kind: IrExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        actions: vec![yes, truth(false)],
                        tree: Box::new(tree),
                    },
                    ty: bool_ty.clone(),
                    span: cond.span,
                })
            }
            _ => self.expr(cond),
        }
    }

    /// An interpolated string, its positional holes filled by `args` in order: each
    /// hole's value and each argument is evaluated once, in that order, then formatted
    /// as `string(x)` formats it and joined with the text.
    fn interpolated(&self, parts: &[StrPart], args: &[Expr], string: &Type, span: Span) -> Result<IrExpr, LowerError> {
        let node = |kind| IrExpr {
            kind,
            ty: string.clone(),
            span,
        };
        let mut stmts = Vec::new();
        let mut bind = |value: IrExpr| {
            let name = format!("fmt.{}.{}", span.start, stmts.len());
            let var = IrExpr {
                kind: IrExprKind::Var(name.clone()),
                ty: value.ty.clone(),
                span: value.span,
            };
            stmts.push(IrStmt {
                span: value.span,
                kind: IrStmtKind::Var {
                    name,
                    ty: value.ty.clone(),
                    init: Some(value),
                },
            });
            var
        };
        let mut holes = Vec::new();
        for part in parts {
            if let StrPart::Hole { expr, .. } = part {
                holes.push(bind(self.expr(expr)?));
            }
        }
        let mut filled = Vec::new();
        for arg in args {
            filled.push(bind(self.expr(arg)?));
        }
        let (mut holes, mut filled) = (holes.into_iter(), filled.into_iter());
        // `string(x)`, or the formatting `spec` asks for.
        let formatted = |value: IrExpr, spec: &Option<FormatSpec>| match spec {
            None => node(IrExprKind::Convert(Box::new(value))),
            Some(spec) => node(IrExprKind::Intrinsic {
                op: Intrinsic::Format(*spec),
                args: vec![value],
            }),
        };
        let mut out: Option<IrExpr> = None;
        for part in parts {
            let piece = match part {
                StrPart::Text(text) if text.is_empty() => continue,
                StrPart::Text(text) => {
                    node(IrExprKind::Str(decode_escapes(text).map_err(|_| LowerError::MalformedLiteral(span))?))
                }
                StrPart::Hole { spec, .. } => formatted(holes.next().expect("bound above"), spec),
                StrPart::Positional { spec, .. } => {
                    formatted(filled.next().expect("the checker matched holes and arguments"), spec)
                }
            };
            out = Some(match out {
                None => piece,
                Some(left) => node(IrExprKind::Binary {
                    op: BinaryOp::Add,
                    lhs: Box::new(left),
                    rhs: Box::new(piece),
                }),
            });
        }
        let result = out.unwrap_or_else(|| node(IrExprKind::Str(String::new())));
        if stmts.is_empty() {
            return Ok(result);
        }
        Ok(node(IrExprKind::Block {
            stmts,
            result: Box::new(result),
        }))
    }

    fn value_block(&self, block: &Block, ty: &Type) -> Result<IrExprKind, LowerError> {
        let ((mut stmts, result), calls) = self.deferring(false, |this| {
            let stmts = this.statements(&block.statements)?;
            let result = if cooper_frontend::typecheck::ends_in_jump(block) {
                IrExpr {
                    kind: IrExprKind::Unreachable,
                    ty: ty.clone(),
                    span: block.result.span,
                }
            } else {
                this.expr(&block.result)?
            };
            Ok((stmts, result))
        })?;
        if calls.is_empty() || cooper_frontend::typecheck::ends_in_jump(block) {
            return Ok(IrExprKind::Block { stmts, result: Box::new(result) });
        }
        // The value is evaluated before the deferred calls run.
        let span = result.span;
        if matches!(result.ty, Type::Unit) {
            stmts.push(IrStmt { kind: IrStmtKind::Expr(result), span });
            stmts.extend(calls);
            let unit = IrExpr { kind: IrExprKind::Unit, ty: Type::Unit, span };
            return Ok(IrExprKind::Block { stmts, result: Box::new(unit) });
        }
        let name = format!("value.{}", span.start);
        let ty = result.ty.clone();
        let saved = IrExpr { kind: IrExprKind::Var(name.clone()), ty: ty.clone(), span };
        stmts.push(IrStmt { kind: IrStmtKind::Var { name, ty, init: Some(result) }, span });
        stmts.extend(calls);
        Ok(IrExprKind::Block { stmts, result: Box::new(saved) })
    }

    /// An expression that runs the jump `stmts` and so never yields its value, of type `ty`.
    fn leaving(&self, stmts: Vec<IrStmt>, ty: Type, span: Span) -> IrExpr {
        IrExpr {
            kind: IrExprKind::Block {
                stmts,
                result: Box::new(IrExpr { kind: IrExprKind::Unreachable, ty: ty.clone(), span }),
            },
            ty,
            span,
        }
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
            PatternKind::Rune(rune) => IrPatternKind::Int {
                value: u128::from(u32::from(*rune)),
                negative: false,
            },
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
            Type::Array(elem) | Type::StaticArray(elem, _) => (**elem).clone(),
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
            // A conversion to `string` of anything but a number or `bool` is expanded
            // after monomorphization, when its operand's type is concrete.
            if is_numeric_name(name) || name == "string" || name == "rune" {
                return Ok(IrExprKind::Convert(Box::new(self.expr(&args[0])?)));
            }
        }
        // A callee of struct type is a type name: `CString(s)`.
        if self.types.get(&strip_group(callee).span).is_some_and(stdlib::is_c_string) {
            return Ok(IrExprKind::Intrinsic {
                op: Intrinsic::ToCString,
                args: self.each(args)?,
            });
        }
        if let Some(reference) = self.refs.get(&strip_group(callee).span) {
            if stdlib::takes_template(&reference.item) {
                return self.template_call(&reference.item, args);
            }
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
        if let Some(call) = self.dispatch_call(callee, args, result)? {
            return Ok(call);
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
            args: self.call_args(callee, args)?,
        })
    }

    /// The arguments of a call. For a variadic function, the arguments after the declared ones
    /// become one array, or are the array a final `xs...` passes.
    fn call_args(&self, callee: &Expr, args: &[Expr]) -> Result<Vec<IrExpr>, LowerError> {
        let callee = strip_group(callee);
        let Some(&fixed) = self.packed.get(&callee.span) else {
            return self.each(args);
        };
        let mut lowered = self.each(&args[..fixed])?;
        let (rest, span) = (&args[fixed..], callee.span);
        if let [Expr { kind: ExprKind::Spread(inner), .. }] = rest {
            lowered.push(self.expr(inner)?);
            return Ok(lowered);
        }
        let Some(Type::Func { param_types, .. }) = self.types.get(&span) else {
            return Err(LowerError::MissingType(span));
        };
        lowered.push(IrExpr {
            kind: IrExprKind::Intrinsic { op: Intrinsic::ArrayLiteral, args: self.each(rest)? },
            ty: param_types[fixed].clone(),
            span,
        });
        Ok(lowered)
    }

    /// A call to `format`, `print`, or `println`: the text of its template, its
    /// positional holes filled by the further arguments, then printed or returned.
    fn template_call(&self, item: &Callable, args: &[Expr]) -> Result<IrExprKind, LowerError> {
        let template = &args[0];
        let string = Type::Primitive("string".to_string());
        let text = match &template.kind {
            ExprKind::Interpolated(parts) => self.interpolated(parts, &args[1..], &string, template.span)?,
            _ => self.expr(template)?,
        };
        if stdlib::is_format(item) {
            return Ok(text.kind);
        }
        // `print` and `println` are ordinary functions of the template's text.
        let callee = IrExpr {
            kind: IrExprKind::FuncRef { item: item.clone(), type_args: Vec::new() },
            ty: Type::Func { return_type: Box::new(Type::Unit), param_types: vec![string] },
            span: template.span,
        };
        Ok(IrExprKind::Call { callee: Box::new(callee), args: vec![text] })
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
        let op = if matches!(self.types.get(&array.span), Some(ty) if is_string(ty)) {
            Intrinsic::StringSlice { inclusive }
        } else {
            Intrinsic::ArraySlice { inclusive }
        };
        Ok(IrExprKind::Intrinsic {
            op,
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
        match self.types.get(&target.span) {
            Some(Type::Array(_) | Type::StaticArray(..)) => {}
            Some(ty) if is_string(ty) && matches!(name.as_str(), "length" | "byteAt" | "data") => {
                let mut lowered = vec![self.expr(target)?];
                for arg in args {
                    lowered.push(self.expr(arg)?);
                }
                let op = match name.as_str() {
                    "byteAt" => Intrinsic::StringByteAt,
                    "data" => Intrinsic::StringData,
                    _ => Intrinsic::StringLength,
                };
                return Ok(Some(IrExprKind::Intrinsic { op, args: lowered }));
            }
            _ => return Ok(None),
        }
        let op = match name.as_str() {
            "length" => Intrinsic::ArrayLength,
            "get" => Intrinsic::ArrayGet,
            "set" => Intrinsic::ArraySet,
            "push" => Intrinsic::ArrayPush,
            "copy" => Intrinsic::ArrayCopy,
            "reserve" => Intrinsic::ArrayReserve,
            "utf8" => Intrinsic::ArrayUtf8,
            _ => unreachable!("a checked array method names a blessed method"),
        };
        let mut lowered = vec![self.expr(target)?];
        for arg in args {
            lowered.push(self.expr(arg)?);
        }
        Ok(Some(IrExprKind::Intrinsic { op, args: lowered }))
    }

    /// A call `value.method(args)` of a method across the variants of a sum type: the receiver
    /// and arguments are evaluated once, in that order, then a `match` on the receiver calls
    /// the method of the variant it holds on that variant's payload.
    fn dispatch_call(&self, callee: &Expr, args: &[Expr], result: &Type) -> Result<Option<IrExprKind>, LowerError> {
        let callee = strip_group(callee);
        let ExprKind::Field { target, .. } = &callee.kind else {
            return Ok(None);
        };
        if !self.dispatch.contains_key(&(callee.span, 0)) {
            return Ok(None);
        }
        let span = callee.span;
        let mut receiver = self.expr(target)?;
        if let Type::Pointer(inner) = &receiver.ty {
            let ty = (**inner).clone();
            receiver = IrExpr { kind: IrExprKind::Deref(Box::new(receiver)), ty, span };
        }
        let oneof = receiver.ty.clone();
        let local = |name: &str, init: IrExpr| {
            let var = IrExpr { kind: IrExprKind::Var(name.to_string()), ty: init.ty.clone(), span };
            let decl = IrStmt {
                kind: IrStmtKind::Var { name: name.to_string(), ty: init.ty.clone(), init: Some(init) },
                span,
            };
            (decl, var)
        };
        let (decl, receiver) = local(&format!("recv.{}", span.start), receiver);
        let mut stmts = vec![decl];
        let mut arguments = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            let (decl, var) = local(&format!("arg.{}.{index}", span.start), self.expr(arg)?);
            stmts.push(decl);
            arguments.push(var);
        }
        let method_ty = Type::Func {
            return_type: Box::new(result.clone()),
            param_types: arguments.iter().map(|a| a.ty.clone()).collect(),
        };
        let payload_name = format!("payload.{}", span.start);
        let mut patterns = Vec::new();
        let mut actions = Vec::new();
        for (index, variant) in self.globals.defs.variant_order(&oneof).iter().enumerate() {
            let payload = self.globals.defs.payload(&oneof, variant).and_then(|p| p.into_iter().next());
            let (Some(payload), Some(method)) = (payload, self.dispatch.get(&(span, index))) else {
                return Err(LowerError::MissingType(span));
            };
            patterns.push(IrPattern {
                kind: IrPatternKind::Variant {
                    variant: variant.clone(),
                    binders: vec![Binder { name: payload_name.clone(), ty: payload.clone() }],
                },
                ty: oneof.clone(),
                span,
            });
            let bound = IrExpr {
                kind: IrExprKind::Method {
                    receiver: Box::new(IrExpr { kind: IrExprKind::Var(payload_name.clone()), ty: payload, span }),
                    item: method.item.clone(),
                    type_args: method.type_args.clone(),
                },
                ty: method_ty.clone(),
                span,
            };
            actions.push(IrExpr {
                kind: IrExprKind::Call { callee: Box::new(bound), args: arguments.clone() },
                ty: result.clone(),
                span,
            });
        }
        let tree = compile_match(&self.globals.defs, &oneof, &patterns);
        let dispatch = IrExpr {
            kind: IrExprKind::Match { scrutinee: Box::new(receiver), actions, tree: Box::new(tree) },
            ty: result.clone(),
            span,
        };
        Ok(Some(IrExprKind::Block { stmts, result: Box::new(dispatch) }))
    }

    /// The variant name a call constructs, if the callee denotes a sum-type variant
    /// rather than a function: a qualified `Oneof.Variant(..)` (target typed as a sum
    /// type) or a bare `Variant(..)` whose result type is a sum type owning that
    /// variant and whose name is not a known function.
    fn variant_callee(&self, callee: &Expr, result: &Type) -> Option<String> {
        match &callee.kind {
            ExprKind::Field { target, name } => match self.types.get(&target.span) {
                Some(oneof @ Type::Oneof { .. }) if self.globals.defs.payload(oneof, name).is_some() => {
                    Some(name.clone())
                }
                _ => None,
            },
            ExprKind::Ident(name) => (variant_named(&self.globals.defs, name, result)
                && self.globals.lookup_func(name).is_none())
            .then(|| name.clone()),
            _ => None,
        }
    }
}

/// Whether the condition `cond` contains a test with `is` that binds variables.
fn has_test(cond: &Expr) -> bool {
    match &cond.kind {
        ExprKind::Group(inner) => has_test(inner),
        ExprKind::Binary { op: BinaryOp::And, lhs, rhs } => has_test(lhs) || has_test(rhs),
        ExprKind::Is { .. } => true,
        _ => false,
    }
}

/// `result` evaluated with the locals `declared` in scope.
fn with_variables(declared: Vec<IrStmt>, result: IrExpr) -> IrExpr {
    if declared.is_empty() {
        return result;
    }
    IrExpr {
        ty: result.ty.clone(),
        span: result.span,
        kind: IrExprKind::Block { stmts: declared, result: Box::new(result) },
    }
}

/// Every name `pattern` binds, with its type.
fn pattern_binders(pattern: &IrPattern, out: &mut Vec<Binder>) {
    match &pattern.kind {
        IrPatternKind::Binding(name) => out.push(Binder { name: name.clone(), ty: pattern.ty.clone() }),
        IrPatternKind::Variant { binders, .. } => {
            out.extend(binders.iter().filter(|b| b.name != "_").cloned());
        }
        IrPatternKind::Tuple(elems) => elems.iter().for_each(|e| pattern_binders(e, out)),
        IrPatternKind::Struct { fields, .. } => fields.iter().for_each(|(_, p)| pattern_binders(p, out)),
        IrPatternKind::Wildcard | IrPatternKind::Bool(_) | IrPatternKind::Int { .. } => {}
    }
}

/// Whether `ty` is the built-in `string`.
fn is_string(ty: &Type) -> bool {
    matches!(ty, Type::Primitive(name) if name == "string")
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
        (stdlib::FFI_MODULE, "copyBytes") => Intrinsic::CopyBytes,
        // `std.ffi` declares nothing else; the rest of `std` is Cooper source, called like
        // any function.
        (module @ stdlib::FFI_MODULE, name) => {
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
