/*
 * The Cooper scheduler: the language's threads, started with the `thread` expression, run one at a
 * time on the program's OS thread by a cooperative scheduler. A thread runs until it blocks (park,
 * sleep) or yields; a thread that never does starves the others, and nothing preempts it yet.
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
 *
 * Single-threaded; the structure follows Go's (a run queue, a timer list, park and ready) so that
 * per-OS-thread queues and stealing can be added around it.
 */

#define _DARWIN_C_SOURCE
#define _GNU_SOURCE
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <time.h>
#include <unistd.h>

#include "cooper.h"

#ifndef MAP_ANON
#define MAP_ANON MAP_ANONYMOUS
#endif
#ifndef MAP_NORESERVE
#define MAP_NORESERVE 0
#endif

enum { RUNNING, RUNNABLE, PARKED, SLEEPING, DEAD };

typedef struct Thread {
    void *sp;                 /* the saved stack pointer, while the thread is not running */
    void *code, *env;         /* what to run: a function taking its environment (adjacent: scanned together) */
    struct Thread *next;      /* the run queue, the timer list, or the free list */
    struct Thread *all_next, *all_prev;
    uint64_t id;
    uintptr_t stack_low, stack_top;
    int state;
    int permit;               /* a cooper_ready that came while the thread was not parked */
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

static Thread main_thread;
static Thread *current = &main_thread;
static Thread *run_head, *run_tail;
/* A binary min-heap by wake time (ties by arrival, so equal sleeps wake in order). */
static Timer *timers;
static size_t timer_count, timer_capacity;
static uint64_t timer_sequence;
static Thread *free_threads;
static Thread *zombie;        /* a finished thread whose stack is still in use until the next switch completes */
static uint64_t next_id = 1;
static size_t stack_size = (size_t)8 << 20, guard_size, page_size;
static uintptr_t main_stack_low;
static int locks_held;        /* lock words held by the running thread (cooper_lock) */
static uint64_t random_state; /* xorshift64 */

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

/* Stacks kept for reuse. Each keeps the pages it touched, a page or more, so this bounds what idle
   stacks hold; a burst of this many threads costs no mapping at all. */
#define MAX_POOLED 4096
static uintptr_t pool[MAX_POOLED];
static size_t pooled;

static uintptr_t stack_alloc(void) {
    if (pooled) return pool[--pooled];
    void *memory = mmap(NULL, stack_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE, -1, 0);
    if (memory == MAP_FAILED) cooper_panic("out of memory for a thread stack");
    if (mprotect(memory, guard_size, PROT_NONE) != 0) cooper_panic("cannot protect a thread stack");
    return (uintptr_t)memory;
}

static void stack_free(uintptr_t low) {
    if (pooled < MAX_POOLED) pool[pooled++] = low;
    else munmap((void *)low, stack_size);
}

/* --- threads --- */

static void enqueue(Thread *thread) {
    thread->state = RUNNABLE;
    thread->next = NULL;
    if (run_tail) run_tail->next = thread;
    else run_head = thread;
    run_tail = thread;
}

static void forget(Thread *thread) {
    thread->all_prev->all_next = thread->all_next;
    thread->all_next->all_prev = thread->all_prev;
}

/* Release a finished thread. Run by the next thread to run, once the dead thread's stack is not in use. */
static void reap(void) {
    Thread *thread = zombie;
    if (!thread) return;
    zombie = NULL;
    stack_free(thread->stack_low);
    thread->next = free_threads;
    free_threads = thread;
}

static int timer_before(const Timer *a, const Timer *b) {
    return a->wake != b->wake ? a->wake < b->wake : a->order < b->order;
}

static void timer_push(Timer timer) {
    if (timer_count == timer_capacity) {
        timer_capacity = timer_capacity ? timer_capacity * 2 : 1024;
        timers = realloc(timers, timer_capacity * sizeof *timers);
        if (!timers) cooper_panic("out of memory");
    }
    timer.order = timer_sequence++;
    size_t i = timer_count++;
    while (i > 0 && timer_before(&timer, &timers[(i - 1) / 2])) {
        timers[i] = timers[(i - 1) / 2];
        i = (i - 1) / 2;
    }
    timers[i] = timer;
}

static void timer_pop(void) {
    Timer last = timers[--timer_count];
    size_t i = 0;
    for (;;) {
        size_t child = 2 * i + 1;
        if (child >= timer_count) break;
        if (child + 1 < timer_count && timer_before(&timers[child + 1], &timers[child])) child++;
        if (!timer_before(&timers[child], &last)) break;
        timers[i] = timers[child];
        i = child;
    }
    if (timer_count) timers[i] = last;
}

static void wake_due(void) {
    if (!timer_count) return;
    int64_t now = now_ns();
    while (timer_count && timers[0].wake <= now) {
        Timer timer = timers[0];
        if (timer.thread) {
            timer_pop();
            enqueue(timer.thread);
        } else if (cooper_cas(&timer.waiter->state, COOPER_WAITING, COOPER_SYNCED)) {
            timer_pop();
            cooper_store(&timer.waiter->chosen, timer.index);
            cooper_ready(timer.waiter->owner);
        } else if (cooper_load(&timer.waiter->state) == COOPER_SYNCED) {
            timer_pop(); /* its sync committed otherwise */
        } else {
            break; /* claimed: its owner settles it within a few instructions; look again on the next pass */
        }
    }
}

/* The next thread to run, waiting for a sleeper if nothing else can run. */
static Thread *pick_next(void) {
    for (;;) {
        wake_due();
        if (run_head) {
            Thread *thread = run_head;
            run_head = thread->next;
            if (!run_head) run_tail = NULL;
            return thread;
        }
        if (!timer_count) cooper_panic("all threads are blocked: deadlock");
        int64_t wait = timers[0].wake - now_ns();
        if (wait > 0) {
            struct timespec t = {wait / 1000000000, wait % 1000000000};
            nanosleep(&t, NULL);
        }
    }
}

/* Switch to the next thread. The caller has already said what becomes of the current one (queued,
   parked, sleeping, or dead). */
static void schedule(void) {
    if (locks_held) cooper_panic("lock held across a switch");
    Thread *prev = current;
    Thread *next = pick_next();
    next->state = RUNNING;
    if (next == prev) return;
    current = next;
    cooper_switch(&prev->sp, next->sp);
    reap();
}

static void thread_entry(void) {
    reap();
    Thread *thread = current;
    ((void (*)(void *))thread->code)(thread->env);
    thread->state = DEAD;
    forget(thread);
    zombie = thread;
    schedule();
    __builtin_trap(); /* a dead thread is never resumed */
}

int64_t cooper_spawn(void *code, void *env) {
    Thread *thread = free_threads;
    if (thread) free_threads = thread->next;
    else {
        thread = calloc(1, sizeof *thread);
        if (!thread) cooper_panic("out of memory");
    }
    thread->code = code;
    thread->env = env;
    thread->id = next_id++;
    thread->permit = 0;
    thread->stack_low = stack_alloc();
    thread->stack_top = thread->stack_low + stack_size;
    char *frame = (char *)thread->stack_top - FRAME_BYTES;
    memset(frame, 0, FRAME_BYTES);
    *(void (**)(void))(frame + FRAME_ENTRY_OFFSET) = thread_entry;
    thread->sp = frame;
    thread->all_next = main_thread.all_next;
    thread->all_prev = &main_thread;
    main_thread.all_next->all_prev = thread;
    main_thread.all_next = thread;
    enqueue(thread);
    return (int64_t)thread->id;
}

void cooper_yield(void) {
    if (locks_held) cooper_panic("lock held across a switch");
    wake_due();
    if (!run_head) return;
    enqueue(current);
    schedule();
}

void *cooper_current_thread(void) {
    return current;
}

void cooper_park(void) {
    if (current->permit) {
        current->permit = 0;
        return;
    }
    current->state = PARKED;
    schedule();
}

void cooper_ready(void *handle) {
    Thread *thread = handle;
    if (thread->state == PARKED) enqueue(thread);
    else thread->permit = 1;
}

int64_t cooper_now(void) {
    return now_ns();
}

void cooper_sleep(int64_t ns) {
    if (ns <= 0) {
        cooper_yield();
        return;
    }
    current->state = SLEEPING;
    timer_push((Timer){.wake = now_ns() + ns, .thread = current});
    schedule();
}

void cooper_wake_at(CooperWaiter *waiter, int64_t index, int64_t deadline) {
    timer_push((Timer){.wake = deadline, .waiter = waiter, .index = index});
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
    locks_held++;
}

void cooper_unlock(int64_t *word) {
    if (!cooper_cas(word, 1, 0)) cooper_panic("unlock of a lock that is not held");
    locks_held--;
}

int64_t cooper_random(int64_t n) {
    random_state ^= random_state << 13;
    random_state ^= random_state >> 7;
    random_state ^= random_state << 17;
    return (int64_t)(random_state % (uint64_t)n);
}

/* --- for the collector --- */

uintptr_t cooper_current_stack_top(void) {
    return current->stack_top;
}

/* Every thread's stack except the running one's, from its saved stack pointer up; the code and
   environment words of each thread, which may be the only reference to a thread that has not started;
   and the timers, which refer to waiters. */
void cooper_scan_threads(void (*scan)(uintptr_t low, uintptr_t high)) {
    if (timer_count) scan((uintptr_t)timers, (uintptr_t)(timers + timer_count));
    Thread *thread = &main_thread;
    do {
        if (thread != current) scan((uintptr_t)thread->sp, thread->stack_top);
        scan((uintptr_t)&thread->code, (uintptr_t)(&thread->env + 1));
        thread = thread->all_next;
    } while (thread != &main_thread);
}

/* --- stack overflow --- */

static char alternate_stack[1 << 16];

static void write_text(const char *text) {
    size_t n = strlen(text);
    if (write(2, text, n) < 0) return;
}

static void on_fault(int signal_number, siginfo_t *info, void *context) {
    (void)context;
    uintptr_t address = (uintptr_t)info->si_addr;
    int overflow = address >= current->stack_low && address < current->stack_low + guard_size && current != &main_thread;
    if (current == &main_thread && address < main_thread.stack_top && address + ((uintptr_t)4 << 20) > main_stack_low &&
        address < main_stack_low + 65536) {
        overflow = 1;
    }
    if (overflow) {
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

void cooper_sched_init(void *base) {
    page_size = (size_t)sysconf(_SC_PAGESIZE);
    guard_size = page_size;
    const char *kib = getenv("COOPER_STACK_KIB");
    if (kib && atol(kib) > 0) stack_size = (size_t)atol(kib) * 1024;
    stack_size = (stack_size + page_size - 1) / page_size * page_size;
    if (stack_size < 4 * page_size) stack_size = 4 * page_size;
    main_thread.stack_top = (uintptr_t)base;
    main_thread.all_next = main_thread.all_prev = &main_thread;
    main_thread.state = RUNNING;
    random_state = (uint64_t)now_ns() | 1;
    struct rlimit limit;
    uintptr_t main_size = getrlimit(RLIMIT_STACK, &limit) == 0 && limit.rlim_cur != RLIM_INFINITY ? (uintptr_t)limit.rlim_cur : (8u << 20);
    main_stack_low = main_thread.stack_top - main_size;
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
