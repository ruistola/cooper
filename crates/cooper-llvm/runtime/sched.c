/*
 * The Cooper scheduler: the language's threads, started with the `thread` expression, run on the program's
 * OS thread one at a time. A thread runs until it blocks (park, sleep) or yields; a thread that never does
 * starves the others, and nothing preempts it yet.
 *
 * The structure is Go's. A P (processor) holds a run queue, timers, a stack pool and a random state; an M
 * (machine, an OS thread) runs threads while it has a P. Each M has a scheduler that runs on the OS thread's
 * own stack: a thread that blocks, yields, sleeps or finishes switches to it, it completes the request (marks
 * the thread parked, arms the timer, frees the stack) and switches to the next thread. Nothing is ever done to
 * a thread on its own stack after it has been given up. The main thread is thread 0 and runs on a stack like any
 * other. There is one M and one P so far.
 *
 * Each thread has a stack of its own: a large virtual reservation (8 MiB by default, COOPER_STACK_KIB)
 * with a guard page at its low end. The operating system commits a page only when the thread first
 * touches it, so a thread costs the pages it uses, and one that recurses deeply has the room an OS
 * thread would. A thread that overflows hits the guard page, and a signal handler reports it. Stacks never move,
 * so pointers into them stay valid. Finished threads' stacks are pooled for reuse.
 *
 * The collector scans every thread's stack from its saved stack pointer (the registers a switch saves are
 * on the stack, so they are covered) to its top.
 *
 * The primitives `std.sync` is built on live here too: park with a permit, atomics and lock words, timers
 * that wake a waiter record (CooperWaiter, in cooper.h), and a random source. They hold the same contracts
 * when threads run in parallel; only their implementation changes.
 */

#define _DARWIN_C_SOURCE
#define _GNU_SOURCE
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

/* Thread states (Thread.state, the low byte; PERMIT is a flag on top of any of them). */
enum { RUNNING, RUNNABLE, PARKING, PARKED, SLEEPING, DEAD };
#define STATE_MASK 0xff
#define PERMIT 0x100          /* a cooper_ready that came while the thread was not parked */

typedef struct Thread {
    void *sp;                 /* the saved stack pointer, while the thread is not running */
    void *code, *env;         /* what to run: a function taking its environment (adjacent: scanned together) */
    struct Thread *next;      /* the global run queue, the timer list, or the free list */
    struct Thread *all_next, *all_prev;
    uint64_t id;
    uintptr_t stack_low, stack_top;
    int64_t state;            /* atomic: a state, with PERMIT possibly set */
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

/* Stacks kept for reuse. Each keeps the pages it touched, a page or more, so this bounds what idle
   stacks hold; a burst of this many threads costs no mapping at all. */
#define MAX_POOLED 4096
#define RUNQ_SIZE 256

/* A processor: the right to run threads, and the queues and caches that go with it. An M runs threads only
   while it holds a P. */
typedef struct {
    Thread *queue[RUNQ_SIZE]; /* the local run queue, a ring: first in, first out */
    uint32_t head, tail;      /* tail - head entries, counted without wrapping */
    /* A binary min-heap by wake time (ties by arrival, so equal sleeps wake in order). */
    Timer *timers;
    size_t timer_count, timer_capacity;
    uint64_t timer_sequence;
    uintptr_t pool[MAX_POOLED]; /* stacks kept for reuse */
    size_t pooled;
    uint64_t random_state;    /* xorshift64 */
} P;

/* What a thread asks the scheduler to do with it once it has switched away. */
enum { ACT_YIELD, ACT_PARK, ACT_SLEEP, ACT_DEAD };

/* A machine: an OS thread. Its scheduler runs on the OS thread's own stack, and every thread switches to it
   and from it (a thread never switches directly to another), so that what must happen after a thread has
   stopped running (completing a park, arming a timer, freeing a dead thread's stack) happens while nothing
   else can resume that thread. */
typedef struct {
    void *sched_sp;           /* the scheduler's saved stack pointer, while a thread runs */
    Thread *current;          /* the running thread; null while the scheduler runs */
    P *p;
    int locks;                /* lock words held by the running thread (cooper_lock) */
    int action;               /* the running thread's request, read by the scheduler after the switch */
    int64_t action_wake;      /* ACT_SLEEP: when to resume */
} M;

/* Every global of the scheduler is classified here by who may touch it. (Only one M and one P exist so far;
   the classes say what changes when there are more.)
     per-M:      this_m (thread-local), m0, the signal alternate stack
     per-P:      p0 (run queue, timers, stack pool, random state)
     immutable after cooper_sched_init: stack_size, guard_size, page_size
     global, under a lock once Ms run in parallel: global_head/global_tail (the global run queue), free_threads,
                 next_id, main_thread's all-threads list links */
static Thread main_thread;
static P p0;
static M m0;
static _Thread_local M *this_m;
static Thread *global_head, *global_tail;
static Thread *free_threads;
static uint64_t next_id = 1;
static size_t stack_size = (size_t)8 << 20, guard_size, page_size;

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
#define FRAME_ALIGN_FIX 0
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

/* --- stacks --- */

static uintptr_t stack_alloc(P *p) {
    if (p->pooled) return p->pool[--p->pooled];
    void *memory = mmap(NULL, stack_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE, -1, 0);
    if (memory == MAP_FAILED) cooper_panic("out of memory for a thread stack");
    if (mprotect(memory, guard_size, PROT_NONE) != 0) cooper_panic("cannot protect a thread stack");
    return (uintptr_t)memory;
}

static void stack_free(P *p, uintptr_t low) {
    if (p->pooled < MAX_POOLED) p->pool[p->pooled++] = low;
    else munmap((void *)low, stack_size);
}

/* --- thread states --- */

/* Move `thread` to `state`, keeping a pending permit. */
static void set_state(Thread *thread, int64_t state) {
    int64_t old = __atomic_load_n(&thread->state, __ATOMIC_SEQ_CST);
    while (!__atomic_compare_exchange_n(&thread->state, &old, (old & PERMIT) | state, false, __ATOMIC_SEQ_CST,
                                        __ATOMIC_SEQ_CST)) {
    }
}

/* --- run queues --- */

/* Queue a runnable thread: on the running M's P, or on the global queue if that is full. */
static void runq_put(Thread *thread) {
    P *p = this_m->p;
    thread->next = NULL;
    if (p->tail - p->head < RUNQ_SIZE) {
        p->queue[p->tail++ % RUNQ_SIZE] = thread;
    } else {
        if (global_tail) global_tail->next = thread;
        else global_head = thread;
        global_tail = thread;
    }
}

static Thread *runq_get(P *p) {
    if (p->head != p->tail) return p->queue[p->head++ % RUNQ_SIZE];
    Thread *thread = global_head;
    if (thread) {
        global_head = thread->next;
        if (!global_head) global_tail = NULL;
    }
    return thread;
}

static bool runq_nonempty(P *p) {
    return p->head != p->tail || global_head;
}

static void forget(Thread *thread) {
    thread->all_prev->all_next = thread->all_next;
    thread->all_next->all_prev = thread->all_prev;
}

/* --- timers --- */

static int timer_before(const Timer *a, const Timer *b) {
    return a->wake != b->wake ? a->wake < b->wake : a->order < b->order;
}

static void timer_push(P *p, Timer timer) {
    if (p->timer_count == p->timer_capacity) {
        p->timer_capacity = p->timer_capacity ? p->timer_capacity * 2 : 1024;
        p->timers = realloc(p->timers, p->timer_capacity * sizeof *p->timers);
        if (!p->timers) cooper_panic("out of memory");
    }
    timer.order = p->timer_sequence++;
    size_t i = p->timer_count++;
    while (i > 0 && timer_before(&timer, &p->timers[(i - 1) / 2])) {
        p->timers[i] = p->timers[(i - 1) / 2];
        i = (i - 1) / 2;
    }
    p->timers[i] = timer;
}

static void timer_pop(P *p) {
    Timer last = p->timers[--p->timer_count];
    size_t i = 0;
    for (;;) {
        size_t child = 2 * i + 1;
        if (child >= p->timer_count) break;
        if (child + 1 < p->timer_count && timer_before(&p->timers[child + 1], &p->timers[child])) child++;
        if (!timer_before(&p->timers[child], &last)) break;
        p->timers[i] = p->timers[child];
        i = child;
    }
    if (p->timer_count) p->timers[i] = last;
}

static void wake_due(P *p) {
    if (!p->timer_count) return;
    int64_t now = now_ns();
    while (p->timer_count && p->timers[0].wake <= now) {
        Timer timer = p->timers[0];
        if (timer.thread) {
            timer_pop(p);
            set_state(timer.thread, RUNNABLE);
            runq_put(timer.thread);
        } else if (cooper_cas(&timer.waiter->state, COOPER_WAITING, COOPER_SYNCED)) {
            timer_pop(p);
            cooper_store(&timer.waiter->chosen, timer.index);
            cooper_ready(timer.waiter->owner);
        } else if (cooper_load(&timer.waiter->state) == COOPER_SYNCED) {
            timer_pop(p); /* its sync committed otherwise */
        } else {
            break; /* claimed: its owner settles it within a few instructions; look again on the next pass */
        }
    }
}

/* --- the scheduler --- */

/* The next thread to run, waiting for a sleeper if nothing else can run. */
static Thread *pick_next(P *p) {
    for (;;) {
        wake_due(p);
        Thread *thread = runq_get(p);
        if (thread) return thread;
        if (!p->timer_count) cooper_panic("all threads are blocked: deadlock");
        int64_t wait = p->timers[0].wake - now_ns();
        if (wait > 0) {
            struct timespec t = {wait / 1000000000, wait % 1000000000};
            nanosleep(&t, NULL);
        }
    }
}

/* Do what the thread that just switched away asked for. */
static void complete(M *m, Thread *thread) {
    switch (m->action) {
    case ACT_YIELD:
        set_state(thread, RUNNABLE);
        runq_put(thread);
        break;
    case ACT_PARK:
        /* A ready that came after the thread chose to park made it RUNNABLE; it is queued here, not by the
           ready, because until now the thread's stack was still in use. */
        if (cooper_cas(&thread->state, PARKING, PARKED)) break;
        runq_put(thread);
        break;
    case ACT_SLEEP:
        timer_push(m->p, (Timer){.wake = m->action_wake, .thread = thread});
        break;
    case ACT_DEAD:
        stack_free(m->p, thread->stack_low);
        thread->next = free_threads;
        free_threads = thread;
        break;
    }
}

/* Run threads for good. This is the M's own stack. */
static _Noreturn void scheduler(M *m) {
    for (;;) {
        Thread *next = pick_next(m->p);
        set_state(next, RUNNING);
        m->current = next;
        cooper_switch(&m->sched_sp, next->sp);
        Thread *prev = m->current;
        m->current = NULL;
        complete(m, prev);
    }
}

/* Switch from the running thread to the scheduler, which does `action` for it. The thread resumes when a
   scheduler next picks it. */
static void switch_out(int action) {
    M *m = this_m;
    if (m->locks) cooper_panic("lock held across a switch");
    m->action = action;
    cooper_switch(&m->current->sp, m->sched_sp);
}

static void thread_entry(void) {
    Thread *thread = this_m->current;
    ((void (*)(void *))thread->code)(thread->env);
    thread->state = DEAD;
    forget(thread);
    switch_out(ACT_DEAD);
    __builtin_trap(); /* a dead thread is never resumed */
}

/* Give `thread` a stack, and a first frame that starts `code(env)`. */
static void thread_prepare(Thread *thread, void *code, void *env) {
    thread->code = code;
    thread->env = env;
    thread->state = RUNNABLE;
    thread->stack_low = stack_alloc(this_m->p);
    thread->stack_top = thread->stack_low + stack_size;
    char *frame = (char *)thread->stack_top - FRAME_BYTES;
    memset(frame, 0, FRAME_BYTES);
    *(void (**)(void))(frame + FRAME_ENTRY_OFFSET) = thread_entry;
    thread->sp = frame;
}

int64_t cooper_spawn(void *code, void *env) {
    Thread *thread = free_threads;
    if (thread) free_threads = thread->next;
    else {
        thread = calloc(1, sizeof *thread);
        if (!thread) cooper_panic("out of memory");
    }
    thread->id = next_id++;
    thread_prepare(thread, code, env);
    thread->all_next = main_thread.all_next;
    thread->all_prev = &main_thread;
    main_thread.all_next->all_prev = thread;
    main_thread.all_next = thread;
    runq_put(thread);
    return (int64_t)thread->id;
}

/* The program ends when the main thread returns. It exits from the main thread's own stack, where exit
   handlers (atexit) may run Cooper code. */
static void main_entry(void *entry) {
    exit(((int (*)(void))entry)());
}

_Noreturn void cooper_run_main(int (*entry)(void)) {
    thread_prepare(&main_thread, main_entry, (void *)entry);
    runq_put(&main_thread);
    scheduler(&m0);
}

void cooper_yield(void) {
    P *p = this_m->p;
    wake_due(p);
    if (!runq_nonempty(p)) return;
    switch_out(ACT_YIELD);
}

void *cooper_current_thread(void) {
    return this_m->current;
}

void cooper_park(void) {
    Thread *thread = this_m->current;
    for (;;) {
        int64_t state = __atomic_load_n(&thread->state, __ATOMIC_SEQ_CST);
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
        int64_t state = __atomic_load_n(&thread->state, __ATOMIC_SEQ_CST);
        switch (state & STATE_MASK) {
        case PARKED:
            if (!cooper_cas(&thread->state, state, RUNNABLE)) continue;
            runq_put(thread);
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
    this_m->action_wake = now_ns() + ns;
    set_state(this_m->current, SLEEPING);
    switch_out(ACT_SLEEP);
}

void cooper_wake_at(CooperWaiter *waiter, int64_t index, int64_t deadline) {
    timer_push(this_m->p, (Timer){.wake = deadline, .waiter = waiter, .index = index});
}

/* --- atomics, lock words, random numbers --- */

bool cooper_cas(int64_t *word, int64_t old, int64_t new_value) {
    return __atomic_compare_exchange_n(word, &old, new_value, false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
}

int64_t cooper_load(int64_t *word) {
    return __atomic_load_n(word, __ATOMIC_SEQ_CST);
}

void cooper_store(int64_t *word, int64_t value) {
    __atomic_store_n(word, value, __ATOMIC_SEQ_CST);
}

/* One OS thread runs every thread, and none switches while holding a lock, so a lock is never found held:
   if it is, a holder switched away. */
void cooper_lock(int64_t *word) {
    if (!cooper_cas(word, 0, 1)) cooper_panic("lock held across a switch");
    this_m->locks++;
}

void cooper_unlock(int64_t *word) {
    if (!cooper_cas(word, 1, 0)) cooper_panic("unlock of a lock that is not held");
    this_m->locks--;
}

int64_t cooper_random(int64_t n) {
    uint64_t *state = &this_m->p->random_state;
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    return (int64_t)(*state % (uint64_t)n);
}

/* --- for the collector --- */

uintptr_t cooper_current_stack_top(void) {
    return this_m->current->stack_top;
}

/* Every thread's stack except the running one's, from its saved stack pointer up; the code and
   environment words of each thread, which may be the only reference to a thread that has not started;
   and the timers, which refer to waiters. The scheduler's own stack holds no heap pointers. */
void cooper_scan_threads(void (*scan)(uintptr_t low, uintptr_t high)) {
    P *p = this_m->p;
    if (p->timer_count) scan((uintptr_t)p->timers, (uintptr_t)(p->timers + p->timer_count));
    Thread *thread = &main_thread;
    do {
        if (thread != this_m->current) scan((uintptr_t)thread->sp, thread->stack_top);
        scan((uintptr_t)&thread->code, (uintptr_t)(&thread->env + 1));
        thread = thread->all_next;
    } while (thread != &main_thread);
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

/* Per M: the stack the fault handler runs on, since the faulting thread's own stack is exhausted. */
static char alternate_stack[1 << 16];

void cooper_sched_init(void) {
    page_size = (size_t)sysconf(_SC_PAGESIZE);
    guard_size = page_size;
    const char *kib = getenv("COOPER_STACK_KIB");
    if (kib && atol(kib) > 0) stack_size = (size_t)atol(kib) * 1024;
    stack_size = (stack_size + page_size - 1) / page_size * page_size;
    if (stack_size < 4 * page_size) stack_size = 4 * page_size;
    main_thread.all_next = main_thread.all_prev = &main_thread;
    p0.random_state = (uint64_t)now_ns() | 1;
    m0.p = &p0;
    this_m = &m0;
    stack_t altstack = {.ss_sp = alternate_stack, .ss_size = sizeof alternate_stack, .ss_flags = 0};
    sigaltstack(&altstack, NULL);
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_sigaction = on_fault;
    action.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&action.sa_mask);
    sigaction(SIGSEGV, &action, NULL);
    sigaction(SIGBUS, &action, NULL);
}
