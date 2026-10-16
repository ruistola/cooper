package main

import "fmt"

type Tree struct{ left, right *Tree }

func make_(depth int) *Tree {
	t := &Tree{}
	if depth > 0 {
		t.left = make_(depth - 1)
		t.right = make_(depth - 1)
	}
	return t
}

func check(t *Tree) int {
	if t.left == nil {
		return 1
	}
	return 1 + check(t.left) + check(t.right)
}

func main() {
	long := make_(22)
	sum := 0
	for i := 0; i < 20000; i++ {
		sum += check(make_(10))
	}
	fmt.Printf("short lived: %d\n", sum)
	fmt.Printf("long lived: %d\n", check(long))
}
