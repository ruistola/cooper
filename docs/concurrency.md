# Concurrency

Work runs in **threads**: lightweight green threads started with the `thread` expression, which the runtime
schedules onto OS threads of its own. The design follows Go's: threads are cheap enough to start by the
thousand, no function colors (any function can run as a thread), and shared memory with explicit
synchronization. `thread` is the only concurrency construct in the language; everything else is the standard
library. Threads run in parallel, one OS thread per core by default.

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

The runtime follows Go's structure. A processor (P) is the right to run threads; there are `COOPER_THREADS` of
them, by default one per core. Each P owns a run queue of 256 slots (overflowing to a global queue), and a machine
(M, an OS thread) runs threads while it holds a P. Each M has a scheduler on the OS thread's own stack: a thread
that blocks, yields, sleeps or finishes switches to it, and it completes the request and switches to the next
thread. The main thread is thread 0 and runs on a stack like the others.

A P runs a thread that was just readied next (so a reply is handled at once), then its queue in the order threads
became ready, then the global queue; an M whose P has nothing steals half of another P's queue. Ms start as work
appears: making a thread runnable wakes an idle M only when a P is free and no M is already looking for work, and
an M that finds nothing sleeps. A thread switches where it blocks: `std.threads.yield()`, `std.threads.sleep(ms)`,
and waiting in `std.sync`. When the main function returns the program ends, whatever the other threads are doing.
If every thread is blocked (and none is sleeping, waiting on `after`, or in a C call) the program stops with a
deadlock error.

Generated code checks a flag at each function entry and loop iteration (a safe point), as do the allocator and
the scheduler. The collector raises it to stop every thread before it collects: a thread that sees it pauses
until the collection is over, and its OS thread helps mark meanwhile. A monitor, an OS thread of the runtime's
own, raises it to preempt: a thread that has run for 10 ms while other threads wait (in a queue, or for a timer
that is due) switches at its next safe point and goes to the back of the global queue. Preemption waits while
the thread is inside one of the runtime's short critical sections (the lock words inside `std.sync`, formatting
a value), and code that runs without reaching a safe point, such as a long C call, is not preempted.

A call to a C function (`extern func`) may block. The thread's OS thread then holds no processor: the call counts
as stopped for the collector, and when it lasts (the monitor looks every 20 µs while calls are in progress,
backing off to every 10 ms when there are none) the monitor gives its processor to another OS thread, so the
other threads keep running even with `COOPER_THREADS=1`. When the call returns the thread takes a processor
back, waiting for one if all are busy. A callback from C into Cooper code holds a processor while it runs. A C
call made inside one of the runtime's critical sections keeps its processor: `print` writes under a lock, so
writing to a full pipe holds one. Each C call costs about 10 ns for this; the runtime's own functions are called
directly.

`std.sync.WaitGroup` waits for threads: `add(n)` before starting them, `done()` as each finishes, `wait()` to
block until all have.

## Mutexes and channels

`Mutex` (`lock`, `tryLock`, `unlock`) guards data that threads share. `Atomic` is an integer threads update
without a lock: `load`, `store`, `add` (giving the new value) and `compareAndSwap(old, new)`. `Channel T` passes
values between threads; `channel(n)` makes one holding up to `n` values, and `channel(0)` is unbuffered: a send and a receive
complete together.

```
let ch: Channel i32 = channel(4)
thread produce(&ch)
match ch.recv() with {          # blocks; None once closed and empty
    Some(v) => use(v)
    None => done()
}
ch.send(5)                      # blocks while full; sending on a closed channel is fatal
ch.close()
```

A channel or mutex is used through its address and must outlive the threads using it, which `&` on a local
arranges as for any escaping variable.

## Events

Communication follows Concurrent ML: an operation is a value, an `Event T` whose result is a `T`, and nothing
happens until a thread calls `sync` on it. A blocking operation `x()` is `sync(xEvent())`: `ch.recv()` is
`sync(ch.recvEvent())`, and `ch.sendEvent(v)`, `wg.waitEvent()`, `group.waitEvent()` and
`group.cancelledEvent()` are the others. Because events are values, a function can return one and its caller
can combine it with others, which Go's `select` statement cannot do.

| Function | Event |
|---|---|
| `sync(e)` | blocks until one operation in `e` commits, and gives its result |
| `choose(e1, e2, ...)` | whichever commits first; one at random among those that can commit at once |
| `wrap(e, f)` | `e`, with `f` applied to its result after it commits, in the syncing thread |
| `guard(f)` | the event `f()` returns, with `f` run afresh at each `sync` |
| `withNack(f)` | as `guard`, but `f` also gets an event that commits if the sync chooses something else |
| `after(ms)` | commits `ms` milliseconds after the `sync` begins |
| `always(v)`, `never()` | commits at once with `v`; never commits |

```
reply := sync(choose(
    wrap(results.recvEvent(), func(r: Maybe i32): string { return "got {r}" }),
    wrap(after(500), func(late: ()): string { return "gave up" }),
))
```

Exactly one of the operations in a `sync` commits: when a send on an unbuffered channel is chosen, a receive
was chosen in some other thread's sync at the same moment, and a send that times out leaves nothing behind.
`after(ms)` counts from each `sync`, so one value serves every pass of a loop.

`withNack` is how an operation learns it was abandoned. In a request to a server, the setup function sends
the request along with the event it is given, and returns the event of receiving the reply. If the caller's
sync chooses something else (a timeout, say), that event commits, and the server, which offers its reply in a
`choose` with it, moves on instead of waiting for a client that is gone. The event `f` is given is created afresh
at each sync, and never commits if the branch is chosen.

A `Latch` is a one-shot broadcast: `release()`, `released()`, and `releasedEvent()`, which commits once it is
released.

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
can stop (a loop, between steps), or include `group.cancelledEvent()` in what they wait for:

```
stopped := wrap(group.cancelledEvent(), func(c: ()): Maybe Job { return None })
while sync(choose(jobs.recvEvent(), stopped)) is Some(job) do handle(job)
```

A plain `thread` remains for fire-and-forget work.

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
with the primitives in `std.sync`. Each `print` and `println` writes its text whole, so lines from different
threads never mix.

## Not yet

`RWMutex`, `Once`, an event form of `Mutex.lock`, nested groups that cancel their children, and timers beyond
`sleep` and `after`. A thread blocked forever on an event nothing can commit stays blocked; only when every thread
is blocked does the program stop with the deadlock error.
