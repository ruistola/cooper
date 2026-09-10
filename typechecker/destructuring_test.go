package typechecker

import "testing"

// A walrus pattern infers each binding's type from the tuple initializer, and the
// bindings are usable afterwards.
func TestWalrusDestructureInfersElementTypes(t *testing.T) {
	expectOK(t, `func main() {
  (a, b) := (1, "x")
  let c: i32 = a
  let d: string = b
}`)
}

// A single-name walrus binding is usable after declaration (the resolver now
// records inferred bindings).
func TestWalrusSingleBindingResolves(t *testing.T) {
	expectOK(t, `func main() {
  x := 5
  let y: i32 = x
}`)
}

// A `let` pattern with a matching tuple annotation type checks.
func TestLetDestructureMatchesAnnotation(t *testing.T) {
	expectOK(t, `func main() {
  let (a, b): (i32, string) = (1, "x")
  let c: string = b
}`)
}

// A `let` pattern whose annotation disagrees with the initializer is rejected.
func TestLetDestructureAnnotationMismatchRejected(t *testing.T) {
	expectErr(t, `func main() {
  let (a, b): (i32, string) = ("x", 1)
}`, "type mismatch")
}

// The pattern arity must equal the tuple's arity.
func TestDestructureArityMismatchRejected(t *testing.T) {
	expectErr(t, `func main() {
  (a, b, c) := (1, "x")
}`, "binds 3 names but value has 2 elements")
}

// Destructuring a non-tuple value is rejected.
func TestDestructureNonTupleRejected(t *testing.T) {
	expectErr(t, `func main() {
  (a, b) := 5
}`, "cannot destructure non-tuple")
}

// A tuple returned from a function can be destructured at the call site.
func TestDestructureFunctionResult(t *testing.T) {
	expectOK(t, `func pair(): (i32, string) {
  return (1, "x")
}
func main() {
  (a, b) := pair()
  let c: string = b
}`)
}

// A parenthesised tuple on the left of `=` reassigns existing variables positionally.
func TestTupleReassignment(t *testing.T) {
	expectOK(t, `func main() {
  let a: i32 = 0
  let b: string = ""
  (a, b) = (1, "x")
}`)
}

// Tuple reassignment enforces element-wise type agreement.
func TestTupleReassignmentMismatchRejected(t *testing.T) {
	expectErr(t, `func main() {
  let a: i32 = 0
  let b: string = ""
  (a, b) = ("x", 1)
}`, "cannot assign")
}
