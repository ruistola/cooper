package parser

import (
	"github.com/ruistola/cooper/ast"
	"github.com/ruistola/cooper/lexer"
	"github.com/yassinebenaid/godump"
	"testing"
)

func TestBasicExpression(t *testing.T) {
	src := "69 + 420"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestBasicExpressionStmt(t *testing.T) {
	src := "x += 69 + 420"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestVarDeclStatement(t *testing.T) {
	src := "let x: i32 = 1"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestVarDeclStatementTypeOnly(t *testing.T) {
	src := "let x: i32"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestVarDeclStatementValueOnly(t *testing.T) {
	src := "let x = 1"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestAssignDeclExpression(t *testing.T) {
	src := "x := 69 + 420"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestIfExpression1(t *testing.T) {
	src := "result = if x < 5 then { 0 } else { 5 }"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestIfExpression2(t *testing.T) {
	src := "result = if x < 5 then 0 else 5"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestIfExpressionNewline(t *testing.T) {
	src := "result = if x < 5 then 0\n else 5"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestIfExpressionSemicolon(t *testing.T) {
	src := "result = if x < 5 then 0; else 5"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestIfExpressionSemicolonNewline(t *testing.T) {
	src := "result = if x < 5 then 0;\nelse 5"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestIfStatementExplicitSemicolon(t *testing.T) {
	src := "if x < 5 then foo(); else bar();"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestIfStatementNewline(t *testing.T) {
	src := "if x < 5 then foo()\nelse bar()"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestIfStatementSemicolonNewline(t *testing.T) {
	src := "if x < 5 then foo();\nelse bar();"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestOneLineIfStatementBlock(t *testing.T) {
	src := "if x < 5 then { foo() } else { bar() }"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestOneLineIfStatementMultiStatementBlock(t *testing.T) {
	src := "if x < 5 then { foo()\nbar() } else { bar(); baz() }"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

func TestOneLineIfStatement(t *testing.T) {
	src := "if x < 5 then foo() else bar()"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
	}
}

// Should this be legal? Is it an expression statement with then-expr & else-expr or statement with then-stmt & else-expr?
func TestOneLineIfStatementComplex(t *testing.T) {
	src := "if x < 5 then foo() else if x > 10 then 100 else 10"
	parsedAst := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(parsedAst)
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
			parsedAst := Parse(lexer.Tokenize(tc.src))
			if testing.Verbose() {
				t.Logf("\nParsing: %s", tc.src)
				godump.Dump(parsedAst)
			}
		})
	}
}

func TestSemicolonInference(t *testing.T) {
	testCases := []struct {
		name string
		src  string
	}{
		{
			"basic inference with newlines",
			`let x: i32 = 5
let y: i32 = 10
x + y`,
		},
		{
			"no semicolon before closing brace",
			`{
  let a: i32 = 5
  let b: i32 = 10
  a + b
}`,
		},
		{
			"explicit semicolon suppresses block value",
			`{
  let a: i32 = 5
  let b: i32 = 10
  a + b;
}`,
		},
		{
			"for loop body suppresses value",
			`for (let i: i32 = 0; i < 10; i += 1) {
  doSomething()
  i * 2
}`,
		},
		{
			"void function body suppresses value",
			`func foo() {
  let x: i32 = 5
  x + 10
}`,
		},
		{
			"non-void function preserves value",
			`func bar(): i32 {
  let x: i32 = 5
  x + 10
}`,
		},
		{
			"if expression blocks preserve values",
			`if x > 0 then {
  doA()
  5
} else {
  doB()
  10
}`,
		},
		{
			"nested block as expression",
			`let result: i32 = {
  let temp: i32 = {
    let a: i32 = 1
    let b: i32 = 2
    a + b
  }
  temp * 10
}`,
		},
		{
			"newlines in parentheses ignored",
			`foo(
  1,
  2,
  3
)`,
		},
		{
			"assignment with newline",
			`let x: i32 = 5
x = 10
y = x + 5`,
		},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			parsedAst := Parse(lexer.Tokenize(tc.src))
			if testing.Verbose() {
				t.Logf("\nParsing: %s", tc.src)
				godump.Dump(parsedAst)
			}
			// Just verify it parses without panic
		})
	}
}

func TestMethodDeclParsesReceiver(t *testing.T) {
	src := `struct Product {
  price: i32,
}
(p: Product) func isAffordable(budget: i32): bool {
  return p.price <= budget
}`
	module := Parse(lexer.Tokenize(src))
	if testing.Verbose() {
		godump.Dump(module)
	}
	if len(module.Statements) != 2 {
		t.Fatalf("expected 2 statements (struct, method), got %d", len(module.Statements))
	}
	fn, ok := module.Statements[1].(*ast.FuncDeclStmt)
	if !ok {
		t.Fatalf("expected second statement to be *ast.FuncDeclStmt, got %T", module.Statements[1])
	}
	if fn.Receiver == nil {
		t.Fatal("expected method to have a receiver, got nil")
	}
	if fn.Receiver.Name != "p" {
		t.Errorf("expected receiver name 'p', got %q", fn.Receiver.Name)
	}
	named, ok := fn.Receiver.Type.(*ast.NamedTypeExpr)
	if !ok || named.TypeName != "Product" {
		t.Errorf("expected receiver type 'Product', got %+v", fn.Receiver.Type)
	}
	if fn.Name != "isAffordable" {
		t.Errorf("expected method name 'isAffordable', got %q", fn.Name)
	}
}

func TestFreeFuncHasNilReceiver(t *testing.T) {
	module := Parse(lexer.Tokenize("func add(x: i32, y: i32): i32 {\n  return x + y\n}"))
	fn, ok := module.Statements[0].(*ast.FuncDeclStmt)
	if !ok {
		t.Fatalf("expected *ast.FuncDeclStmt, got %T", module.Statements[0])
	}
	if fn.Receiver != nil {
		t.Errorf("expected free function to have nil receiver, got %+v", fn.Receiver)
	}
}

// A leading `( ident := ... )` must remain a grouped walrus expression, not be
// mistaken for a method receiver clause. The COLON vs COLON_EQUALS token is the
// sole discriminator.
func TestGroupedWalrusNotMistakenForMethod(t *testing.T) {
	module := Parse(lexer.Tokenize("(x := 5)"))
	stmt, ok := module.Statements[0].(*ast.ExpressionStmt)
	if !ok {
		t.Fatalf("expected *ast.ExpressionStmt, got %T", module.Statements[0])
	}
	group, ok := stmt.Expr.(*ast.GroupExpr)
	if !ok {
		t.Fatalf("expected *ast.GroupExpr, got %T", stmt.Expr)
	}
	if _, ok := group.Expr.(*ast.VarDeclAssignExpr); !ok {
		t.Errorf("expected grouped *ast.VarDeclAssignExpr, got %T", group.Expr)
	}
}

// Two consecutive top-level declarations must both parse: declarations consume
// their trailing (inferred) statement terminator.
func TestConsecutiveTopLevelDecls(t *testing.T) {
	src := `struct P {
  price: i32,
}
(p: P) func cost(): i32 {
  return p.price
}
func main() {
  return
}`
	module := Parse(lexer.Tokenize(src))
	if len(module.Statements) != 3 {
		t.Fatalf("expected 3 top-level statements, got %d", len(module.Statements))
	}
}

func TestBlockValueSemantics(t *testing.T) {
	src := `{
  let a: i32 = 5
  a + 10
}`
	parsedAst := Parse(lexer.Tokenize(src))

	// Verify the block has statements and no suppression
	if len(parsedAst.Statements) != 1 {
		t.Errorf("Expected 1 statement (the block), got %d", len(parsedAst.Statements))
	}

	if exprStmt, ok := parsedAst.Statements[0].(*ast.ExpressionStmt); ok {
		if _, ok := exprStmt.Expr.(*ast.BlockExpr); !ok {
			t.Errorf("Expected block expression, got %t", exprStmt.Expr)
		}
	}
}
