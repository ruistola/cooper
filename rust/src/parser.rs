use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Token, TokenKind};

use TokenKind::*;

/// A marker that unwinds the current parse to the nearest recovery point. The
/// actual diagnostic has already been recorded in `Parser::errors` by the time an
/// `Err(Recover)` is produced, so recovery sites only need to resynchronize.
#[derive(Debug, Clone, Copy)]
pub struct Recover;

type PResult<T> = Result<T, Recover>;

/// The result of parsing: the recovered module plus every diagnostic collected
/// along the way. A non-empty `errors` means the module is best-effort.
pub struct Parsed {
    pub module: Vec<Stmt>,
    pub errors: Vec<Diagnostic>,
}

pub fn parse(tokens: Vec<Token>) -> Parsed {
    let mut p = Parser::new(tokens);
    let module = p.parse_module();
    Parsed {
        module,
        errors: p.errors,
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    paren_stack: Vec<TokenKind>,
    in_then_branch: bool,
    errors: Vec<Diagnostic>,
}

/// An end-of-line may become a semicolon only when the preceding token is one of
/// these (mirrors the Go `beforeSemicolon` set).
fn can_precede_semicolon(k: TokenKind) -> bool {
    matches!(
        k,
        Number
            | String
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

/// An end-of-line may become a semicolon only when the following token is one of
/// these (mirrors the Go `afterSemicolon` set).
fn can_follow_semicolon(k: TokenKind) -> bool {
    matches!(
        k,
        Eof | Number
            | String
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

/// Right binding power of a prefix operator in head (NUD) position.
fn prefix_bp(k: TokenKind) -> i32 {
    match k {
        Plus | Dash => 10,
        Ampersand => 12,
        _ => 0,
    }
}

/// (left, right) binding power of a token in tail (LED) position. A zero left
/// binding power terminates the expression.
fn tail_bp(k: TokenKind) -> (i32, i32) {
    match k {
        Equals | PlusEquals | DashEquals | ColonEquals => (1, 2),
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
            text: std::string::String::new(),
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

    /// Read-only lookahead over the next `n` significant tokens (end-of-line
    /// tokens skipped). Never mutates the stream, so it is safe for speculative
    /// dispatch.
    fn lookahead(&self, n: usize) -> Vec<Token> {
        let mut out = Vec::with_capacity(n);
        let mut i = self.pos;
        while i < self.tokens.len() && out.len() < n {
            let t = &self.tokens[i];
            if t.kind == Eol {
                i += 1;
                continue;
            }
            out.push(t.clone());
            i += 1;
        }
        out
    }

    /// A statement leading with `( ident :` is a method declaration receiver.
    fn is_method_decl_ahead(&self) -> bool {
        let ahead = self.lookahead(3);
        ahead.len() == 3
            && ahead[0].kind == OpenParen
            && ahead[1].kind == Identifier
            && ahead[2].kind == Colon
    }

    /// Returns the current token, performing end-of-line-to-semicolon inference:
    /// an `Eol` is rewritten to a `Semicolon` where a statement can terminate, and
    /// otherwise dropped as insignificant whitespace. This keeps the rest of the
    /// parser whitespace-agnostic.
    fn peek(&mut self) -> Token {
        let curr = self.current_token();
        if curr.kind == Eol {
            // Never infer a terminator right before a closing brace: a block's
            // final value must be kept unless an explicit `;` discards it.
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
            CloseCurly => {
                if self.paren_stack.last() == Some(&OpenCurly) {
                    self.pop_paren(CloseCurly);
                }
            }
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
        let matches = match self.paren_stack.last() {
            Some(OpenParen) => closing == CloseParen,
            Some(OpenBracket) => closing == CloseBracket,
            Some(OpenCurly) => closing == CloseCurly,
            _ => false,
        };
        if matches {
            self.paren_stack.pop();
        } else {
            let sp = self.current_token().span;
            self.error(sp, format!("unmatched {}", closing));
        }
    }

    fn error(&mut self, span: Span, message: impl Into<std::string::String>) -> Recover {
        self.errors.push(Diagnostic::error(span, message));
        Recover
    }

    /// Skip tokens until the next plausible statement boundary so parsing can
    /// resume after an error. This is what lets one run report many errors.
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
        let k = self.peek().kind;
        match k {
            Semicolon => {
                self.advance();
                return Ok(());
            }
            Eof | CloseCurly => return Ok(()),
            Else if self.in_then_branch => return Ok(()),
            _ => {}
        }
        if self.prev_token().kind == CloseCurly {
            Ok(())
        } else {
            let sp = self.peek().span;
            Err(self.error(sp, "expected statement terminator"))
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
            return Ok(Stmt::FuncDecl(self.parse_func_decl()?));
        }
        match self.peek().kind {
            For => self.parse_for_stmt(),
            Func => Ok(Stmt::FuncDecl(self.parse_func_decl()?)),
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
            let token = self.peek();
            let (lbp, rbp) = tail_bp(token.kind);
            if lbp <= min_bp {
                break;
            }
            left = self.parse_tail_expr(left, rbp)?;
        }
        Ok(left)
    }

    fn parse_head_expr(&mut self, token: Token) -> PResult<Expr> {
        match token.kind {
            Number => Ok(Expr::Number(token.text)),
            String => Ok(Expr::Str(token.text)),
            Identifier => Ok(Expr::Ident(token.text)),
            True | False => Ok(Expr::Bool(token.kind == True)),
            Plus | Dash => {
                let rhs = self.parse_expr(prefix_bp(token.kind))?;
                Ok(Expr::Unary {
                    op: token.kind,
                    rhs: Box::new(rhs),
                })
            }
            Ampersand => {
                let operand = self.parse_expr(prefix_bp(token.kind))?;
                Ok(Expr::AddressOf(Box::new(operand)))
            }
            Nil => Ok(Expr::Nil),
            OpenParen => self.parse_paren_head(),
            If => self.parse_if_expr(),
            Match => self.parse_match_expr(),
            OpenCurly => {
                let block = self.parse_block_expr();
                self.expect(CloseCurly)?;
                Ok(Expr::Block(block))
            }
            _ => Err(self.error(
                token.span,
                format!("expected an expression, found {}", token.kind),
            )),
        }
    }

    /// `()` unit, `(e)` group, or `(e, e, ...)` tuple, disambiguated by contents.
    fn parse_paren_head(&mut self) -> PResult<Expr> {
        if self.peek().kind == CloseParen {
            self.expect(CloseParen)?;
            return Ok(Expr::Unit);
        }
        let first = self.parse_expr(0)?;
        if self.peek().kind != Comma {
            self.expect(CloseParen)?;
            return Ok(Expr::Group(Box::new(first)));
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
        Ok(Expr::Tuple(elems))
    }

    fn parse_tail_expr(&mut self, head: Expr, rbp: i32) -> PResult<Expr> {
        let curr = self.peek();
        match curr.kind {
            ColonEquals => self.parse_decl_assign_expr(head),
            Equals | PlusEquals | DashEquals | StarEquals | SlashEquals => {
                let op = self.advance().kind;
                let rhs = self.parse_expr(rbp)?;
                Ok(Expr::Assign {
                    target: Box::new(head),
                    op,
                    value: Box::new(rhs),
                })
            }
            Plus | Dash | Star | Slash | Percent | Less | LessEquals | Greater | GreaterEquals
            | Or | And | DoubleEquals | NotEquals => {
                let op = self.advance().kind;
                let rhs = self.parse_expr(rbp)?;
                Ok(Expr::Binary {
                    lhs: Box::new(head),
                    op,
                    rhs: Box::new(rhs),
                })
            }
            OpenParen => self.parse_func_call_expr(head),
            OpenCurly => self.parse_struct_literal_expr(head),
            OpenBracket => self.parse_array_index_expr(head),
            Dot => self.parse_struct_member_expr(head),
            Chevron => {
                self.expect(Chevron)?;
                Ok(Expr::Deref(Box::new(head)))
            }
            _ => Err(self.error(
                curr.span,
                format!("{} cannot continue an expression", curr.kind),
            )),
        }
    }

    // --- type expressions ---

    fn parse_type_expr(&mut self) -> PResult<TypeExpr> {
        let head = self.parse_type_atom()?;
        if !self.next_starts_type_atom() {
            return Ok(head);
        }
        let mut args = Vec::new();
        while self.next_starts_type_atom() {
            args.push(self.parse_type_atom()?);
        }
        Ok(TypeExpr::Application {
            constructor: Box::new(head),
            args,
        })
    }

    fn next_starts_type_atom(&mut self) -> bool {
        matches!(self.peek().kind, Identifier | OpenParen | Func)
    }

    fn parse_type_atom(&mut self) -> PResult<TypeExpr> {
        let mut t = if self.peek().kind == OpenParen {
            self.expect(OpenParen)?;
            let inner = if self.peek().kind == CloseParen {
                TypeExpr::Unit
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
                    elems.pop().unwrap()
                } else {
                    TypeExpr::Tuple(elems)
                }
            };
            self.expect(CloseParen)?;
            inner
        } else if self.peek().kind == Func {
            self.parse_func_type_expr()?
        } else {
            let name = self.expect(Identifier)?.text;
            TypeExpr::Named(name)
        };
        loop {
            match self.peek().kind {
                OpenBracket => {
                    self.expect(OpenBracket)?;
                    self.expect(CloseBracket)?;
                    t = TypeExpr::Array(Box::new(t));
                }
                Chevron => {
                    self.expect(Chevron)?;
                    t = TypeExpr::Pointer(Box::new(t));
                }
                _ => return Ok(t),
            }
        }
    }

    fn parse_func_type_expr(&mut self) -> PResult<TypeExpr> {
        self.expect(Func)?;
        self.expect(OpenParen)?;
        let mut param_types = Vec::new();
        while self.peek().kind != CloseParen {
            if self.peek().kind == Identifier {
                let name = self.expect(Identifier)?.text;
                if self.peek().kind == Colon {
                    self.expect(Colon)?;
                    param_types.push(self.parse_type_expr()?);
                } else {
                    param_types.push(TypeExpr::Named(name));
                }
            } else {
                param_types.push(self.parse_type_expr()?);
            }
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        let return_type = if self.peek().kind == Colon {
            self.expect(Colon)?;
            self.parse_type_expr()?
        } else {
            TypeExpr::Unit
        };
        Ok(TypeExpr::Func {
            return_type: Box::new(return_type),
            param_types,
        })
    }

    // --- declaration-assignment ---

    fn parse_decl_assign_expr(&mut self, head: Expr) -> PResult<Expr> {
        self.expect(ColonEquals)?;
        match head {
            Expr::Ident(name) => Ok(Expr::VarDeclAssign {
                name,
                value: Box::new(self.parse_expr(0)?),
            }),
            Expr::Tuple(elems) => {
                let names = self.pattern_names(elems)?;
                Ok(Expr::TupleDeclAssign {
                    names,
                    ty: None,
                    value: Box::new(self.parse_expr(0)?),
                })
            }
            _ => {
                let sp = self.current_token().span;
                Err(self.error(
                    sp,
                    "the left-hand side of ':=' must be an identifier or a tuple pattern",
                ))
            }
        }
    }

    fn pattern_names(&mut self, elems: Vec<Expr>) -> PResult<Vec<std::string::String>> {
        let mut names = Vec::with_capacity(elems.len());
        for elem in elems {
            match elem {
                Expr::Ident(name) => names.push(name),
                _ => {
                    let sp = self.current_token().span;
                    return Err(self.error(sp, "a destructuring pattern may only bind identifiers"));
                }
            }
        }
        Ok(names)
    }

    // --- statements ---

    fn parse_var_decl_stmt(&mut self) -> PResult<Stmt> {
        self.expect(Let)?;
        if self.peek().kind == OpenParen {
            let expr = self.parse_let_destructure()?;
            return Ok(Stmt::Expression {
                expr,
                explicit_semicolon: false,
            });
        }
        let name = self.expect(Identifier)?.text;
        let ty = if self.peek().kind == Colon {
            self.expect(Colon)?;
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let init = if !self.statement_terminates() {
            self.expect(Equals)?;
            Some(self.parse_expr(0)?)
        } else {
            None
        };
        self.consume_statement_terminator()?;
        Ok(Stmt::VarDecl { name, ty, init })
    }

    fn parse_let_destructure(&mut self) -> PResult<Expr> {
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
        Ok(Expr::TupleDeclAssign {
            names,
            ty,
            value: Box::new(value),
        })
    }

    fn parse_func_decl(&mut self) -> PResult<FuncDecl> {
        let receiver = if self.peek().kind == OpenParen {
            self.expect(OpenParen)?;
            let name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let ty = self.parse_type_expr()?;
            self.expect(CloseParen)?;
            Some(TypedIdent { name, ty })
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
            let param_name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let param_type = self.parse_type_expr()?;
            params.push(TypedIdent {
                name: param_name,
                ty: param_type,
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
        })
    }

    fn parse_struct_decl_stmt(&mut self) -> PResult<Stmt> {
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
            let member_name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let member_type = self.parse_type_expr()?;
            members.push(TypedIdent {
                name: member_name,
                ty: member_type,
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Stmt::StructDecl {
            name,
            type_params,
            members,
        })
    }

    fn parse_oneof_decl_stmt(&mut self) -> PResult<Stmt> {
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
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Stmt::OneofDecl {
            name,
            type_params,
            variants,
        })
    }

    fn parse_if_expr(&mut self) -> PResult<Expr> {
        let cond = self.parse_expr(0)?;
        self.expect(Then)?;
        let then = self.parse_branch_expr()?;
        // A semicolon may have been inferred before `else`; swallow it.
        if self.peek().kind == Semicolon {
            self.expect(Semicolon)?;
        }
        self.expect(Else)?;
        let els = self.parse_branch_expr()?;
        Ok(Expr::If {
            cond: Box::new(cond),
            then: Box::new(then),
            els: Box::new(els),
        })
    }

    /// A branch of an if-expression: either a braced value block or a bare expr.
    fn parse_branch_expr(&mut self) -> PResult<Expr> {
        if self.peek().kind == OpenCurly {
            self.expect(OpenCurly)?;
            let block = self.parse_block_expr();
            self.expect(CloseCurly)?;
            Ok(Expr::Block(block))
        } else {
            self.parse_expr(0)
        }
    }

    fn parse_if_stmt(&mut self) -> PResult<Stmt> {
        self.expect(If)?;
        let cond = self.parse_expr(0)?;
        self.expect(Then)?;
        let then = if self.peek().kind == OpenCurly {
            self.expect(OpenCurly)?;
            let body = self.parse_block_stmt();
            self.expect(CloseCurly)?;
            Stmt::Block(body)
        } else {
            self.in_then_branch = true;
            let s = self.parse_stmt()?;
            self.in_then_branch = false;
            s
        };
        let els = if self.peek().kind == Else {
            self.expect(Else)?;
            let s = if self.peek().kind == OpenCurly {
                self.expect(OpenCurly)?;
                let body = self.parse_block_stmt();
                self.expect(CloseCurly)?;
                Stmt::Block(body)
            } else {
                self.parse_stmt()?
            };
            Some(Box::new(s))
        } else {
            None
        };
        Ok(Stmt::If {
            cond,
            then: Box::new(then),
            els,
        })
    }

    fn parse_pattern(&mut self) -> PResult<Pattern> {
        if self.peek().kind == Identifier && self.peek().text == "_" {
            self.expect(Identifier)?;
            return Ok(Pattern::Wildcard);
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
        Ok(Pattern::Variant {
            type_name,
            variant,
            binders,
        })
    }

    fn parse_match_stmt(&mut self) -> PResult<Stmt> {
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
            let body = if self.peek().kind == OpenCurly {
                self.expect(OpenCurly)?;
                let body = self.parse_block_stmt();
                self.expect(CloseCurly)?;
                Stmt::Block(body)
            } else {
                self.parse_stmt()?
            };
            arms.push(MatchStmtArm { pattern, body });
        }
        self.expect(CloseCurly)?;
        Ok(Stmt::Match { scrutinee, arms })
    }

    fn parse_match_expr(&mut self) -> PResult<Expr> {
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
            arms.push(MatchExprArm { pattern, body });
            self.consume_statement_terminator()?;
        }
        self.expect(CloseCurly)?;
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
        })
    }

    fn parse_for_stmt(&mut self) -> PResult<Stmt> {
        self.expect(For)?;
        self.expect(OpenParen)?;
        let init = self.parse_stmt()?;
        let cond = match self.parse_expression_stmt()? {
            Stmt::Expression { expr, .. } => expr,
            _ => unreachable!("parse_expression_stmt always yields an expression statement"),
        };
        let iter = self.parse_expr(0)?;
        self.expect(CloseParen)?;
        self.expect(OpenCurly)?;
        let body = self.parse_block_stmt();
        self.expect(CloseCurly)?;
        Ok(Stmt::For {
            init: Box::new(init),
            cond,
            iter,
            body,
        })
    }

    fn parse_func_call_expr(&mut self, func: Expr) -> PResult<Expr> {
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
        Ok(Expr::FuncCall {
            func: Box::new(func),
            args,
        })
    }

    fn parse_struct_literal_expr(&mut self, target: Expr) -> PResult<Expr> {
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut members = Vec::new();
        while self.peek().kind != CloseCurly {
            let member_name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            members.push(MemberAssign {
                name: member_name,
                value: self.parse_expr(0)?,
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Expr::StructLiteral {
            target: Box::new(target),
            members,
        })
    }

    fn parse_struct_member_expr(&mut self, target: Expr) -> PResult<Expr> {
        self.expect(Dot)?;
        let member = self.expect(Identifier)?.text;
        Ok(Expr::StructMember {
            target: Box::new(target),
            member,
        })
    }

    fn parse_array_index_expr(&mut self, array: Expr) -> PResult<Expr> {
        self.expect(OpenBracket)?;
        let index = self.parse_expr(0)?;
        self.expect(CloseBracket)?;
        Ok(Expr::ArrayIndex {
            array: Box::new(array),
            index: Box::new(index),
        })
    }

    fn parse_return_stmt(&mut self) -> PResult<Stmt> {
        self.expect(Return)?;
        if self.statement_terminates() {
            self.consume_statement_terminator()?;
            return Ok(Stmt::Return(None));
        }
        let expr = self.parse_expr(0)?;
        self.consume_statement_terminator()?;
        Ok(Stmt::Return(Some(expr)))
    }

    fn parse_expression_stmt(&mut self) -> PResult<Stmt> {
        let expr = self.parse_expr(0)?;
        let explicit_semicolon = self.peek().kind == Semicolon;
        self.consume_statement_terminator()?;
        Ok(Stmt::Expression {
            expr,
            explicit_semicolon,
        })
    }

    /// Parse statements up to a closing brace or end of input, recovering past
    /// any statement that fails so the block still yields as much as possible.
    fn parse_block_stmt(&mut self) -> Vec<Stmt> {
        let mut statements = Vec::new();
        loop {
            let k = self.peek().kind;
            if k == Eof || k == CloseCurly {
                break;
            }
            if k == Semicolon {
                self.advance();
                continue;
            }
            match self.parse_stmt() {
                Ok(s) => statements.push(s),
                Err(_) => self.synchronize(),
            }
        }
        statements
    }

    /// A value block: its trailing expression statement (if any) becomes the
    /// result; otherwise the block yields unit.
    fn parse_block_expr(&mut self) -> Block {
        let mut statements = self.parse_block_stmt();
        let mut result = Expr::Unit;
        if let Some(Stmt::Expression { .. }) = statements.last() {
            if let Some(Stmt::Expression { expr, .. }) = statements.pop() {
                result = expr;
            }
        }
        Block {
            statements,
            result: Box::new(result),
        }
    }

    fn parse_use_decl_stmt(&mut self) -> PResult<Stmt> {
        self.expect(Use)?;
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut specs = Vec::new();
        while self.peek().kind != CloseCurly {
            let ahead = self.lookahead(2);
            let alias = if ahead.len() == 2
                && ahead[0].kind == Identifier
                && ahead[1].kind == Colon
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
        Ok(Stmt::Use(specs))
    }

    fn parse_module_path(&mut self) -> PResult<Vec<std::string::String>> {
        let mut path = vec![self.expect(Identifier)?.text];
        while self.peek().kind == Dot {
            self.expect(Dot)?;
            path.push(self.expect(Identifier)?.text);
        }
        Ok(path)
    }
}
