# Concurrency

Work runs in **tasks**, lightweight coroutines started with `co`. The design follows Go's: many cheap
tasks multiplexed by a scheduler onto threads, no function colors (any function can run as a task), and
shared memory with explicit synchronization. Today the scheduler runs all tasks on one thread; threads,
channels, and preemption are planned.

## Starting a task

```
co worker(id, &group)          # a function
co account.deposit(50)         # a method (a pointer receiver on a local keeps that variable alive)
co handlers[i](request)        # a function value
```

`co` is a statement and must be followed by a call. The function and its arguments are evaluated at once,
as with Go's `go`, and the task runs the call later; its result is discarded. Anything the task may use
after the starting function returns, such as a variable whose address was passed, moves to the heap
automatically (escape analysis treats `co` as a place values escape to).

## Scheduling

Tasks run one at a time, in the order they became ready, and switch where they block: `std.task.yield()`,
`std.task.sleep(ms)`, and waiting in `std.sync`. A task that never blocks keeps the others from running, since
nothing preempts it yet. When the main function returns the program ends, whatever the other tasks are doing.
If every task is blocked (and none is sleeping) the program stops with a deadlock error.

`std.sync.WaitGroup` waits for tasks: `add(n)` before starting them, `done()` as each finishes, `wait()` to
block until all have.

## Stacks

Every task has a stack of its own: 8 MiB of reserved address space with a guard page at the low end, which
the operating system commits a page at a time as the task uses it. A task therefore costs only the pages it
touches (one page, 4 KiB on Linux or 16 KiB on Apple silicon, for a shallow one), and one that recurses deeply
has the room a thread would. Stacks do not move or grow. `COOPER_STACK_KIB` sets the reservation.

A task that overflows its stack reaches the guard page, and the program stops with
`panic: stack overflow in task N`. Large frames are probed a page at a time so they cannot step over the guard.
Finished tasks' stacks are kept for reuse (up to 4096), so starting a task costs about 0.4 µs.

## Memory model

Tasks share memory. A data race, two tasks accessing the same variable with at least one writing and no
synchronization, is a bug the language does not prevent or detect. On multiword values (strings, arrays, sum
types) a race can tear a value, as in Go, and a torn sum type can break memory safety. Protect shared data
with the primitives in `std.sync`.

## Not yet

Running tasks on several threads, `Mutex`, `Channel T`, `select`, task groups with cancellation and error
propagation, preemption, timers beyond `sleep`, and handing a blocking C call's thread to other tasks.
