//! Statements: bindings, conditionals, loops, `return`, and blocks.

use super::*;

impl Parser {
    fn parse_stmt(&mut self) -> PResult<Stmt> {
        if self.peek().kind == OpenParen && self.is_method_decl_ahead() {
            let decl = self.parse_func_decl()?;
            let span = decl.span;
            return Ok(Stmt {
                kind: StmtKind::FuncDecl(decl),
                span,
            });
        }
        match self.peek().kind {
            For => self.parse_for_stmt(),
            // `func name …` declares a nested function; `func(…)` begins a literal.
            Func if self.lookahead(2).get(1).is_some_and(|t| t.kind == Identifier) => {
                let decl = self.parse_func_decl()?;
                let span = decl.span;
                Ok(Stmt {
                    kind: StmtKind::FuncDecl(decl),
                    span,
                })
            }
            If => self.parse_if_stmt(),
            While => self.parse_pre_test_loop(),
            Until => self.parse_pre_test_loop(),
            Do => self.parse_post_test_loop(),
            Repeat => self.parse_post_test_loop(),
            Break => self.parse_break_continue(),
            Continue => self.parse_break_continue(),
            Let => self.parse_var_decl_stmt(),
            Match => self.parse_match_stmt(),
            Return => self.parse_return_stmt(),
            Struct => self.parse_struct_decl_stmt(),
            Oneof => self.parse_oneof_decl_stmt(),
            _ => self.parse_expression_stmt(),
        }
    }

    pub(super) fn parse_decl_assign_expr(&mut self, head: Expr) -> PResult<Expr> {
        let start = head.span;
        self.expect(ColonEquals)?;
        let kind = match head.kind {
            ExprKind::Ident(name) => ExprKind::Let {
                name,
                value: Box::new(self.parse_expr(0)?),
            },
            ExprKind::Tuple(elems) => {
                let names = self.pattern_names(elems)?;
                ExprKind::LetTuple {
                    names,
                    ty: None,
                    value: Box::new(self.parse_expr(0)?),
                }
            }
            _ => {
                return Err(self.error(
                    head.span,
                    "the left-hand side of ':=' must be an identifier or a tuple pattern",
                ));
            }
        };
        Ok(Expr {
            kind,
            span: start.to(self.prev_token().span),
        })
    }

    fn pattern_names(&mut self, elems: Vec<Expr>) -> PResult<Vec<String>> {
        elems
            .into_iter()
            .map(|elem| match elem.kind {
                ExprKind::Ident(name) => Ok(name),
                _ => Err(self.error(elem.span, "a destructuring pattern may only bind identifiers")),
            })
            .collect()
    }

    pub(super) fn parse_var_decl_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Let)?;
        if self.peek().kind == OpenParen {
            let expr = self.parse_let_destructure(start)?;
            let span = expr.span;
            return Ok(Stmt {
                kind: StmtKind::Expression(expr),
                span,
            });
        }
        let name = self.expect(Identifier)?.text;
        let ty = if self.peek().kind == Colon {
            self.expect(Colon)?;
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let init = if self.statement_terminates() {
            None
        } else {
            self.expect(Equals)?;
            Some(self.parse_expr(0)?)
        };
        self.consume_statement_terminator()?;
        Ok(Stmt {
            kind: StmtKind::VarDecl { name, ty, init },
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_let_destructure(&mut self, start: Span) -> PResult<Expr> {
        self.expect(OpenParen)?;
        let mut names = Vec::new();
        while self.peek().kind != CloseParen {
            names.push(self.expect(Identifier)?.text);
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        let ty = if self.peek().kind == Colon {
            self.expect(Colon)?;
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(Equals)?;
        let value = self.parse_expr(0)?;
        self.consume_statement_terminator()?;
        Ok(Expr {
            kind: ExprKind::LetTuple {
                names,
                ty,
                value: Box::new(value),
            },
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_if_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(If)?;
        let cond = self.parse_header_expr()?;
        self.expect(Then)?;
        let then = self.parse_branch_stmt()?;
        let els = if self.peek().kind == Else {
            self.expect(Else)?;
            Some(Box::new(self.parse_branch_stmt()?))
        } else {
            None
        };
        Ok(Stmt {
            kind: StmtKind::If {
                cond,
                then: Box::new(then),
                els,
            },
            span: start.to(self.prev_token().span),
        })
    }

    /// A branch of an if-statement: a braced block or a single guarded statement.
    pub(super) fn parse_branch_stmt(&mut self) -> PResult<Stmt> {
        if self.peek().kind == OpenCurly {
            let start = self.peek().span;
            self.expect(OpenCurly)?;
            let body = self.parse_block_stmt();
            self.expect(CloseCurly)?;
            Ok(Stmt {
                kind: StmtKind::Block(body),
                span: start.to(self.prev_token().span),
            })
        } else {
            self.closers.push(Else);
            let s = self.parse_stmt();
            self.closers.pop();
            s
        }
    }

    /// `for BINDINGS in ITER do BODY`. BINDINGS is a single identifier or a
    /// parenthesised list of identifiers; ITER is any expression (typically a range
    /// `a..b` or an array); the `do` keyword delimits the body.
    fn parse_for_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(For)?;
        let bindings = self.parse_for_bindings()?;
        self.expect(In)?;
        let iterable = self.parse_header_expr()?;
        self.expect(Do)?;
        let body = self.parse_loop_body()?;
        Ok(Stmt {
            kind: StmtKind::ForIn {
                bindings,
                iterable,
                body,
            },
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_for_bindings(&mut self) -> PResult<Vec<String>> {
        if self.peek().kind != OpenParen {
            return Ok(vec![self.expect(Identifier)?.text]);
        }
        self.expect(OpenParen)?;
        let mut names = Vec::new();
        while self.peek().kind != CloseParen {
            names.push(self.expect(Identifier)?.text);
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        Ok(names)
    }

    /// `while COND do BODY` / `until COND repeat BODY` — the condition is tested
    /// before each iteration. `until` loops while the condition is false.
    fn parse_pre_test_loop(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        let until = self.advance().kind == Until;
        let cond = self.parse_header_expr()?;
        self.expect(if until { Repeat } else { Do })?;
        let body = self.parse_loop_body()?;
        Ok(Stmt {
            kind: StmtKind::While {
                cond,
                body,
                post_test: false,
                until,
            },
            span: start.to(self.prev_token().span),
        })
    }

    /// `do BODY while COND` / `repeat BODY until COND` — the body runs before the
    /// first test, so it executes at least once. `until` loops while COND is false.
    fn parse_post_test_loop(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        let until = self.advance().kind == Repeat;
        self.closers.push(if until { Until } else { While });
        let body = self.parse_loop_body();
        self.closers.pop();
        let body = body?;
        self.expect(if until { Until } else { While })?;
        let cond = self.parse_expr(0)?;
        self.consume_statement_terminator()?;
        Ok(Stmt {
            kind: StmtKind::While {
                cond,
                body,
                post_test: true,
                until,
            },
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_break_continue(&mut self) -> PResult<Stmt> {
        let token = self.advance();
        let kind = if token.kind == Break {
            StmtKind::Break
        } else {
            StmtKind::Continue
        };
        self.consume_statement_terminator()?;
        Ok(Stmt {
            kind,
            span: token.span,
        })
    }

    /// A loop body: a braced block or a single statement, mirroring an if-branch.
    fn parse_loop_body(&mut self) -> PResult<Vec<Stmt>> {
        if self.peek().kind == OpenCurly {
            self.expect(OpenCurly)?;
            let body = self.parse_block_stmt();
            self.expect(CloseCurly)?;
            Ok(body)
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    fn parse_return_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Return)?;
        let value = if self.statement_terminates() {
            None
        } else {
            Some(self.parse_expr(0)?)
        };
        self.consume_statement_terminator()?;
        Ok(Stmt {
            kind: StmtKind::Return(value),
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_expression_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        let expr = self.parse_expr(0)?;
        self.consume_statement_terminator()?;
        Ok(Stmt {
            kind: StmtKind::Expression(expr),
            span: start.to(self.prev_token().span),
        })
    }

    /// Parse statements up to a closing brace or end of input, recovering past any
    /// failed statement so the block still yields as much as possible. A braced block
    /// is a statement context even inside a header expression, so header
    /// newline-insignificance is suspended for its extent.
    pub(super) fn parse_block_stmt(&mut self) -> Vec<Stmt> {
        let saved_header_depth = self.header_depth;
        self.header_depth = 0;
        // Likewise for enclosing parentheses: a function literal's body passed as an
        // argument still separates its statements by newlines.
        let saved_parens = std::mem::take(&mut self.paren_stack);
        let saved_closers = std::mem::take(&mut self.closers);
        let mut statements = Vec::new();
        loop {
            match self.peek().kind {
                Eof | CloseCurly => break,
                Semicolon => {
                    self.advance();
                }
                _ => match self.parse_stmt() {
                    Ok(s) => statements.push(s),
                    Err(_) => self.synchronize(),
                },
            }
        }
        self.header_depth = saved_header_depth;
        self.paren_stack = saved_parens;
        self.closers = saved_closers;
        statements
    }
}
