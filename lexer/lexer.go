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

	// Multicharacter tokens
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

	// Single- character tokens
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
	SEMICOLON     // ;
	COLON         // :
	COMMA         // ,
	OPEN_BRACKET  // [
	CLOSE_BRACKET // ]
	OPEN_CURLY    // {
	CLOSE_CURLY   // }
	OPEN_PAREN    // (
	CLOSE_PAREN   // )

	// Reserved keywords
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

	// Sentinel value
	NUM_TOKENS
)

type tokenPattern struct {
	tokenType TokenType
	pattern   *regexp.Regexp
}

// NOTE: Order matters! (e.g. need to try `!=` pattern before `!` to correctly identify the token NOT_EQUALS)
var tokenPatterns []tokenPattern = []tokenPattern{
	// Literals, comments, and special tokens
	{EOL, regexp.MustCompile(`^\r?\n|\r`)},
	{WHITESPACE, regexp.MustCompile(`^\s+`)},
	{WORD, regexp.MustCompile(`^[a-zA-Z_][a-zA-Z0-9_]*`)},
	{COMMENT, regexp.MustCompile(`^\/\/.*`)},
	{NUMBER, regexp.MustCompile(`^[0-9]+(\.[0-9]+)?`)},
	{STRING, regexp.MustCompile(`^"[^"]*"`)},

	// Multicharacter tokens
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

	// Single- character tokens
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
	{SEMICOLON, regexp.MustCompile(`^;`)},
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

// A lookup table for the Stringer interface implementation
var tokenDisplayNames map[TokenType]string = map[TokenType]string{
	// Literals, comments, and special tokens
	EOF:        "eof",
	EOL:        "eol",
	WHITESPACE: "whitespace",
	WORD:       "word",
	COMMENT:    "comment",
	NUMBER:     "number",
	STRING:     "string",

	// Multicharacter tokens
	COLON_EQUALS:   "colon_equals",
	DOUBLE_EQUALS:  "double_equals",
	NOT_EQUALS:     "not_equals",
	LESS_EQUALS:    "less_equals",
	GREATER_EQUALS: "greater_equals",
	PLUS_EQUALS:    "plus_equals",
	DASH_EQUALS:    "dash_equals",
	STAR_EQUALS:    "star_equals",
	SLASH_EQUALS:   "slash_equals",
	PERCENT_EQUALS: "percent_equals",

	// Single- character tokens
	EQUALS:        "equals",
	NOT:           "not",
	PIPE:          "pipe",
	AMPERSAND:     "ampersand",
	CHEVRON:       "chevron",
	LESS:          "less",
	GREATER:       "greater",
	PLUS:          "plus",
	DASH:          "dash",
	UNDERSCORE:    "underscore",
	SLASH:         "slash",
	STAR:          "star",
	PERCENT:       "percent",
	DOT:           "dot",
	SEMICOLON:     "semicolon",
	COLON:         "colon",
	COMMA:         "comma",
	OPEN_BRACKET:  "open_bracket",
	CLOSE_BRACKET: "close_bracket",
	OPEN_CURLY:    "open_curly",
	CLOSE_CURLY:   "close_curly",
	OPEN_PAREN:    "open_paren",
	CLOSE_PAREN:   "close_paren",

	// Reserved keywords
	LET:    "let",
	STRUCT: "struct",
	TRUE:   "true",
	FALSE:  "false",
	FUNC:   "func",
	IF:     "if",
	OR:     "or",
	AND:    "and",
	ELSE:   "else",
	FOR:    "for",
	RETURN: "return",
}

// Implement Stringer for TokenType.
func (tokenType TokenType) String() string {
	if str, found := tokenDisplayNames[tokenType]; found {
		return str
	}
	return fmt.Sprintf("unknown (%d)", tokenType)
}

type Token struct {
	Type  TokenType
	Value string
}

func tryMatchPattern(src string, re *regexp.Regexp, tokenType TokenType) (int, Token) {
	// Try to match the regex
	matchRange := re.FindStringIndex(src)
	// No match, return empty Token
	if matchRange == nil {
		return 0, Token{}
	}

	// Sanity check: All regexes should start with `^`, because we always want to match
	// against the beginning of the remaining (unprocessed) source text.
	if matchRange[0] > 0 {
		panic(fmt.Sprintf("Internal error: regex matched at non-zero index %d!", matchRange[0]))
	}

	// Because the beginning of the match range must be 0, length is just the end of the range
	length := matchRange[1]
	match := src[:length]

	// If we're not matching against a WORD token, return the type as is.
	// If we are, check the result string in detail to see if it is a reserved keyword.
	// If it's not, then it must be an identifier.
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
	// While there is unprocessed source left...
	for pos < len(src) {
		// Keep track of whether we have found a matching token for the input beginning at the current position
		found := false
		// Get the remaining part as a slice
		remainingSrc := src[pos:]
		// For each pattern, try to match them in order.
		for _, tp := range tokenPatterns {
			// If some pattern did match, add the token to the collection and update the position
			if length, newToken := tryMatchPattern(remainingSrc, tp.pattern, tp.tokenType); length != 0 {
				if newToken.Type != WHITESPACE {
					tokens = append(tokens, newToken)
				}
				found = true
				pos += length
				break
			}
		}
		if !found {
			panic(fmt.Sprintf("Internal error: failed to tokenize source at %d", pos))
		}
	}
	return tokens
}

// During package initialization, sanity check that we have a string representation for all tokens
func init() {
	if len(tokenDisplayNames) != int(NUM_TOKENS) {
		panic(fmt.Sprintf("Internal error: Expected %d token display names, found %d", len(tokenDisplayNames), NUM_TOKENS))
	}
}
