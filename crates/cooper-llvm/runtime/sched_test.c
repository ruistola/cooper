/* A test of the scheduler on its own (see tests/scheduler.rs), run with COOPER_THREADS=1 and with several. */

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "cooper.h"

_Noreturn void cooper_panic(const char *message) {
    fprintf(stderr, "panic: %s\n", message);
    exit(101);
}

static int failures;
#define CHECK(condition) do { if (!(condition)) { fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #condition); failures++; } } while (0)

static char log_buffer[4096];
static void note(const char *text) { strcat(log_buffer, text); }

/* Threads that report finishing by counting down; the main thread parks until the count reaches zero. */
static int64_t pending;
static void *waiting_main;

static void start(int64_t n) {
    waiting_main = cooper_current_thread();
    cooper_store(&pending, n);
}

static void finished(void) {
    if (cooper_add(&pending, -1) == 0) cooper_ready(waiting_main);
}

static void wait_all(void) {
    while (cooper_load(&pending) > 0) cooper_park();
}

typedef struct { const char *name; int rounds; } Worker;

static void worker(void *env) {
    Worker *w = env;
    for (int i = 0; i < w->rounds; i++) {
        char text[32];
        snprintf(text, sizeof text, "%s%d ", w->name, i);
        note(text);
        cooper_yield();
    }
}

/* With one P, threads that yield take turns in the order they became ready. */
static void test_yield_interleaves_in_order(void) {
    log_buffer[0] = 0;
    Worker a = {"a", 3}, b = {"b", 2}, c = {"c", 3};
    cooper_spawn(worker, &a);
    cooper_spawn(worker, &b);
    cooper_spawn(worker, &c);
    for (int i = 0; i < 10; i++) cooper_yield();
    CHECK(strcmp(log_buffer, "a0 b0 c0 a1 b1 c1 a2 c2 ") == 0);
}

static void *parked;
static int64_t woke;

static void sleeper_a(void *env) { (void)env; cooper_sleep(30000000); note("slow "); finished(); }
static void sleeper_b(void *env) { (void)env; cooper_sleep(10000000); note("fast "); finished(); }

static void parker(void *env) {
    (void)env;
    cooper_store((int64_t *)&parked, (int64_t)cooper_current_thread());
    while (!cooper_load(&woke)) cooper_park();
    cooper_store(&woke, 2);
    finished();
}

static void test_sleep_and_park(void) {
    log_buffer[0] = 0;
    start(3);
    cooper_spawn(sleeper_a, NULL);
    cooper_spawn(sleeper_b, NULL);
    cooper_spawn(parker, NULL);
    cooper_sleep(60000000);
    CHECK(strcmp(log_buffer, "fast slow ") == 0);
    CHECK(cooper_load((int64_t *)&parked) && cooper_load(&woke) == 0);
    cooper_store(&woke, 1);
    cooper_ready(parked);
    wait_all();
    CHECK(cooper_load(&woke) == 2);
}

static void test_ready_before_park_leaves_a_permit(void) {
    cooper_ready(cooper_current_thread());
    cooper_park(); /* returns at once, consuming the permit; with nothing else to run it would be a deadlock */
}

/* A waiter whose sync committed otherwise (stale) and one still waiting, each with a timer. */
static CooperWaiter stale_waiter, timed_waiter;
static int64_t woke_after;

static void wait_for_timer(void *env) {
    (void)env;
    int64_t start_time = cooper_now();
    stale_waiter = (CooperWaiter){COOPER_SYNCED, 0, cooper_current_thread()};
    timed_waiter = (CooperWaiter){COOPER_WAITING, -1, cooper_current_thread()};
    cooper_wake_at(&stale_waiter, 1, start_time + 5000000);
    cooper_wake_at(&timed_waiter, 3, start_time + 10000000);
    while (cooper_load(&timed_waiter.chosen) < 0) cooper_park();
    woke_after = cooper_now() - start_time;
    finished();
}

static void test_timers_commit_waiters(void) {
    start(1);
    cooper_spawn(wait_for_timer, NULL);
    wait_all();
    cooper_sleep(10000000);
    CHECK(timed_waiter.state == COOPER_SYNCED && timed_waiter.chosen == 3);
    CHECK(woke_after >= 10000000);
    CHECK(stale_waiter.chosen == 0); /* dropped, not committed again */
}

static int64_t counter;
static void count(void *env) {
    (void)env;
    cooper_add(&counter, 1);
    cooper_yield();
    cooper_add(&counter, 1);
    finished();
}

static void test_many_threads_and_stack_reuse(void) {
    counter = 0;
    for (int round = 0; round < 3; round++) {
        start(2000);
        for (int i = 0; i < 2000; i++) cooper_spawn(count, NULL);
        wait_all();
    }
    CHECK(counter == 3 * 2000 * 2);
}

static long depth_reached;
static long recurse(long n) {
    volatile char pad[96];
    pad[0] = (char)n;
    if (n == 0) return pad[0];
    return recurse(n - 1) + pad[0] % 2;
}
static void deep(void *env) { depth_reached = recurse((long)env) + 1; finished(); }

static void test_deep_recursion_on_a_thread_stack(void) {
    /* About 60000 frames of over 100 bytes: several MiB, well inside the 8 MiB reservation. */
    start(1);
    cooper_spawn(deep, (void *)60000);
    wait_all();
    CHECK(depth_reached > 0);
}

static int ranges_seen;
static size_t largest_range;
static void inspect(void *context, uintptr_t low, uintptr_t high) {
    (void)context;
    ranges_seen++;
    if (high - low > largest_range) largest_range = high - low;
}

static void test_scan_visits_every_stack(void) {
    Worker z = {"z", 1};
    cooper_spawn(worker, &z);
    volatile char here = 0;
    cooper_save_stack_pointer((void *)&here);
    cooper_scan_threads(inspect, NULL, 1);
    /* The two threads' stacks (the new one's prepared frame, and the running one's from here up), and each
       one's code and environment words. */
    CHECK(ranges_seen == 4);
    CHECK(largest_range >= 64);
    /* Each thread is scanned once for a cycle. */
    cooper_scan_threads(inspect, NULL, 1);
    CHECK(ranges_seen == 4);
    for (int i = 0; i < 3; i++) cooper_yield();
}

/* --- with several Ps --- */

/* Two threads hand a turn back and forth through park and ready, wherever each happens to run. A player that
   has not started yet needs no ready: it looks at the turn before it parks. */
static int64_t turn;
static int64_t players[2];

static void player(void *env) {
    int64_t me = (int64_t)env;
    cooper_store(&players[me], (int64_t)cooper_current_thread());
    for (int round = 0; round < 20000; round++) {
        while (cooper_load(&turn) % 2 != me) cooper_park();
        cooper_add(&turn, 1);
        void *other = (void *)cooper_load(&players[1 - me]);
        if (other) cooper_ready(other);
    }
    finished();
}

static void test_ping_pong_across_ms(void) {
    start(2);
    cooper_spawn(player, (void *)0);
    cooper_spawn(player, (void *)1);
    wait_all();
    CHECK(cooper_load(&turn) == 40000);
}

/* Threads that never block hold their M: the others must be stolen by other Ms to run at all. */
static pthread_t ran_on[8];
static void busy(void *env) {
    ran_on[(long)env] = pthread_self();
    int64_t until = cooper_now() + 20000000;
    while (cooper_now() < until) {
    }
    finished();
}

static void test_idle_ms_steal_work(void) {
    start(8);
    for (long i = 0; i < 8; i++) cooper_spawn(busy, (void *)i);
    wait_all();
    int distinct = 0;
    for (int i = 0; i < 8; i++) {
        int seen = 0;
        for (int j = 0; j < i; j++) seen |= pthread_equal(ran_on[i], ran_on[j]);
        distinct += !seen;
    }
    CHECK(distinct >= 2);
}

/* Threads that poll as generated code does all stop while the world is stopped. They yield now and then,
   so the main thread gets an M however few there are. */
static int64_t polls, stop_requested;
static void poller(void *env) {
    (void)env;
    while (!cooper_load(&stop_requested)) {
        if (cooper_load(&cooper_poll_word)) cooper_poll_slow();
        if (cooper_add(&polls, 1) % 1000 == 0) cooper_yield();
    }
    finished();
}

static void test_stop_the_world(void) {
    start(3);
    for (int i = 0; i < 3; i++) cooper_spawn(poller, NULL);
    while (cooper_load(&polls) < 1000) cooper_yield();
    for (int round = 0; round < 50; round++) {
        CHECK(cooper_stop_the_world());
        int64_t before = cooper_load(&polls);
        usleep(1000);
        CHECK(cooper_load(&polls) == before);
        cooper_start_the_world();
    }
    cooper_store(&stop_requested, 1);
    wait_all();
}

/* A thread that polls but never blocks is preempted, so the main thread wakes from its sleep even on one P. */
static int64_t spin_stop;
static void spinner(void *env) {
    (void)env;
    while (!cooper_load(&spin_stop)) {
        if (cooper_load(&cooper_poll_word)) cooper_poll_slow();
    }
    finished();
}

static void test_a_spinning_thread_is_preempted(void) {
    start(1);
    cooper_spawn(spinner, NULL);
    cooper_sleep(30000000);
    cooper_store(&spin_stop, 1);
    wait_all();
}

/* A thread blocked in C, and in a callback from C between, gives its P to other threads, and counts as
   stopped for the collector. */
static int64_t c_done;
static void blocker(void *env) {
    (void)env;
    int entered = cooper_enter_c();
    usleep(40000);
    uintptr_t saved[COOPER_CALLBACK_WORDS];
    int called = cooper_callback_enter(saved);
    cooper_yield();
    cooper_callback_leave(called, saved);
    usleep(40000);
    cooper_exit_c(entered);
    cooper_store(&c_done, 1);
    finished();
}

static void test_a_blocking_c_call_gives_up_its_p(void) {
    start(1);
    cooper_spawn(blocker, NULL);
    cooper_sleep(10000000);
    for (int i = 0; i < 10; i++) {
        CHECK(cooper_stop_the_world());
        cooper_start_the_world();
        cooper_sleep(2000000);
    }
    CHECK(cooper_load(&c_done) == 0);
    wait_all();
    CHECK(cooper_load(&c_done) == 1);
}

/* While the collector marks with the program running, each thread's stack is scanned once in a cycle, and before
   the thread runs again: one running when marking begins scans itself as it resumes from the stop, one waiting
   to run or asleep is scanned before it is switched to or by an idle M, and one in C by an idle M or as it
   leaves C. */
enum { SCANNED = 6, CYCLES = 30 };
static int64_t frame_of[SCANNED], scans_of[SCANNED], marking, ran_unscanned, stop_scanned;

static void count_scan(uintptr_t low, uintptr_t high) {
    for (int i = 0; i < SCANNED; i++) {
        uintptr_t frame = (uintptr_t)cooper_load(&frame_of[i]);
        if (frame && low <= frame && frame < high) cooper_add(&scans_of[i], 1);
    }
}

static bool idle_scan(bool (*stop)(void)) {
    (void)stop;
    cooper_scan_waiting_threads();
    return false;
}

static bool no_worker(bool (*stop)(void)) {
    (void)stop;
    return false;
}

static bool never_due(int p) {
    (void)p;
    return false;
}

static void scanned_thread(void *env) {
    int me = (int)(intptr_t)env;
    volatile char frame = 0;
    cooper_store(&frame_of[me], (int64_t)(uintptr_t)&frame);
    while (!cooper_load(&stop_scanned)) {
        if (cooper_load(&cooper_poll_word)) cooper_poll_slow();
        if (cooper_load(&marking) && cooper_load(&scans_of[me]) == 0) cooper_add(&ran_unscanned, 1);
        if (me % 3 == 0) {
            cooper_yield();
        } else if (me % 3 == 1) {
            cooper_sleep(200000);
        } else {
            int entered = cooper_enter_c();
            usleep(200);
            cooper_exit_c(entered);
        }
    }
    finished();
}

static void test_marking_scans_each_stack_once_before_it_runs(void) {
    start(SCANNED);
    for (intptr_t i = 0; i < SCANNED; i++) cooper_spawn(scanned_thread, (void *)i);
    for (int i = 0; i < SCANNED; i++) {
        while (!cooper_load(&frame_of[i])) cooper_yield();
    }
    for (uint64_t cycle = 1; cycle <= CYCLES; cycle++) {
        CHECK(cooper_stop_the_world());
        for (int i = 0; i < SCANNED; i++) cooper_store(&scans_of[i], 0);
        cooper_store(&marking, 1);
        cooper_mark_begin(1000 + cycle, &(CooperMarkHooks){count_scan, no_worker, idle_scan, never_due, 0});
        cooper_start_the_world();
        cooper_sleep(1000000);
        while (cooper_scan_waiting_threads()) cooper_yield();
        CHECK(cooper_stop_the_world());
        for (int i = 0; i < SCANNED; i++) CHECK(cooper_load(&scans_of[i]) == 1);
        cooper_store(&marking, 0);
        cooper_mark_end();
        cooper_start_the_world();
    }
    CHECK(cooper_load(&ran_unscanned) == 0);
    cooper_store(&stop_scanned, 1);
    wait_all();
}

static int run_tests(void) {
    const char *threads = getenv("COOPER_THREADS");
    int one = threads && atoi(threads) == 1;
    if (one) test_yield_interleaves_in_order();
    test_sleep_and_park();
    test_ready_before_park_leaves_a_permit();
    test_timers_commit_waiters();
    test_many_threads_and_stack_reuse();
    test_deep_recursion_on_a_thread_stack();
    test_a_spinning_thread_is_preempted();
    test_a_blocking_c_call_gives_up_its_p();
    test_marking_scans_each_stack_once_before_it_runs();
    if (one) {
        test_scan_visits_every_stack();
    } else {
        test_ping_pong_across_ms();
        test_idle_ms_steal_work();
        test_stop_the_world();
    }
    if (failures) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    puts("ok");
    return 0;
}

int main(void) {
    cooper_sched_init();
    cooper_run_main(run_tests);
}
