# Concurrency

Work runs in **threads**: lightweight green threads started with the `thread` expression, which the runtime
schedules onto OS threads of its own. The design follows Go's: threads are cheap enough to start by the
thousand, no function colors (any function can run as a thread), and shared memory with explicit
synchronization. `thread` is the only concurrency construct in the language; everything else is the standard
library. Today the scheduler runs all threads on one OS thread; parallelism and preemption are planned.

## Starting a thread

```
thread worker(id, &group)          # a function
thread account.deposit(50)         # a method (a pointer receiver on a local keeps that variable alive)
thread handlers[i](request)        # a function value
t := thread fetch(url)             # the value is the new thread's handle
```

`thread` must be followed by a call. The function and its arguments are evaluated at once, as with Go's `go`,
and the new thread runs the call later; the call's result is discarded. The expression's value is a `Thread`
(`struct Thread { id: i64 }`, in scope everywhere like `Maybe`), the new thread's handle; the main thread's id is
0 and others count up from 1. Anything the thread may use after the starting function returns, such as a
variable whose address was passed, moves to the heap automatically (escape analysis treats `thread` as a place
values escape to).

## Scheduling

Threads run one at a time, in the order they became ready, and switch where they block: `std.threads.yield()`,
`std.threads.sleep(ms)`, and waiting in `std.sync`. A thread that never blocks keeps the others from running, since
nothing preempts it yet. When the main function returns the program ends, whatever the other threads are doing.
If every thread is blocked (and none is sleeping) the program stops with a deadlock error.

`std.sync.WaitGroup` waits for threads: `add(n)` before starting them, `done()` as each finishes, `wait()` to
block until all have.

## Mutexes and channels

`Mutex` (`lock`, `tryLock`, `unlock`) guards data used across a blocking call. `Channel T` passes values between
threads; `channel(n)` makes one holding up to `n` values, and `channel(0)` is unbuffered (a send completes when a
receiver is waiting):

```
ch: Channel i32 = channel(4)
thread produce(&ch)
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

A channel or mutex is used through its address and must outlive the threads using it, which `&` on a local
arranges as for any escaping variable.

## Thread groups

A `ThreadGroup` owns the threads started through it, so none outlives the code that waits for the group:

```
let group: ThreadGroup
for url in urls do group.start(func() { fetch(url, &group) })
match group.wait() with {       # returns when every thread has finished
    Some(message) => println("failed: {message}")
    None => {}
}
```

`group.fail(message)` records the first failure and cancels the group; `group.cancel()` cancels without one.
Cancellation is cooperative: threads are never stopped from outside, they check `group.cancelled()` where they
can stop (a loop, between steps). A plain `thread` remains for fire-and-forget work.

Nothing checks that a group is waited for, so write `defer group.wait()` right after declaring it (or call `wait`
before every return) to keep the guarantee.

## Stacks

Every thread has a stack of its own: 8 MiB of reserved address space with a guard page at the low end, which
the operating system commits a page at a time as the thread uses it. A thread therefore costs only the pages it
touches (one page, 4 KiB on Linux or 16 KiB on Apple silicon, for a shallow one), and one that recurses deeply
has the room an OS thread would. Stacks do not move or grow. `COOPER_STACK_KIB` sets the reservation.

A thread that overflows its stack reaches the guard page, and the program stops with
`panic: stack overflow in thread N`. Large frames are probed a page at a time so they cannot step over the guard.
Finished threads' stacks are kept for reuse (up to 4096), so starting a thread costs about 0.4 µs.

## Memory model

Threads share memory. A data race, two threads accessing the same variable with at least one writing and no
synchronization, is a bug the language does not prevent or detect. On multiword values (strings, arrays, sum
types) a race can tear a value, as in Go, and a torn sum type can break memory safety. Protect shared data
with the primitives in `std.sync`.

## Not yet

Running threads in parallel on several OS threads, `RWMutex`, `Once`, atomics, events in the manner of Concurrent ML
(composable choice and cancellation, replacing the `select` above), nested groups that cancel their children,
preemption, timers beyond `sleep`, and handing a blocked C call's OS thread to other threads.
