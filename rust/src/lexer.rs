use logos::Logos;

use crate::diag::Span;

/// The lexical token kinds. `logos` compiles these annotations into a single fast
/// state machine, replacing the hand-rolled ordered-regex loop of the Go lexer.
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
    #[regex(r"0[xX][0-9a-fA-F](_?[0-9a-fA-F])*|0[bB][01](_?[01])*|[0-9](_?[0-9])*(\.([0-9](_?[0-9])*)?)?([eE][+-]?[0-9](_?[0-9])*)?")]
    Number,
    #[regex(r#""([^"\\]|\\.)*""#)]
    String,
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Identifier,

    // Multi-character operators
    #[token(":=")]
    ColonEquals,
    #[token("=>")]
    FatArrow,
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
    #[token("let")]
    Let,
    #[token("nil")]
    Nil,
    #[token("or")]
    Or,
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
    #[token("use")]
    Use,
    #[token("match")]
    Match,
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
            String => "string",
            Identifier => "identifier",
            ColonEquals => "':='",
            FatArrow => "'=>'",
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
            Else => "'else'",
            False => "'false'",
            For => "'for'",
            Func => "'func'",
            If => "'if'",
            Let => "'let'",
            Nil => "'nil'",
            Or => "'or'",
            Return => "'return'",
            Struct => "'struct'",
            Oneof => "'oneof'",
            Then => "'then'",
            True => "'true'",
            Use => "'use'",
            Match => "'match'",
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
/// character yields a diagnostic-bearing `Err` carrying its span.
pub fn tokenize(src: &str) -> Result<Vec<Token>, Span> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut lex = TokenKind::lexer(src);
    while let Some(result) = lex.next() {
        let span: Span = lex.span().into();
        let kind = result.map_err(|_| span)?;
        // Collapse consecutive end-of-line tokens into a single one.
        if kind == TokenKind::Eol {
            if matches!(tokens.last(), Some(t) if t.kind == TokenKind::Eol) {
                continue;
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
