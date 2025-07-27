package lexer

import (
	"flag"
	"fmt"
	"github.com/yassinebenaid/godump"
	"os"
	"strings"
	"testing"
)

var (
	showTokens     = flag.Bool("show-tokens", false, "Show detailed token dumps")
	showTokenTypes = flag.Bool("show-tokens-short", false, "Show just token types (less verbose than full dumps)")
)

// Initialize custom flags
func TestMain(m *testing.M) {
	flag.Parse()
	os.Exit(m.Run())
}

// Test helper for colorized output
type testLogger struct {
	t *testing.T
}

func (tl testLogger) Success(format string, args ...any) {
	tl.t.Helper()
	tl.t.Logf("\033[32m✓ "+format+"\033[0m", args...)
}

func (tl testLogger) Info(format string, args ...any) {
	tl.t.Helper()
	tl.t.Logf("\033[94m"+format+"\033[0m", args...)
}

func (tl testLogger) Error(format string, args ...any) {
	tl.t.Helper()
	tl.t.Errorf("\033[31m✗ "+format+"\033[0m", args...)
}

func (tl testLogger) Dump(tokens []Token) {
	if *showTokens {
		godump.Dump(tokens)
	} else if *showTokenTypes {
		types := make([]string, len(tokens))
		for i, tok := range tokens {
			types[i] = tok.Type.String()
		}
		tl.Info("Token types: [%s]", strings.Join(types, ", "))
	}
}

// Main test helper
func testTokenization(t *testing.T, src string, expected ...TokenType) {
	t.Helper()
	log := testLogger{t}

	log.Info("Input: %q", src)
	tokens := Tokenize(src)

	// Check token count
	if len(tokens) != len(expected) {
		log.Error("Expected %d tokens, got %d", len(expected), len(tokens))
		log.Dump(tokens)
		t.FailNow()
		return
	}

	// Check token types
	for i := range len(tokens) {
		if tokens[i].Type != expected[i] {
			log.Error("Token %d: expected %s, got %s", i, expected[i], tokens[i].Type)
			log.Dump(tokens)
			t.FailNow()
			return
		}
	}

	log.Success("OK - %d tokens", len(tokens))
	log.Dump(tokens)
}

// Test helper that expects tokenization to panic
func testTokenizationPanic(t *testing.T, src string, expectedPanic string) {
	t.Helper()
	log := testLogger{t}

	log.Info("Input (expecting panic): %q", src)

	defer func() {
		if r := recover(); r != nil {
			if expectedPanic != "" {
				// Check if panic message contains expected string
				panicStr := fmt.Sprintf("%v", r)
				if !strings.Contains(panicStr, expectedPanic) {
					log.Error("Expected panic containing %q, got %q", expectedPanic, panicStr)
					t.FailNow()
				}
			}
			log.Success("OK - panicked as expected")
		} else {
			log.Error("Expected panic but tokenization succeeded")
			t.FailNow()
		}
	}()

	Tokenize(src)
}

// ----------
// Test cases
// ----------

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

// Test basic tokens
func TestBasicTokens(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected []TokenType
	}{
		{"single ampersand", "&", []TokenType{AMPERSAND}},
		{"assignment operator", ":=", []TokenType{COLON_EQUALS}},
		{"identifier", "foo", []TokenType{IDENTIFIER}},
		{"keywords", "if else for", []TokenType{IF, ELSE, FOR}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			testTokenization(t, tt.input, tt.expected...)
		})
	}
}

// Test integer numbers
func TestIntegerNumbers(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected []TokenType
	}{
		{"simple integer", "42", []TokenType{NUMBER}},
		{"integer with underscores", "1_000_000", []TokenType{NUMBER}},
		{"hex number", "0xFF", []TokenType{NUMBER}},
		{"hex with underscores", "0xDEAD_BEEF", []TokenType{NUMBER}},
		{"binary number", "0b1010", []TokenType{NUMBER}},
		{"binary with underscores", "0b1111_0000", []TokenType{NUMBER}},
		{"negative integer", "-42", []TokenType{DASH, NUMBER}},
		{"negative hex", "-0xFF", []TokenType{DASH, NUMBER}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			testTokenization(t, tt.input, tt.expected...)
		})
	}
}

// Test floating point numbers
func TestFloatingPointNumbers(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected []TokenType
	}{
		{"simple float", "3.14", []TokenType{NUMBER}},
		{"float with trailing dot", "1.", []TokenType{NUMBER}},
		{"leading dot is separate", ".5", []TokenType{DOT, NUMBER}},
		{"multiple dots", "1.2.3", []TokenType{NUMBER, DOT, NUMBER}},
		{"negative float", "-3.14", []TokenType{DASH, NUMBER}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			testTokenization(t, tt.input, tt.expected...)
		})
	}
}

// Test scientific notation
func TestScientificNotation(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected []TokenType
	}{
		{"simple e notation", "1e10", []TokenType{NUMBER}},
		{"capital E", "1E10", []TokenType{NUMBER}},
		{"float with exponent", "3.14e10", []TokenType{NUMBER}},
		{"positive exponent", "1.5E+10", []TokenType{NUMBER}},
		{"negative exponent", "1e-10", []TokenType{NUMBER}},
		{"underscore before e", "1_000e10", []TokenType{NUMBER}},
		{"negative with exponent", "-5e-5", []TokenType{DASH, NUMBER}},

		// Edge cases that tokenize differently
		{"incomplete exponent", "1e", []TokenType{NUMBER, IDENTIFIER}},
		{"exponent without digits", "1e+", []TokenType{NUMBER, IDENTIFIER, PLUS}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			testTokenization(t, tt.input, tt.expected...)
		})
	}
}

// Test numbers in expressions
func TestNumbersInExpressions(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected []TokenType
	}{
		{
			"addition with scientific",
			"x+5e-5",
			[]TokenType{IDENTIFIER, PLUS, NUMBER},
		},
		{
			"multiplication with negative",
			"y*-3.14",
			[]TokenType{IDENTIFIER, STAR, DASH, NUMBER},
		},
		{
			"double negative",
			"5--5",
			[]TokenType{NUMBER, DASH, DASH, NUMBER},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			testTokenization(t, tt.input, tt.expected...)
		})
	}
}

// Test complete statements
func TestCompleteStatements(t *testing.T) {
	t.Run("for loop", func(t *testing.T) {
		testTokenization(t,
			"for i := 0; i <= 100; i += 1;",
			FOR, IDENTIFIER, COLON_EQUALS, NUMBER, SEMICOLON,
			IDENTIFIER, LESS_EQUALS, NUMBER, SEMICOLON,
			IDENTIFIER, PLUS_EQUALS, NUMBER, SEMICOLON,
		)
	})

	t.Run("if statement with EOL", func(t *testing.T) {
		testTokenization(t,
			`if 1 > 0 {
        foo()
    }`,
			IF, NUMBER, GREATER, NUMBER, OPEN_CURLY, EOL,
			IDENTIFIER, OPEN_PAREN, CLOSE_PAREN, EOL,
			CLOSE_CURLY,
		)
	})
}

// Test malformed numbers that should fail
func TestMalformedNumbers(t *testing.T) {
	if !testing.Short() {
		t.Skip("Skipping malformed number tests - not yet implemented")
	}

	tests := []struct {
		name          string
		input         string
		expectedPanic string
	}{
		{"consecutive underscores", "1__000", "invalid number"},
		{"trailing underscore", "1_", "invalid number"},
		{"leading underscore", "_100", "invalid number"},
		{"hex without digits", "0x", "invalid number"},
		{"binary without digits", "0b", "invalid number"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			testTokenizationPanic(t, tt.input, tt.expectedPanic)
		})
	}
}

// Benchmark tokenization performance
func BenchmarkTokenizeNumbers(b *testing.B) {
	inputs := []string{
		"42",
		"3.14159",
		"1_000_000",
		"0xDEADBEEF",
		"1.23e-45",
	}

	b.ResetTimer()
	for b.Loop() {
		for _, input := range inputs {
			Tokenize(input)
		}
	}
}
