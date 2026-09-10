package typechecker

import "testing"

// A generic struct's field access yields the substituted member type at an
// annotated instantiation.
func TestGenericStructFieldAccess(t *testing.T) {
	expectOK(t, `struct Box T { value: T }
func unbox(b: Box i32): i32 { return b.value }`)
}

// A member typed with a type parameter is substituted, so a mismatched use is
// rejected.
func TestGenericStructFieldTypeMismatch(t *testing.T) {
	expectErr(t, `struct Box T { value: T }
func unbox(b: Box i32): string { return b.value }`, "mismatch")
}

// Constructing a generic struct infers its type arguments from the members.
func TestGenericStructConstructionInfers(t *testing.T) {
	expectOK(t, `struct Box T { value: T }
func make(): Box i32 { return Box { value: 5 } }`)
}

// An inferred instantiation is nominal by its type arguments, so a differing
// annotation is a type error.
func TestGenericInstantiationNominalByArgs(t *testing.T) {
	expectErr(t, `struct Box T { value: T }
func make(): Box string { return Box { value: 5 } }`, "mismatch")
}

// A two-parameter generic struct substitutes each member independently.
func TestGenericTwoParams(t *testing.T) {
	expectOK(t, `struct Pair A B { first: A, second: B }
func fst(p: Pair i32 string): i32 { return p.first }`)
}

// Applying a generic type with the wrong arity is rejected.
func TestGenericArityMismatch(t *testing.T) {
	expectErr(t, `struct Box T { value: T }
func f(b: Box i32 string): i32 { return 0 }`, "argument")
}

// Using a generic type without arguments is rejected.
func TestGenericMissingArgs(t *testing.T) {
	expectErr(t, `struct Box T { value: T }
func f(b: Box): i32 { return 0 }`, "type argument")
}

// A method on a generic receiver type checks its body against the receiver's
// type parameters.
func TestGenericMethodDeclaration(t *testing.T) {
	expectOK(t, `struct Box T { value: T }
(b: Box T) func get(): T { return b.value }`)
}
