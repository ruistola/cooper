package parser

import (
	"github.com/ruistola/cooper/ast"
	"github.com/ruistola/cooper/lexer"
	"testing"
)

// matchStmtOf parses src and returns the first statement as a *ast.MatchStmt.
func matchStmtOf(t *testing.T, src string) *ast.MatchStmt {
	t.Helper()
	module := Parse(lexer.Tokenize(src))
	stmt, ok := module.Statements[0].(*ast.MatchStmt)
	if !ok {
		t.Fatalf("expected *ast.MatchStmt, got %T", module.Statements[0])
	}
	return stmt
}

// matchExprOf parses src whose first statement is a walrus binding and returns
// the right-hand side as a *ast.MatchExpr.
func matchExprOf(t *testing.T, src string) *ast.MatchExpr {
	t.Helper()
	module := Parse(lexer.Tokenize(src))
	exprStmt, ok := module.Statements[0].(*ast.ExpressionStmt)
	if !ok {
		t.Fatalf("expected *ast.ExpressionStmt, got %T", module.Statements[0])
	}
	assign, ok := exprStmt.Expr.(*ast.VarDeclAssignExpr)
	if !ok {
		t.Fatalf("expected *ast.VarDeclAssignExpr, got %T", exprStmt.Expr)
	}
	match, ok := assign.AssignedValue.(*ast.MatchExpr)
	if !ok {
		t.Fatalf("expected *ast.MatchExpr, got %T", assign.AssignedValue)
	}
	return match
}

// A statement-position match dispatches to MatchStmt, covering variant patterns
// with payload binders, wildcard binder slots, and a trailing `_` catch-all.
func TestMatchStmtParsing(t *testing.T) {
	stmt := matchStmtOf(t, `match shape with {
  Shape.Circle(r) => a := r
  Shape.Rect(w, _) => a := w
  _ => a := 0
}`)
	if _, ok := stmt.Scrutinee.(*ast.IdentExpr); !ok {
		t.Fatalf("expected ident scrutinee, got %T", stmt.Scrutinee)
	}
	if len(stmt.Arms) != 3 {
		t.Fatalf("expected 3 arms, got %d", len(stmt.Arms))
	}
	circle, ok := stmt.Arms[0].Pattern.(*ast.VariantPattern)
	if !ok || circle.TypeName != "Shape" || circle.Variant != "Circle" {
		t.Fatalf("expected Shape.Circle pattern, got %#v", stmt.Arms[0].Pattern)
	}
	if len(circle.Binders) != 1 || circle.Binders[0] != "r" {
		t.Fatalf("expected binder [r], got %#v", circle.Binders)
	}
	rect := stmt.Arms[1].Pattern.(*ast.VariantPattern)
	if len(rect.Binders) != 2 || rect.Binders[1] != "_" {
		t.Fatalf("expected binders [w _], got %#v", rect.Binders)
	}
	if _, ok := stmt.Arms[2].Pattern.(*ast.WildcardPattern); !ok {
		t.Fatalf("expected wildcard catch-all, got %T", stmt.Arms[2].Pattern)
	}
}

// An expression-position match dispatches to MatchExpr with expression arm bodies.
func TestMatchExprParsing(t *testing.T) {
	match := matchExprOf(t, `a := match shape with {
  Shape.Circle(r) => r
  _ => 0
}`)
	if len(match.Arms) != 2 {
		t.Fatalf("expected 2 arms, got %d", len(match.Arms))
	}
	if _, ok := match.Arms[0].Body.(*ast.IdentExpr); !ok {
		t.Fatalf("expected ident arm body, got %T", match.Arms[0].Body)
	}
}

// A payload-free variant pattern carries no binders and needs no parentheses.
func TestMatchPayloadFreeVariantPattern(t *testing.T) {
	stmt := matchStmtOf(t, `match c with {
  Color.Red => a := 1
  Color.Green => a := 2
  Color.Blue => a := 3
}`)
	red := stmt.Arms[0].Pattern.(*ast.VariantPattern)
	if red.Variant != "Red" || len(red.Binders) != 0 {
		t.Fatalf("expected Red with no binders, got %#v", red)
	}
}

// A braced block body arm self-terminates and holds a multi-statement block.
func TestMatchBlockBodyArm(t *testing.T) {
	stmt := matchStmtOf(t, `match shape with {
  Shape.Circle(r) => {
    a := r
    b := r
  }
  _ => a := 0
}`)
	block, ok := stmt.Arms[0].Body.(*ast.BlockStmt)
	if !ok {
		t.Fatalf("expected block body, got %T", stmt.Arms[0].Body)
	}
	if len(block.Statements) != 2 {
		t.Fatalf("expected 2 statements in block, got %d", len(block.Statements))
	}
}

// Explicit semicolons may separate arms in place of inferred newlines.
func TestMatchExplicitSemicolonArms(t *testing.T) {
	stmt := matchStmtOf(t, `match c with { Color.Red => a := 1; _ => a := 0 }`)
	if len(stmt.Arms) != 2 {
		t.Fatalf("expected 2 arms, got %d", len(stmt.Arms))
	}
}

// A malformed pattern (missing the variant after the dot) is rejected.
func TestMatchRejectsMalformedPattern(t *testing.T) {
	if !parsePanics(`match c with { Color. => a := 1 }`) {
		t.Fatalf("expected parse to panic on malformed pattern")
	}
}
