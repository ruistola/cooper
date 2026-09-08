package parser

import (
	"github.com/ruistola/cooper/ast"
	"github.com/ruistola/cooper/lexer"
	"github.com/yassinebenaid/godump"
	"slices"
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
		{
			"consecutive func declarations",
			`func a(): i32 {
  return 1
}
func b(): i32 {
  return 2
}`,
		},
		{
			"consecutive struct declarations",
			`struct A {
  x: i32,
}
struct B {
  y: i32,
}`,
		},
		{
			"if-block followed by statement",
			`if x < 5 then {
  foo()
}
bar()`,
		},
		{
			"for-block followed by statement",
			`for (i := 0; i < 10; i += 1) {
  foo()
}
bar()`,
		},
		{
			"value block followed by statement",
			`let x: i32 = {
  5
}
foo()`,
		},
		{
			"empty statements pruned",
			`;;
let x: i32 = 5;;
foo();`,
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

func TestUseDeclParsesPathsAndAliases(t *testing.T) {
	src := `use {
  std.io,
  http,
  auth: server.auth,
}`
	module := Parse(lexer.Tokenize(src))
	if len(module.Statements) != 1 {
		t.Fatalf("expected 1 statement (use block), got %d", len(module.Statements))
	}
	use, ok := module.Statements[0].(*ast.UseDeclStmt)
	if !ok {
		t.Fatalf("expected *ast.UseDeclStmt, got %T", module.Statements[0])
	}
	if len(use.UseSpecs) != 3 {
		t.Fatalf("expected 3 use specs, got %d", len(use.UseSpecs))
	}

	assertSpec := func(i int, alias string, path ...string) {
		spec := use.UseSpecs[i]
		if spec.Alias != alias {
			t.Errorf("spec %d: expected alias %q, got %q", i, alias, spec.Alias)
		}
		if !slices.Equal(spec.Path, path) {
			t.Errorf("spec %d: expected path %v, got %v", i, path, spec.Path)
		}
	}
	assertSpec(0, "", "std", "io")
	assertSpec(1, "", "http")
	assertSpec(2, "auth", "server", "auth")
}

// The structure is fixed regardless of formatting: a single-line use block parses
// identically to its multi-line counterpart.
func TestUseDeclOneLinerMatchesMultiline(t *testing.T) {
	src := `use { io: std.io, http, }`
	module := Parse(lexer.Tokenize(src))
	use, ok := module.Statements[0].(*ast.UseDeclStmt)
	if !ok {
		t.Fatalf("expected *ast.UseDeclStmt, got %T", module.Statements[0])
	}
	if len(use.UseSpecs) != 2 {
		t.Fatalf("expected 2 use specs, got %d", len(use.UseSpecs))
	}
	if use.UseSpecs[0].Alias != "io" || !slices.Equal(use.UseSpecs[0].Path, []string{"std", "io"}) {
		t.Errorf("unexpected first spec: %+v", use.UseSpecs[0])
	}
	if use.UseSpecs[1].Alias != "" || !slices.Equal(use.UseSpecs[1].Path, []string{"http"}) {
		t.Errorf("unexpected second spec: %+v", use.UseSpecs[1])
	}
}

// --- Pointers ---

// A postfix `^` in a variable type is a pointer type; composing with `[]` follows
// left-to-right postfix reading.
func TestPointerTypeExprParsing(t *testing.T) {
	cases := []struct {
		src    string
		verify func(t *testing.T, ty ast.TypeExpr)
	}{
		{
			"let p: Point^",
			func(t *testing.T, ty ast.TypeExpr) {
				ptr, ok := ty.(*ast.PointerTypeExpr)
				if !ok {
					t.Fatalf("expected *ast.PointerTypeExpr, got %T", ty)
				}
				if named, ok := ptr.UnderlyingType.(*ast.NamedTypeExpr); !ok || named.TypeName != "Point" {
					t.Fatalf("expected pointer to Point, got %+v", ptr.UnderlyingType)
				}
			},
		},
		{
			"let pp: Point^^",
			func(t *testing.T, ty ast.TypeExpr) {
				outer, ok := ty.(*ast.PointerTypeExpr)
				if !ok {
					t.Fatalf("expected outer *ast.PointerTypeExpr, got %T", ty)
				}
				if _, ok := outer.UnderlyingType.(*ast.PointerTypeExpr); !ok {
					t.Fatalf("expected pointer to pointer, got %T", outer.UnderlyingType)
				}
			},
		},
		{
			"let ps: Point^[]", // array of pointers
			func(t *testing.T, ty ast.TypeExpr) {
				arr, ok := ty.(*ast.ArrayTypeExpr)
				if !ok {
					t.Fatalf("expected *ast.ArrayTypeExpr, got %T", ty)
				}
				if _, ok := arr.UnderlyingType.(*ast.PointerTypeExpr); !ok {
					t.Fatalf("expected array of pointers, got %T", arr.UnderlyingType)
				}
			},
		},
		{
			"let sp: Point[]^", // pointer to array
			func(t *testing.T, ty ast.TypeExpr) {
				ptr, ok := ty.(*ast.PointerTypeExpr)
				if !ok {
					t.Fatalf("expected *ast.PointerTypeExpr, got %T", ty)
				}
				if _, ok := ptr.UnderlyingType.(*ast.ArrayTypeExpr); !ok {
					t.Fatalf("expected pointer to array, got %T", ptr.UnderlyingType)
				}
			},
		},
	}
	for _, tc := range cases {
		t.Run(tc.src, func(t *testing.T) {
			module := Parse(lexer.Tokenize(tc.src))
			decl, ok := module.Statements[0].(*ast.VarDeclStmt)
			if !ok {
				t.Fatalf("expected *ast.VarDeclStmt, got %T", module.Statements[0])
			}
			tc.verify(t, decl.Var.Type)
		})
	}
}

func TestAddressOfAndDerefExprParsing(t *testing.T) {
	// `&x^` is `&(x^)`: deref binds tighter than address-of.
	module := Parse(lexer.Tokenize("&x^"))
	addr, ok := module.Statements[0].(*ast.ExpressionStmt).Expr.(*ast.AddressOfExpr)
	if !ok {
		t.Fatalf("expected *ast.AddressOfExpr, got %T", module.Statements[0].(*ast.ExpressionStmt).Expr)
	}
	if _, ok := addr.Operand.(*ast.DerefExpr); !ok {
		t.Fatalf("expected address-of a deref, got %T", addr.Operand)
	}
}

// Deref chains through member access: `p^.field` is `(p^).field`, and `p.field^`
// is `(p.field)^`.
func TestDerefBindsWithMemberAccess(t *testing.T) {
	module := Parse(lexer.Tokenize("p^.field"))
	member, ok := module.Statements[0].(*ast.ExpressionStmt).Expr.(*ast.StructMemberExpr)
	if !ok {
		t.Fatalf("expected *ast.StructMemberExpr, got %T", module.Statements[0].(*ast.ExpressionStmt).Expr)
	}
	if _, ok := member.Struct.(*ast.DerefExpr); !ok {
		t.Fatalf("expected member access on a deref, got %T", member.Struct)
	}

	module = Parse(lexer.Tokenize("p.field^"))
	deref, ok := module.Statements[0].(*ast.ExpressionStmt).Expr.(*ast.DerefExpr)
	if !ok {
		t.Fatalf("expected *ast.DerefExpr, got %T", module.Statements[0].(*ast.ExpressionStmt).Expr)
	}
	if _, ok := deref.Operand.(*ast.StructMemberExpr); !ok {
		t.Fatalf("expected deref of a member access, got %T", deref.Operand)
	}
}

// Address-of binds looser than binary arithmetic on its right: `&x + y` is `(&x) + y`.
func TestAddressOfBindsLooserThanArithmetic(t *testing.T) {
	module := Parse(lexer.Tokenize("&x + y"))
	bin, ok := module.Statements[0].(*ast.ExpressionStmt).Expr.(*ast.BinaryExpr)
	if !ok {
		t.Fatalf("expected *ast.BinaryExpr at top, got %T", module.Statements[0].(*ast.ExpressionStmt).Expr)
	}
	if _, ok := bin.Lhs.(*ast.AddressOfExpr); !ok {
		t.Fatalf("expected address-of on the left of +, got %T", bin.Lhs)
	}
}

func TestNilLiteralParsing(t *testing.T) {
	module := Parse(lexer.Tokenize("nil"))
	if _, ok := module.Statements[0].(*ast.ExpressionStmt).Expr.(*ast.NilLiteralExpr); !ok {
		t.Fatalf("expected *ast.NilLiteralExpr, got %T", module.Statements[0].(*ast.ExpressionStmt).Expr)
	}
}
