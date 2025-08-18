package main

import (
	"fmt"
	"github.com/ruistola/cooper/lexer"
	"github.com/ruistola/cooper/parser"
	"github.com/yassinebenaid/godump"
	"os"
	"time"
)

func main() {
	filename := "examples/program.coo"
	sourceBytes, _ := os.ReadFile(filename)
	src := string(sourceBytes)

	fmt.Printf("Raw source (%s):\n--\n%s--\n", filename, src)

	totalDuration := time.Duration(0)

	startTokenization := time.Now()
	tokens := lexer.Tokenize(src)
	durationTokenization := time.Since(startTokenization)
	totalDuration += durationTokenization
	fmt.Printf("Tokenized %s in %v.\n\n", filename, durationTokenization)
	fmt.Println("Tokens:")
	godump.Dump(tokens)

	startParsing := time.Now()
	ast := parser.Parse(tokens)
	durationParsing := time.Since(startParsing)
	totalDuration += durationParsing
	fmt.Printf("Parsed %s in %v.\n\n", filename, durationParsing)

	fmt.Println("Parsed AST:")
	godump.Dump(ast)

	// startTypeChecking := time.Now()
	// errors := typechecker.Check(ast)
	// durationTypeChecking := time.Since(startTypeChecking)
	// totalDuration += durationTypeChecking
	// if len(errors) == 0 {
	// 	fmt.Println("0 errors.")
	// } else {
	// 	for _, err := range errors {
	// 		fmt.Println(err)
	// 	}
	// }
	// fmt.Printf("Type checked %s in %v.\n\n", filename, durationTypeChecking)

	fmt.Printf("Done in %v.\n", totalDuration)
}
