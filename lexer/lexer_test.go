package lexer

import (
	"testing"
)

// Ensure that we handle all token type values in some way, and that we have a string representation for each
func TestCompleteness(t *testing.T) {
	numTokens := int(NUM_TOKENS)

	// Add 2 for EOF and IDENTIFIER which are not present in neither patterns nor keywords
	if numPatternsKeywords := len(tokenPatterns) + len(reservedKeywords) + 2; numPatternsKeywords != numTokens {
		t.Fatalf("Lexer error! Expected %d token patterns and/or keywords, found %d", numTokens, numPatternsKeywords)
	}

	if numNames := len(tokenDisplayNames); numNames != numTokens {
		t.Fatalf("Lexer error! Expected %d token display names, found %d", numTokens, numNames)
	}
}

// Test the actual tokenization
func TestTokenize(t *testing.T) {
	test := func(src string, expected ...TokenType) {
		tokens := Tokenize(src)
		numTokens := len(tokens)
		if numTokens != len(expected) {
			t.Fatalf("Tokenization failed! Expected %d tokens, found %d", len(expected), numTokens)
		}

		for i := range numTokens {
			if tokens[i].Type != expected[i] {
				t.Fatalf("Tokenization mismatch! Expected %s, found %s", expected[i], tokens[i].Type)
			}
		}
	}

	test("&", AMPERSAND)
	test(":=", COLON_EQUALS)
	test("for i := 0; i <= 100; i += 1;",
		FOR, IDENTIFIER, COLON_EQUALS, NUMBER, SEMICOLON,
		IDENTIFIER, LESS_EQUALS, NUMBER, SEMICOLON,
		IDENTIFIER, PLUS_EQUALS, NUMBER, SEMICOLON,
	)
}
