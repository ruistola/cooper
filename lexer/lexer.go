package lexer

import (
	"fmt"
	"regexp"
)

type TokenType int

const (
	// Literals, comments, and special tokens
	EOF        TokenType = iota // End of File
	EOL                         // End of Line
	WHITESPACE                  // UTF-8 whitespace (tabs, spaces, etc.)
	WORD                        // Evaluates into a keyword or an identifier
	COMMENT                     // Double slash until EOL is a comment
	NUMBER                      // Number literal, e.g. 123, -5e5, 3.141, 0xFF, 0b10101010
	STRING                      // Double quote delimited string literal, e.g. "Hello, World!"
	IDENTIFIER                  // If a word is not a reserved keyword, then it must be an identifier

	// Multicharacter symbols
	COLON_EQUALS   // :=
	DOUBLE_EQUALS  // ==
	NOT_EQUALS     // !=
	LESS_EQUALS    // <=
	GREATER_EQUALS // >=
	PLUS_EQUALS    // +=
	DASH_EQUALS    // -=
	STAR_EQUALS    // *=
	SLASH_EQUALS   // /=
	PERCENT_EQUALS // %=

	// Single- character symbols
	EQUALS        // =
	NOT           // !
	PIPE          // |
	AMPERSAND     // &
	CHEVRON       // ^
	LESS          // <
	GREATER       // >
	PLUS          // +
	DASH          // -
	UNDERSCORE    // _
	SLASH         // /
	STAR          // *
	PERCENT       // %
	DOT           // .
	SEMI_COLON    // ;
	COLON         // :
	COMMA         // ,
	OPEN_BRACKET  // [
	CLOSE_BRACKET // ]
	OPEN_CURLY    // {
	CLOSE_CURLY   // }
	OPEN_PAREN    // (
	CLOSE_PAREN   // )

	// Keywords
	LET
	STRUCT
	TRUE
	FALSE
	FUNC
	IF
	OR
	AND
	ELSE
	FOR
	RETURN
)

type tokenPattern struct {
	tokenType TokenType
	pattern   *regexp.Regexp
}

var tokenPatterns []tokenPattern = []tokenPattern{
	{EOL, regexp.MustCompile(`^\r?\n|\r`)},
	{WHITESPACE, regexp.MustCompile(`^\s+`)},
	{WORD, regexp.MustCompile(`^[a-zA-Z_][a-zA-Z0-9_]*`)},
	{COMMENT, regexp.MustCompile(`^\/\/.*`)},
	{NUMBER, regexp.MustCompile(`^[0-9]+(\.[0-9]+)?`)},
	{STRING, regexp.MustCompile(`^"[^"]*"`)},

	{COLON_EQUALS, regexp.MustCompile(`^:=`)},
	{DOUBLE_EQUALS, regexp.MustCompile(`^==`)},
	{NOT_EQUALS, regexp.MustCompile(`^!=`)},
	{LESS_EQUALS, regexp.MustCompile(`^<=`)},
	{GREATER_EQUALS, regexp.MustCompile(`^>=`)},
	{PLUS_EQUALS, regexp.MustCompile(`^\+=`)},
	{DASH_EQUALS, regexp.MustCompile(`^-=`)},
	{STAR_EQUALS, regexp.MustCompile(`^\*=`)},
	{SLASH_EQUALS, regexp.MustCompile(`^/=`)},
	{PERCENT_EQUALS, regexp.MustCompile(`^%=`)},

	{EQUALS, regexp.MustCompile(`^=`)},
	{NOT, regexp.MustCompile(`^!`)},
	{PIPE, regexp.MustCompile(`^\|`)},
	{AMPERSAND, regexp.MustCompile(`^&`)},
	{CHEVRON, regexp.MustCompile(`^\^`)},
	{LESS, regexp.MustCompile(`^<`)},
	{GREATER, regexp.MustCompile(`^>`)},
	{PLUS, regexp.MustCompile(`^\+`)},
	{DASH, regexp.MustCompile(`^-`)},
	{UNDERSCORE, regexp.MustCompile(`^_`)},
	{SLASH, regexp.MustCompile(`^/`)},
	{STAR, regexp.MustCompile(`^\*`)},
	{PERCENT, regexp.MustCompile(`^%`)},
	{DOT, regexp.MustCompile(`^\.`)},
	{SEMI_COLON, regexp.MustCompile(`^;`)},
	{COLON, regexp.MustCompile(`^:`)},
	{COMMA, regexp.MustCompile(`^,`)},
	{OPEN_BRACKET, regexp.MustCompile(`^\[`)},
	{CLOSE_BRACKET, regexp.MustCompile(`^\]`)},
	{OPEN_CURLY, regexp.MustCompile(`^\{`)},
	{CLOSE_CURLY, regexp.MustCompile(`^\}`)},
	{OPEN_PAREN, regexp.MustCompile(`^\(`)},
	{CLOSE_PAREN, regexp.MustCompile(`^\)`)},
}

var reservedKeywords map[string]TokenType = map[string]TokenType{
	"let":    LET,
	"struct": STRUCT,
	"true":   TRUE,
	"false":  FALSE,
	"func":   FUNC,
	"if":     IF,
	"or":     OR,
	"and":    AND,
	"else":   ELSE,
	"for":    FOR,
	"return": RETURN,
}

func (tokenType TokenType) String() string {
	switch tokenType {
	case EOF:
		return "eof"
	case WHITESPACE:
		return "whitespace"
	case WORD:
		return "word"
	case COMMENT:
		return "comment"
	case NUMBER:
		return "number"
	case STRING:
		return "string"
	case TRUE:
		return "true"
	case FALSE:
		return "false"
	case IDENTIFIER:
		return "identifier"
	case OPEN_BRACKET:
		return "open_bracket"
	case CLOSE_BRACKET:
		return "close_bracket"
	case OPEN_CURLY:
		return "open_curly"
	case CLOSE_CURLY:
		return "close_curly"
	case OPEN_PAREN:
		return "open_paren"
	case CLOSE_PAREN:
		return "close_paren"
	case EQUALS:
		return "assignment"
	case DOUBLE_EQUALS:
		return "equals"
	case NOT_EQUALS:
		return "not_equals"
	case NOT:
		return "not"
	case LESS:
		return "less"
	case LESS_EQUALS:
		return "less_equals"
	case GREATER:
		return "greater"
	case GREATER_EQUALS:
		return "greater_equals"
	case DOT:
		return "dot"
	case SEMI_COLON:
		return "semi_colon"
	case COLON:
		return "colon"
	case COMMA:
		return "comma"
	case PLUS_EQUALS:
		return "plus_equals"
	case DASH_EQUALS:
		return "minus_equals"
	case PLUS:
		return "plus"
	case DASH:
		return "dash"
	case SLASH:
		return "slash"
	case STAR:
		return "star"
	case PERCENT:
		return "percent"
	case LET:
		return "let"
	case FUNC:
		return "func"
	case IF:
		return "if"
	case OR:
		return "or"
	case AND:
		return "and"
	case ELSE:
		return "else"
	case FOR:
		return "for"
	case STRUCT:
		return "struct"
	case RETURN:
		return "return"
	default:
		return fmt.Sprintf("unknown(%d)", tokenType)
	}
}

type Token struct {
	Type  TokenType
	Value string
}

func tryMatchPattern(src string, re *regexp.Regexp, tokenType TokenType) (int, Token) {
	matchRange := re.FindStringIndex(src)
	if matchRange == nil {
		return 0, Token{}
	}

	if matchRange[0] > 0 {
		panic(fmt.Sprintf("Internal error: regex matched at non-zero index %d!", matchRange[0]))
	}

	match := src[:matchRange[1]]
	length := matchRange[1]

	if tokenType != WORD {
		return length, Token{
			Type:  tokenType,
			Value: match,
		}
	} else if keywordTokenType, found := reservedKeywords[match]; found {
		return length, Token{
			Type:  keywordTokenType,
			Value: match,
		}
	} else {
		return length, Token{
			Type:  IDENTIFIER,
			Value: match,
		}
	}
}

func Tokenize(src string) []Token {
	pos := 0
	tokens := make([]Token, 0)

	for pos < len(src) {
		remainingSrc := src[pos:]
		for _, tp := range tokenPatterns {
			if length, newToken := tryMatchPattern(remainingSrc, tp.pattern, tp.tokenType); length != 0 {
				if newToken.Type != WHITESPACE && newToken.Type != COMMENT {
					tokens = append(tokens, newToken)
				}
				pos += length
				break
			}
		}
	}

	return tokens
}
