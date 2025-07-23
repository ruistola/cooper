package lexer

import (
	"testing"
)

func TestTokenize(t *testing.T) {
	tokens := Tokenize(":=")
	if len(tokens) < 1 || tokens[0].Type != COLON_EQUALS {
		t.Fatalf("Tokenization failed: %s", tokens)
	}
}
