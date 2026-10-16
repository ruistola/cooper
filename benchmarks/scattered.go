package main

import "fmt"

type Node struct {
	a, b *Node
	id   int64
}

var all []*Node

func main() {
	n := 3000000
	for i := 0; i < n; i++ {
		all = append(all, &Node{id: int64(i)})
	}
	var seed uint64 = 12345
	for i := 0; i < n; i++ {
		seed ^= seed << 13
		seed ^= seed >> 7
		seed ^= seed << 17
		all[i].a = all[seed%uint64(n)]
		seed ^= seed << 13
		seed ^= seed >> 7
		seed ^= seed << 17
		all[i].b = all[seed%uint64(n)]
	}
	head := all[0]
	all = nil
	var sum int64
	for round := 0; round < 400; round++ {
		var junk []*Node
		for i := 0; i < 300000; i++ {
			junk = append(junk, &Node{id: int64(i)})
		}
		sum += int64(len(junk))
	}
	fmt.Println(sum, head.id)
}
