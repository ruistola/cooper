package main

import (
	"fmt"
	"strings"
)

func main() {
	total := 0
	for round := 0; round < 60; round++ {
		var parts []string
		for i := 0; i < 20000; i++ {
			parts = append(parts, fmt.Sprintf("item %d of round %d", i, round))
		}
		text := strings.Join(parts, ",")
		total += len(text)
		pieces := strings.Split(text, ",")
		total += len(pieces)
	}
	fmt.Println(total)
}
