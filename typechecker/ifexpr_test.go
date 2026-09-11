package typechecker

import "testing"

// An if-expression in walrus position type checks when both branches agree.
func TestIfExprSimple(t *testing.T) {
	expectOK(t, `func f(b: bool): i32 {
  x := if b then 1 else 2
  return x
}`)
}

// If-expression branches may themselves be value blocks.
func TestIfExprBlockBranches(t *testing.T) {
	expectOK(t, `func f(b: bool): i32 {
  x := if b then { 1 } else { 2 }
  return x
}`)
}

// Branches of disagreeing types are rejected under strict equality.
func TestIfExprMismatchedBranches(t *testing.T) {
	expectErr(t, `func f(b: bool): i32 {
  x := if b then 1 else "no"
  return x
}`, "mismatched types")
}

// A non-boolean condition is rejected.
func TestIfExprNonBoolCond(t *testing.T) {
	expectErr(t, `func f(): i32 {
  x := if 3 then 1 else 2
  return x
}`, "boolean")
}

// A value block's type is that of its trailing result expression.
func TestBlockExprResultType(t *testing.T) {
	expectOK(t, `func f(): i32 {
  x := { 5 }
  return x
}`)
}

// A match expression with braced-body arms type checks: block-body expression
// arms produce a BlockExpr whose type is its result expression.
func TestMatchExprBracedArmBodies(t *testing.T) {
	expectOK(t, `oneof Shape {
  Circle(i32),
  Rect(i32, i32),
}
func area(s: Shape): i32 {
  a := match s with {
    Shape.Circle(r) => { r }
    Shape.Rect(w, h) => { w }
  }
  return a
}`)
}
