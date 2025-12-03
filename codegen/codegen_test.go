package codegen

import (
	"testing"

	"github.com/ruistola/cooper/ast"
)

func TestCodeGen(t *testing.T) {
	asm := GenerateProgram(ast.BlockStmt{Statements: nil})
	if asm == "" {
		t.Fatal("GenerateProgram failed")
	}

	err := Compile(asm, "./", "./test")
	if err != nil {
		t.Error("compile failed:", err)
	}
}
