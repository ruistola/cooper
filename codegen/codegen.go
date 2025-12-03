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

// func (g *Generator) Emit(format string, args ...any)
// func (g *Generator) String() string

func GenerateProgram(module ast.BlockStmt) string {
	return `.global _main
.align 4

_main:
    mov w0, #42
    ret`
}

func Compile(assembly string, workingDir string, outputPath string) error {
	if workingDir == "" {
		workingDir = "./"
	}

	// Create a temporary `build/` directory
	cmd := exec.Command("mkdir", "-p", fmt.Sprintf("%s/build", workingDir))
	out, err := cmd.Output()
	if err != nil {
		return fmt.Errorf("failed to create a temporary build directory: %w", err)
	}
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

	// Clean up the build directory
	cmd = exec.Command("rm", "-rf", fmt.Sprintf("%s/build", workingDir))
	out, err = cmd.Output()
	if err != nil {
		return fmt.Errorf("failed to clean up temporary build directory: %w", err)
	}
	fmt.Printf("Cleaned up: %s", out)

	return nil
}
