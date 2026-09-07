package typechecker

import (
	"testing"
)

// The walrus declaration-expression binds a name to an inferred type without a
// preceding `let`, and must type check as a standalone expression statement.
func TestWalrusDeclTypeChecks(t *testing.T) {
	expectOK(t, "x := 2 + 2")
}
