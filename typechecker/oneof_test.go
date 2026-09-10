package typechecker

import "testing"

// A non-generic sum type value can be constructed from a payload-free variant and
// from a variant with a payload.
func TestOneofNonGenericConstruction(t *testing.T) {
	expectOK(t, `oneof Shape {
  Circle(i32),
  Rect(i32, i32),
}
func make(): Shape { return Shape.Rect(3, 4) }`)
}

// A payload-free variant of a non-generic sum type is a complete value.
func TestOneofPayloadFreeConstruction(t *testing.T) {
	expectOK(t, `oneof Color { Red, Green, Blue }
func pick(): Color { return Color.Green }`)
}

// Constructing a generic variant infers the sum type's arguments from the payload.
func TestOneofGenericConstructionInfers(t *testing.T) {
	expectOK(t, `oneof Maybe T {
  None,
  Some(T),
}
func wrap(): Maybe i32 { return Maybe.Some(5) }`)
}

// A generic instantiation is nominal by its inferred arguments.
func TestOneofGenericNominalByArgs(t *testing.T) {
	expectErr(t, `oneof Maybe T {
  None,
  Some(T),
}
func wrap(): Maybe string { return Maybe.Some(5) }`, "mismatch")
}

// A two-parameter sum type infers each argument from the relevant variant.
func TestOneofResultConstruction(t *testing.T) {
	expectOK(t, `oneof Result T E {
  Ok(T),
  Err(E),
}
func good(): Result i32 string { return Result.Ok(5) }
func bad(): Result i32 string { return Result.Err("boom") }`)
}

// Wrong payload arity is rejected.
func TestOneofArityMismatch(t *testing.T) {
	expectErr(t, `oneof Shape {
  Rect(f32, f32),
}
func make(): Shape { return Shape.Rect(3.0) }`, "expects 2 argument")
}

// An unknown variant is rejected.
func TestOneofUnknownVariant(t *testing.T) {
	expectErr(t, `oneof Color { Red, Green }
func pick(): Color { return Color.Blue }`, "not a variant")
}

// A payload argument of the wrong type is rejected.
func TestOneofPayloadTypeMismatch(t *testing.T) {
	expectErr(t, `oneof Boxed { Wrap(i32) }
func make(): Boxed { return Boxed.Wrap("nope") }`, "mismatch")
}

// A payload-free variant of a generic sum type cannot be inferred without an
// expected type: a bare walrus binding supplies no context to fix its arguments.
func TestOneofGenericPayloadFreeNeedsAnnotation(t *testing.T) {
	expectErr(t, `oneof Maybe T {
  None,
  Some(T),
}
func empty() { m := Maybe.None }`, "cannot infer")
}

// A payload-free variant of a generic sum type is inferred from the return context.
func TestOneofGenericPayloadFreeFromContext(t *testing.T) {
	expectOK(t, `oneof Maybe T {
  None,
  Some(T),
}
func empty(): Maybe i32 { return Maybe.None }`)
}

// A sum type can carry a generic struct as a payload slot.
func TestOneofWithGenericStructPayload(t *testing.T) {
	expectOK(t, `struct Box T { value: T }
oneof Holder {
  Full(Box i32),
}
func make(b: Box i32): Holder { return Holder.Full(b) }`)
}
