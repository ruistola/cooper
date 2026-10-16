/*
 * The Cooper scheduler: tasks (coroutines) started with `co`, run one at a time on the program's
 * thread by a cooperative scheduler. A task runs until it blocks (park, sleep) or yields; a task that
 * never does starves the others, and nothing preempts it yet.
 *
 * Each task has a stack of its own: a large virtual reservation (8 MiB by default, COOPER_STACK_KIB)
 * with a guard page at its low end. The operating system commits a page only when the task first
 * touches it, so a task costs the pages it uses, and one that recurses deeply has the room a thread
 * would. A task that overflows hits the guard page, and a signal handler reports it. Stacks never move,
 * so pointers into them stay valid. Finished tasks' stacks are pooled for reuse.
 *
 * The collector scans every task's stack from its saved stack pointer (the registers a switch saves are
 * on the stack, so they are covered) to its top.
 *
 * Single-threaded; the structure follows Go's (a run queue, a timer list, park and ready) so that
 * per-thread queues and stealing can be added around it.
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

typedef struct Task {
    void *sp;                 /* the saved stack pointer, while the task is not running */
    void *code, *env;         /* what to run: a function taking its environment (adjacent: scanned together) */
    struct Task *next;        /* the run queue, the timer list, or the free list */
    struct Task *all_next, *all_prev;
    uint64_t id;
    uintptr_t stack_low, stack_top;
    int64_t wake;             /* when a sleeping task is due, in nanoseconds on the monotonic clock */
    uint64_t wake_order;      /* arrival order among sleepers, to break ties */
    int state;
} Task;

static Task main_task;
static Task *current = &main_task;
static Task *run_head, *run_tail;
/* Sleeping tasks, a binary min-heap by wake time (ties by arrival, so equal sleeps wake in order). */
static Task **timers;
static size_t timer_count, timer_capacity;
static uint64_t timer_sequence;
static Task *free_tasks;
static Task *zombie;          /* a finished task whose stack is still in use until the next switch completes */
static uint64_t next_id = 1;
static size_t stack_size = (size_t)8 << 20, guard_size, page_size;
static uintptr_t main_stack_low;

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
   stacks hold; a burst of this many tasks costs no mapping at all. */
#define MAX_POOLED 4096
static uintptr_t pool[MAX_POOLED];
static size_t pooled;

static uintptr_t stack_alloc(void) {
    if (pooled) return pool[--pooled];
    void *memory = mmap(NULL, stack_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE, -1, 0);
    if (memory == MAP_FAILED) cooper_panic("out of memory for a task stack");
    if (mprotect(memory, guard_size, PROT_NONE) != 0) cooper_panic("cannot protect a task stack");
    return (uintptr_t)memory;
}

static void stack_free(uintptr_t low) {
    if (pooled < MAX_POOLED) pool[pooled++] = low;
    else munmap((void *)low, stack_size);
}

/* --- tasks --- */

static void enqueue(Task *task) {
    task->state = RUNNABLE;
    task->next = NULL;
    if (run_tail) run_tail->next = task;
    else run_head = task;
    run_tail = task;
}

static void forget(Task *task) {
    task->all_prev->all_next = task->all_next;
    task->all_next->all_prev = task->all_prev;
}

/* Release a finished task. Run by the next task to run, once the dead task's stack is not in use. */
static void reap(void) {
    Task *task = zombie;
    if (!task) return;
    zombie = NULL;
    stack_free(task->stack_low);
    task->next = free_tasks;
    free_tasks = task;
}

static int timer_before(const Task *a, const Task *b) {
    return a->wake != b->wake ? a->wake < b->wake : a->wake_order < b->wake_order;
}

static void timer_push(Task *task) {
    if (timer_count == timer_capacity) {
        timer_capacity = timer_capacity ? timer_capacity * 2 : 1024;
        timers = realloc(timers, timer_capacity * sizeof *timers);
        if (!timers) cooper_panic("out of memory");
    }
    size_t i = timer_count++;
    while (i > 0 && timer_before(task, timers[(i - 1) / 2])) {
        timers[i] = timers[(i - 1) / 2];
        i = (i - 1) / 2;
    }
    timers[i] = task;
}

static Task *timer_pop(void) {
    Task *top = timers[0];
    Task *last = timers[--timer_count];
    size_t i = 0;
    for (;;) {
        size_t child = 2 * i + 1;
        if (child >= timer_count) break;
        if (child + 1 < timer_count && timer_before(timers[child + 1], timers[child])) child++;
        if (!timer_before(timers[child], last)) break;
        timers[i] = timers[child];
        i = child;
    }
    if (timer_count) timers[i] = last;
    return top;
}

static void wake_due(void) {
    if (!timer_count) return;
    int64_t now = now_ns();
    while (timer_count && timers[0]->wake <= now) enqueue(timer_pop());
}

/* The next task to run, waiting for a sleeper if nothing else can run. */
static Task *pick_next(void) {
    for (;;) {
        wake_due();
        if (run_head) {
            Task *task = run_head;
            run_head = task->next;
            if (!run_head) run_tail = NULL;
            return task;
        }
        if (!timer_count) cooper_panic("all tasks are blocked: deadlock");
        int64_t wait = timers[0]->wake - now_ns();
        if (wait > 0) {
            struct timespec t = {wait / 1000000000, wait % 1000000000};
            nanosleep(&t, NULL);
        }
    }
}

/* Switch to the next task. The caller has already said what becomes of the current one (queued,
   parked, sleeping, or dead). */
static void schedule(void) {
    Task *prev = current;
    Task *next = pick_next();
    next->state = RUNNING;
    if (next == prev) return;
    current = next;
    cooper_switch(&prev->sp, next->sp);
    reap();
}

static void task_entry(void) {
    reap();
    Task *task = current;
    ((void (*)(void *))task->code)(task->env);
    task->state = DEAD;
    forget(task);
    zombie = task;
    schedule();
    __builtin_trap(); /* a dead task is never resumed */
}

void cooper_co(void *code, void *env) {
    Task *task = free_tasks;
    if (task) free_tasks = task->next;
    else {
        task = calloc(1, sizeof *task);
        if (!task) cooper_panic("out of memory");
    }
    task->code = code;
    task->env = env;
    task->id = next_id++;
    task->stack_low = stack_alloc();
    task->stack_top = task->stack_low + stack_size;
    char *frame = (char *)task->stack_top - FRAME_BYTES;
    memset(frame, 0, FRAME_BYTES);
    *(void (**)(void))(frame + FRAME_ENTRY_OFFSET) = task_entry;
    task->sp = frame;
    task->all_next = main_task.all_next;
    task->all_prev = &main_task;
    main_task.all_next->all_prev = task;
    main_task.all_next = task;
    enqueue(task);
}

void cooper_yield(void) {
    wake_due();
    if (!run_head) return;
    enqueue(current);
    schedule();
}

void *cooper_current_task(void) {
    return current;
}

/* Stop the current task until another calls cooper_ready on it. */
void cooper_park(void) {
    current->state = PARKED;
    schedule();
}

/* Make a parked task runnable. */
void cooper_ready(void *handle) {
    Task *task = handle;
    if (task->state == PARKED) enqueue(task);
}

void cooper_sleep(int64_t ns) {
    if (ns <= 0) {
        cooper_yield();
        return;
    }
    int64_t wake = now_ns() + ns;
    current->wake = wake;
    current->wake_order = timer_sequence++;
    current->state = SLEEPING;
    timer_push(current);
    schedule();
}

/* --- for the collector --- */

uintptr_t cooper_current_stack_top(void) {
    return current->stack_top;
}

/* Every task's stack except the running one's, from its saved stack pointer up; and the code and
   environment words of each task, which may be the only reference to a task that has not started. */
void cooper_scan_tasks(void (*scan)(uintptr_t low, uintptr_t high)) {
    Task *task = &main_task;
    do {
        if (task != current) scan((uintptr_t)task->sp, task->stack_top);
        scan((uintptr_t)&task->code, (uintptr_t)(&task->env + 1));
        task = task->all_next;
    } while (task != &main_task);
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
    int overflow = address >= current->stack_low && address < current->stack_low + guard_size && current != &main_task;
    if (current == &main_task && address < main_task.stack_top && address + ((uintptr_t)4 << 20) > main_stack_low &&
        address < main_stack_low + 65536) {
        overflow = 1;
    }
    if (overflow) {
        char number[24];
        size_t length = 0;
        uint64_t id = current->id;
        do number[length++] = (char)('0' + id % 10); while (id /= 10);
        write_text("panic: stack overflow in ");
        if (current == &main_task) {
            write_text("the main task");
        } else {
            write_text("task ");
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
    main_task.stack_top = (uintptr_t)base;
    main_task.all_next = main_task.all_prev = &main_task;
    main_task.state = RUNNING;
    struct rlimit limit;
    uintptr_t main_size = getrlimit(RLIMIT_STACK, &limit) == 0 && limit.rlim_cur != RLIM_INFINITY ? (uintptr_t)limit.rlim_cur : (8u << 20);
    main_stack_low = main_task.stack_top - main_size;
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
