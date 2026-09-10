package parser

import (
	"github.com/ruistola/cooper/ast"
	"github.com/ruistola/cooper/lexer"
	"testing"
)

// typeOf parses `let x: <src> = ...`-free by using a var decl and returns the
// annotated type expression.
func typeOf(t *testing.T, typeSrc string) ast.TypeExpr {
	t.Helper()
	module := Parse(lexer.Tokenize("let x: " + typeSrc))
	decl, ok := module.Statements[0].(*ast.VarDeclStmt)
	if !ok {
		t.Fatalf("expected *ast.VarDeclStmt, got %T", module.Statements[0])
	}
	return decl.Var.Type
}

// A juxtaposition type application parses as a constructor applied to a spine of
// argument atoms.
func TestTypeApplicationParsing(t *testing.T) {
	app, ok := typeOf(t, "Map string i32").(*ast.TypeApplicationExpr)
	if !ok {
		t.Fatalf("expected *ast.TypeApplicationExpr, got %T", typeOf(t, "Map string i32"))
	}
	ctor, ok := app.Constructor.(*ast.NamedTypeExpr)
	if !ok || ctor.TypeName != "Map" {
		t.Fatalf("expected constructor Map, got %#v", app.Constructor)
	}
	if len(app.Args) != 2 {
		t.Fatalf("expected 2 args, got %d", len(app.Args))
	}
	if a0, ok := app.Args[0].(*ast.NamedTypeExpr); !ok || a0.TypeName != "string" {
		t.Fatalf("expected first arg string, got %#v", app.Args[0])
	}
	if a1, ok := app.Args[1].(*ast.NamedTypeExpr); !ok || a1.TypeName != "i32" {
		t.Fatalf("expected second arg i32, got %#v", app.Args[1])
	}
}

// A single named type is not wrapped in an application.
func TestNonApplicationNamedType(t *testing.T) {
	if _, ok := typeOf(t, "i32").(*ast.NamedTypeExpr); !ok {
		t.Fatalf("expected *ast.NamedTypeExpr, got %T", typeOf(t, "i32"))
	}
}

// Postfix `[]`/`^` bind tighter than application: `Box i32[]` applies Box to an
// array-of-i32 argument, whereas an array of the whole application needs parens.
func TestPostfixBindsTighterThanApplication(t *testing.T) {
	app, ok := typeOf(t, "Box i32[]").(*ast.TypeApplicationExpr)
	if !ok {
		t.Fatalf("expected *ast.TypeApplicationExpr, got %T", typeOf(t, "Box i32[]"))
	}
	if _, ok := app.Args[0].(*ast.ArrayTypeExpr); !ok {
		t.Fatalf("expected array-typed argument, got %T", app.Args[0])
	}

	arr, ok := typeOf(t, "(Box i32)[]").(*ast.ArrayTypeExpr)
	if !ok {
		t.Fatalf("expected *ast.ArrayTypeExpr, got %T", typeOf(t, "(Box i32)[]"))
	}
	if _, ok := arr.UnderlyingType.(*ast.TypeApplicationExpr); !ok {
		t.Fatalf("expected application under array, got %T", arr.UnderlyingType)
	}
}

// A compound argument must be parenthesised: `Map (i32, string) bool`.
func TestParenthesisedApplicationArgument(t *testing.T) {
	app, ok := typeOf(t, "Map (i32, string) bool").(*ast.TypeApplicationExpr)
	if !ok {
		t.Fatalf("expected *ast.TypeApplicationExpr, got %T", typeOf(t, "Map (i32, string) bool"))
	}
	if _, ok := app.Args[0].(*ast.TupleTypeExpr); !ok {
		t.Fatalf("expected tuple argument, got %T", app.Args[0])
	}
}

// A struct declares its type-parameter binders after the name.
func TestStructTypeParamBinders(t *testing.T) {
	module := Parse(lexer.Tokenize("struct Map K V { key: K, value: V }"))
	decl, ok := module.Statements[0].(*ast.StructDeclStmt)
	if !ok {
		t.Fatalf("expected *ast.StructDeclStmt, got %T", module.Statements[0])
	}
	if len(decl.TypeParams) != 2 || decl.TypeParams[0] != "K" || decl.TypeParams[1] != "V" {
		t.Fatalf("expected binders [K V], got %#v", decl.TypeParams)
	}
}

// A non-generic struct has no binders.
func TestStructNoTypeParams(t *testing.T) {
	module := Parse(lexer.Tokenize("struct P { x: i32 }"))
	decl := module.Statements[0].(*ast.StructDeclStmt)
	if len(decl.TypeParams) != 0 {
		t.Fatalf("expected no binders, got %#v", decl.TypeParams)
	}
}

// A free function declares method-local type parameters after its name.
func TestFuncTypeParamBinders(t *testing.T) {
	module := Parse(lexer.Tokenize("func id T (x: T): T { return x }"))
	decl, ok := module.Statements[0].(*ast.FuncDeclStmt)
	if !ok {
		t.Fatalf("expected *ast.FuncDeclStmt, got %T", module.Statements[0])
	}
	if len(decl.TypeParams) != 1 || decl.TypeParams[0] != "T" {
		t.Fatalf("expected binder [T], got %#v", decl.TypeParams)
	}
}

// A method on a generic receiver parses the receiver type as an application whose
// arguments are the receiver-pattern parameters.
func TestGenericReceiverParsing(t *testing.T) {
	module := Parse(lexer.Tokenize("(m: Map K V) func get(k: K): V { return m.value }"))
	decl, ok := module.Statements[0].(*ast.FuncDeclStmt)
	if !ok {
		t.Fatalf("expected *ast.FuncDeclStmt, got %T", module.Statements[0])
	}
	if decl.Receiver == nil {
		t.Fatalf("expected a receiver")
	}
	if _, ok := decl.Receiver.Type.(*ast.TypeApplicationExpr); !ok {
		t.Fatalf("expected receiver type application, got %T", decl.Receiver.Type)
	}
}
