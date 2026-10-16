use logos::Logos;

use crate::diag::{Diagnostic, Span};

/// The lexical token kinds.
///
/// Horizontal whitespace and `#` line comments are skipped outright. End-of-line
/// is kept as a token because semicolon inference in the parser depends on it.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[logos(skip r"[ \t\f]+")]
pub enum TokenKind {
    /// A `#` comment to the end of the line. It never reaches the parser: [`tokenize`]
    /// drops it, keeping a comment block that documents the token after it (see [`Docs`]).
    #[regex(r"#[^\n\r]*")]
    Comment,

    #[regex(r"\r\n|\n|\r")]
    Eol,

    // Literals
    #[regex(r"0[xX][0-9a-fA-F](_?[0-9a-fA-F])*|0[bB][01](_?[01])*|[0-9](_?[0-9])*\.[0-9](_?[0-9])*([eE][+-]?[0-9](_?[0-9])*)?|[0-9](_?[0-9])*([eE][+-]?[0-9](_?[0-9])*)?")]
    Number,
    /// A string literal, holes included: `"X is {x}"` (see [`scan_string`]).
    #[token("\"", scan_string)]
    Str,
    /// A rune literal: `'x'`, `'\n'`, `'\u{1F600}'` (see [`scan_rune`]).
    #[token("'", scan_rune)]
    Char,
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
    #[token("...")]
    Ellipsis,
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
    #[token("co")]
    Co,
    #[token("defer")]
    Defer,
    #[token("type")]
    Type,
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
    #[token("is")]
    Is,
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
            Char => "rune literal",
            ColonEquals => "':='",
            FatArrow => "'=>'",
            DotDot => "'..'",
            DotDotEquals => "'..='",
            Ellipsis => "'...'",
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
            Co => "'co'",
            Defer => "'defer'",
            Type => "'type'",
            Comment => "comment",
            Do => "'do'",
            Else => "'else'",
            Extern => "'extern'",
            False => "'false'",
            For => "'for'",
            Func => "'func'",
            If => "'if'",
            In => "'in'",
            Is => "'is'",
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

/// Scan the rest of a rune literal whose opening quote `lex` has matched, through its
/// closing quote.
fn scan_rune(lex: &mut logos::Lexer<TokenKind>) -> bool {
    match rune_end(lex.remainder().as_bytes()) {
        Some(len) => {
            lex.bump(len);
            true
        }
        None => false,
    }
}

/// The length of a rune literal's body starting at `rest[0]`, closing quote included: one
/// character, or one escape.
pub(crate) fn rune_end(rest: &[u8]) -> Option<usize> {
    let body = match rest.first()? {
        b'\\' => escape_len(rest),
        b'\'' | b'\n' | b'\r' => return None,
        first => match first {
            0x00..=0x7F => 1,
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            _ => 4,
        },
    };
    (rest.get(body) == Some(&b'\'')).then_some(body + 1)
}

/// The length of the escape sequence starting at the backslash `rest[0]`: two bytes, or
/// through the closing brace of a `\u{...}`.
pub(crate) fn escape_len(rest: &[u8]) -> usize {
    if rest.get(1) == Some(&b'u') && rest.get(2) == Some(&b'{') {
        if let Some(close) = rest.iter().position(|&b| b == b'}') {
            return close + 1;
        }
    }
    2
}

/// The length of the literal body starting at `rest[0]`, closing quote included.
fn string_end(rest: &[u8]) -> Option<usize> {
    let mut i = 0;
    loop {
        match rest.get(i)? {
            b'\\' => i += escape_len(&rest[i..]),
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
            // A rune literal may hold a quote or brace; a lone apostrophe is not one.
            b'\'' => i += 1 + rune_end(&rest[i + 1..]).unwrap_or(0),
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
    tokenize_with_docs(src).map(|(tokens, _)| tokens)
}

/// The documentation comments of a source file: the text of each comment block that sits
/// directly above a token (no blank line between), by the byte offset where that token
/// starts. The `#` and one following space are removed from each line. A comment that
/// follows code on its own line documents nothing.
pub type Docs = std::collections::HashMap<usize, String>;

/// How many line breaks `src[from..to]` holds.
fn breaks(src: &str, from: usize, to: usize) -> usize {
    src[from..to].chars().filter(|&c| c == '\n').count()
}

/// [`tokenize`], also returning the documentation comments.
pub fn tokenize_with_docs(src: &str) -> Result<(Vec<Token>, Docs), Diagnostic> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut docs = Docs::new();
    // The comment lines of the block just read: each line's end offset and text.
    let mut block: Vec<(usize, String)> = Vec::new();
    let mut lex = TokenKind::lexer(src);
    while let Some(result) = lex.next() {
        let span: Span = lex.span().into();
        let kind = result.map_err(|_| {
            let message = match lex.slice().chars().next() {
                Some('"') => "unterminated string literal",
                Some('\'') => "a rune literal holds exactly one character, as in 'a', '\\n', or '\\u{1F600}'",
                _ => "unexpected character",
            };
            Diagnostic::error(span, message)
        })?;
        if kind == TokenKind::Comment {
            let alone = tokens.last().map_or(true, |t| t.kind == TokenKind::Eol);
            let contiguous = block.last().is_some_and(|(end, _)| breaks(src, *end, span.start) == 1);
            if !alone || !contiguous {
                block.clear();
            }
            if alone {
                let text = lex.slice().trim_start_matches('#');
                block.push((span.end, text.strip_prefix(' ').unwrap_or(text).to_string()));
            }
            continue;
        }
        if kind != TokenKind::Eol {
            if let Some((end, _)) = block.last() {
                if breaks(src, *end, span.start) == 1 {
                    let lines: Vec<&str> = block.iter().map(|(_, line)| line.as_str()).collect();
                    docs.insert(span.start, lines.join("\n"));
                }
            }
            block.clear();
        }
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
    Ok((tokens, docs))
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
