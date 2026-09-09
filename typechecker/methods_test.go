package typechecker

import (
	"strings"
	"testing"

	"github.com/ruistola/cooper/lexer"
	"github.com/ruistola/cooper/parser"
)

// checkSrc runs the full frontend (resolve, type check, semantic analysis) on a
// source snippet and returns the collected diagnostics.
func checkSrc(src string) []string {
	return Check(parser.Parse(lexer.Tokenize(src)))
}

// expectOK fails the test if any diagnostics are produced.
func expectOK(t *testing.T, src string) {
	t.Helper()
	if errs := checkSrc(src); len(errs) != 0 {
		t.Fatalf("expected no errors, got:\n%s", strings.Join(errs, "\n"))
	}
}

// expectErr fails the test unless at least one diagnostic contains substr.
func expectErr(t *testing.T, src string, substr string) {
	t.Helper()
	errs := checkSrc(src)
	if len(errs) == 0 {
		t.Fatalf("expected an error containing %q, got none", substr)
	}
	for _, e := range errs {
		if strings.Contains(e, substr) {
			return
		}
	}
	t.Fatalf("expected an error containing %q, got:\n%s", substr, strings.Join(errs, "\n"))
}

// --- Methods ---

func TestMethodCallTypeChecks(t *testing.T) {
	expectOK(t, `struct Product {
  name: string,
  price: i32,
}
(p: Product) func isAffordable(budget: i32): bool {
  return p.price <= budget
}
func main() {
  let prod: Product = Product{name: "x", price: 5,}
  let cheap: bool = prod.isAffordable(10)
}`)
}

// `x.method` (without a call) yields the receiver-stripped function type, so it
// can be bound to a matching function-typed variable.
func TestMethodBindingYieldsStrippedFuncType(t *testing.T) {
	expectOK(t, `struct Product {
  price: i32,
}
(p: Product) func isAffordable(budget: i32): bool {
  return p.price <= budget
}
func main() {
  let prod: Product = Product{price: 5,}
  let f: func(i32): bool = prod.isAffordable
}`)
}

func TestMethodBadReturnTypeRejected(t *testing.T) {
	expectErr(t, `struct Product {
  price: i32,
}
(p: Product) func isAffordable(budget: i32): bool {
  return p.price
}`, "return type mismatch")
}

func TestMethodWrongArgTypeRejected(t *testing.T) {
	expectErr(t, `struct Product {
  price: i32,
}
(p: Product) func isAffordable(budget: i32): bool {
  return p.price <= budget
}
func main() {
  let prod: Product = Product{price: 5,}
  let cheap: bool = prod.isAffordable(true)
}`, "type mismatch")
}

func TestMethodOnNonStructReceiverRejected(t *testing.T) {
	expectErr(t, `(x: i32) func foo(): i32 {
  return x
}`, "must be a struct or pointer-to-struct type")
}

// A pointer receiver `(p: Product^)` keys the same method set as a value
// receiver and auto-derefs on field access, so mutation through it type checks.
func TestPointerReceiverTypeChecks(t *testing.T) {
	expectOK(t, `struct Product {
  price: i32,
}
(p: Product^) func discount(amount: i32) {
  p.price -= amount
}
func main() {
  let prod: Product = Product{price: 5,}
  prod.discount(2)
}`)
}

// A pointer receiver is bound as a pointer, so it may be dereferenced explicitly.
func TestPointerReceiverIsPointer(t *testing.T) {
	expectOK(t, `struct Product {
  price: i32,
}
(p: Product^) func priced(): i32 {
  return p^.price
}`)
}

// A `Product^^` receiver is not a struct or pointer-to-struct and is rejected.
func TestDoublePointerReceiverRejected(t *testing.T) {
	expectErr(t, `struct Product {
  price: i32,
}
(p: Product^^) func foo(): i32 {
  return 0
}`, "must be a struct or pointer-to-struct type")
}

func TestMethodNameCollidesWithFieldRejected(t *testing.T) {
	expectErr(t, `struct Product {
  price: i32,
}
(p: Product) func price(): i32 {
  return p.price
}`, "collides with a field")
}

func TestDuplicateMethodRejected(t *testing.T) {
	expectErr(t, `struct Product {
  price: i32,
}
(p: Product) func cost(): i32 {
  return p.price
}
(p: Product) func cost(): i32 {
  return p.price
}`, "redeclared method")
}

func TestUnknownMethodRejected(t *testing.T) {
	expectErr(t, `struct Product {
  price: i32,
}
func main() {
  let prod: Product = Product{price: 5,}
  let x: i32 = prod.notAMethod()
}`, "not a member of struct")
}

// A method name and a free function name may coincide without colliding, since
// methods are keyed by their receiver type.
func TestMethodAndFreeFuncSameNameAllowed(t *testing.T) {
	expectOK(t, `struct Product {
  price: i32,
}
(p: Product) func cost(): i32 {
  return p.price
}
func cost(): i32 {
  return 0
}`)
}

// The receiver is not part of the bound signature: a two-declared-parameter
// method binds to a one-parameter function type.
func TestReceiverExcludedFromBoundSignature(t *testing.T) {
	expectErr(t, `struct Product {
  price: i32,
}
(p: Product) func isAffordable(budget: i32): bool {
  return p.price <= budget
}
func main() {
  let prod: Product = Product{price: 5,}
  let f: func(Product, i32): bool = prod.isAffordable
}`, "type mismatch")
}

// --- Core (non-method) coverage that had shifted into the frontend ---

func TestStructFieldAccessTypeChecks(t *testing.T) {
	expectOK(t, `struct Point {
  x: i32,
  y: i32,
}
func main() {
  let p: Point = Point{x: 1, y: 2,}
  let sum: i32 = p.x + p.y
}`)
}

func TestUnknownStructFieldRejected(t *testing.T) {
	expectErr(t, `struct Point {
  x: i32,
}
func main() {
  let p: Point = Point{x: 1,}
  let z: i32 = p.z
}`, "not a member of struct")
}

func TestFuncCallArityRejected(t *testing.T) {
	expectErr(t, `func add(x: i32, y: i32): i32 {
  return x + y
}
func main() {
  let n: i32 = add(1)
}`, "wrong number of arguments")
}

func TestMissingReturnPathRejected(t *testing.T) {
	expectErr(t, `func foo(): i32 {
  let x: i32 = 5
}`, "does not return a value in all code paths")
}
