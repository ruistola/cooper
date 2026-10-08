//! Expressions: the Pratt parser, calls, fields, indexing, and value blocks.

use super::*;

impl Parser {
    pub(super) fn parse_expr(&mut self, min_bp: i32) -> PResult<Expr> {
        let token = self.advance();
        let mut left = self.parse_head_expr(token)?;
        loop {
            let (lbp, rbp) = tail_bp(self.peek().kind);
            if lbp <= min_bp {
                break;
            }
            left = self.parse_tail_expr(left, rbp)?;
        }
        Ok(left)
    }

    fn parse_head_expr(&mut self, token: Token) -> PResult<Expr> {
        let start = token.span;
        let kind = match token.kind {
            Number => ExprKind::Number(token.text),
            Str => ExprKind::Str(token.text),
            Identifier => ExprKind::Ident(token.text),
            True | False => ExprKind::Bool(token.kind == True),
            Plus | Dash | Not => {
                let op = match token.kind {
                    Plus => UnaryOp::Pos,
                    Dash => UnaryOp::Neg,
                    _ => UnaryOp::Not,
                };
                let operand = self.parse_expr(prefix_bp(token.kind))?;
                ExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                }
            }
            Ampersand => {
                let operand = self.parse_expr(prefix_bp(token.kind))?;
                ExprKind::AddressOf(Box::new(operand))
            }
            Nil => ExprKind::Nil,
            OpenParen => self.parse_paren_head()?,
            OpenBracket => self.parse_array_literal()?,
            If => self.parse_if_expr()?,
            Match => self.parse_match_expr()?,
            OpenCurly => {
                let block = self.parse_block_expr();
                self.expect(CloseCurly)?;
                ExprKind::Block(block)
            }
            other => {
                return Err(self.error(start, format!("expected an expression, found {}", other)));
            }
        };
        Ok(Expr {
            kind,
            span: start.to(self.prev_token().span),
        })
    }

    /// `()` unit, `(e)` group, or `(e, e, ...)` tuple, disambiguated by contents.
    fn parse_paren_head(&mut self) -> PResult<ExprKind> {
        if self.peek().kind == CloseParen {
            self.expect(CloseParen)?;
            return Ok(ExprKind::Unit);
        }
        let first = self.parse_expr(0)?;
        if self.peek().kind != Comma {
            self.expect(CloseParen)?;
            return Ok(ExprKind::Group(Box::new(first)));
        }
        let mut elems = vec![first];
        while self.peek().kind == Comma {
            self.expect(Comma)?;
            if self.peek().kind == CloseParen {
                break; // tolerate a trailing comma
            }
            elems.push(self.parse_expr(0)?);
        }
        self.expect(CloseParen)?;
        Ok(ExprKind::Tuple(elems))
    }

    /// `[]` or `[e, e, ...]`, tolerating a trailing comma. The opening bracket is
    /// already consumed.
    fn parse_array_literal(&mut self) -> PResult<ExprKind> {
        let mut elems = Vec::new();
        while self.peek().kind != CloseBracket {
            elems.push(self.parse_expr(0)?);
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseBracket)?;
        Ok(ExprKind::Array(elems))
    }

    fn parse_tail_expr(&mut self, head: Expr, rbp: i32) -> PResult<Expr> {
        let start = head.span;
        let kind = match self.peek().kind {
            ColonEquals => return self.parse_decl_assign_expr(head),
            k if assign_op(k).is_some() => {
                let op = assign_op(self.advance().kind).unwrap();
                let value = self.parse_expr(rbp)?;
                ExprKind::Assign {
                    op,
                    target: Box::new(head),
                    value: Box::new(value),
                }
            }
            k if binary_op(k).is_some() => {
                let op = binary_op(self.advance().kind).unwrap();
                let rhs = self.parse_expr(rbp)?;
                ExprKind::Binary {
                    op,
                    lhs: Box::new(head),
                    rhs: Box::new(rhs),
                }
            }
            DotDot | DotDotEquals => {
                let inclusive = self.advance().kind == DotDotEquals;
                let end = self.parse_expr(rbp)?;
                ExprKind::Range {
                    start: Box::new(head),
                    end: Box::new(end),
                    inclusive,
                }
            }
            OpenParen => self.parse_call_args(head)?,
            OpenCurly => self.parse_struct_literal(head)?,
            OpenBracket => self.parse_index(head)?,
            Dot => self.parse_field(head)?,
            Chevron => {
                self.expect(Chevron)?;
                ExprKind::Deref(Box::new(head))
            }
            other => {
                let span = self.peek().span;
                return Err(self.error(span, format!("{} cannot continue an expression", other)));
            }
        };
        Ok(Expr {
            kind,
            span: start.to(self.prev_token().span),
        })
    }

    /// Parse a control-flow header expression (an `if`/`while`/`until` condition, a
    /// `for` iterable, or a `match` scrutinee) with newline-insignificance enabled,
    /// so the expression may wrap across lines and its delimiter keyword may sit on a
    /// later line.
    pub(super) fn parse_header_expr(&mut self) -> PResult<Expr> {
        self.header_depth += 1;
        let result = self.parse_expr(0);
        self.header_depth -= 1;
        result
    }

    fn parse_if_expr(&mut self) -> PResult<ExprKind> {
        let cond = self.parse_header_expr()?;
        self.expect(Then)?;
        let then = self.parse_branch_expr()?;
        if self.peek().kind == Semicolon {
            self.expect(Semicolon)?;
        }
        self.expect(Else)?;
        let els = self.parse_branch_expr()?;
        Ok(ExprKind::If {
            cond: Box::new(cond),
            then: Box::new(then),
            els: Box::new(els),
        })
    }

    /// A branch of an if-expression: a braced value block or a bare expression.
    pub(super) fn parse_branch_expr(&mut self) -> PResult<Expr> {
        if self.peek().kind == OpenCurly {
            let start = self.peek().span;
            self.expect(OpenCurly)?;
            let block = self.parse_block_expr();
            self.expect(CloseCurly)?;
            Ok(Expr {
                kind: ExprKind::Block(block),
                span: start.to(self.prev_token().span),
            })
        } else {
            self.parse_expr(0)
        }
    }

    fn parse_call_args(&mut self, callee: Expr) -> PResult<ExprKind> {
        self.expect(OpenParen)?;
        let mut args = Vec::new();
        while self.peek().kind != CloseParen {
            args.push(self.parse_expr(0)?);
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        Ok(ExprKind::Call {
            callee: Box::new(callee),
            args,
        })
    }

    fn parse_struct_literal(&mut self, target: Expr) -> PResult<ExprKind> {
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut members = Vec::new();
        while self.peek().kind != CloseCurly {
            let mstart = self.peek().span;
            let member_name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let value = self.parse_expr(0)?;
            members.push(MemberInit {
                name: member_name,
                value,
                span: mstart.to(self.prev_token().span),
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(ExprKind::StructLiteral {
            target: Box::new(target),
            members,
        })
    }

    fn parse_field(&mut self, target: Expr) -> PResult<ExprKind> {
        self.expect(Dot)?;
        let name = self.expect(Identifier)?.text;
        Ok(ExprKind::Field {
            target: Box::new(target),
            name,
        })
    }

    /// `array[index]`, or a slice `array[start..end]` (`..=` for an inclusive end)
    /// with either bound optional: `xs[2..]`, `xs[..n]`, `xs[..]`.
    fn parse_index(&mut self, array: Expr) -> PResult<ExprKind> {
        self.expect(OpenBracket)?;
        let is_range = |kind| matches!(kind, DotDot | DotDotEquals);
        // A bound stops short of `..`, which binds at 3.
        let start = if is_range(self.peek().kind) {
            None
        } else {
            Some(self.parse_expr(3)?)
        };
        if !is_range(self.peek().kind) {
            let index = start.expect("an index without `..` has an expression");
            self.expect(CloseBracket)?;
            return Ok(ExprKind::Index {
                array: Box::new(array),
                index: Box::new(index),
            });
        }
        let inclusive = self.advance().kind == DotDotEquals;
        let end = if self.peek().kind == CloseBracket {
            if inclusive {
                let span = self.peek().span;
                return Err(self.error(span, "an inclusive slice `..=` needs an end bound"));
            }
            None
        } else {
            Some(Box::new(self.parse_expr(3)?))
        };
        self.expect(CloseBracket)?;
        Ok(ExprKind::Slice {
            array: Box::new(array),
            start: start.map(Box::new),
            end,
            inclusive,
        })
    }

    /// A value block: a trailing expression statement becomes the result;
    /// otherwise the block yields unit.
    fn parse_block_expr(&mut self) -> Block {
        let mut statements = self.parse_block_stmt();
        let result = match statements.last() {
            Some(Stmt {
                kind: StmtKind::Expression(_),
                ..
            }) => match statements.pop() {
                Some(Stmt {
                    kind: StmtKind::Expression(expr),
                    ..
                }) => expr,
                _ => unreachable!(),
            },
            _ => {
                let span = self.prev_token().span;
                Expr {
                    kind: ExprKind::Unit,
                    span,
                }
            }
        };
        Block {
            statements,
            result: Box::new(result),
        }
    }
}

/// Right binding power of a prefix operator.
fn prefix_bp(kind: TokenKind) -> i32 {
    match kind {
        Plus | Dash | Not => 10,
        Ampersand => 12,
        _ => 0,
    }
}

/// `(left, right)` binding power of a token in infix/postfix position. A zero left
/// binding power terminates the expression.
fn tail_bp(kind: TokenKind) -> (i32, i32) {
    match kind {
        Equals | PlusEquals | DashEquals | StarEquals | SlashEquals | ColonEquals => (1, 2),
        DotDot | DotDotEquals => (3, 4),
        Or | And => (4, 3),
        DoubleEquals | NotEquals => (5, 6),
        Less | LessEquals | Greater | GreaterEquals => (8, 7),
        Plus | Dash => (10, 9),
        Star | Slash | Percent => (12, 11),
        OpenCurly => (13, 0),
        OpenParen | OpenBracket => (14, 0),
        Dot | Chevron => (16, 15),
        _ => (0, 0),
    }
}

fn binary_op(kind: TokenKind) -> Option<BinaryOp> {
    Some(match kind {
        Plus => BinaryOp::Add,
        Dash => BinaryOp::Sub,
        Star => BinaryOp::Mul,
        Slash => BinaryOp::Div,
        Percent => BinaryOp::Rem,
        DoubleEquals => BinaryOp::Eq,
        NotEquals => BinaryOp::Ne,
        Less => BinaryOp::Lt,
        LessEquals => BinaryOp::Le,
        Greater => BinaryOp::Gt,
        GreaterEquals => BinaryOp::Ge,
        And => BinaryOp::And,
        Or => BinaryOp::Or,
        _ => return None,
    })
}

fn assign_op(kind: TokenKind) -> Option<AssignOp> {
    Some(match kind {
        Equals => AssignOp::Assign,
        PlusEquals => AssignOp::Add,
        DashEquals => AssignOp::Sub,
        StarEquals => AssignOp::Mul,
        SlashEquals => AssignOp::Div,
        _ => return None,
    })
}
