//! A hand-written recursive-descent parser with Pratt expression parsing.
//!
//! The parser collects diagnostics and recovers at statement boundaries instead
//! of bailing on the first error, so one run reports as many problems as it can.
//! Every produced node carries the source span it was parsed from.

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Token, TokenKind};

use TokenKind::*;

/// The outcome of parsing: a best-effort module plus any diagnostics. A non-empty
/// `errors` means some nodes were recovered rather than cleanly parsed.
pub struct Parsed {
    pub module: Vec<Stmt>,
    pub errors: Vec<Diagnostic>,
}

/// Parse a token stream into a module.
pub fn parse(tokens: Vec<Token>) -> Parsed {
    let mut parser = Parser::new(tokens);
    let module = parser.parse_module();
    Parsed {
        module,
        errors: parser.errors,
    }
}

/// Unwinds the current production to the nearest recovery point. The diagnostic is
/// already recorded by the time this is returned, so recovery sites only need to
/// resynchronize.
struct Recover;

type PResult<T> = Result<T, Recover>;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    paren_stack: Vec<TokenKind>,
    in_then_branch: bool,
    errors: Vec<Diagnostic>,
}

/// An end-of-line becomes a semicolon only when the preceding token can end a
/// statement.
fn can_precede_semicolon(kind: TokenKind) -> bool {
    matches!(
        kind,
        Number
            | Str
            | Identifier
            | Comma
            | CloseBracket
            | CloseParen
            | Chevron
            | Else
            | False
            | Nil
            | Return
            | Then
            | True
    )
}

/// An end-of-line becomes a semicolon only when the following token can begin one.
fn can_follow_semicolon(kind: TokenKind) -> bool {
    matches!(
        kind,
        Eof | Number
            | Str
            | Identifier
            | Semicolon
            | OpenCurly
            | CloseCurly
            | OpenParen
            | Ampersand
            | False
            | For
            | Func
            | If
            | Let
            | Match
            | Nil
            | Return
            | Struct
            | Oneof
            | True
            | Use
    )
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

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            paren_stack: Vec::new(),
            in_then_branch: false,
            errors: Vec::new(),
        }
    }

    // --- token access ---

    fn eof_token(&self) -> Token {
        let end = self.tokens.last().map(|t| t.span.end).unwrap_or(0);
        Token {
            kind: Eof,
            text: String::new(),
            span: Span::new(end, end),
        }
    }

    fn current_token(&self) -> Token {
        self.tokens
            .get(self.pos)
            .cloned()
            .unwrap_or_else(|| self.eof_token())
    }

    fn prev_token(&self) -> Token {
        if self.pos > 0 {
            self.tokens[self.pos - 1].clone()
        } else {
            self.eof_token()
        }
    }

    fn next_token(&self) -> Token {
        self.tokens
            .get(self.pos + 1)
            .cloned()
            .unwrap_or_else(|| self.eof_token())
    }

    /// Read-only lookahead over the next `n` significant tokens (end-of-line tokens
    /// skipped), never mutating the stream.
    fn lookahead(&self, n: usize) -> Vec<Token> {
        self.tokens[self.pos..]
            .iter()
            .filter(|t| t.kind != Eol)
            .take(n)
            .cloned()
            .collect()
    }

    /// Whether the upcoming tokens form a method receiver `( ident :`.
    fn is_method_decl_ahead(&self) -> bool {
        let ahead = self.lookahead(3);
        ahead.len() == 3
            && ahead[0].kind == OpenParen
            && ahead[1].kind == Identifier
            && ahead[2].kind == Colon
    }

    /// The current token, with end-of-line-to-semicolon inference applied: an
    /// `Eol` is rewritten to a `Semicolon` where a statement may terminate, and
    /// dropped as insignificant otherwise. This keeps the rest of the parser
    /// whitespace-agnostic.
    fn peek(&mut self) -> Token {
        let curr = self.current_token();
        if curr.kind == Eol {
            // Never infer a terminator right before a closing brace: a block's
            // trailing value must survive unless an explicit `;` discards it.
            if self.next_token().kind == CloseCurly {
                self.tokens.remove(self.pos);
                return self.peek();
            }
            let outside_parens = self.paren_stack.is_empty();
            let can_terminate = can_precede_semicolon(self.prev_token().kind)
                && can_follow_semicolon(self.next_token().kind);
            if outside_parens && can_terminate && self.next_token().kind != Eof {
                self.tokens[self.pos] = Token {
                    kind: Semicolon,
                    text: ";".to_string(),
                    span: curr.span,
                };
            } else {
                self.tokens.remove(self.pos);
                return self.peek();
            }
        }
        self.current_token()
    }

    /// Consume the current token unconditionally, maintaining the paren stack.
    fn advance(&mut self) -> Token {
        let t = self.peek();
        match t.kind {
            OpenParen | OpenBracket => self.paren_stack.push(t.kind),
            CloseParen | CloseBracket => self.pop_paren(t.kind),
            CloseCurly if self.paren_stack.last() == Some(&OpenCurly) => self.pop_paren(CloseCurly),
            _ => {}
        }
        self.pos += 1;
        t
    }

    /// Consume the current token, requiring it to be `kind`; records a diagnostic
    /// and unwinds otherwise.
    fn expect(&mut self, kind: TokenKind) -> PResult<Token> {
        let t = self.peek();
        if t.kind != kind {
            return Err(self.error(t.span, format!("expected {}, found {}", kind, t.kind)));
        }
        Ok(self.advance())
    }

    fn pop_paren(&mut self, closing: TokenKind) {
        let matched = matches!(
            (self.paren_stack.last(), closing),
            (Some(OpenParen), CloseParen)
                | (Some(OpenBracket), CloseBracket)
                | (Some(OpenCurly), CloseCurly)
        );
        if matched {
            self.paren_stack.pop();
        } else {
            let span = self.current_token().span;
            self.error(span, format!("unmatched {}", closing));
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) -> Recover {
        self.errors.push(Diagnostic::error(span, message));
        Recover
    }

    /// Skip tokens until the next plausible statement boundary so parsing can
    /// resume after an error.
    fn synchronize(&mut self) {
        loop {
            match self.peek().kind {
                Eof | CloseCurly => return,
                Semicolon => {
                    self.advance();
                    return;
                }
                For | Func | If | Let | Match | Return | Struct | Oneof | Use => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    // --- statement terminators ---

    fn statement_terminates(&mut self) -> bool {
        match self.peek().kind {
            Semicolon | Eof | CloseCurly => true,
            Else => self.in_then_branch,
            _ => self.prev_token().kind == CloseCurly,
        }
    }

    fn consume_statement_terminator(&mut self) -> PResult<()> {
        match self.peek().kind {
            Semicolon => {
                self.advance();
                Ok(())
            }
            Eof | CloseCurly => Ok(()),
            Else if self.in_then_branch => Ok(()),
            _ if self.prev_token().kind == CloseCurly => Ok(()),
            _ => {
                let span = self.peek().span;
                Err(self.error(span, "expected statement terminator"))
            }
        }
    }

    // --- top level ---

    fn parse_module(&mut self) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        while self.peek().kind != Eof {
            if self.peek().kind == Semicolon {
                self.advance();
                continue;
            }
            match self.parse_stmt() {
                Ok(s) => stmts.push(s),
                Err(_) => self.synchronize(),
            }
        }
        stmts
    }

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
            Func => {
                let decl = self.parse_func_decl()?;
                let span = decl.span;
                Ok(Stmt {
                    kind: StmtKind::FuncDecl(decl),
                    span,
                })
            }
            If => self.parse_if_stmt(),
            Let => self.parse_var_decl_stmt(),
            Match => self.parse_match_stmt(),
            Return => self.parse_return_stmt(),
            Struct => self.parse_struct_decl_stmt(),
            Oneof => self.parse_oneof_decl_stmt(),
            Use => self.parse_use_decl_stmt(),
            _ => self.parse_expression_stmt(),
        }
    }

    // --- Pratt expression parser ---

    fn parse_expr(&mut self, min_bp: i32) -> PResult<Expr> {
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

    // --- type expressions ---

    fn parse_type_expr(&mut self) -> PResult<TypeExpr> {
        let head = self.parse_type_atom()?;
        if !self.next_starts_type_atom() {
            return Ok(head);
        }
        let start = head.span;
        let mut args = Vec::new();
        while self.next_starts_type_atom() {
            args.push(self.parse_type_atom()?);
        }
        Ok(TypeExpr {
            kind: TypeExprKind::Application {
                constructor: Box::new(head),
                args,
            },
            span: start.to(self.prev_token().span),
        })
    }

    fn next_starts_type_atom(&mut self) -> bool {
        matches!(self.peek().kind, Identifier | OpenParen | Func)
    }

    fn parse_type_atom(&mut self) -> PResult<TypeExpr> {
        let start = self.peek().span;
        let mut kind = if self.peek().kind == OpenParen {
            self.expect(OpenParen)?;
            let inner = if self.peek().kind == CloseParen {
                TypeExprKind::Unit
            } else {
                let mut elems = vec![self.parse_type_expr()?];
                while self.peek().kind == Comma {
                    self.expect(Comma)?;
                    if self.peek().kind == CloseParen {
                        break;
                    }
                    elems.push(self.parse_type_expr()?);
                }
                if elems.len() == 1 {
                    elems.pop().unwrap().kind
                } else {
                    TypeExprKind::Tuple(elems)
                }
            };
            self.expect(CloseParen)?;
            inner
        } else if self.peek().kind == Func {
            self.parse_func_type_expr()?
        } else {
            TypeExprKind::Named(self.expect(Identifier)?.text)
        };
        let mut ty = TypeExpr {
            kind,
            span: start.to(self.prev_token().span),
        };
        loop {
            kind = match self.peek().kind {
                OpenBracket => {
                    self.expect(OpenBracket)?;
                    self.expect(CloseBracket)?;
                    TypeExprKind::Array(Box::new(ty))
                }
                Chevron => {
                    self.expect(Chevron)?;
                    TypeExprKind::Pointer(Box::new(ty))
                }
                _ => return Ok(ty),
            };
            ty = TypeExpr {
                kind,
                span: start.to(self.prev_token().span),
            };
        }
    }

    fn parse_func_type_expr(&mut self) -> PResult<TypeExprKind> {
        self.expect(Func)?;
        self.expect(OpenParen)?;
        let mut params = Vec::new();
        while self.peek().kind != CloseParen {
            if self.peek().kind == Identifier {
                let name_tok = self.expect(Identifier)?;
                if self.peek().kind == Colon {
                    self.expect(Colon)?;
                    params.push(self.parse_type_expr()?);
                } else {
                    params.push(TypeExpr {
                        kind: TypeExprKind::Named(name_tok.text),
                        span: name_tok.span,
                    });
                }
            } else {
                params.push(self.parse_type_expr()?);
            }
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        let ret = if self.peek().kind == Colon {
            self.expect(Colon)?;
            self.parse_type_expr()?
        } else {
            let span = self.prev_token().span;
            TypeExpr {
                kind: TypeExprKind::Unit,
                span,
            }
        };
        Ok(TypeExprKind::Func {
            params,
            ret: Box::new(ret),
        })
    }

    // --- declaration-assignment ---

    fn parse_decl_assign_expr(&mut self, head: Expr) -> PResult<Expr> {
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

    // --- statements ---

    fn parse_var_decl_stmt(&mut self) -> PResult<Stmt> {
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

    fn parse_func_decl(&mut self) -> PResult<FuncDecl> {
        let start = self.peek().span;
        let receiver = if self.peek().kind == OpenParen {
            let rstart = self.peek().span;
            self.expect(OpenParen)?;
            let name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let ty = self.parse_type_expr()?;
            self.expect(CloseParen)?;
            Some(TypedIdent {
                name,
                ty,
                span: rstart.to(self.prev_token().span),
            })
        } else {
            None
        };
        self.expect(Func)?;
        let name = self.expect(Identifier)?.text;
        let mut type_params = Vec::new();
        while self.peek().kind == Identifier {
            type_params.push(self.expect(Identifier)?.text);
        }
        self.expect(OpenParen)?;
        let mut params = Vec::new();
        while self.peek().kind != CloseParen {
            let pstart = self.peek().span;
            let param_name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let param_type = self.parse_type_expr()?;
            params.push(TypedIdent {
                name: param_name,
                ty: param_type,
                span: pstart.to(self.prev_token().span),
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        let return_type = if self.peek().kind == Colon {
            self.expect(Colon)?;
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(OpenCurly)?;
        let body = self.parse_block_stmt();
        self.expect(CloseCurly)?;
        Ok(FuncDecl {
            receiver,
            name,
            type_params,
            params,
            return_type,
            body,
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_struct_decl_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Struct)?;
        let name = self.expect(Identifier)?.text;
        let mut type_params = Vec::new();
        while self.peek().kind == Identifier {
            type_params.push(self.expect(Identifier)?.text);
        }
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut members = Vec::new();
        while self.peek().kind != CloseCurly {
            let mstart = self.peek().span;
            let member_name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let member_type = self.parse_type_expr()?;
            members.push(TypedIdent {
                name: member_name,
                ty: member_type,
                span: mstart.to(self.prev_token().span),
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Stmt {
            kind: StmtKind::StructDecl {
                name,
                type_params,
                members,
            },
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_oneof_decl_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Oneof)?;
        let name = self.expect(Identifier)?.text;
        let mut type_params = Vec::new();
        while self.peek().kind == Identifier {
            type_params.push(self.expect(Identifier)?.text);
        }
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut variants = Vec::new();
        while self.peek().kind != CloseCurly {
            let vstart = self.peek().span;
            let variant_name = self.expect(Identifier)?.text;
            let mut payload = Vec::new();
            if self.peek().kind == OpenParen {
                self.expect(OpenParen)?;
                while self.peek().kind != CloseParen {
                    payload.push(self.parse_type_expr()?);
                    if self.peek().kind == Comma {
                        self.expect(Comma)?;
                    } else {
                        break;
                    }
                }
                self.expect(CloseParen)?;
            }
            variants.push(VariantDef {
                name: variant_name,
                payload,
                span: vstart.to(self.prev_token().span),
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Stmt {
            kind: StmtKind::OneofDecl {
                name,
                type_params,
                variants,
            },
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_if_expr(&mut self) -> PResult<ExprKind> {
        let cond = self.parse_expr(0)?;
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
    fn parse_branch_expr(&mut self) -> PResult<Expr> {
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

    fn parse_if_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(If)?;
        let cond = self.parse_expr(0)?;
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
    fn parse_branch_stmt(&mut self) -> PResult<Stmt> {
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
            let was_then = self.in_then_branch;
            self.in_then_branch = true;
            let s = self.parse_stmt();
            self.in_then_branch = was_then;
            s
        }
    }

    fn parse_pattern(&mut self) -> PResult<Pattern> {
        let start = self.peek().span;
        if self.peek().kind == Identifier && self.peek().text == "_" {
            self.expect(Identifier)?;
            return Ok(Pattern {
                kind: PatternKind::Wildcard,
                span: start,
            });
        }
        let type_name = self.expect(Identifier)?.text;
        self.expect(Dot)?;
        let variant = self.expect(Identifier)?.text;
        let mut binders = Vec::new();
        if self.peek().kind == OpenParen {
            self.expect(OpenParen)?;
            while self.peek().kind != CloseParen {
                binders.push(self.expect(Identifier)?.text);
                if self.peek().kind == Comma {
                    self.expect(Comma)?;
                } else {
                    break;
                }
            }
            self.expect(CloseParen)?;
        }
        Ok(Pattern {
            kind: PatternKind::Variant {
                type_name,
                variant,
                binders,
            },
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_match_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Match)?;
        let scrutinee = self.parse_expr(0)?;
        self.expect(With)?;
        self.expect(OpenCurly)?;
        let mut arms = Vec::new();
        while self.peek().kind != CloseCurly && self.peek().kind != Eof {
            if self.peek().kind == Semicolon {
                self.advance();
                continue;
            }
            let pattern = self.parse_pattern()?;
            self.expect(FatArrow)?;
            let body = self.parse_branch_stmt()?;
            arms.push(StmtArm { pattern, body });
        }
        self.expect(CloseCurly)?;
        Ok(Stmt {
            kind: StmtKind::Match { scrutinee, arms },
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_match_expr(&mut self) -> PResult<ExprKind> {
        let scrutinee = self.parse_expr(0)?;
        self.expect(With)?;
        self.expect(OpenCurly)?;
        let mut arms = Vec::new();
        while self.peek().kind != CloseCurly && self.peek().kind != Eof {
            if self.peek().kind == Semicolon {
                self.advance();
                continue;
            }
            let pattern = self.parse_pattern()?;
            self.expect(FatArrow)?;
            let body = self.parse_branch_expr()?;
            arms.push(ExprArm { pattern, body });
            self.consume_statement_terminator()?;
        }
        self.expect(CloseCurly)?;
        Ok(ExprKind::Match {
            scrutinee: Box::new(scrutinee),
            arms,
        })
    }

    fn parse_for_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(For)?;
        self.expect(OpenParen)?;
        let init = self.parse_stmt()?;
        let cond = match self.parse_expression_stmt()?.kind {
            StmtKind::Expression(expr) => expr,
            _ => unreachable!("parse_expression_stmt yields an expression statement"),
        };
        let iter = self.parse_expr(0)?;
        self.expect(CloseParen)?;
        self.expect(OpenCurly)?;
        let body = self.parse_block_stmt();
        self.expect(CloseCurly)?;
        Ok(Stmt {
            kind: StmtKind::For {
                init: Box::new(init),
                cond,
                iter,
                body,
            },
            span: start.to(self.prev_token().span),
        })
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

    fn parse_index(&mut self, array: Expr) -> PResult<ExprKind> {
        self.expect(OpenBracket)?;
        let index = self.parse_expr(0)?;
        self.expect(CloseBracket)?;
        Ok(ExprKind::Index {
            array: Box::new(array),
            index: Box::new(index),
        })
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
    /// failed statement so the block still yields as much as possible.
    fn parse_block_stmt(&mut self) -> Vec<Stmt> {
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
        statements
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

    fn parse_use_decl_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Use)?;
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut specs = Vec::new();
        while self.peek().kind != CloseCurly {
            let ahead = self.lookahead(2);
            let alias = if ahead.len() == 2 && ahead[0].kind == Identifier && ahead[1].kind == Colon
            {
                let a = self.expect(Identifier)?.text;
                self.expect(Colon)?;
                Some(a)
            } else {
                None
            };
            let path = self.parse_module_path()?;
            specs.push(UseSpec { alias, path });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Stmt {
            kind: StmtKind::Use(specs),
            span: start.to(self.prev_token().span),
        })
    }

    fn parse_module_path(&mut self) -> PResult<Vec<String>> {
        let mut path = vec![self.expect(Identifier)?.text];
        while self.peek().kind == Dot {
            self.expect(Dot)?;
            path.push(self.expect(Identifier)?.text);
        }
        Ok(path)
    }
}
