package parser

import (
	"github.com/ruistola/cooper/lexer"
	"github.com/yassinebenaid/godump"
	"testing"
)

func TestBasicExpression(t *testing.T) {
	src := "69 + 420"
	ast := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(ast)
	}
}

func TestIfExpression(t *testing.T) {
	src := "if x < 5 then { 0 } else { 5 }"
	ast := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(ast)
	}
}

func TestIfExpressionNewline(t *testing.T) {
	src := "if x < 5 then 0\n else 5"
	ast := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(ast)
	}
}

func TestIfExpressionStatements(t *testing.T) {
	src := "if x < 5 then foo(); else bar();"
	ast := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(ast)
	}
}

func TestIfExpressionStatementsNewline(t *testing.T) {
	src := "if x < 5 then foo();\nelse bar();"
	ast := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(ast)
	}
}

func TestOneLineIfExpression(t *testing.T) {
	src := "if x < 5 then foo() else if x > 10 then 100 else 10"
	ast := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(ast)
	}
}

func TestFuncArrayType(t *testing.T) {
	testCases := []struct {
		name string
		src  string
	}{
		{
			"function taking array of functions",
			"let takesArrayOfCallbacks: func( (func(i32):bool)[] )",
		},
		{
			"nested arrays with functions",
			"let matrix: ((func():i32)[])[]",
		},
		{
			"function returning array of functions",
			"let factory: func():(func():bool)[]",
		},
		{
			"array of functions returning arrays",
			"let callbacks: (func():i32[])[]",
		},
		{
			"simple parenthesized type",
			"let x: (i32)",
		},
		{
			"deeply nested parentheses",
			"let y: (((bool)))",
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			ast := Parse(lexer.Tokenize(tc.src))
			if testing.Verbose() {
				t.Logf("\nParsing: %s", tc.src)
				godump.Dump(ast)
			}
		})
	}
}
