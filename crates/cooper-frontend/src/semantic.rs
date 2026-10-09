//! Semantic analysis: the final pass — control-flow validation.
//!
//! Running only after type checking succeeds, this pass verifies that every
//! function with a non-unit return type returns on all paths and reports code made
//! unreachable by a preceding return. It relies on the type checker having already
//! rejected non-exhaustive matches, so a match returns exactly when all its arms do.

use crate::ast::{Expr, ExprKind, FuncDecl, Stmt, StmtKind, TypeExpr, TypeExprKind};
use crate::diag::{Diagnostic, Span};
use crate::resolve::{self, Globals};
use crate::types::{is_unit, Type};

/// Analyze `module` for control-flow soundness, returning any diagnostics.
pub fn analyze(module: &[Stmt], globals: &Globals) -> Vec<Diagnostic> {
    let mut analyzer = SemanticAnalyzer {
        globals,
        diags: Vec::new(),
        depth: 0,
        valued_returns: Vec::new(),
    };
    analyzer.analyze_block(module);
    analyzer.diags
}

struct SemanticAnalyzer<'g> {
    globals: &'g Globals,
    diags: Vec<Diagnostic>,
    /// How many function bodies enclose the statement being analyzed: a function
    /// declared inside one is nested, with no entry in the globals.
    depth: u32,
    /// For each function literal being analyzed, innermost last, whether a `return`
    /// in its body carries a value.
    valued_returns: Vec<bool>,
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
            StmtKind::StructDecl { .. } | StmtKind::OneofDecl { .. } => {}
            StmtKind::FuncDecl(func) if self.depth > 0 => {
                let label = format!("function '{}'", func.name);
                self.analyze_literal(&func.return_type, false, &func.body, func.span, &label)
            }
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
            StmtKind::ForIn { iterable, body, .. } => {
                self.analyze_expr(iterable);
                self.analyze_block(body);
            }
            StmtKind::While { cond, body, .. } => {
                self.analyze_expr(cond);
                self.analyze_block(body);
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    if let Some(valued) = self.valued_returns.last_mut() {
                        *valued = true;
                    }
                    self.analyze_expr(expr);
                }
            }
            StmtKind::Expression(expr) => self.analyze_expr(expr),
        }
    }

    /// A function literal or nested function: its body must return on every path
    /// when it returns a value: when its declared return type is not unit, or, with
    /// the return type `inferred`, when any of its `return`s carries a value.
    fn analyze_literal(&mut self, return_type: &Option<TypeExpr>, inferred: bool, body: &[Stmt], span: Span, label: &str) {
        self.depth += 1;
        self.valued_returns.push(false);
        self.analyze_block(body);
        let any_valued = self.valued_returns.pop().expect("pushed above");
        self.depth -= 1;
        let returns_value = match return_type {
            Some(ty) => !matches!(ty.kind, TypeExprKind::Unit),
            None => inferred && any_valued,
        };
        if returns_value && !block_returns(body) {
            self.err(span, format!("{label} does not return a value in all code paths"));
        }
    }

    fn analyze_func_decl(&mut self, func: &FuncDecl) {
        self.depth += 1;
        self.analyze_block(&func.body);
        self.depth -= 1;

        // Look up the declared return type: methods are keyed by receiver struct.
        let return_type = match &func.receiver {
            Some(receiver) => resolve::underlying_struct_name(&receiver.ty)
                .and_then(|name| self.globals.structs.get(name))
                .and_then(|id| self.globals.lookup_method(id, &func.name)),
            None => self.globals.lookup_func(&func.name),
        };
        let Some(Type::Func { return_type, .. }) = return_type.map(|sig| &sig.ty) else {
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
            ExprKind::Range { start, end, .. } => {
                self.analyze_expr(start);
                self.analyze_expr(end);
            }
            ExprKind::Group(inner) => self.analyze_expr(inner),
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => elems.iter().for_each(|e| self.analyze_expr(e)),
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
            ExprKind::Slice {
                array, start, end, ..
            } => {
                self.analyze_expr(array);
                start.iter().chain(end).for_each(|bound| self.analyze_expr(bound));
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
            ExprKind::Func {
                return_type, body, ..
            } => self.analyze_literal(return_type, true, body, expr.span, "function literal"),
            ExprKind::Chain { operands, .. } => operands.iter().for_each(|e| self.analyze_expr(e)),
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
                StmtKind::ForIn { body, .. } | StmtKind::While { body, .. } => {
                    self.check_unreachable_code(body)
                }
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
