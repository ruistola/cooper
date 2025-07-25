package parser

import (
	"cooper/lexer"
	"github.com/yassinebenaid/godump"
	"testing"
)

func TestBasicExpression(t *testing.T) {
	src := "69 + 420"
	ast := Parse(lexer.Tokenize(src))
	godump.Dump(ast)
}
