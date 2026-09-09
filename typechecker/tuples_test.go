package typechecker

import "testing"

// A tuple binding type checks when the literal's element types match the
// declared tuple type.
func TestTupleBindingTypeChecks(t *testing.T) {
	expectOK(t, `func main() {
  let pair: (i32, string) = (1, "a")
}`)
}

// A tuple whose element types differ from the declared type is rejected.
func TestTupleElementMismatchRejected(t *testing.T) {
	expectErr(t, `func main() {
  let pair: (i32, string) = ("a", 1)
}`, "type mismatch")
}

// Tuple arity is part of the type: differing lengths do not match.
func TestTupleArityMismatchRejected(t *testing.T) {
	expectErr(t, `func main() {
  let pair: (i32, string) = (1, "a", 2)
}`, "type mismatch")
}

// A single parenthesised value is not a tuple; (i32) is just i32.
func TestSingleElementParensIsNotTuple(t *testing.T) {
	expectOK(t, `func main() {
  let x: i32 = (1)
}`)
}

// A tuple can be returned from a function as an ordinary single value.
func TestTupleAsReturnValue(t *testing.T) {
	expectOK(t, `func pair(): (i32, string) {
  return (1, "a")
}`)
}

// A tuple can nest other tuples.
func TestNestedTupleTypeChecks(t *testing.T) {
	expectOK(t, `func main() {
  let nested: (i32, (string, bool)) = (1, ("a", true))
}`)
}
