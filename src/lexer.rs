use logos::Logos;

use crate::diag::{Diagnostic, Span};

/// The lexical token kinds.
///
/// Horizontal whitespace and `//` line comments are skipped outright. End-of-line
/// is kept as a token because semicolon inference in the parser depends on it.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[logos(skip r"[ \t\f]+")]
#[logos(skip r"//[^\n\r]*")]
pub enum TokenKind {
    #[regex(r"\r\n|\n|\r")]
    Eol,

    // Literals
    #[regex(r"0[xX][0-9a-fA-F](_?[0-9a-fA-F])*|0[bB][01](_?[01])*|[0-9](_?[0-9])*\.[0-9](_?[0-9])*([eE][+-]?[0-9](_?[0-9])*)?|[0-9](_?[0-9])*([eE][+-]?[0-9](_?[0-9])*)?")]
    Number,
    #[regex(r#""([^"\\]|\\.)*""#)]
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
            Eof => "end of input",
        };
        f.write_str(name)
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
        let kind = result.map_err(|_| Diagnostic::error(span, "unexpected character"))?;
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
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(TokenKind, String)> {
        tokenize(src)
            .expect("lexing succeeds")
            .into_iter()
            .filter(|t| t.kind != TokenKind::Eof)
            .map(|t| (t.kind, t.text))
            .collect()
    }

    #[test]
    fn range_between_integers_is_not_a_float() {
        assert_eq!(
            kinds("0..10"),
            vec![
                (TokenKind::Number, "0".to_string()),
                (TokenKind::DotDot, "..".to_string()),
                (TokenKind::Number, "10".to_string()),
            ]
        );
    }

    #[test]
    fn float_literal_keeps_its_dot() {
        assert_eq!(kinds("3.14"), vec![(TokenKind::Number, "3.14".to_string())]);
    }

    #[test]
    fn tuple_field_access_on_number_splits_the_dot() {
        assert_eq!(
            kinds("3.foo"),
            vec![
                (TokenKind::Number, "3".to_string()),
                (TokenKind::Dot, ".".to_string()),
                (TokenKind::Identifier, "foo".to_string()),
            ]
        );
    }

    #[test]
    fn inclusive_range_fuses_regardless_of_spacing() {
        // Digit-adjacent, where the number first swallows a dot, and cleanly spaced
        // both recover a single `..=`.
        let expected = |lo: &str, hi: &str| {
            vec![
                (TokenKind::Number, lo.to_string()),
                (TokenKind::DotDotEquals, "..=".to_string()),
                (TokenKind::Number, hi.to_string()),
            ]
        };
        assert_eq!(kinds("0..=10"), expected("0", "10"));
        assert_eq!(kinds("0 ..= 10"), expected("0", "10"));
    }
}
