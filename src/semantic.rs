//! Semantic analysis: the final pass — control-flow validation.
//!
//! Running only after type checking succeeds, this pass verifies that every
//! function with a non-unit return type returns on all paths and reports code made
//! unreachable by a preceding return. It relies on the type checker having already
//! rejected non-exhaustive matches, so a match returns exactly when all its arms do.

use crate::ast::{Expr, ExprKind, FuncDecl, Stmt, StmtKind};
use crate::diag::{Diagnostic, Span};
use crate::resolve::{self, Globals};
use crate::types::{is_unit, Type};

/// Analyze `module` for control-flow soundness, returning any diagnostics.
pub fn analyze(module: &[Stmt], globals: &Globals) -> Vec<Diagnostic> {
    let mut analyzer = SemanticAnalyzer {
        globals,
        diags: Vec::new(),
    };
    analyzer.analyze_block(module);
    analyzer.diags
}

struct SemanticAnalyzer<'g> {
    globals: &'g Globals,
    diags: Vec<Diagnostic>,
}

impl SemanticAnalyzer<'_> {
    fn analyze_block(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.analyze_stmt(stmt);
        }
        self.check_unreachable_code(stmts);
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Block(stmts) => self.analyze_block(stmts),
            StmtKind::VarDecl { init, .. } => {
                if let Some(init) = init {
                    self.analyze_expr(init);
                }
            }
            StmtKind::StructDecl { .. } | StmtKind::OneofDecl { .. } | StmtKind::Use(_) => {}
            StmtKind::FuncDecl(func) => self.analyze_func_decl(func),
            StmtKind::If { cond, then, els } => {
                self.analyze_expr(cond);
                self.analyze_stmt(then);
                if let Some(els) = els {
                    self.analyze_stmt(els);
                }
            }
            StmtKind::Match { scrutinee, arms } => {
                self.analyze_expr(scrutinee);
                for arm in arms {
                    self.analyze_stmt(&arm.body);
                }
            }
            StmtKind::For {
                init,
                cond,
                iter,
                body,
            } => {
                self.analyze_stmt(init);
                self.analyze_expr(cond);
                self.analyze_expr(iter);
                self.analyze_block(body);
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.analyze_expr(expr);
                }
            }
            StmtKind::Expression(expr) => self.analyze_expr(expr),
        }
    }

    fn analyze_func_decl(&mut self, func: &FuncDecl) {
        self.analyze_block(&func.body);

        // Look up the declared return type: methods are keyed by receiver struct.
        let return_type = match &func.receiver {
            Some(receiver) => resolve::underlying_struct_name(&receiver.ty)
                .and_then(|name| self.globals.lookup_method(name, &func.name)),
            None => self.globals.lookup_func(&func.name),
        };
        let Some(Type::Func { return_type, .. }) = return_type else {
            return;
        };
        if !is_unit(return_type) && !block_returns(&func.body) {
            self.err(
                func.span,
                format!(
                    "function '{}' with return type {return_type} does not return a value in all code paths",
                    func.name
                ),
            );
        }
    }

    fn analyze_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Number(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Nil
            | ExprKind::Unit
            | ExprKind::Ident(_) => {}
            ExprKind::AddressOf(operand) | ExprKind::Deref(operand) => self.analyze_expr(operand),
            ExprKind::Unary { operand, .. } => self.analyze_expr(operand),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.analyze_expr(lhs);
                self.analyze_expr(rhs);
            }
            ExprKind::Group(inner) => self.analyze_expr(inner),
            ExprKind::Tuple(elems) => elems.iter().for_each(|e| self.analyze_expr(e)),
            ExprKind::Call { callee, args } => {
                self.analyze_expr(callee);
                args.iter().for_each(|a| self.analyze_expr(a));
            }
            ExprKind::StructLiteral { target, members } => {
                self.analyze_expr(target);
                members.iter().for_each(|m| self.analyze_expr(&m.value));
            }
            ExprKind::Field { target, .. } => self.analyze_expr(target),
            ExprKind::Index { array, index } => {
                self.analyze_expr(array);
                self.analyze_expr(index);
            }
            ExprKind::Assign { target, value, .. } => {
                self.analyze_expr(target);
                self.analyze_expr(value);
            }
            ExprKind::Let { value, .. } => self.analyze_expr(value),
            ExprKind::LetTuple { value, .. } => self.analyze_expr(value),
            ExprKind::Match { scrutinee, arms } => {
                self.analyze_expr(scrutinee);
                arms.iter().for_each(|a| self.analyze_expr(&a.body));
            }
            ExprKind::If { cond, then, els } => {
                self.analyze_expr(cond);
                self.analyze_expr(then);
                self.analyze_expr(els);
            }
            ExprKind::Block(block) => {
                for stmt in &block.statements {
                    self.analyze_stmt(stmt);
                }
                self.analyze_expr(&block.result);
            }
        }
    }

    fn check_unreachable_code(&mut self, stmts: &[Stmt]) {
        for (i, stmt) in stmts.iter().enumerate().take(stmts.len().saturating_sub(1)) {
            if stmt_returns(stmt) {
                self.err(
                    stmts[i + 1].span,
                    format!("unreachable code after statement {}", i + 1),
                );
                break;
            }
        }
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Block(inner) => self.check_unreachable_code(inner),
                StmtKind::If { then, els, .. } => {
                    if let StmtKind::Block(inner) = &then.kind {
                        self.check_unreachable_code(inner);
                    }
                    if let Some(els) = els {
                        if let StmtKind::Block(inner) = &els.kind {
                            self.check_unreachable_code(inner);
                        }
                    }
                }
                StmtKind::For { body, .. } => self.check_unreachable_code(body),
                _ => {}
            }
        }
    }

    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::error(span, msg.into()));
    }
}

/// Whether a statement returns on all paths.
fn stmt_returns(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Block(stmts) => block_returns(stmts),
        StmtKind::Return(_) => true,
        StmtKind::If {
            then,
            els: Some(els),
            ..
        } => stmt_returns(then) && stmt_returns(els),
        StmtKind::If { els: None, .. } => false,
        // A match is exhaustive by the time this pass runs, so it returns on all
        // paths exactly when every arm does.
        StmtKind::Match { arms, .. } => !arms.is_empty() && arms.iter().all(|a| stmt_returns(&a.body)),
        _ => false,
    }
}

/// Whether a block returns on all paths — true when any of its statements does.
fn block_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_returns)
}
