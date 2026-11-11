/* Declarations shared by the Cooper runtime's C files. */

#ifndef COOPER_H
#define COOPER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct CooperType CooperType;

/* A sum type value whose pointer words depend on its variant: where its tag (a u32) and payload
   are in the value, and a descriptor for the payload of each variant (null: it holds no pointers). */
typedef struct {
    uint64_t tag_offset;
    uint64_t payload_offset;
    uint64_t variants;
    const CooperType *const *payloads;
} CooperSite;

/* How the collector sees a type: its size in bytes, flags, the number of 8-byte words it covers,
   one bit per word that is set when the word holds a pointer whatever variant it holds, and the
   sum type values in it (sites). */
struct CooperType {
    uint64_t size;
    uint64_t layout_size; /* what the compiler computed; it must equal `size` */
    uint64_t flags;
    uint64_t words;
    uint64_t nsites;
    const CooperSite *sites;
    uint64_t bits[];
};

/* Stop the program on a runtime error. `message` names the error and its source location. */
_Noreturn void cooper_panic(const char *message);

/* Allocate `size` zeroed bytes on the heap. `type` describes one value of the type
   allocated, and the allocation holds a whole number of them (an array's storage holds
   several); null means it holds no pointers. */
void *cooper_alloc(size_t size, const CooperType *type);

/* As cooper_alloc, with one more word after the `size` bytes, a multiple of 8 so the word is
   aligned. The collector may read the word as one of the values' pointers (tiling and size
   classes can cover it), so it may hold only zero or an address within the allocation. */
void *cooper_alloc_with_word(size_t size, const CooperType *type);

/* The scheduler (sched.c), which runs the language's threads. Start a thread running `code(env)`: a
   function taking its environment, as a closure's code does. It first runs when the current thread blocks or
   yields. Gives the thread's id. */
int64_t cooper_spawn(void *code, void *env);
void cooper_yield(void);
void cooper_sleep(int64_t nanoseconds);
void *cooper_current_thread(void);
/* Stop the current thread until cooper_ready is called on its handle. A ready that comes first leaves a
   permit, which the next park consumes by returning at once; a park may also return for no reason, so every
   caller loops on its own condition. */
void cooper_park(void);
void cooper_ready(void *thread);

/* A thread blocked in `std.sync`'s `sync`, mirroring its `Waiter` struct. `state` goes WAITING -> SYNCED
   when something commits it (a peer, a source, a timer), or WAITING -> CLAIMED -> SYNCED (or back to
   WAITING) while its own thread commits a match. Whoever wins the move to SYNCED then sets `chosen`
   (from -1) to the index of the event it commits, and readies `owner`, which waits for `chosen`. */
typedef struct {
    int64_t state;
    int64_t chosen;
    void *owner;
} CooperWaiter;
enum { COOPER_WAITING, COOPER_CLAIMED, COOPER_SYNCED };

/* At `deadline` (cooper_now), commit `waiter` to `index` if it is still waiting. */
void cooper_wake_at(CooperWaiter *waiter, int64_t index, int64_t deadline);

/* Sequentially consistent atomics on a word. cooper_add gives the new value. */
bool cooper_cas(int64_t *word, int64_t old, int64_t new_value);
int64_t cooper_load(int64_t *word);
void cooper_store(int64_t *word, int64_t value);
int64_t cooper_add(int64_t *word, int64_t delta);
/* A spin lock on a word (0 free, 1 held). It is never held across a park or a yield. */
void cooper_lock(int64_t *word);
void cooper_unlock(int64_t *word);
/* A pseudo-random number in [0, n), for n > 0. */
int64_t cooper_random(int64_t n);
/* Monotonic time in nanoseconds. */
int64_t cooper_now(void);
/* Set up the scheduler. */
void cooper_sched_init(void);
/* The number of Ps (at most COOPER_MAX_PROCS), and the index in [0, count) of the running thread's P (0 before
   the scheduler runs). A thread keeps its P until it blocks, yields, reaches a safe point or calls C, so the
   index is stable across an allocation. */
#define COOPER_MAX_PROCS 256
int cooper_proc_count(void);
int cooper_proc_id(void);
/* Run `entry` as the main thread (thread 0) on a stack of its own, and the scheduler on the caller's stack.
   The process exits with `entry`'s result when it returns. */
_Noreturn void cooper_run_main(int (*entry)(void));
/* For the collector, with the world stopped. Record the running thread's stack pointer, so that its stack is
   scanned from there up like a stopped thread's. Scan the threads, their stacks, and the scheduler's timers
   that no other caller has claimed for `cycle` (a number the collector chooses, new for each scan of all).
   Run `helper` on an M for each P other than the caller's: the Ms stopped at safe points and idle Ms, started
   if need be. A helper may also run late, after the world restarts. */
void cooper_save_stack_pointer(void *sp);
void cooper_scan_threads(void (*scan)(void *context, uintptr_t low, uintptr_t high), void *context, uint64_t cycle);
void cooper_start_helpers(void (*helper)(void));
/* Marking while the program runs, for the collector. Between cooper_mark_begin and cooper_mark_end, both called with
   the world stopped, the scheduler has every thread's stack scanned once for `cycle` (which cooper_scan_threads
   then counts as claimed), by `scan`, which scans a range of possible pointers with the running P's mark worker:
   no thread runs Cooper code before its stack is scanned. `scan` also covers what a thread started or a timer set
   meanwhile holds, and the timers, when marking begins. An M looking for a thread to run first calls
   `worker_mark(stop)`, which marks as the P's dedicated or fractional mark worker if it is one; an M with nothing to
   run calls `idle_mark(stop)` before it idles its P. Both mark until `stop()` is true (the world is stopping, or for
   an idle P, a thread is ready) or their share is done, and give whether they did any work. As marking begins, the
   scheduler has up to `dedicated` Ps leave the threads they run, idle ones first, and while marking, the monitor
   has a P that runs a thread leave it when `worker_due(p)` says the P should mark.
   cooper_scan_waiting_threads scans the stacks of threads that are not running and are not yet scanned, and gives
   whether some may still need it. */
typedef struct {
    void (*scan)(uintptr_t low, uintptr_t high);
    bool (*worker_mark)(bool (*stop)(void));
    bool (*idle_mark)(bool (*stop)(void));
    bool (*worker_due)(int p);
    int dedicated;
} CooperMarkHooks;
void cooper_mark_begin(uint64_t cycle, const CooperMarkHooks *hooks);
void cooper_mark_end(void);
bool cooper_scan_waiting_threads(void);

/* Safe points. Generated code polls cooper_poll_word at function entry and loop back-edges, and calls
   cooper_poll_slow when it is nonzero, which stops the running thread while the world is stopped, and switches
   to another thread when the monitor asks it to (preemption). */
extern int64_t cooper_poll_word;
void cooper_poll_slow(void);
/* Stop every other M holding a P at a safe point, the running thread's keeps running. If another thread got
   there first, this one stops at a safe point instead until the world restarts, and the result is false. */
bool cooper_stop_the_world(void);
void cooper_start_the_world(void);
/* Whether a stop of the world was asked for (so the running thread should reach a safe point soon). Unlike a
   nonzero cooper_poll_word, it leaves out preemption, which a thread holding a lock word does not honour. */
bool cooper_world_stopping(void);

/* A call to a C function the program declares runs between cooper_enter_c, which records the thread's stack
   pointer and callee-saved registers (so the collector scans it as stopped) and lets the monitor hand its P to
   another M if the call blocks, and cooper_exit_c, given what cooper_enter_c returned, which takes a P back,
   waiting for one if need be. A C callback into Cooper code brackets itself with cooper_callback_enter and
   cooper_callback_leave, which keep the C call's state in `saved` (COOPER_CALLBACK_WORDS words) meanwhile. */
int cooper_enter_c(void);
void cooper_exit_c(int entered);
#define COOPER_CALLBACK_WORDS 13
int cooper_callback_enter(uintptr_t *saved);
void cooper_callback_leave(int entered, uintptr_t *saved);
/* Between these, the running thread neither switches nor enters the C state: it stays on its M (it may stop
   for the collector). For state kept per M, such as formatting's. */
void cooper_acquire_m(void);
void cooper_release_m(void);

/* The write barrier (gc.c): while cooper_barrier_on is set, the stores of pointer words outside the storing
   function's frame shade the old and the new pointers. Before such a store, generated code passes the value
   overwritten and the one stored to cooper_barrier_pointer for a pointer, and to cooper_barrier_typed, as copies
   in memory, for a value of `type`. cooper_typed_copy copies `bytes` bytes of values of `type` (null: they hold no pointers), with the
   barrier, between memory that does not overlap; it is a safe point. C uses cooper_store_pointer (cooper_ffi.h). */
extern int32_t cooper_barrier_on;
void cooper_barrier_pointer(void *old, void *value);
void cooper_barrier_typed(const void *old, const void *value, const CooperType *type);
void cooper_typed_copy(void *dst, const void *src, size_t bytes, const CooperType *type);
#include "cooper_ffi.h"

/* Start collecting: allocation may collect from now on, and the caller is on a thread of the scheduler. */
void cooper_gc_init(void);

/* Collect garbage now. */
void cooper_gc_collect(void);

#endif
