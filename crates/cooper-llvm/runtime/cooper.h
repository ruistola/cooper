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
/* Run `entry` as the main thread (thread 0) on a stack of its own, and the scheduler on the caller's stack.
   The process exits with `entry`'s result when it returns. */
_Noreturn void cooper_run_main(int (*entry)(void));
/* For the collector: the top of the running thread's stack, and the other threads' stacks. */
uintptr_t cooper_current_stack_top(void);
void cooper_scan_threads(void (*scan)(uintptr_t low, uintptr_t high));

/* Safe points. Generated code polls cooper_poll_word at function entry and loop back-edges, and calls
   cooper_poll_slow when it is nonzero, which stops the running thread while the world is stopped. */
extern int64_t cooper_poll_word;
void cooper_poll_slow(void);
/* Stop every other M holding a P at a safe point, the running thread's keeps running. If another thread got
   there first, this one stops at a safe point instead until the world restarts, and the result is false. */
bool cooper_stop_the_world(void);
void cooper_start_the_world(void);

/* Start collecting: allocation may collect from now on, and the caller is on a thread of the scheduler. */
void cooper_gc_init(void);

/* Collect garbage now. */
void cooper_gc_collect(void);

#endif
