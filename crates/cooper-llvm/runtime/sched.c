/*
 * The Cooper scheduler: the language's threads, started with the `thread` expression, run in parallel on
 * several OS threads. A thread runs until it blocks (park, sleep) or yields; nothing preempts it yet, so a
 * thread that never blocks keeps its OS thread to itself.
 *
 * The structure is Go's. A P (processor) is the right to run threads: COOPER_THREADS of them (by default one
 * per core), each with a local run queue. An M (machine, an OS thread) runs threads while it holds a P. Each M
 * has a scheduler that runs on the OS thread's own stack: a thread that blocks, yields, sleeps or finishes
 * switches to it, it completes the request (marks the thread parked, arms the timer, frees the stack) and
 * switches to the next thread. Nothing is ever done to a thread on its own stack after it has been given up.
 * The main thread is thread 0 and runs on a stack like any other.
 *
 * Finding work: a readied thread runs next on the P that readied it; then come the local queue (the global
 * queue first on every 61st pick, so it is not starved), the global queue, and stealing half of another P's
 * queue. An M that finds nothing gives its P back and sleeps on a condition variable. Ms start lazily: making
 * a thread runnable wakes an M (or starts one) only if a P is idle and no M is already looking for work.
 *
 * Each thread has a stack of its own: a large virtual reservation (8 MiB by default, COOPER_STACK_KIB)
 * with a guard page at its low end. The operating system commits a page only when the thread first
 * touches it, so a thread costs the pages it uses, and one that recurses deeply has the room an OS
 * thread would. A thread that overflows hits the guard page, and a signal handler reports it. Stacks never move,
 * so pointers into them stay valid. Finished threads are kept with their stacks for reuse.
 *
 * The collector stops the world: every M holding a P stops at a safe point (contract 6 of the plan: compiler
 * polls of cooper_poll_word, the allocator, lock spins, the scheduler loop). A thread stopped at a safe point
 * stays on its M with its callee-saved registers spilled and its stack pointer recorded, so the collector
 * scans every thread's stack the same way, from its saved stack pointer to its top.
 *
 * The primitives `std.sync` is built on live here too: park with a permit, atomics and lock words, timers
 * that wake a waiter record (CooperWaiter, in cooper.h), and a random source.
 */

#define _DARWIN_C_SOURCE
#define _GNU_SOURCE
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include "cooper.h"

#ifndef MAP_ANON
#define MAP_ANON MAP_ANONYMOUS
#endif
#ifndef MAP_NORESERVE
#define MAP_NORESERVE 0
#endif

/* Under ThreadSanitizer (the C tests), each thread's stack is a fiber it is told about at every switch. */
#if defined(__has_feature)
#if __has_feature(thread_sanitizer)
void *__tsan_get_current_fiber(void);
void *__tsan_create_fiber(unsigned flags);
void __tsan_switch_to_fiber(void *fiber, unsigned flags);
#define TSAN_FIBER(fiber) (fiber = fiber ? fiber : __tsan_create_fiber(0))
#define TSAN_CURRENT(fiber) (fiber = __tsan_get_current_fiber())
#define TSAN_SWITCH(fiber) __tsan_switch_to_fiber(fiber, 0)
#endif
#endif
#ifndef TSAN_SWITCH
#define TSAN_FIBER(fiber) ((void)0)
#define TSAN_CURRENT(fiber) ((void)0)
#define TSAN_SWITCH(fiber) ((void)0)
#endif

#define LOAD(p) __atomic_load_n((p), __ATOMIC_SEQ_CST)
#define STORE(p, v) __atomic_store_n((p), (v), __ATOMIC_SEQ_CST)
#define ADD(p, v) __atomic_add_fetch((p), (v), __ATOMIC_SEQ_CST)

/* Thread states (Thread.state, the low byte; PERMIT is a flag on top of any of them). */
enum { RUNNING, RUNNABLE, PARKING, PARKED, SLEEPING, DEAD };
#define STATE_MASK 0xff
#define PERMIT 0x100          /* a cooper_ready that came while the thread was not parked */

typedef struct Thread {
    void *sp;                 /* the saved stack pointer, while the thread is not running */
    void *code, *env;         /* what to run: a function taking its environment (adjacent: scanned together) */
    struct Thread *next;      /* the global run queue or a free list */
    struct Thread *all_next;  /* every thread record ever made; set once */
    uint64_t id;
    uintptr_t stack_low, stack_top; /* 0 when a finished thread's stack was returned to the system */
    int64_t state;            /* atomic: a state, with PERMIT possibly set */
    void *fiber;              /* ThreadSanitizer's */
} Thread;

/* A timer: a sleeping thread to resume, or a waiter to commit to its event `index`, when `wake` (nanoseconds
   on the monotonic clock) is reached. */
typedef struct {
    int64_t wake;
    uint64_t order;           /* arrival order, to break ties */
    Thread *thread;
    CooperWaiter *waiter;
    int64_t index;
} Timer;

#define RUNQ_SIZE 256
/* Finished threads kept with their stacks, in all. Each keeps the pages it touched, a page or more, so this
   bounds what idle stacks hold; a burst of this many threads costs no mapping at all. */
#define MAX_POOLED 4096
/* A P keeps up to this many finished threads to itself, and shares the rest through the global list. */
#define LOCAL_FREE 64
/* Readied threads that run next, one after another, before the local queue gets a turn: two threads that
   ready each other must not starve the rest. */
#define RUNNEXT_TURNS 16

enum { P_IDLE, P_RUNNING, P_STOPPED };

typedef struct M M;

/* A processor: the right to run threads, and the queues and caches that go with it. */
typedef struct P {
    /* The local run queue, a ring: only the owner adds (at tail); the owner and thieves take (CAS on head). */
    Thread *queue[RUNQ_SIZE];
    uint32_t head, tail;      /* atomic; tail - head entries, counted without wrapping */
    Thread *runnext;          /* atomic: a readied thread, to run before the queue */
    uint32_t schedtick;       /* threads picked; every 61st looks at the global queue first */
    int runnext_turns;
    Thread *free;             /* finished threads, with their stacks */
    int nfree;
    uint64_t random_state;    /* xorshift64 */
    /* Under sched.lock: */
    int status;
    M *m;                     /* the M holding it, while not idle */
    struct P *idle_next;
} P;

/* What a thread asks the scheduler to do with it once it has switched away. */
enum { ACT_YIELD, ACT_PARK, ACT_SLEEP, ACT_DEAD };

/* A machine: an OS thread. Its scheduler runs on the OS thread's own stack, and every thread switches to it
   and from it (a thread never switches directly to another), so that what must happen after a thread has
   stopped running (completing a park, arming a timer, freeing a dead thread's stack) happens while nothing
   else can resume that thread. */
struct M {
    void *sched_sp;           /* the scheduler's saved stack pointer, while a thread runs */
    Thread *current;          /* the running thread; null while the scheduler runs */
    P *p;
    int locks;                /* lock words held by the running thread (cooper_lock) */
    int action;               /* the running thread's request, read by the scheduler after the switch */
    int64_t action_wake;      /* ACT_SLEEP: when to resume */
    int spinning;             /* looking for work to steal, counted in sched.nmspinning */
    /* Under sched.lock: */
    P *nextp;                 /* a P handed over to this sleeping M */
    M *idle_next;
    pthread_cond_t wake;
    void *fiber;              /* ThreadSanitizer's, for the scheduler */
};

/* Every global of the scheduler is classified here by who may touch it.
     per-M:      this_m (thread-local) and what it points to, the signal alternate stack
     per-P:      the fields of P not marked otherwise
     immutable after cooper_sched_init: nprocs, allp, stack_size, guard_size, page_size
     atomic:     next_id, all_threads, pooled, cooper_poll_word, and the fields of sched so marked
     sched.lock: the rest of sched, P.status, P.m, P.idle_next, M.nextp, M.idle_next */
static struct {
    pthread_mutex_t lock;
    Thread *global_head, *global_tail;
    int64_t global_count;     /* atomic (written under the lock): threads in the global queue */
    Thread *free;             /* finished threads no P keeps (read without the lock to see if it is empty) */
    P *idle_ps;
    int32_t npidle;           /* atomic (written under the lock) */
    int32_t nmspinning;       /* atomic */
    M *idle_ms;
    /* A binary min-heap by wake time (ties by arrival, so equal sleeps wake in order). */
    Timer *timers;
    size_t timer_count, timer_capacity;
    uint64_t timer_sequence;
    int64_t timer_next;       /* atomic (written under the lock): the earliest wake, INT64_MAX if none */
    M *timer_m;               /* the sleeping M that wakes for the earliest timer, and when it wakes */
    int64_t timer_m_until;
    /* Stopping the world. */
    int gcwaiting;
    int stopwait;             /* Ps yet to stop */
    uint64_t restarts;
    pthread_cond_t stopped, restart;
} sched = {.lock = PTHREAD_MUTEX_INITIALIZER, .timer_next = INT64_MAX};

static int nprocs = 1;
static P *allp;
static Thread main_thread;
static M m0;
static _Thread_local M *this_m;
static Thread *all_threads;
static uint64_t next_id = 1;
static int64_t pooled;        /* finished threads holding a stack */
static size_t stack_size = (size_t)8 << 20, guard_size, page_size;
int64_t cooper_poll_word;

/* The running M. A thread may resume on another M after any switch, so code on a thread's stack reads this
   afresh after each one; the call keeps the compiler from keeping the thread-local's address across it. */
static __attribute__((noinline)) M *get_m(void) {
    __asm__ volatile("" ::: "memory");
    return this_m;
}

static void cpu_relax(void) {
#if defined(__aarch64__)
    __asm__ volatile("yield");
#elif defined(__x86_64__)
    __asm__ volatile("pause");
#endif
}

/* --- context switch --- */

void cooper_switch(void **save_sp, void *new_sp);

#ifdef __APPLE__
#define SYM(name) "_" name
#else
#define SYM(name) name
#endif

#if defined(__aarch64__)
__asm__(
    ".text\n.p2align 2\n.globl " SYM("cooper_switch") "\n" SYM("cooper_switch") ":\n"
    "    stp x19, x20, [sp, #-160]!\n"
    "    stp x21, x22, [sp, #16]\n"
    "    stp x23, x24, [sp, #32]\n"
    "    stp x25, x26, [sp, #48]\n"
    "    stp x27, x28, [sp, #64]\n"
    "    stp x29, x30, [sp, #80]\n"
    "    stp d8, d9, [sp, #96]\n"
    "    stp d10, d11, [sp, #112]\n"
    "    stp d12, d13, [sp, #128]\n"
    "    stp d14, d15, [sp, #144]\n"
    "    mov x2, sp\n"
    "    str x2, [x0]\n"
    "    mov sp, x1\n"
    "    ldp d14, d15, [sp, #144]\n"
    "    ldp d12, d13, [sp, #128]\n"
    "    ldp d10, d11, [sp, #112]\n"
    "    ldp d8, d9, [sp, #96]\n"
    "    ldp x29, x30, [sp, #80]\n"
    "    ldp x27, x28, [sp, #64]\n"
    "    ldp x25, x26, [sp, #48]\n"
    "    ldp x23, x24, [sp, #32]\n"
    "    ldp x21, x22, [sp, #16]\n"
    "    ldp x19, x20, [sp], #160\n"
    "    ret\n");
#define FRAME_BYTES 160
#define FRAME_ENTRY_OFFSET 88
#elif defined(__x86_64__)
__asm__(
    ".text\n.globl " SYM("cooper_switch") "\n" SYM("cooper_switch") ":\n"
    "    pushq %rbp\n    pushq %rbx\n    pushq %r12\n    pushq %r13\n    pushq %r14\n    pushq %r15\n"
    "    movq %rsp, (%rdi)\n"
    "    movq %rsi, %rsp\n"
    "    popq %r15\n    popq %r14\n    popq %r13\n    popq %r12\n    popq %rbx\n    popq %rbp\n"
    "    retq\n");
/* Six saved registers and the entry address; the entry then finds the stack as after a call. */
#define FRAME_BYTES 64
#define FRAME_ENTRY_OFFSET 48
#else
#error "no context switch for this architecture"
#endif

/* --- time --- */

static int64_t now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (int64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}

/* Wait on `cond` (with sched.lock held) until `deadline` on the monotonic clock, or a signal. */
static void wait_until(pthread_cond_t *cond, int64_t deadline) {
    int64_t wait = deadline - now_ns();
    if (wait <= 0) return;
#ifdef __APPLE__
    struct timespec t = {wait / 1000000000, wait % 1000000000};
    pthread_cond_timedwait_relative_np(cond, &sched.lock, &t);
#else
    struct timespec t = {deadline / 1000000000, deadline % 1000000000};
    pthread_cond_timedwait(cond, &sched.lock, &t);
#endif
}

static void cond_init(pthread_cond_t *cond) {
    pthread_condattr_t attr;
    pthread_condattr_init(&attr);
#ifndef __APPLE__
    pthread_condattr_setclock(&attr, CLOCK_MONOTONIC);
#endif
    pthread_cond_init(cond, &attr);
    pthread_condattr_destroy(&attr);
}

/* --- thread states --- */

/* Move `thread` to `state`, keeping a pending permit. */
static void set_state(Thread *thread, int64_t state) {
    int64_t old = LOAD(&thread->state);
    while (!__atomic_compare_exchange_n(&thread->state, &old, (old & PERMIT) | state, false, __ATOMIC_SEQ_CST,
                                        __ATOMIC_SEQ_CST)) {
    }
}

/* --- run queues --- */

static void global_put(Thread *thread) {
    pthread_mutex_lock(&sched.lock);
    thread->next = NULL;
    if (sched.global_tail) sched.global_tail->next = thread;
    else sched.global_head = thread;
    sched.global_tail = thread;
    STORE(&sched.global_count, sched.global_count + 1);
    pthread_mutex_unlock(&sched.lock);
}

/* Queue a runnable thread on `p`, which the running M holds: as the next to run, or at the tail of the local
   queue (the global queue's, if that is full). A thread displaced from runnext goes to the tail. */
static void runq_put(P *p, Thread *thread, bool next) {
    if (next) {
        thread = __atomic_exchange_n(&p->runnext, thread, __ATOMIC_SEQ_CST);
        if (!thread) return;
    }
    uint32_t head = __atomic_load_n(&p->head, __ATOMIC_ACQUIRE);
    uint32_t tail = p->tail;
    if (tail - head < RUNQ_SIZE) {
        __atomic_store_n(&p->queue[tail % RUNQ_SIZE], thread, __ATOMIC_RELAXED);
        __atomic_store_n(&p->tail, tail + 1, __ATOMIC_SEQ_CST);
        return;
    }
    global_put(thread);
}

static Thread *runq_pop(P *p) {
    for (;;) {
        uint32_t head = __atomic_load_n(&p->head, __ATOMIC_ACQUIRE);
        if (head == p->tail) return NULL;
        Thread *thread = __atomic_load_n(&p->queue[head % RUNQ_SIZE], __ATOMIC_RELAXED);
        if (__atomic_compare_exchange_n(&p->head, &head, head + 1, false, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED)) {
            return thread;
        }
    }
}

static bool runq_has_work(P *p) {
    return LOAD(&p->head) != LOAD(&p->tail) || LOAD(&p->runnext);
}

/* Take up to `max` threads from the global queue, with sched.lock held: the first to run, the rest onto `p`'s
   local queue, which must have room for them. */
static Thread *global_get(P *p, int64_t max) {
    int64_t n = sched.global_count;
    if (n == 0) return NULL;
    int64_t take = n / nprocs + 1;
    if (take > n) take = n;
    if (take > max) take = max;
    Thread *first = sched.global_head;
    Thread *thread = first;
    for (int64_t i = 0; i < take; i++) {
        Thread *next = thread->next;
        if (i > 0) runq_put(p, thread, false);
        thread = next;
    }
    sched.global_head = thread;
    if (!thread) sched.global_tail = NULL;
    STORE(&sched.global_count, n - take);
    return first;
}

/* Steal half of `victim`'s queue (or, if `with_next` and the queue is empty, its next thread) onto `p`'s empty
   local queue, returning one of them to run. */
static Thread *runq_steal(P *p, P *victim, bool with_next) {
    Thread *batch[RUNQ_SIZE / 2];
    uint32_t n;
    for (;;) {
        uint32_t head = __atomic_load_n(&victim->head, __ATOMIC_ACQUIRE);
        uint32_t tail = __atomic_load_n(&victim->tail, __ATOMIC_ACQUIRE);
        n = tail - head;
        n -= n / 2;
        if (n == 0) {
            if (!with_next) return NULL;
            Thread *next = LOAD(&victim->runnext);
            if (next && __atomic_compare_exchange_n(&victim->runnext, &next, NULL, false, __ATOMIC_SEQ_CST,
                                                    __ATOMIC_RELAXED)) {
                return next;
            }
            return NULL;
        }
        if (n > RUNQ_SIZE / 2) continue; /* head and tail read at different moments */
        for (uint32_t i = 0; i < n; i++) batch[i] = __atomic_load_n(&victim->queue[(head + i) % RUNQ_SIZE], __ATOMIC_RELAXED);
        if (__atomic_compare_exchange_n(&victim->head, &head, head + n, false, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED)) {
            break;
        }
    }
    for (uint32_t i = 1; i < n; i++) runq_put(p, batch[i], false);
    return batch[0];
}

/* --- Ps and Ms --- */

static uint64_t random_next(P *p) {
    uint64_t *state = &p->random_state;
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    return *state;
}

/* With sched.lock held. */
static void p_put_idle(P *p) {
    p->status = P_IDLE;
    p->m = NULL;
    p->idle_next = sched.idle_ps;
    sched.idle_ps = p;
    STORE(&sched.npidle, sched.npidle + 1);
}

static P *p_get_idle(void) {
    P *p = sched.idle_ps;
    if (!p) return NULL;
    sched.idle_ps = p->idle_next;
    p->status = P_RUNNING;
    STORE(&sched.npidle, sched.npidle - 1);
    return p;
}

static void m_remove_idle(M *m) {
    for (M **link = &sched.idle_ms; *link; link = &(*link)->idle_next) {
        if (*link == m) {
            *link = m->idle_next;
            return;
        }
    }
}

static _Noreturn void scheduler(M *m);

/* Per M: the stack the fault handler runs on, since the faulting thread's own stack is exhausted. */
static void install_alternate_stack(void) {
    static char m0_stack[1 << 16];
    stack_t altstack = {.ss_sp = m0_stack, .ss_size = sizeof m0_stack, .ss_flags = 0};
    if (this_m != &m0) {
        altstack.ss_sp = malloc(altstack.ss_size);
        if (!altstack.ss_sp) cooper_panic("out of memory");
    }
    sigaltstack(&altstack, NULL);
}

static void *m_main(void *arg) {
    M *m = arg;
    this_m = m;
    install_alternate_stack();
    pthread_mutex_lock(&sched.lock);
    m->p = m->nextp;
    m->nextp = NULL;
    m->p->m = m;
    pthread_mutex_unlock(&sched.lock);
    scheduler(m);
}

/* Run `p` (an idle one if null) on a sleeping M, or a new one. A spinning M starts out looking for work to
   steal, already counted in sched.nmspinning. */
static void startm(P *p, bool spinning) {
    pthread_mutex_lock(&sched.lock);
    if (!p) p = p_get_idle();
    if (!p) {
        pthread_mutex_unlock(&sched.lock);
        if (spinning) ADD(&sched.nmspinning, -1);
        return;
    }
    M *m = sched.idle_ms;
    if (m) {
        sched.idle_ms = m->idle_next;
        m->nextp = p;
        m->spinning = spinning;
        pthread_cond_signal(&m->wake);
        pthread_mutex_unlock(&sched.lock);
        return;
    }
    m = calloc(1, sizeof *m);
    if (!m) cooper_panic("out of memory");
    cond_init(&m->wake);
    m->nextp = p;
    m->spinning = spinning;
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    pthread_t os_thread;
    if (pthread_create(&os_thread, &attr, m_main, m) != 0) cooper_panic("cannot start an OS thread");
    pthread_attr_destroy(&attr);
    pthread_mutex_unlock(&sched.lock);
}

/* Work was made runnable: if a P is idle and no M is looking for work, start one looking. */
static void wakep(void) {
    if (LOAD(&sched.npidle) == 0 || LOAD(&sched.nmspinning) != 0) return;
    int32_t zero = 0;
    if (!__atomic_compare_exchange_n(&sched.nmspinning, &zero, 1, false, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED)) return;
    startm(NULL, true);
}

/* --- stopping the world --- */

/* Stop `m`, which holds a P, until the world restarts. */
static void gc_park(M *m) {
    pthread_mutex_lock(&sched.lock);
    if (sched.gcwaiting) {
        m->p->status = P_STOPPED;
        if (--sched.stopwait == 0) pthread_cond_signal(&sched.stopped);
        /* Not `while (gcwaiting)`: the next stop may begin before this M wakes, and it must stop for that one
           again, counted. */
        uint64_t restarts = sched.restarts;
        while (sched.restarts == restarts) pthread_cond_wait(&sched.restart, &sched.lock);
    }
    pthread_mutex_unlock(&sched.lock);
}

/* The running thread's stack pointer is recorded below its caller's frame, so the scan covers the registers
   cooper_poll_slow spilled there. */
static __attribute__((noinline)) void stop_thread(M *m) {
    volatile char here = 0;
    m->current->sp = (void *)&here;
    gc_park(m);
}

void cooper_poll_slow(void) {
    __builtin_unwind_init();
    if (LOAD(&cooper_poll_word)) stop_thread(get_m());
}

bool cooper_stop_the_world(void) {
    M *m = get_m();
    pthread_mutex_lock(&sched.lock);
    if (sched.gcwaiting) {
        pthread_mutex_unlock(&sched.lock);
        cooper_poll_slow();
        return false;
    }
    sched.gcwaiting = 1;
    STORE(&cooper_poll_word, 1);
    sched.stopwait = 0;
    for (int i = 0; i < nprocs; i++) {
        P *p = &allp[i];
        if (p == m->p) continue;
        if (p->status == P_IDLE) p->status = P_STOPPED;
        else sched.stopwait++;
    }
    sched.idle_ps = NULL;
    STORE(&sched.npidle, 0);
    while (sched.stopwait > 0) pthread_cond_wait(&sched.stopped, &sched.lock);
    pthread_mutex_unlock(&sched.lock);
    return true;
}

void cooper_start_the_world(void) {
    pthread_mutex_lock(&sched.lock);
    sched.gcwaiting = 0;
    sched.restarts++;
    STORE(&cooper_poll_word, 0);
    for (int i = nprocs - 1; i >= 0; i--) {
        P *p = &allp[i];
        if (p->status != P_STOPPED) continue;
        if (p->m) p->status = P_RUNNING;
        else p_put_idle(p);
    }
    pthread_cond_broadcast(&sched.restart);
    pthread_mutex_unlock(&sched.lock);
    if (LOAD(&sched.global_count)) wakep();
}

/* --- timers --- */

static int timer_before(const Timer *a, const Timer *b) {
    return a->wake != b->wake ? a->wake < b->wake : a->order < b->order;
}

static void timer_push(Timer timer) {
    pthread_mutex_lock(&sched.lock);
    if (sched.timer_count == sched.timer_capacity) {
        sched.timer_capacity = sched.timer_capacity ? sched.timer_capacity * 2 : 1024;
        sched.timers = realloc(sched.timers, sched.timer_capacity * sizeof *sched.timers);
        if (!sched.timers) cooper_panic("out of memory");
    }
    timer.order = sched.timer_sequence++;
    size_t i = sched.timer_count++;
    while (i > 0 && timer_before(&timer, &sched.timers[(i - 1) / 2])) {
        sched.timers[i] = sched.timers[(i - 1) / 2];
        i = (i - 1) / 2;
    }
    sched.timers[i] = timer;
    STORE(&sched.timer_next, sched.timers[0].wake);
    /* A sleeping M must wake for the earliest timer: the one waiting for a later one, or any if none is. */
    M *watcher = sched.timer_m ? sched.timer_m : sched.idle_ms;
    if (watcher && (!sched.timer_m || timer.wake < sched.timer_m_until)) pthread_cond_signal(&watcher->wake);
    pthread_mutex_unlock(&sched.lock);
}

static void timer_pop(void) {
    Timer *timers = sched.timers;
    Timer last = timers[--sched.timer_count];
    size_t i = 0;
    for (;;) {
        size_t child = 2 * i + 1;
        if (child >= sched.timer_count) break;
        if (child + 1 < sched.timer_count && timer_before(&timers[child + 1], &timers[child])) child++;
        if (!timer_before(&timers[child], &last)) break;
        timers[i] = timers[child];
        i = child;
    }
    if (sched.timer_count) timers[i] = last;
    STORE(&sched.timer_next, sched.timer_count ? timers[0].wake : INT64_MAX);
}

/* Run the timers that are due onto `p`, which the running M holds. */
static void wake_due(P *p) {
    int64_t next = LOAD(&sched.timer_next);
    if (next == INT64_MAX) return;
    int64_t now = now_ns();
    if (next > now) return;
    Timer due[64];
    int n = 0;
    pthread_mutex_lock(&sched.lock);
    while (n < 64 && sched.timer_count && sched.timers[0].wake <= now) {
        Timer timer = sched.timers[0];
        if (timer.thread || cooper_cas(&timer.waiter->state, COOPER_WAITING, COOPER_SYNCED)) {
            due[n++] = timer;
        } else if (cooper_load(&timer.waiter->state) != COOPER_SYNCED) {
            break; /* claimed: its owner settles it within a few instructions; look again on the next pass */
        }
        timer_pop(); /* due, or its sync committed otherwise */
    }
    pthread_mutex_unlock(&sched.lock);
    for (int i = 0; i < n; i++) {
        if (due[i].thread) {
            set_state(due[i].thread, RUNNABLE);
            runq_put(p, due[i].thread, false);
        } else {
            cooper_store(&due[i].waiter->chosen, due[i].index);
            cooper_ready(due[i].waiter->owner);
        }
    }
    if (n) wakep();
}

/* --- the scheduler --- */

/* Sleep until handed a P. The last M to sleep with nothing left to wake it finds a deadlock. */
static void stopm(M *m) {
    pthread_mutex_lock(&sched.lock);
    m->idle_next = sched.idle_ms;
    sched.idle_ms = m;
    bool timers_late = false; /* timers are due, but every P is busy: their Ms will run them */
    while (!m->nextp) {
        if (sched.npidle == nprocs && !sched.timer_count && !sched.global_count && !sched.gcwaiting) {
            cooper_panic("all threads are blocked: deadlock");
        }
        if (sched.timer_count && !timers_late && (!sched.timer_m || sched.timer_m == m)) {
            sched.timer_m = m;
            sched.timer_m_until = sched.timers[0].wake;
            wait_until(&m->wake, sched.timer_m_until);
            if (m->nextp || !sched.timer_count || sched.timers[0].wake > now_ns()) continue;
            sched.timer_m = NULL;
            P *p = p_get_idle();
            if (p) {
                m_remove_idle(m);
                m->nextp = p;
            } else {
                timers_late = true;
            }
        } else {
            if (sched.timer_m == m) sched.timer_m = NULL;
            pthread_cond_wait(&m->wake, &sched.lock);
            timers_late = false;
        }
    }
    if (sched.timer_m == m) sched.timer_m = NULL;
    m->p = m->nextp;
    m->nextp = NULL;
    m->p->m = m;
    pthread_mutex_unlock(&sched.lock);
}

static Thread *steal_work(M *m) {
    for (int round = 0; round < 4; round++) {
        uint32_t start = (uint32_t)(random_next(m->p) % (uint64_t)nprocs);
        for (int i = 0; i < nprocs; i++) {
            if (LOAD(&cooper_poll_word)) return NULL;
            P *victim = &allp[(start + (uint32_t)i) % (uint32_t)nprocs];
            if (victim == m->p) continue;
            Thread *thread = runq_steal(m->p, victim, round == 3);
            if (thread) return thread;
        }
    }
    return NULL;
}

/* The next thread for `m` to run. Without any, the M gives up its P and sleeps until handed one. */
static Thread *find_runnable(M *m) {
    for (;;) {
        if (LOAD(&cooper_poll_word)) gc_park(m);
        P *p = m->p;
        wake_due(p);
        p->schedtick++;
        if (p->schedtick % 61 == 0 && LOAD(&sched.global_count)) {
            pthread_mutex_lock(&sched.lock);
            Thread *thread = global_get(p, 1);
            pthread_mutex_unlock(&sched.lock);
            if (thread) return thread;
        }
        Thread *next = LOAD(&p->runnext);
        if (next && __atomic_compare_exchange_n(&p->runnext, &next, NULL, false, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED)) {
            if (p->runnext_turns < RUNNEXT_TURNS || !runq_has_work(p)) {
                p->runnext_turns++;
                return next;
            }
            runq_put(p, next, false);
        }
        p->runnext_turns = 0;
        Thread *thread = runq_pop(p);
        if (thread) return thread;
        if (LOAD(&sched.global_count)) {
            pthread_mutex_lock(&sched.lock);
            thread = global_get(p, RUNQ_SIZE / 2);
            pthread_mutex_unlock(&sched.lock);
            if (thread) return thread;
        }
        if (m->spinning || 2 * LOAD(&sched.nmspinning) < nprocs - LOAD(&sched.npidle)) {
            if (!m->spinning) {
                m->spinning = 1;
                ADD(&sched.nmspinning, 1);
            }
            thread = steal_work(m);
            if (thread) return thread;
        }
        /* Nothing to run: give the P back. */
        pthread_mutex_lock(&sched.lock);
        if (sched.gcwaiting) {
            pthread_mutex_unlock(&sched.lock);
            continue;
        }
        thread = global_get(p, RUNQ_SIZE / 2);
        if (thread) {
            pthread_mutex_unlock(&sched.lock);
            return thread;
        }
        p_put_idle(p);
        m->p = NULL;
        pthread_mutex_unlock(&sched.lock);
        bool was_spinning = m->spinning;
        if (was_spinning) {
            m->spinning = 0;
            ADD(&sched.nmspinning, -1);
        }
        /* Work readied while this M was spinning may have woken nobody, the readier counting on this M to find
           it: look once more. */
        bool work = LOAD(&sched.global_count) != 0;
        for (int i = 0; i < nprocs && !work; i++) work = runq_has_work(&allp[i]);
        if (work) {
            pthread_mutex_lock(&sched.lock);
            P *idle = p_get_idle();
            if (idle) {
                m->p = idle;
                idle->m = m;
            }
            pthread_mutex_unlock(&sched.lock);
            if (m->p) {
                if (was_spinning) {
                    m->spinning = 1;
                    ADD(&sched.nmspinning, 1);
                }
                continue;
            }
        }
        stopm(m);
    }
}

/* Do what the thread that just switched away asked for. */
static void complete(M *m, Thread *thread) {
    switch (m->action) {
    case ACT_YIELD:
        set_state(thread, RUNNABLE);
        runq_put(m->p, thread, false);
        break;
    case ACT_PARK:
        /* A ready that came after the thread chose to park made it RUNNABLE; it is queued here, not by the
           ready, because until now the thread's stack was still in use. */
        if (cooper_cas(&thread->state, PARKING, PARKED)) break;
        runq_put(m->p, thread, true);
        break;
    case ACT_SLEEP:
        timer_push((Timer){.wake = m->action_wake, .thread = thread});
        break;
    case ACT_DEAD: {
        P *p = m->p;
        if (LOAD(&pooled) >= MAX_POOLED) {
            munmap((void *)thread->stack_low, stack_size);
            thread->stack_low = thread->stack_top = 0;
        } else {
            ADD(&pooled, 1);
        }
        thread->next = p->free;
        p->free = thread;
        if (++p->nfree > LOCAL_FREE) {
            pthread_mutex_lock(&sched.lock);
            while (p->nfree > LOCAL_FREE / 2) {
                Thread *moved = p->free;
                p->free = moved->next;
                p->nfree--;
                moved->next = sched.free;
                __atomic_store_n(&sched.free, moved, __ATOMIC_RELAXED);
            }
            pthread_mutex_unlock(&sched.lock);
        }
        break;
    }
    }
}

/* Run threads for good. This is the M's own stack. */
static _Noreturn void scheduler(M *m) {
    TSAN_CURRENT(m->fiber);
    for (;;) {
        Thread *next = find_runnable(m);
        if (m->spinning) {
            /* The last M looking for work found some; there may be more. */
            m->spinning = 0;
            if (ADD(&sched.nmspinning, -1) == 0) wakep();
        }
        set_state(next, RUNNING);
        m->current = next;
        TSAN_SWITCH(next->fiber);
        cooper_switch(&m->sched_sp, next->sp);
        Thread *prev = m->current;
        m->current = NULL;
        complete(m, prev);
    }
}

/* Switch from the running thread to the scheduler, which does `action` for it. The thread resumes when a
   scheduler next picks it, possibly on another M. */
static void switch_out(int action) {
    M *m = get_m();
    if (m->locks) cooper_panic("lock held across a switch");
    m->action = action;
    TSAN_SWITCH(m->fiber);
    cooper_switch(&m->current->sp, m->sched_sp);
}

static void thread_entry(void) {
    Thread *thread = get_m()->current;
    ((void (*)(void *))thread->code)(thread->env);
    STORE(&thread->state, DEAD);
    switch_out(ACT_DEAD);
    __builtin_trap(); /* a dead thread is never resumed */
}

/* Give `thread` a stack (unless it kept one), and a first frame that starts `code(env)`. */
static void thread_prepare(Thread *thread, void *code, void *env) {
    thread->code = code;
    thread->env = env;
    if (thread->stack_low) {
        ADD(&pooled, -1);
    } else {
        void *memory = mmap(NULL, stack_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE, -1, 0);
        if (memory == MAP_FAILED) cooper_panic("out of memory for a thread stack");
        if (mprotect(memory, guard_size, PROT_NONE) != 0) cooper_panic("cannot protect a thread stack");
        thread->stack_low = (uintptr_t)memory;
        thread->stack_top = thread->stack_low + stack_size;
    }
    char *frame = (char *)thread->stack_top - FRAME_BYTES;
    memset(frame, 0, FRAME_BYTES);
    *(void (**)(void))(frame + FRAME_ENTRY_OFFSET) = thread_entry;
    thread->sp = frame;
    TSAN_FIBER(thread->fiber);
    STORE(&thread->state, RUNNABLE);
}

/* A finished thread to reuse, or a new record. */
static Thread *thread_get(P *p) {
    if (!p->free && __atomic_load_n(&sched.free, __ATOMIC_RELAXED)) {
        pthread_mutex_lock(&sched.lock);
        while (sched.free && p->nfree < LOCAL_FREE / 2) {
            Thread *moved = sched.free;
            __atomic_store_n(&sched.free, moved->next, __ATOMIC_RELAXED);
            moved->next = p->free;
            p->free = moved;
            p->nfree++;
        }
        pthread_mutex_unlock(&sched.lock);
    }
    Thread *thread = p->free;
    if (thread) {
        p->free = thread->next;
        p->nfree--;
        return thread;
    }
    thread = calloc(1, sizeof *thread);
    if (!thread) cooper_panic("out of memory");
    thread->state = DEAD;
    thread->all_next = LOAD(&all_threads);
    while (!__atomic_compare_exchange_n(&all_threads, &thread->all_next, thread, false, __ATOMIC_SEQ_CST,
                                        __ATOMIC_SEQ_CST)) {
    }
    return thread;
}

int64_t cooper_spawn(void *code, void *env) {
    P *p = get_m()->p;
    Thread *thread = thread_get(p);
    thread->id = __atomic_fetch_add(&next_id, 1, __ATOMIC_SEQ_CST);
    thread_prepare(thread, code, env);
    runq_put(p, thread, false);
    wakep();
    return (int64_t)thread->id;
}

/* The program ends when the main thread returns. It exits from the main thread's own stack, where exit
   handlers (atexit) may run Cooper code. */
static void main_entry(void *entry) {
    exit(((int (*)(void))entry)());
}

_Noreturn void cooper_run_main(int (*entry)(void)) {
    thread_prepare(&main_thread, main_entry, (void *)entry);
    runq_put(m0.p, &main_thread, false);
    scheduler(&m0);
}

void cooper_yield(void) {
    if (LOAD(&cooper_poll_word)) cooper_poll_slow();
    P *p = get_m()->p;
    wake_due(p);
    if (!runq_has_work(p) && !LOAD(&sched.global_count)) return;
    switch_out(ACT_YIELD);
}

void *cooper_current_thread(void) {
    return get_m()->current;
}

void cooper_park(void) {
    Thread *thread = get_m()->current;
    for (;;) {
        int64_t state = LOAD(&thread->state);
        if (state & PERMIT) {
            if (cooper_cas(&thread->state, state, state & ~PERMIT)) return;
        } else if (cooper_cas(&thread->state, RUNNING, PARKING)) {
            break;
        }
    }
    switch_out(ACT_PARK);
}

void cooper_ready(void *handle) {
    Thread *thread = handle;
    for (;;) {
        int64_t state = LOAD(&thread->state);
        switch (state & STATE_MASK) {
        case PARKED:
            if (!cooper_cas(&thread->state, state, RUNNABLE)) continue;
            runq_put(get_m()->p, thread, true);
            wakep();
            return;
        case PARKING:
            /* Its scheduler finds the state changed and queues the thread. */
            if (!cooper_cas(&thread->state, state, RUNNABLE)) continue;
            return;
        case DEAD:
            return;
        default:
            if (state & PERMIT || cooper_cas(&thread->state, state, state | PERMIT)) return;
        }
    }
}

int64_t cooper_now(void) {
    return now_ns();
}

void cooper_sleep(int64_t ns) {
    if (ns <= 0) {
        cooper_yield();
        return;
    }
    M *m = get_m();
    m->action_wake = now_ns() + ns;
    set_state(m->current, SLEEPING);
    switch_out(ACT_SLEEP);
}

void cooper_wake_at(CooperWaiter *waiter, int64_t index, int64_t deadline) {
    timer_push((Timer){.wake = deadline, .waiter = waiter, .index = index});
}

/* --- atomics, lock words, random numbers --- */

bool cooper_cas(int64_t *word, int64_t old, int64_t new_value) {
    return __atomic_compare_exchange_n(word, &old, new_value, false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
}

int64_t cooper_load(int64_t *word) {
    return LOAD(word);
}

void cooper_store(int64_t *word, int64_t value) {
    STORE(word, value);
}

int64_t cooper_add(int64_t *word, int64_t delta) {
    return ADD(word, delta);
}

/* A holder runs on another M and never switches away while holding the lock, so a waiter spins; after a
   while it yields, letting its own M run other threads. With one P a lock is never found held: if it is,
   a holder switched away. A stop for the collector is a safe point even with locks held, since the thread
   stays on its M. */
void cooper_lock(int64_t *word) {
    for (int spins = 0; !(__atomic_load_n(word, __ATOMIC_RELAXED) == 0 && cooper_cas(word, 0, 1)); spins++) {
        if (nprocs == 1) cooper_panic("lock held across a switch");
        if (LOAD(&cooper_poll_word)) cooper_poll_slow();
        if (spins < 100) cpu_relax();
        else if (get_m()->locks == 0) cooper_yield();
        else sched_yield();
    }
    get_m()->locks++;
}

void cooper_unlock(int64_t *word) {
    if (!cooper_cas(word, 1, 0)) cooper_panic("unlock of a lock that is not held");
    get_m()->locks--;
}

int64_t cooper_random(int64_t n) {
    return (int64_t)(random_next(get_m()->p) % (uint64_t)n);
}

/* --- for the collector --- */

uintptr_t cooper_current_stack_top(void) {
    return get_m()->current->stack_top;
}

/* With the world stopped: every thread's stack except the running one's, from its saved stack pointer up; the
   code and environment words of each thread, which may be the only reference to a thread that has not
   started; and the timers, which refer to waiters. The schedulers' own stacks hold no heap pointers. */
void cooper_scan_threads(void (*scan)(uintptr_t low, uintptr_t high)) {
    Thread *running = get_m()->current;
    if (sched.timer_count) scan((uintptr_t)sched.timers, (uintptr_t)(sched.timers + sched.timer_count));
    for (Thread *thread = LOAD(&all_threads); thread; thread = thread->all_next) {
        if (LOAD(&thread->state) == DEAD) continue;
        if (thread != running) scan((uintptr_t)thread->sp, thread->stack_top);
        scan((uintptr_t)&thread->code, (uintptr_t)(&thread->env + 1));
    }
}

/* --- stack overflow --- */

static void write_text(const char *text) {
    size_t n = strlen(text);
    if (write(2, text, n) < 0) return;
}

static void on_fault(int signal_number, siginfo_t *info, void *context) {
    (void)context;
    uintptr_t address = (uintptr_t)info->si_addr;
    Thread *current = this_m ? this_m->current : NULL;
    if (current && address >= current->stack_low && address < current->stack_low + guard_size) {
        char number[24];
        size_t length = 0;
        uint64_t id = current->id;
        do number[length++] = (char)('0' + id % 10); while (id /= 10);
        write_text("panic: stack overflow in ");
        if (current == &main_thread) {
            write_text("the main thread");
        } else {
            write_text("thread ");
            while (length) if (write(2, &number[--length], 1) < 0) break;
        }
        write_text("\n");
        _exit(101);
    }
    signal(signal_number, SIG_DFL); /* not ours: the fault happens again, and the system reports it */
}

void cooper_sched_init(void) {
    page_size = (size_t)sysconf(_SC_PAGESIZE);
    guard_size = page_size;
    const char *kib = getenv("COOPER_STACK_KIB");
    if (kib && atol(kib) > 0) stack_size = (size_t)atol(kib) * 1024;
    stack_size = (stack_size + page_size - 1) / page_size * page_size;
    if (stack_size < 4 * page_size) stack_size = 4 * page_size;
    const char *threads = getenv("COOPER_THREADS");
    nprocs = threads && atoi(threads) > 0 ? atoi(threads) : (int)sysconf(_SC_NPROCESSORS_ONLN);
    if (nprocs < 1) nprocs = 1;
    if (nprocs > 256) nprocs = 256;
    allp = calloc((size_t)nprocs, sizeof *allp);
    if (!allp) cooper_panic("out of memory");
    uint64_t seed = (uint64_t)now_ns() | 1;
    for (int i = nprocs - 1; i >= 0; i--) {
        allp[i].random_state = seed * (uint64_t)(2 * i + 1) | 1;
        if (i > 0) p_put_idle(&allp[i]);
    }
    allp[0].status = P_RUNNING;
    allp[0].m = &m0;
    m0.p = &allp[0];
    cond_init(&m0.wake);
    cond_init(&sched.stopped);
    cond_init(&sched.restart);
    this_m = &m0;
    main_thread.state = DEAD;
    main_thread.all_next = all_threads;
    all_threads = &main_thread;
    install_alternate_stack();
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_sigaction = on_fault;
    action.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&action.sa_mask);
    sigaction(SIGSEGV, &action, NULL);
    sigaction(SIGBUS, &action, NULL);
}
