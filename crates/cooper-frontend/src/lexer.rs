use logos::Logos;

use crate::diag::{Diagnostic, Span};

/// The lexical token kinds.
///
/// Horizontal whitespace and `#` line comments are skipped outright. End-of-line
/// is kept as a token because semicolon inference in the parser depends on it.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[logos(skip r"[ \t\f]+")]
#[logos(skip r"#[^\n\r]*")]
pub enum TokenKind {
    #[regex(r"\r\n|\n|\r")]
    Eol,

    // Literals
    #[regex(r"0[xX][0-9a-fA-F](_?[0-9a-fA-F])*|0[bB][01](_?[01])*|[0-9](_?[0-9])*\.[0-9](_?[0-9])*([eE][+-]?[0-9](_?[0-9])*)?|[0-9](_?[0-9])*([eE][+-]?[0-9](_?[0-9])*)?")]
    Number,
    /// A string literal, holes included: `"X is {x}"` (see [`scan_string`]).
    #[token("\"", scan_string)]
    Str,
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Identifier,

    // Multi-character operators
    #[token(":=")]
    ColonEquals,
    #[token("=>")]
    FatArrow,
    #[token("..")]
    DotDot,
    #[token("..=")]
    DotDotEquals,
    #[token("==")]
    DoubleEquals,
    #[token("!=")]
    NotEquals,
    #[token("<=")]
    LessEquals,
    #[token(">=")]
    GreaterEquals,
    #[token("+=")]
    PlusEquals,
    #[token("-=")]
    DashEquals,
    #[token("*=")]
    StarEquals,
    #[token("/=")]
    SlashEquals,
    #[token("%=")]
    PercentEquals,
    #[token("<<")]
    ShiftLeft,
    #[token(">>")]
    ShiftRight,

    // Single-character tokens
    #[token("=")]
    Equals,
    #[token("!")]
    Not,
    #[token("|")]
    Pipe,
    #[token("&")]
    Ampersand,
    #[token("^")]
    Chevron,
    #[token("<")]
    Less,
    #[token(">")]
    Greater,
    #[token("+")]
    Plus,
    #[token("-")]
    Dash,
    #[token("/")]
    Slash,
    #[token("*")]
    Star,
    #[token("%")]
    Percent,
    #[token(".")]
    Dot,
    #[token(";")]
    Semicolon,
    #[token(":")]
    Colon,
    #[token(",")]
    Comma,
    #[token("[")]
    OpenBracket,
    #[token("]")]
    CloseBracket,
    #[token("{")]
    OpenCurly,
    #[token("}")]
    CloseCurly,
    #[token("(")]
    OpenParen,
    #[token(")")]
    CloseParen,

    // Reserved keywords
    #[token("and")]
    And,
    #[token("as")]
    As,
    #[token("break")]
    Break,
    #[token("continue")]
    Continue,
    #[token("do")]
    Do,
    #[token("else")]
    Else,
    #[token("extern")]
    Extern,
    #[token("false")]
    False,
    #[token("for")]
    For,
    #[token("func")]
    Func,
    #[token("if")]
    If,
    #[token("in")]
    In,
    #[token("let")]
    Let,
    #[token("nil")]
    Nil,
    #[token("or")]
    Or,
    #[token("repeat")]
    Repeat,
    #[token("return")]
    Return,
    #[token("struct")]
    Struct,
    #[token("oneof")]
    Oneof,
    #[token("then")]
    Then,
    #[token("true")]
    True,
    #[token("until")]
    Until,
    #[token("use")]
    Use,
    #[token("match")]
    Match,
    #[token("while")]
    While,
    #[token("with")]
    With,
    #[token("xor")]
    Xor,

    /// Synthetic end-of-input marker appended after tokenization.
    Eof,
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use TokenKind::*;
        let name = match self {
            Eol => "end of line",
            Number => "number",
            Str => "string",
            Identifier => "identifier",
            ColonEquals => "':='",
            FatArrow => "'=>'",
            DotDot => "'..'",
            DotDotEquals => "'..='",
            DoubleEquals => "'=='",
            NotEquals => "'!='",
            LessEquals => "'<='",
            GreaterEquals => "'>='",
            PlusEquals => "'+='",
            DashEquals => "'-='",
            StarEquals => "'*='",
            SlashEquals => "'/='",
            PercentEquals => "'%='",
            ShiftLeft => "'<<'",
            ShiftRight => "'>>'",
            Equals => "'='",
            Not => "'!'",
            Pipe => "'|'",
            Ampersand => "'&'",
            Chevron => "'^'",
            Less => "'<'",
            Greater => "'>'",
            Plus => "'+'",
            Dash => "'-'",
            Slash => "'/'",
            Star => "'*'",
            Percent => "'%'",
            Dot => "'.'",
            Semicolon => "';'",
            Colon => "':'",
            Comma => "','",
            OpenBracket => "'['",
            CloseBracket => "']'",
            OpenCurly => "'{'",
            CloseCurly => "'}'",
            OpenParen => "'('",
            CloseParen => "')'",
            And => "'and'",
            As => "'as'",
            Break => "'break'",
            Continue => "'continue'",
            Do => "'do'",
            Else => "'else'",
            Extern => "'extern'",
            False => "'false'",
            For => "'for'",
            Func => "'func'",
            If => "'if'",
            In => "'in'",
            Let => "'let'",
            Nil => "'nil'",
            Or => "'or'",
            Repeat => "'repeat'",
            Return => "'return'",
            Struct => "'struct'",
            Oneof => "'oneof'",
            Then => "'then'",
            True => "'true'",
            Until => "'until'",
            Use => "'use'",
            Match => "'match'",
            While => "'while'",
            With => "'with'",
            Xor => "'xor'",
            Eof => "end of input",
        };
        f.write_str(name)
    }
}

/// Scan the rest of a string literal whose opening quote `lex` has matched, through its
/// closing quote. A `\` escapes the next character. A `{` opens a hole holding an
/// expression, which may contain braces and string literals of its own, up to the
/// matching `}`. An unterminated literal or hole is an error.
fn scan_string(lex: &mut logos::Lexer<TokenKind>) -> bool {
    match string_end(lex.remainder().as_bytes()) {
        Some(len) => {
            lex.bump(len);
            true
        }
        None => false,
    }
}

/// The length of the literal body starting at `rest[0]`, closing quote included.
fn string_end(rest: &[u8]) -> Option<usize> {
    let mut i = 0;
    loop {
        match rest.get(i)? {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            b'{' => i += 1 + hole_end(&rest[i + 1..])?,
            _ => i += 1,
        }
    }
}

/// The length of a hole's source starting at `rest[0]`, closing brace included.
pub(crate) fn hole_end(rest: &[u8]) -> Option<usize> {
    let mut depth = 0;
    let mut i = 0;
    loop {
        match rest.get(i)? {
            b'"' => i += 1 + string_end(&rest[i + 1..])?,
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' if depth == 0 => return Some(i + 1),
            b'}' => {
                depth -= 1;
                i += 1;
            }
            b'\n' | b'\r' => return None,
            _ => i += 1,
        }
    }
}

/// A lexed token: its kind, the exact source slice it spans, and that span.
#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub span: Span,
}

/// Tokenize `src`. Repeated end-of-line tokens are collapsed to one (so blank
/// lines never produce spurious statement terminators), horizontal whitespace and
/// comments are dropped, and a trailing `Eof` token is appended. An unexpected
/// character yields an `Err` diagnostic locating it.
///
/// Logos matches maximally without rewinding, so a numeric literal immediately
/// followed by a range operator (`0..10`) is lexed as a number ending in a dot
/// (`0.`) plus a stray `.`. A valid float always has a digit after its dot, so a
/// number whose text ends in `.` is unambiguously this case: the trailing dot is
/// peeled off into its own token and adjacent dots are merged into `..`.
pub fn tokenize(src: &str) -> Result<Vec<Token>, Diagnostic> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut lex = TokenKind::lexer(src);
    while let Some(result) = lex.next() {
        let span: Span = lex.span().into();
        let kind = result.map_err(|_| {
            let message = if lex.slice().starts_with('"') { "unterminated string literal" } else { "unexpected character" };
            Diagnostic::error(span, message)
        })?;
        // Collapse consecutive end-of-line tokens into a single one.
        if kind == TokenKind::Eol && matches!(tokens.last(), Some(t) if t.kind == TokenKind::Eol) {
            continue;
        }
        // Peel a trailing dot off a stuck numeric literal into a standalone dot.
        if kind == TokenKind::Number && lex.slice().ends_with('.') {
            let text = lex.slice();
            let dot_start = span.end - 1;
            tokens.push(Token {
                kind: TokenKind::Number,
                text: text[..text.len() - 1].to_string(),
                span: Span::new(span.start, dot_start),
            });
            push_dot(&mut tokens, Span::new(dot_start, span.end));
            continue;
        }
        if kind == TokenKind::Dot {
            push_dot(&mut tokens, span);
            continue;
        }
        // A digit-adjacent `..=` reaches here as `..` `=` (the number swallowed the
        // first dot and was split above); fuse the `=` back on to recover `..=`.
        if kind == TokenKind::Equals {
            if let Some(prev) = tokens.last_mut() {
                if prev.kind == TokenKind::DotDot && prev.span.end == span.start {
                    prev.kind = TokenKind::DotDotEquals;
                    prev.text = "..=".to_string();
                    prev.span = prev.span.to(span);
                    continue;
                }
            }
        }
        tokens.push(Token {
            kind,
            text: lex.slice().to_string(),
            span,
        });
    }
    let end = src.len();
    tokens.push(Token {
        kind: TokenKind::Eof,
        text: String::new(),
        span: Span::new(end, end),
    });
    Ok(tokens)
}

/// Push a `.` token, merging it with an immediately preceding `.` into a `..`.
fn push_dot(tokens: &mut Vec<Token>, span: Span) {
    if let Some(prev) = tokens.last_mut() {
        if prev.kind == TokenKind::Dot && prev.span.end == span.start {
            prev.kind = TokenKind::DotDot;
            prev.text = "..".to_string();
            prev.span = prev.span.to(span);
            return;
        }
    }
    tokens.push(Token {
        kind: TokenKind::Dot,
        text: ".".to_string(),
        span,
    });
}

#[cfg(test)]
mod tests;
