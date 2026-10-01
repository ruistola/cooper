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
    AssignOp, BinaryOp, Block, Expr, ExprKind, FuncDecl, Stmt, StmtKind, TypeExpr, TypedIdent,
    UnaryOp,
};
use cooper_frontend::diag::Span;
use cooper_frontend::resolve::{receiver_pattern_params, resolve_type, Globals};
use cooper_frontend::typecheck::{decode_number_literal, LiteralValue};
use cooper_frontend::types::Type;

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
    Block(Vec<IrStmt>),
    Break,
    Continue,
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
            StmtKind::ForIn { .. } => return unsupported_stmt(stmt.span, "for-in loop"),
            StmtKind::Match { .. } => return unsupported_stmt(stmt.span, "match statement"),
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
            ExprKind::Ident(name) => IrExprKind::Var(name.clone()),
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
            ExprKind::Call { callee, args } => IrExprKind::Call {
                callee: Box::new(self.expr(callee)?),
                args: self.each(args)?,
            },
            ExprKind::Field { target, name } => IrExprKind::Field {
                target: Box::new(self.expr(target)?),
                name: name.clone(),
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
            ExprKind::Group(_) => unreachable!("grouping collapsed above"),
            ExprKind::Range { .. } => return unsupported(expr.span, "range"),
            ExprKind::StructLiteral { .. } => return unsupported(expr.span, "struct literal"),
            ExprKind::Match { .. } => return unsupported(expr.span, "match expression"),
            ExprKind::LetTuple { .. } => return unsupported(expr.span, "tuple destructuring"),
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

    fn each(&self, exprs: &[Expr]) -> Result<Vec<IrExpr>, LowerError> {
        exprs.iter().map(|e| self.expr(e)).collect()
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
    fn for_in_loops_are_reported_not_panicked() {
        let (decls, globals, types) = check(
            "func sum(xs: i32[]): i32 {\n\
             \ttotal := 0\n\
             \tfor x in xs do { total += x }\n\
             \treturn total\n\
             }",
        );
        let err = lower_module(&decls, &globals, &types).expect_err("for-in is unsupported");
        assert!(matches!(err, LowerError::Unsupported { what: "for-in loop", .. }));
    }
}
