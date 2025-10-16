package typechecker

import (
	"fmt"
	"github.com/ruistola/cooper/lexer"
	"github.com/ruistola/cooper/parser"
	"github.com/yassinebenaid/godump"
	"testing"
)

func Test(t *testing.T) {
	src := "x := 2 + 2"
	parsedAst := parser.Parse(lexer.Tokenize(src))
	errors := Check(parsedAst)
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
	if len(errors) == 0 {
		fmt.Println("Type checker: 0 errors.")
	} else {
		fmt.Println("Type checker errors:")
		for _, err := range errors {
			fmt.Println(err)
		}
	}
}
