package codegen

import (
	"testing"

	"github.com/ruistola/cooper/lexer"
	"github.com/ruistola/cooper/parser"
	"github.com/ruistola/cooper/typechecker"
)

func TestCodeGen(t *testing.T) {
	src := "func main(): i32 { return 69 }"

	tokens := lexer.Tokenize(src)
	module := parser.Parse(tokens)
	errors := typechecker.Check(module)

	if len(errors) > 0 {
		for _, err := range errors {
			t.Log(err)
		}
		t.Fatal("typechecking failed")
	}

	asm := GenerateProgram(module)
	if asm == "" {
		t.Fatal("GenerateProgram failed")
	} else {
		t.Logf("Generated assembly:\n%s", asm)
	}

	err := Compile(asm, "./", "./test")
	if err != nil {
		t.Error("compile failed:", err)
	}
}
