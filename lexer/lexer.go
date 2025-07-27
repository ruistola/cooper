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
	THEN
	ELSE
	FOR
	RETURN

	// Sentinel value
	NUM_TOKENS
)

// Golang has no built-in ordered map type, so we store an array of (token type, regex pattern) pairs.
// The order in which they are stored (or rather, iterated, during tokenization) matters,
// e.g. the lexer must try `!=` pattern before `!` to correctly identify a NOT_EQUALS token.
type tokenPattern struct {
	tokenType TokenType
	pattern   *regexp.Regexp
}

// Sanity check: All regexes should start with `^` so that we only match a token
// at the beginning of the remaining unprocessed input text. Otherwise we might be
// skipping some sections of the input text entirely.
var tokenPatterns []tokenPattern = []tokenPattern{
	// Literals, comments, and special tokens
	{EOL, regexp.MustCompile(`^\r?\n|\r`)},
	{WHITESPACE, regexp.MustCompile(`^\s+`)},
	{WORD, regexp.MustCompile(`^[a-zA-Z_][a-zA-Z0-9_]*`)},
	{COMMENT, regexp.MustCompile(`^\/\/.*`)},
	{NUMBER, regexp.MustCompile(`^(0[xX][0-9a-fA-F](_?[0-9a-fA-F])*|0[bB][01](_?[01])*|[0-9](_?[0-9])*(\.([0-9](_?[0-9])*)?)?([eE][+-]?[0-9](_?[0-9])*)?)`)},
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

// A lookup table for WORD type patterns. If a key matching the pattern is found,
// then we read the token type from the value in this map. Otherwise, we store
// the token as an IDENTIFIER.
var reservedKeywords map[string]TokenType = map[string]TokenType{
	"let":    LET,
	"struct": STRUCT,
	"true":   TRUE,
	"false":  FALSE,
	"func":   FUNC,
	"if":     IF,
	"or":     OR,
	"and":    AND,
	"then":   THEN,
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
	IDENTIFIER: "identifier",

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
	THEN:   "then",
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

// Token stores a type identifier with the corresponding raw string section of the source.
// For example: {NOT_EQUALS, "!="}, {NUMBER, "3.141"}, {KEYWORD, "return"}
type Token struct {
	Type  TokenType
	Value string
}

// tryMatchPattern tests a regex against the input text and if there is a match, produces a new Token
// of the type specified by the `tokenType` argument (or a refined type, if tokenType is WORD).
func tryMatchPattern(src string, re *regexp.Regexp, tokenType TokenType) (int, Token) {
	// Try to find a range in the input text where the reges matches
	matchRange := re.FindStringIndex(src)
	// No match, return empty Token
	if matchRange == nil {
		return 0, Token{}
	}

	// Sanity check: any match range must always start at 0.
	// Otherwise we would be skipping some sections of the input text entirely.
	if matchRange[0] > 0 {
		panic(fmt.Sprintf("Internal error: regex matched at non-zero index %d!", matchRange[0]))
	}

	// Because the beginning of the match range is now guaranteed to be 0,
	// length is just the end of the range.
	length := matchRange[1]
	match := src[:length]

	// If we're not matching against a WORD token, simply return the type provided as an argument.
	// If we are, check the matched string to see if it is one of the reserved keywords, or an IDENTIFIER.
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

// Tokenize converts a raw text source into a slice of tokens that can then be fed as input for the parser.
func Tokenize(src string) []Token {
	pos := 0
	tokens := make([]Token, 0)
	// While there is unprocessed source left...
	for pos < len(src) {
		// Keep track of whether we did end up finding a match
		found := false
		// Get the remaining part as a slice
		remainingSrc := src[pos:]
		// Find which TokenType represents the next token in the input
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
