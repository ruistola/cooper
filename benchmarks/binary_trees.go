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
	maxDepth := 18
	long := make_(maxDepth)
	for d := 4; d <= maxDepth; d += 4 {
		iters := 1 << (maxDepth - d + 4)
		sum := 0
		for i := 0; i < iters; i++ {
			sum += check(make_(d))
		}
		fmt.Printf("%d trees of depth %d: %d\n", iters, d, sum)
	}
	fmt.Printf("long lived: %d\n", check(long))
}
