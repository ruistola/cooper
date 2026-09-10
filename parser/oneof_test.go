package parser

import (
	"github.com/ruistola/cooper/ast"
	"github.com/ruistola/cooper/lexer"
	"testing"
)

func oneofOf(t *testing.T, src string) *ast.OneofDeclStmt {
	t.Helper()
	module := Parse(lexer.Tokenize(src))
	decl, ok := module.Statements[0].(*ast.OneofDeclStmt)
	if !ok {
		t.Fatalf("expected *ast.OneofDeclStmt, got %T", module.Statements[0])
	}
	return decl
}

// A generic sum type declares type-parameter binders by juxtaposition and each
// variant carries a parenthesised, comma-delimited payload slot list.
func TestOneofDeclParsing(t *testing.T) {
	decl := oneofOf(t, `oneof Result T E {
  Ok(T),
  Err(E),
}`)
	if decl.Name != "Result" {
		t.Fatalf("expected name Result, got %s", decl.Name)
	}
	if len(decl.TypeParams) != 2 || decl.TypeParams[0] != "T" || decl.TypeParams[1] != "E" {
		t.Fatalf("expected binders [T E], got %#v", decl.TypeParams)
	}
	if len(decl.Variants) != 2 {
		t.Fatalf("expected 2 variants, got %d", len(decl.Variants))
	}
	if decl.Variants[0].Name != "Ok" || len(decl.Variants[0].Payload) != 1 {
		t.Fatalf("expected Ok with one slot, got %#v", decl.Variants[0])
	}
	if slot, ok := decl.Variants[0].Payload[0].(*ast.NamedTypeExpr); !ok || slot.TypeName != "T" {
		t.Fatalf("expected Ok payload T, got %#v", decl.Variants[0].Payload[0])
	}
}

// A payload-free variant (like None) carries no slots.
func TestOneofPayloadFreeVariant(t *testing.T) {
	decl := oneofOf(t, `oneof Maybe T {
  None,
  Some(T),
}`)
	if decl.Variants[0].Name != "None" || len(decl.Variants[0].Payload) != 0 {
		t.Fatalf("expected None with no slots, got %#v", decl.Variants[0])
	}
	if decl.Variants[1].Name != "Some" || len(decl.Variants[1].Payload) != 1 {
		t.Fatalf("expected Some with one slot, got %#v", decl.Variants[1])
	}
}

// A variant may declare multiple positional slots, and a compound slot type
// (e.g. a type application) parses within a single slot.
func TestOneofMultiSlotAndCompoundSlot(t *testing.T) {
	decl := oneofOf(t, `oneof Shape {
  Rect(f32, f32),
  Boxed(Box i32),
}`)
	if len(decl.Variants[0].Payload) != 2 {
		t.Fatalf("expected Rect with two slots, got %d", len(decl.Variants[0].Payload))
	}
	if _, ok := decl.Variants[1].Payload[0].(*ast.TypeApplicationExpr); !ok {
		t.Fatalf("expected application slot, got %T", decl.Variants[1].Payload[0])
	}
}

// A non-generic sum type has no binders and a trailing comma is optional.
func TestOneofNonGenericTrailingComma(t *testing.T) {
	decl := oneofOf(t, `oneof Color { Red, Green, Blue }`)
	if len(decl.TypeParams) != 0 {
		t.Fatalf("expected no binders, got %#v", decl.TypeParams)
	}
	if len(decl.Variants) != 3 {
		t.Fatalf("expected 3 variants, got %d", len(decl.Variants))
	}
}
