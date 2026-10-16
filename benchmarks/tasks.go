package main

import (
	"fmt"
	"runtime"
	"sync"
	"time"
)

func ping(n int, wg *sync.WaitGroup) {
	for i := 0; i < n; i++ {
		runtime.Gosched()
	}
	wg.Done()
}

func noop(wg *sync.WaitGroup) { wg.Done() }

func hold(wg *sync.WaitGroup) {
	time.Sleep(200 * time.Millisecond)
	wg.Done()
}

func main() {
	runtime.GOMAXPROCS(1)
	var wg sync.WaitGroup
	wg.Add(2)
	go ping(2500000, &wg)
	go ping(2500000, &wg)
	wg.Wait()
	fmt.Println("5M switches")
	for round := 0; round < 1000; round++ {
		wg.Add(1000)
		for i := 0; i < 1000; i++ {
			go noop(&wg)
		}
		wg.Wait()
	}
	fmt.Println("1M tasks")
	wg.Add(100000)
	for i := 0; i < 100000; i++ {
		go hold(&wg)
	}
	wg.Wait()
	fmt.Println("100k tasks alive at once")
}
