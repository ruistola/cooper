//! Expressions: the Pratt parser, calls, fields, indexing, and value blocks.

use super::*;

impl Parser {
    pub(super) fn parse_expr(&mut self, min_bp: i32) -> PResult<Expr> {
        // A closer where an expression should start is left for the construct it
        // closes, so a missing operand does not swallow its enclosing block's brace.
        let next = self.peek();
        if matches!(next.kind, CloseCurly | CloseParen | CloseBracket | Semicolon | Eof) {
            return Err(self.error(next.span, format!("expected an expression, found {}", next.kind)));
        }
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
            Str => self.string_literal(&token)?,
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
            Func => {
                let (params, return_type, body) = self.parse_func_rest(false)?;
                ExprKind::Func {
                    params,
                    return_type,
                    body,
                }
            }
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

    /// A string literal: plain, or interpolated when it has holes. Each hole's source
    /// is parsed as an expression on its own, with spans placing it in the file.
    fn string_literal(&mut self, token: &Token) -> PResult<ExprKind> {
        let text = &token.text;
        let inner = &text[1..text.len() - 1];
        let base = token.span.start + 1;
        let mut parts = Vec::new();
        let mut segment = 0;
        let bytes = inner.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'{' => {
                    let len = crate::lexer::hole_end(&bytes[i + 1..]).expect("the lexer closed every hole");
                    parts.push(StrPart::Text(inner[segment..i].to_string()));
                    let start = base + i + 1;
                    let source = &inner[i + 1..i + len];
                    parts.push(if source.trim().is_empty() {
                        StrPart::Positional(Span::new(start - 1, start + len))
                    } else {
                        StrPart::Hole(self.hole(source, start)?)
                    });
                    i += 1 + len;
                    segment = i;
                }
                _ => i += 1,
            }
        }
        if parts.is_empty() {
            return Ok(ExprKind::Str(text.clone()));
        }
        parts.push(StrPart::Text(inner[segment..].to_string()));
        Ok(ExprKind::Interpolated(parts))
    }

    /// Parse the source of a hole, which begins at byte `start` of the file.
    fn hole(&mut self, source: &str, start: usize) -> PResult<Expr> {
        let mut tokens = match crate::lexer::tokenize(source) {
            Ok(tokens) => tokens,
            Err(mut diag) => {
                diag.span = Span::new(diag.span.start + start, diag.span.end + start);
                self.errors.push(diag);
                return Err(Recover);
            }
        };
        for token in &mut tokens {
            token.span = Span::new(token.span.start + start, token.span.end + start);
        }
        let mut parser = Parser::new(tokens);
        // A hole is an expression wherever it sits, so newlines carry no meaning.
        parser.header_depth = 1;
        let result = parser.parse_expr(0);
        if result.is_ok() && parser.peek().kind != Eof {
            let extra = parser.peek();
            parser.error(extra.span, format!("unexpected {} in a hole", extra.kind));
        }
        let failed = !parser.errors.is_empty();
        for diag in &mut parser.errors {
            diag.message = diag.message.replace("end of input", "the end of the hole");
        }
        self.errors.append(&mut parser.errors);
        match result {
            Ok(expr) if !failed => Ok(expr),
            _ => Err(Recover),
        }
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
                let op_token = self.advance();
                let op = binary_op(op_token.kind).unwrap();
                let rhs = self.parse_expr(rbp)?;
                if let Some(ascending) = ordering(op) {
                    return self.extend_chain(head, op, op_token.span, ascending, rhs);
                }
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

    /// Continue a comparison `head op rhs`: when `head` is itself an unparenthesized
    /// ordering comparison, the two join into a [`ExprKind::Chain`], whose operators
    /// must all run in one direction.
    fn extend_chain(
        &mut self,
        head: Expr,
        op: BinaryOp,
        op_span: Span,
        ascending: bool,
        rhs: Expr,
    ) -> PResult<Expr> {
        let span = head.span.to(rhs.span);
        let (mut operands, mut ops) = match head.kind {
            ExprKind::Binary { op: first, lhs, rhs: middle } if ordering(first).is_some() => {
                (vec![*lhs, *middle], vec![first])
            }
            ExprKind::Chain { operands, ops } => (operands, ops),
            kind => {
                let head = Expr { kind, span: head.span };
                return Ok(Expr {
                    kind: ExprKind::Binary {
                        op,
                        lhs: Box::new(head),
                        rhs: Box::new(rhs),
                    },
                    span,
                });
            }
        };
        if ordering(ops[0]) != Some(ascending) {
            return Err(self.error(
                op_span,
                "a comparison chain must use only `<`/`<=` or only `>`/`>=`",
            ));
        }
        operands.push(rhs);
        ops.push(op);
        Ok(Expr {
            kind: ExprKind::Chain { operands, ops },
            span,
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
        // A bound stops short of `..`.
        let start = if is_range(self.peek().kind) {
            None
        } else {
            Some(self.parse_expr(RANGE_BP)?)
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
            Some(Box::new(self.parse_expr(RANGE_BP)?))
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

// Binding powers, loosest to tightest. Levels sit two apart so that the right power of a
// left-associative operator (its left power plus one) stays below the next level's.
const ASSIGN_BP: i32 = 2;
const RANGE_BP: i32 = 4;
const OR_BP: i32 = 6;
const XOR_BP: i32 = 8;
const AND_BP: i32 = 10;
const EQUALITY_BP: i32 = 12;
const COMPARISON_BP: i32 = 14;
const SHIFT_BP: i32 = 16;
const ADDITIVE_BP: i32 = 18;
const MULTIPLICATIVE_BP: i32 = 20;
const STRUCT_LITERAL_BP: i32 = 22;
const CALL_BP: i32 = 24;
const MEMBER_BP: i32 = 26;

/// Right binding power of a prefix operator. Its operand is a postfix chain, so `-a * b`
/// is `(-a) * b` while `-a.b` is `-(a.b)`.
fn prefix_bp(kind: TokenKind) -> i32 {
    match kind {
        Plus | Dash | Not | Ampersand => MULTIPLICATIVE_BP,
        _ => 0,
    }
}

/// `(left, right)` binding power of a token in infix/postfix position. A zero left
/// binding power terminates the expression. Binary operators group left to right, which
/// puts the right power one above the left; assignment groups right to left.
fn tail_bp(kind: TokenKind) -> (i32, i32) {
    match kind {
        Equals | PlusEquals | DashEquals | StarEquals | SlashEquals | PercentEquals
        | ColonEquals => {
            (ASSIGN_BP, ASSIGN_BP - 1)
        }
        DotDot | DotDotEquals => (RANGE_BP, RANGE_BP + 1),
        Or => (OR_BP, OR_BP + 1),
        Xor => (XOR_BP, XOR_BP + 1),
        And => (AND_BP, AND_BP + 1),
        DoubleEquals | NotEquals => (EQUALITY_BP, EQUALITY_BP + 1),
        Less | LessEquals | Greater | GreaterEquals => (COMPARISON_BP, COMPARISON_BP + 1),
        ShiftLeft | ShiftRight => (SHIFT_BP, SHIFT_BP + 1),
        Plus | Dash => (ADDITIVE_BP, ADDITIVE_BP + 1),
        Star | Slash | Percent => (MULTIPLICATIVE_BP, MULTIPLICATIVE_BP + 1),
        OpenCurly => (STRUCT_LITERAL_BP, 0),
        OpenParen | OpenBracket => (CALL_BP, 0),
        Dot | Chevron => (MEMBER_BP, MEMBER_BP - 1),
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
        Xor => BinaryOp::Xor,
        ShiftLeft => BinaryOp::Shl,
        ShiftRight => BinaryOp::Shr,
        _ => return None,
    })
}

/// For an ordering comparison, whether it is ascending (`<`, `<=`) or descending.
fn ordering(op: BinaryOp) -> Option<bool> {
    match op {
        BinaryOp::Lt | BinaryOp::Le => Some(true),
        BinaryOp::Gt | BinaryOp::Ge => Some(false),
        _ => None,
    }
}

fn assign_op(kind: TokenKind) -> Option<AssignOp> {
    Some(match kind {
        Equals => AssignOp::Assign,
        PlusEquals => AssignOp::Add,
        DashEquals => AssignOp::Sub,
        StarEquals => AssignOp::Mul,
        SlashEquals => AssignOp::Div,
        PercentEquals => AssignOp::Rem,
        _ => return None,
    })
}
