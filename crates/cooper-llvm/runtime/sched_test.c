/* A test of the scheduler on its own (see tests/scheduler.rs). */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "cooper.h"

_Noreturn void cooper_panic(const char *message) {
    fprintf(stderr, "panic: %s\n", message);
    exit(101);
}

/* The scheduler does not allocate from the collector; these are what it asks of it. */
void cooper_gc_collect(void) {}

static int failures;
#define CHECK(condition) do { if (!(condition)) { fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #condition); failures++; } } while (0)

static char log_buffer[4096];
static void note(const char *text) { strcat(log_buffer, text); }

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

static void test_yield_interleaves_in_order(void) {
    log_buffer[0] = 0;
    Worker a = {"a", 3}, b = {"b", 2}, c = {"c", 3};
    cooper_co(worker, &a);
    cooper_co(worker, &b);
    cooper_co(worker, &c);
    for (int i = 0; i < 10; i++) cooper_yield();
    CHECK(strcmp(log_buffer, "a0 b0 c0 a1 b1 c1 a2 c2 ") == 0);
}

static void *parked;
static int woke;

static void sleeper_a(void *env) { (void)env; cooper_sleep(30000000); note("slow "); }
static void sleeper_b(void *env) { (void)env; cooper_sleep(10000000); note("fast "); }

static void parker(void *env) {
    (void)env;
    parked = cooper_current_task();
    cooper_park();
    woke = 1;
}

static void test_sleep_and_park(void) {
    log_buffer[0] = 0;
    cooper_co(sleeper_a, NULL);
    cooper_co(sleeper_b, NULL);
    cooper_co(parker, NULL);
    cooper_sleep(60000000);
    CHECK(strcmp(log_buffer, "fast slow ") == 0);
    CHECK(parked && !woke);
    cooper_ready(parked);
    cooper_yield();
    CHECK(woke);
}

static long counter;
static void count(void *env) { (void)env; counter++; cooper_yield(); counter++; }

static void test_many_tasks_and_stack_reuse(void) {
    counter = 0;
    for (int round = 0; round < 3; round++) {
        for (int i = 0; i < 2000; i++) cooper_co(count, NULL);
        for (int i = 0; i < 4; i++) cooper_yield();
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
static void deep(void *env) { depth_reached = recurse((long)env); }

static void test_deep_recursion_on_a_task_stack(void) {
    /* About 60000 frames of over 100 bytes: several MiB, well inside the 8 MiB reservation. */
    cooper_co(deep, (void *)60000);
    for (int i = 0; i < 3; i++) cooper_yield();
    CHECK(depth_reached >= 0);
}

static int ranges_seen;
static size_t largest_range;
static void inspect(uintptr_t low, uintptr_t high) {
    ranges_seen++;
    if (high - low > largest_range) largest_range = high - low;
}

static void test_scan_visits_other_stacks(void) {
    Worker z = {"z", 1};
    cooper_co(worker, &z);
    cooper_scan_tasks(inspect);
    /* The new task's stack (its prepared frame), and each of the two tasks' code and environment
       words. The running task's own stack is the collector's to scan, and is not offered. */
    CHECK(ranges_seen == 3);
    CHECK(largest_range >= 64);
    for (int i = 0; i < 3; i++) cooper_yield();
}

int main(void) {
    int base;
    cooper_sched_init(&base);
    test_yield_interleaves_in_order();
    test_sleep_and_park();
    test_many_tasks_and_stack_reuse();
    test_deep_recursion_on_a_task_stack();
    test_scan_visits_other_stacks();
    if (failures) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    puts("ok");
    return 0;
}
