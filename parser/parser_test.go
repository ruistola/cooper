package parser

import (
	"cooper/lexer"
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
	src := "if x < 5 then { 0 } else if x > 10 then 100 else 10"
	ast := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(ast)
	}
}
