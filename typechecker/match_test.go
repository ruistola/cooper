package typechecker

import "testing"

// A match statement over a non-generic sum type type checks when every variant is
// covered and each arm binds the correct payload arity.
func TestMatchStmtExhaustive(t *testing.T) {
	expectOK(t, `oneof Shape {
  Circle(i32),
  Rect(i32, i32),
}
func area(s: Shape) {
  match s with {
    Shape.Circle(r) => a := r
    Shape.Rect(w, h) => a := w
  }
}`)
}

// A trailing wildcard arm satisfies exhaustiveness without listing every variant.
func TestMatchStmtWildcardExhaustive(t *testing.T) {
	expectOK(t, `oneof Color { Red, Green, Blue }
func name(c: Color) {
  match c with {
    Color.Red => a := 1
    _ => a := 0
  }
}`)
}

// Failing to cover every variant without a wildcard is a non-exhaustive match.
func TestMatchStmtNonExhaustive(t *testing.T) {
	expectErr(t, `oneof Color { Red, Green, Blue }
func name(c: Color) {
  match c with {
    Color.Red => a := 1
    Color.Green => a := 2
  }
}`, "non-exhaustive")
}

// A binder count that disagrees with the variant's payload arity is rejected.
func TestMatchArityMismatch(t *testing.T) {
	expectErr(t, `oneof Shape {
  Circle(i32),
  Rect(i32, i32),
}
func area(s: Shape) {
  match s with {
    Shape.Circle(r) => a := r
    Shape.Rect(w) => a := w
  }
}`, "binds 1 slot")
}

// A pattern naming a non-existent variant is rejected.
func TestMatchUnknownVariant(t *testing.T) {
	expectErr(t, `oneof Color { Red, Green }
func name(c: Color) {
  match c with {
    Color.Red => a := 1
    Color.Blue => a := 2
  }
}`, "not a variant")
}

// Matching on a non-sum-type value is rejected.
func TestMatchNonSumType(t *testing.T) {
	expectErr(t, `func f(x: i32) {
  match x with {
    _ => a := 0
  }
}`, "non-sum-type")
}

// A payload binder is bound to its slot type and usable in the arm body.
func TestMatchBinderTyped(t *testing.T) {
	expectOK(t, `oneof Boxed { Wrap(i32) }
func unwrap(b: Boxed): i32 {
  match b with {
    Boxed.Wrap(n) => return n
  }
}`)
}

// A binder's slot type flows into the arm body: using it at the wrong type errors.
func TestMatchBinderTypeMismatch(t *testing.T) {
	expectErr(t, `oneof Boxed { Wrap(i32) }
func unwrap(b: Boxed): string {
  match b with {
    Boxed.Wrap(n) => return n
  }
}`, "mismatch")
}

// A match expression's arm types must unify; its value has that common type.
func TestMatchExprUnifies(t *testing.T) {
	expectOK(t, `oneof Shape {
  Circle(i32),
  Rect(i32, i32),
}
func area(s: Shape): i32 {
  a := match s with {
    Shape.Circle(r) => r
    Shape.Rect(w, h) => w
  }
  return a
}`)
}

// Match expression arms of disagreeing types are rejected.
func TestMatchExprMismatchedArms(t *testing.T) {
	expectErr(t, `oneof Shape {
  Circle(i32),
  Rect(i32, i32),
}
func area(s: Shape) {
  a := match s with {
    Shape.Circle(r) => r
    Shape.Rect(w, h) => "no"
  }
}`, "mismatched types")
}

// A wildcard binder slot ignores its payload and binds no name.
func TestMatchWildcardBinderSlot(t *testing.T) {
	expectOK(t, `oneof Shape {
  Rect(i32, i32),
}
func width(s: Shape): i32 {
  match s with {
    Shape.Rect(w, _) => return w
  }
}`)
}
