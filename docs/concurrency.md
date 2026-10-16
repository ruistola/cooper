# Concurrency

Work runs in **tasks**, lightweight coroutines started with `co`. The design follows Go's: many cheap
tasks multiplexed by a scheduler onto threads, no function colors (any function can run as a task), and
shared memory with explicit synchronization. Today the scheduler runs all tasks on one thread; threads
and preemption are planned.

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

## Mutexes and channels

`Mutex` (`lock`, `try_lock`, `unlock`) guards data used across a blocking call. `Channel T` passes values between
tasks; `channel(n)` makes one holding up to `n` values, and `channel(0)` is unbuffered (a send completes when a
receiver is waiting):

```
ch: Channel i32 = channel(4)
co produce(&ch)
match ch.recv() with {          # blocks; None once closed and empty
    Some(v) => use(v)
    None => done()
}
ch.send(5)                      # blocks while full; sending on a closed channel is fatal
ch.close()
```

`select` waits for whichever of its cases can complete first (the earliest listed wins a tie) and runs that
case's function. The cases are `recv(&ch, f)`, `send(&ch, value, f)` and `timeout(ms, f)`:

```
select(
    recv(&results, func(r: Maybe i32) { handle(r) }),
    timeout(500, func() { println("gave up") }),
)
```

A channel or mutex is used through its address and must outlive the tasks using it, which `&` on a local
arranges as for any escaping variable.

## Task groups

A `TaskGroup` owns the tasks started through it, so none outlives the code that waits for the group:

```
let group: TaskGroup
for url in urls do group.start(func() { fetch(url, &group) })
match group.wait() with {       # returns when every task has finished
    Some(message) => println("failed: {message}")
    None => {}
}
```

`group.fail(message)` records the first failure and cancels the group; `group.cancel()` cancels without one.
Cancellation is cooperative: tasks are never stopped from outside, they check `group.cancelled()` where they
can stop (a loop, between steps). Plain `co` remains for fire-and-forget work.

Nothing checks that a group is waited for, so write `defer group.wait()` right after declaring it (or call `wait`
before every return) to keep the guarantee.

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

Running tasks on several threads, `RWMutex`, `Once`, atomics, nested groups that cancel their children, preemption, timers beyond `sleep`, and handing a blocking C call's thread to other tasks.
