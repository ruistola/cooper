package typechecker

import "testing"

// --- Pointers ---

func TestAddressOfAndDerefRoundTrip(t *testing.T) {
	expectOK(t, `func main() {
  let x: i32 = 5
  let p: i32^ = &x
  let y: i32 = p^
}`)
}

func TestDerefAssignThroughPointer(t *testing.T) {
	expectOK(t, `func main() {
  let x: i32 = 5
  let p: i32^ = &x
  p^ = 10
}`)
}

func TestPointerToStructAutoDerefsMemberAccess(t *testing.T) {
	expectOK(t, `struct Point {
  x: i32,
  y: i32,
}
func main() {
  let pt: Point = Point{x: 1, y: 2,}
  let p: Point^ = &pt
  let a: i32 = p.x
  p.y = 9
}`)
}

func TestPointerToStructAutoDerefsMethodCall(t *testing.T) {
	expectOK(t, `struct Point {
  x: i32,
}
(self: Point) func getX(): i32 {
  return self.x
}
func main() {
  let pt: Point = Point{x: 1,}
  let p: Point^ = &pt
  let a: i32 = p.getX()
}`)
}

func TestNilAssignableToPointer(t *testing.T) {
	expectOK(t, `func main() {
  let p: i32^ = nil
}`)
}

func TestPointerTypeMismatchRejected(t *testing.T) {
	expectErr(t, `func main() {
  let x: i32 = 5
  let p: bool^ = &x
}`, "type mismatch")
}

func TestDerefOfNonPointerRejected(t *testing.T) {
	expectErr(t, `func main() {
  let x: i32 = 5
  let y: i32 = x^
}`, "cannot dereference non-pointer")
}

func TestAddressOfTemporaryRejected(t *testing.T) {
	expectErr(t, `func main() {
  let p: i32^ = &5
}`, "non-addressable")
}

func TestNilNotAssignableToNonPointer(t *testing.T) {
	expectErr(t, `func main() {
  let x: i32 = nil
}`, "type mismatch")
}

func TestPointerToPointer(t *testing.T) {
	expectOK(t, `func main() {
  let x: i32 = 5
  let p: i32^ = &x
  let pp: i32^^ = &p
  let y: i32 = pp^^
}`)
}
