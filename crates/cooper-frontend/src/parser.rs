//! A hand-written recursive-descent parser with Pratt expression parsing.
//!
//! The parser collects diagnostics and recovers at statement boundaries instead
//! of bailing on the first error, so one run reports as many problems as it can.
//! Every produced node carries the source span it was parsed from.
//!
//! This module holds the token cursor, error recovery, semicolon inference, and the
//! file and top-level drivers. Its submodules extend `Parser` by syntactic category:
//! `exprs`, `stmts`, `patterns` (with `match`), and `decls` (declarations, `use`
//! blocks, and type expressions).

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Token, TokenKind};

use TokenKind::*;

mod decls;
mod exprs;
mod patterns;
mod stmts;

/// The outcome of parsing one source file: its `use` imports and top-level
/// declarations, plus any diagnostics. A non-empty `errors` means some nodes were
/// recovered rather than cleanly parsed.
pub struct ParsedFile {
    pub uses: Vec<UseSpec>,
    pub decls: Vec<Stmt>,
    pub errors: Vec<Diagnostic>,
}

/// Parse a token stream into a source file.
pub fn parse(tokens: Vec<Token>) -> ParsedFile {
    let mut parser = Parser::new(tokens);
    let (uses, decls) = parser.parse_file();
    ParsedFile {
        uses,
        decls,
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
    /// Keywords that end the unbraced single-statement bodies being parsed: `else`
    /// after a `then` branch, `while`/`until` after a `do`/`repeat` body. None of
    /// them can continue a statement, so meeting one terminates it. A braced block
    /// starts afresh.
    closers: Vec<TokenKind>,
    /// How many control-flow header expressions we are nested inside. While this
    /// is non-zero, newlines carry no meaning (like inside brackets): a long
    /// condition or iterable may span lines, and the delimiter keyword need not
    /// share the header's last line. A braced statement block resets it to zero so
    /// statements within the header still terminate at newlines.
    header_depth: u32,
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
            | Break
            | Continue
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
            | Not
            | Plus
            | Dash
            | Break
            | Continue
            | Do
            | False
            | For
            | Func
            | If
            | Let
            | Match
            | Nil
            | Repeat
            | Return
            | Struct
            | Oneof
            | True
            | Until
            | Use
            | While
    )
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            paren_stack: Vec::new(),
            closers: Vec::new(),
            header_depth: 0,
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
            let outside_parens = self.paren_stack.is_empty() && self.header_depth == 0;
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
    /// resume after an error. A braced block the failed statement opened is skipped
    /// whole, so its closing brace is not taken for the enclosing block's.
    fn synchronize(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek().kind {
                Eof => return,
                CloseCurly if depth == 0 => return,
                Semicolon if depth == 0 => {
                    self.advance();
                    return;
                }
                For | Func | If | Let | Match | Return | Struct | Oneof | Use | While | Until
                | Do | Repeat | Break | Continue
                    if depth == 0 =>
                {
                    return
                }
                kind => {
                    match kind {
                        OpenCurly => depth += 1,
                        CloseCurly => depth -= 1,
                        _ => {}
                    }
                    self.advance();
                }
            }
        }
    }

    // --- statement terminators ---

    fn statement_terminates(&mut self) -> bool {
        match self.peek().kind {
            Semicolon | Eof | CloseCurly => true,
            kind if self.closers.contains(&kind) => true,
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
            kind if self.closers.contains(&kind) => Ok(()),
            _ if self.prev_token().kind == CloseCurly => Ok(()),
            _ => {
                let span = self.peek().span;
                Err(self.error(span, "expected statement terminator"))
            }
        }
    }

    // --- top level ---

    /// Parse a whole file: any number of top-of-file `use` blocks, then top-level
    /// declarations. A `use` appearing after a declaration, or a non-declaration at
    /// top level (a bare expression or control-flow statement), is a diagnostic —
    /// a file is a set of declarations, not a script.
    fn parse_file(&mut self) -> (Vec<UseSpec>, Vec<Stmt>) {
        let mut uses = Vec::new();
        let mut decls = Vec::new();
        let mut seen_decl = false;
        while self.peek().kind != Eof {
            let before = self.pos;
            if self.peek().kind == Semicolon {
                self.advance();
                continue;
            }
            if self.peek().kind == Use {
                let span = self.peek().span;
                match self.parse_use_block() {
                    Ok(specs) => {
                        if seen_decl {
                            self.errors.push(Diagnostic::error(
                                span,
                                "use declarations must appear at the top of the file",
                            ));
                        }
                        uses.extend(specs);
                    }
                    Err(_) => self.synchronize(),
                }
            } else {
                match self.parse_top_level_decl() {
                    Ok(s) => {
                        seen_decl = true;
                        decls.push(s);
                    }
                    Err(_) => self.synchronize(),
                }
            }
            // Guarantee forward progress. A stray closing brace halts
            // `synchronize` without being consumed, and it cannot begin a
            // top-level declaration, so recovery would otherwise retry it
            // forever; drop one token whenever an iteration consumed nothing.
            if self.pos == before {
                self.advance();
            }
        }
        (uses, decls)
    }

    /// Parse a single top-level declaration: a function or method, a struct, a sum
    /// type, or a module-level variable. Anything else — a bare expression or a
    /// control-flow statement — is rejected: those only belong inside a body.
    fn parse_top_level_decl(&mut self) -> PResult<Stmt> {
        if self.peek().kind == OpenParen && self.is_method_decl_ahead() {
            let decl = self.parse_func_decl()?;
            let span = decl.span;
            return Ok(Stmt {
                kind: StmtKind::FuncDecl(decl),
                span,
            });
        }
        match self.peek().kind {
            Func => {
                let decl = self.parse_func_decl()?;
                let span = decl.span;
                Ok(Stmt {
                    kind: StmtKind::FuncDecl(decl),
                    span,
                })
            }
            Struct => self.parse_struct_decl_stmt(),
            Oneof => self.parse_oneof_decl_stmt(),
            Let => self.parse_var_decl_stmt(),
            _ => {
                let token = self.peek();
                Err(self.error(
                    token.span,
                    format!("expected a top-level declaration, found {}", token.kind),
                ))
            }
        }
    }

}
