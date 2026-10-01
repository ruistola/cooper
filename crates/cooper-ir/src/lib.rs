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

use std::collections::HashMap;

use cooper_frontend::ast::{AssignOp, BinaryOp, Expr, ExprKind, UnaryOp};
use cooper_frontend::diag::Span;
use cooper_frontend::types::Type;

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
    /// A numeric literal in its source spelling; its resolved type is on the node.
    /// Exact-magnitude decoding is a later lowering step.
    Number(String),
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
}

/// Why an expression could not be lowered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    /// The type checker attributed no type to this span — the expression did not
    /// check, so it cannot be lowered.
    MissingType(Span),
    /// A syntactic form not yet handled by lowering.
    Unsupported { span: Span, what: &'static str },
}

/// The type map keyed by span that the type checker hands off.
pub type TypeTable = HashMap<Span, Type>;

/// Lower one type-checked expression into the typed IR, reading each subexpression's
/// type from `types`. Grouping parentheses carry no semantics and are collapsed.
pub fn lower_expr(expr: &Expr, types: &TypeTable) -> Result<IrExpr, LowerError> {
    if let ExprKind::Group(inner) = &expr.kind {
        return lower_expr(inner, types);
    }
    let ty = types
        .get(&expr.span)
        .cloned()
        .ok_or(LowerError::MissingType(expr.span))?;
    let kind = match &expr.kind {
        ExprKind::Number(text) => IrExprKind::Number(text.clone()),
        ExprKind::Bool(b) => IrExprKind::Bool(*b),
        ExprKind::Str(s) => IrExprKind::Str(s.clone()),
        ExprKind::Nil => IrExprKind::Nil,
        ExprKind::Unit => IrExprKind::Unit,
        ExprKind::Ident(name) => IrExprKind::Var(name.clone()),
        ExprKind::Tuple(elems) => {
            IrExprKind::Tuple(lower_each(elems, types)?)
        }
        ExprKind::Unary { op, operand } => IrExprKind::Unary {
            op: *op,
            operand: Box::new(lower_expr(operand, types)?),
        },
        ExprKind::Binary { op, lhs, rhs } => IrExprKind::Binary {
            op: *op,
            lhs: Box::new(lower_expr(lhs, types)?),
            rhs: Box::new(lower_expr(rhs, types)?),
        },
        ExprKind::Call { callee, args } => IrExprKind::Call {
            callee: Box::new(lower_expr(callee, types)?),
            args: lower_each(args, types)?,
        },
        ExprKind::Field { target, name } => IrExprKind::Field {
            target: Box::new(lower_expr(target, types)?),
            name: name.clone(),
        },
        ExprKind::Index { array, index } => IrExprKind::Index {
            array: Box::new(lower_expr(array, types)?),
            index: Box::new(lower_expr(index, types)?),
        },
        ExprKind::Deref(operand) => IrExprKind::Deref(Box::new(lower_expr(operand, types)?)),
        ExprKind::AddressOf(operand) => {
            IrExprKind::AddressOf(Box::new(lower_expr(operand, types)?))
        }
        ExprKind::Assign { op, target, value } => IrExprKind::Assign {
            op: *op,
            target: Box::new(lower_expr(target, types)?),
            value: Box::new(lower_expr(value, types)?),
        },
        ExprKind::Let { name, value } => IrExprKind::Let {
            name: name.clone(),
            value: Box::new(lower_expr(value, types)?),
        },
        ExprKind::If { cond, then, els } => IrExprKind::If {
            cond: Box::new(lower_expr(cond, types)?),
            then: Box::new(lower_expr(then, types)?),
            els: Box::new(lower_expr(els, types)?),
        },
        ExprKind::Group(_) => unreachable!("grouping collapsed above"),
        ExprKind::Range { .. } => return unsupported(expr.span, "range"),
        ExprKind::Block(_) => return unsupported(expr.span, "block expression"),
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

fn lower_each(exprs: &[Expr], types: &TypeTable) -> Result<Vec<IrExpr>, LowerError> {
    exprs.iter().map(|e| lower_expr(e, types)).collect()
}

fn unsupported(span: Span, what: &'static str) -> Result<IrExpr, LowerError> {
    Err(LowerError::Unsupported { span, what })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cooper_frontend::ast::{Stmt, StmtKind};
    use cooper_frontend::resolve::{resolve_into, Globals};
    use cooper_frontend::{lexer, parser, typecheck};

    /// Run the frontend over a one-function snippet and return its declarations
    /// paired with the type table the checker hands off.
    fn check(source: &str) -> (Vec<Stmt>, TypeTable) {
        let tokens = lexer::tokenize(source).expect("snippet lexes");
        let parsed = parser::parse(tokens);
        assert!(parsed.errors.is_empty(), "snippet parses: {:?}", parsed.errors);
        let (globals, diags) = resolve_into(Globals::default(), &parsed.decls);
        assert!(diags.is_empty(), "snippet resolves: {diags:?}");
        let checked = typecheck::check(&parsed.decls, &globals, &Default::default());
        assert!(checked.diags.is_empty(), "snippet checks: {:?}", checked.diags);
        (parsed.decls, checked.types)
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
        let (decls, types) = check("func add(a: i32, b: i32): i32 { return a + b }");
        let ir = lower_expr(return_expr(&decls), &types).expect("lowers");

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
        let (decls, types) = check("func f(x: i64): i64 { return (x + 1) }");
        let ir = lower_expr(return_expr(&decls), &types).expect("lowers");

        // The parenthesised group is gone; the binary is the root.
        let IrExprKind::Binary { rhs, .. } = &ir.kind else {
            panic!("expected a binary node, got {:?}", ir.kind);
        };
        // The bare literal `1` adopted the i64 width from `x`.
        assert!(matches!(&rhs.kind, IrExprKind::Number(n) if n == "1"));
        assert!(is_primitive(&rhs.ty, "i64"), "literal width: {:?}", rhs.ty);
    }

    #[test]
    fn unsupported_forms_are_reported_not_panicked() {
        let (decls, types) = check("func g(): i32 { return { 0 } }");
        let err = lower_expr(return_expr(&decls), &types).expect_err("block expr is unsupported");
        assert!(matches!(err, LowerError::Unsupported { what: "block expression", .. }));
    }
}
