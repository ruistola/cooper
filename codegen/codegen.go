package codegen

import (
	"fmt"
	"github.com/ruistola/cooper/ast"
	"os"
	"os/exec"
	"strings"
)

type Generator struct {
	buf strings.Builder
}

func (g *Generator) emit(format string, args ...any) {
	fmt.Fprintf(&g.buf, format, args...)
	g.buf.WriteString("\n")
}

func (g *Generator) String() string {
	return g.buf.String()
}

func (g *Generator) generateFunction(fn *ast.FuncDeclStmt) {
	// macOS requires underscore prefix for symbols
	g.emit(".global _%s", fn.Name)
	g.emit(".align 4")
	g.emit("")
	g.emit("_%s:", fn.Name)

	// TODO: Prologue

	// Generate function body
	for _, stmt := range fn.Body.Statements {
		g.generateStmt(stmt)
	}

	// TODO: Epilogue

	g.emit("ret")
}

func (g *Generator) generateStmt(stmt ast.Stmt) {
	switch s := stmt.(type) {
	case *ast.ReturnStmt:
		// Evaluate the expression, result in w0
		g.generateExpr(s.Expr)
		// ret is emitted by generateFunction epilogue
	default:
		panic(fmt.Sprintf("unhandled statement type: %T", stmt))
	}
}

func (g *Generator) generateExpr(expr ast.Expr) string {
	switch e := expr.(type) {
	case *ast.NumberLiteralExpr:
		g.emit("  mov w0, #%s", e.Value)
		return "w0"
	default:
		panic(fmt.Sprintf("unhandled expression type: %T", expr))
	}
}

func GenerateModuleAsm(module *ast.BlockStmt) string {
	g := &Generator{}

	for _, stmt := range module.Statements {
		switch s := stmt.(type) {
		case *ast.FuncDeclStmt:
			g.generateFunction(s)
		default:
			// TODO: other top-level statements
		}
	}

	return g.String()
}

func CompileAsm(assembly string, workingDir string, outputPath string) (err error) {
	if workingDir == "" {
		workingDir = "./"
	}

	// Create a temporary `build/` directory (defer cleanup)
	cmd := exec.Command("mkdir", "-p", fmt.Sprintf("%s/build", workingDir))
	out, err := cmd.Output()
	if err != nil {
		return fmt.Errorf("failed to create a temporary build directory: %w", err)
	}
	defer func() {
		cmd = exec.Command("rm", "-rf", fmt.Sprintf("%s/build", workingDir))
		out, err = cmd.Output()
		if err != nil {
			err = fmt.Errorf("failed to clean up temporary build directory: %s", err)
		}
		fmt.Printf("Cleaned up: %s", out)
	}()
	fmt.Printf("Created temporary build directory: %s", out)

	// Write the assembly into a file (no unique name needed when it goes into an empty directory just created
	os.WriteFile(fmt.Sprintf("%s/build/generated.s", workingDir), []byte(assembly), 0644)

	// Generate the object file from the assembly
	cmd = exec.Command("as", "-o", fmt.Sprintf("%s/build/generated.o", workingDir), fmt.Sprintf("%s/build/generated.s", workingDir))
	out, err = cmd.Output()
	if err != nil {
		return fmt.Errorf("failed to generate the object file: %w", err)
	}
	fmt.Printf("Created object file: %s", out)

	if outputPath == "" {
		outputPath = "./main"
	}

	// Link the executable (just using shell expansion here for this PoC impl)
	shellCmd := fmt.Sprintf("ld -o %s %s/build/generated.o -lSystem -syslibroot `xcrun --show-sdk-path` -e _main -arch arm64",
		outputPath, workingDir)
	cmd = exec.Command("sh", "-c", shellCmd)
	out, err = cmd.Output()
	if err != nil {
		return fmt.Errorf("linker error: %w", err)
	}
	fmt.Printf("Compiled: %s", out)

	return nil
}
