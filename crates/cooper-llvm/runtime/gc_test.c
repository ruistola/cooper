/* A test of the heap allocator on its own (see tests/allocator.rs). */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "cooper.h"

int cooper_heap_lookup(const void *address, void **slot, size_t *slot_size, int *noscan, size_t *value_offset);
int cooper_heap_is_pointer_word(void *slot, size_t word);
void cooper_heap_stats(size_t *bytes, size_t *spans);

/* The test has no module-level variables of its own. */
const struct { const uint64_t *address; const CooperType *type; } cooper_roots[1] = {{0, 0}};
const int64_t cooper_root_count = 0;

_Noreturn void cooper_panic(const char *message) {
    fprintf(stderr, "panic: %s\n", message);
    exit(101);
}

/* ThreadSanitizer makes the tests that collect many times over a live list some 20 times slower: they shrink. */
#ifndef __has_feature
#define __has_feature(x) 0
#endif
#if defined(__SANITIZE_THREAD__) || __has_feature(thread_sanitizer)
#define SCALE 10
#else
#define SCALE 1
#endif

static int failures;
#define CHECK(condition) do { if (!(condition)) { fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #condition); failures++; } } while (0)

typedef struct { uint64_t size, layout_size, flags, words, nsites; const void *sites; uint64_t bits[2]; } Type;

static void test_sizes_and_zeroing(void) {
    static const size_t sizes[] = {1, 7, 8, 9, 16, 24, 100, 512, 513, 1024, 5000, 32000, 32760, 32761, 32768, 40000, 100000};
    for (size_t i = 0; i < sizeof sizes / sizeof *sizes; i++) {
        for (int noscan = 0; noscan < 2; noscan++) {
            Type type = {8, 8, 0, 1, 0, 0, {1}};
            size_t size = sizes[i] & ~(size_t)7 ? (sizes[i] + 7) & ~(size_t)7 : 8;
            unsigned char *p = cooper_alloc(size, noscan ? NULL : (CooperType *)&type);
            CHECK(p && ((uintptr_t)p & 7) == 0);
            for (size_t b = 0; b < size; b++) if (p[b]) { CHECK(0 && "not zeroed"); break; }
            memset(p, 0xAB, size);
            void *slot; size_t slot_size; int ns; size_t offset;
            CHECK(cooper_heap_lookup(p, &slot, &slot_size, &ns, &offset));
            CHECK(ns == noscan);
            CHECK((char *)p == (char *)slot + offset);
            CHECK(slot_size >= size + offset);
            /* Any interior address finds the same object. */
            void *slot2; size_t size2; int ns2; size_t offset2;
            CHECK(cooper_heap_lookup(p + size - 1, &slot2, &size2, &ns2, &offset2) && slot2 == slot);
        }
    }
}

static void test_distinct_objects(void) {
    enum { N = 20000 };
    static unsigned char *objects[N];
    for (int i = 0; i < N; i++) {
        size_t size = 8 + (size_t)(i % 97) * 8;
        objects[i] = cooper_alloc(size, NULL);
        memset(objects[i], i & 0xFF, size);
    }
    for (int i = 0; i < N; i++) {
        size_t size = 8 + (size_t)(i % 97) * 8;
        for (size_t b = 0; b < size; b++) if (objects[i][b] != (i & 0xFF)) { CHECK(0 && "objects overlap"); return; }
    }
}

static void test_pointer_words(void) {
    /* A 24-byte value with pointers in words 0 and 2. */
    Type type = {24, 24, 0, 3, 0, 0, {0x5}};
    for (size_t count = 1; count <= 400; count = count < 30 ? count + 1 : count * 3) {
        size_t size = count * 24;
        char *p = cooper_alloc(size, (CooperType *)&type);
        void *slot; size_t slot_size; int ns; size_t offset;
        CHECK(cooper_heap_lookup(p, &slot, &slot_size, &ns, &offset));
        for (size_t word = 0; word < size / 8; word++) {
            int expected = word % 3 != 1;
            if (cooper_heap_is_pointer_word(slot, word) != expected) {
                fprintf(stderr, "size %zu word %zu\n", size, word);
                CHECK(0 && "pointer word");
                break;
            }
        }
        CHECK(!cooper_heap_is_pointer_word(slot, size / 8 + 1) || size / 8 + 1 >= slot_size / 8);
    }
    /* A value larger than a word of bitmap: 70 words, pointers at 0 and 69. */
    Type wide = {560, 560, 0, 70, 0, 0, {1, 1 << 5}};
    for (int copies = 1; copies <= 70; copies *= 7) {
        char *p = cooper_alloc(560 * (size_t)copies, (CooperType *)&wide);
        void *slot; size_t slot_size; int ns; size_t offset;
        CHECK(cooper_heap_lookup(p, &slot, &slot_size, &ns, &offset));
        for (size_t word = 0; word < 70 * (size_t)copies; word++) {
            int expected = word % 70 == 0 || word % 70 == 69;
            if (cooper_heap_is_pointer_word(slot, word) != expected) { CHECK(0 && "wide pointer word"); break; }
        }
    }
}

static void test_reuse_of_slots_clears_bits(void) {
    Type type = {16, 16, 0, 2, 0, 0, {0x3}};
    Type other = {16, 16, 0, 2, 0, 0, {0x2}};
    void *a = cooper_alloc(16, (CooperType *)&type);
    void *b = cooper_alloc(16, (CooperType *)&other);
    void *slot; size_t slot_size; int ns; size_t offset;
    CHECK(cooper_heap_lookup(a, &slot, &slot_size, &ns, &offset));
    CHECK(cooper_heap_is_pointer_word(slot, 0) && cooper_heap_is_pointer_word(slot, 1));
    CHECK(cooper_heap_lookup(b, &slot, &slot_size, &ns, &offset));
    CHECK(!cooper_heap_is_pointer_word(slot, 0) && cooper_heap_is_pointer_word(slot, 1));
}

static void test_foreign_addresses(void) {
    int local = 0;
    static int global;
    void *slot; size_t slot_size; int ns; size_t offset;
    CHECK(!cooper_heap_lookup(NULL, &slot, &slot_size, &ns, &offset));
    CHECK(!cooper_heap_lookup(&local, &slot, &slot_size, &ns, &offset));
    CHECK(!cooper_heap_lookup(&global, &slot, &slot_size, &ns, &offset));
    CHECK(!cooper_heap_lookup("text", &slot, &slot_size, &ns, &offset));
    /* The part of a span past the last slot, and a free slot, are not objects. */
    char *p = cooper_alloc(8, NULL);
    CHECK(!cooper_heap_lookup(p + 4096, &slot, &slot_size, &ns, &offset) || (char *)slot != p);
    CHECK(cooper_alloc(0, NULL) == cooper_alloc(0, NULL));
}

static void test_many_arenas(void) {
    size_t before_bytes, before_spans;
    cooper_heap_stats(&before_bytes, &before_spans);
    char *last = NULL;
    for (int i = 0; i < 90000; i++) {
        last = cooper_alloc(1024, NULL);
        last[0] = 1;
    }
    size_t bytes, spans;
    cooper_heap_stats(&bytes, &spans);
    CHECK(bytes - before_bytes >= 90000u * 1024 && spans > before_spans);
    void *slot; size_t slot_size; int ns; size_t offset;
    CHECK(cooper_heap_lookup(last + 1023, &slot, &slot_size, &ns, &offset) && slot == last);
    /* An object larger than an arena, and its far end. */
    size_t huge = 70u << 20;
    char *big = cooper_alloc(huge, NULL);
    big[huge - 1] = 7;
    CHECK(cooper_heap_lookup(big + huge - 1, &slot, &slot_size, &ns, &offset) && slot == big && slot_size == huge);
    CHECK(cooper_heap_lookup(big + (40u << 20), &slot, &slot_size, &ns, &offset) && slot == big);
}

/* Overwrite the stack below the caller, so no stale pointer from an earlier test is left in
   a slot a later frame does not initialize (a conservative collector keeps what such a word
   points at). */
static __attribute__((noinline)) void scrub_stack(void) {
    volatile char buffer[1 << 20];
    memset((void *)buffer, 0, sizeof buffer);
}

typedef struct Node { struct Node *next; uint64_t value; } Node;
static Type node_type = {16, 16, 0, 2, 0, 0, {0x1, 0}};

static size_t heap_size(void) {
    size_t bytes, spans;
    cooper_heap_stats(&bytes, &spans);
    return bytes;
}

static void test_garbage_is_reclaimed(void) {
    scrub_stack();
    for (int i = 0; i < 3000000; i++) {
        volatile char *p = cooper_alloc(64, NULL);
        p[0] = 1;
    }
    /* Without reclamation these would be 192 MB. */
    CHECK(heap_size() < (64u << 20));
}

static void test_reachable_objects_survive(void) {
    Node *volatile head = NULL;
    for (uint64_t i = 0; i < 100000 / SCALE; i++) {
        Node *node = cooper_alloc(16, (CooperType *)&node_type);
        node->value = i;
        node->next = head;
        head = node;
    }
    for (int i = 0; i < 2000000 / SCALE; i++) {
        volatile char *p = cooper_alloc(48, NULL);
        p[0] = 1;
    }
    cooper_gc_collect();
    uint64_t expect = 100000 / SCALE - 1;
    for (Node *node = head; node; node = node->next, expect--) {
        if (node->value != expect) { CHECK(0 && "a list node was lost or reused"); return; }
    }
    CHECK(expect == (uint64_t)-1);
}

static void test_interior_pointer_keeps_an_object(void) {
    char *volatile inside;
    {
        char *p = cooper_alloc(1000, NULL);
        memset(p, 0x5A, 1000);
        inside = p + 500;
    }
    for (int i = 0; i < 1000000; i++) {
        volatile char *q = cooper_alloc(1000, NULL);
        q[0] = 1;
    }
    cooper_gc_collect();
    CHECK(inside[-500] == 0x5A && inside[499] == 0x5A);
}

static void test_large_objects_are_reused(void) {
    for (int i = 0; i < 60; i++) {
        volatile char *p = cooper_alloc((size_t)10 << 20, NULL);
        p[0] = 1;
        p[((size_t)10 << 20) - 1] = 1;
        cooper_gc_collect();
    }
    /* 600 MB were allocated in all, of which at most a few are live at once. */
    CHECK(heap_size() < (200u << 20));
}

/* Allocate `size` bytes filled with `fill`, store their address in `*payload` unless that is NULL, and give
   the address complemented, which is not a pointer to the collector. The address is in hand only in this
   frame, below the caller's, which `scrub_stack` then overwrites: in the caller's own frame an optimizing
   compiler or ThreadSanitizer's instrumentation would keep it in a register or a spill slot the collector
   scans. */
static __attribute__((noinline)) uintptr_t allocate_hidden(size_t size, unsigned char fill, uint64_t *payload) {
    unsigned char *p = cooper_alloc(size, NULL);
    memset(p, fill, size);
    if (payload) *payload = (uintptr_t)p;
    return ~(uintptr_t)p;
}

/* The address `allocate_hidden` gave. The empty asm keeps the compiler from holding the address itself,
   rather than its complement, across the collection. */
static unsigned char *reveal(uintptr_t hidden) {
    __asm__ volatile("" : "+r"(hidden));
    return (unsigned char *)~hidden;
}

static void test_freed_memory_is_poisoned(void) {
    uintptr_t hidden = allocate_hidden(256, 1, NULL);
    for (int i = 0; i < 10; i++) cooper_alloc(256, NULL);
    scrub_stack();
    cooper_gc_collect();
    /* Nothing refers to it any more; the poison is what a stale pointer would find. */
    CHECK(*reveal(hidden) == 0xDB);
}

/* A sum type { Num(u64), Ref(ptr) } as the compiler lays it out: a u32 tag, then the payload at byte 8. */
typedef struct { uint32_t tag; uint64_t payload; } Sum;
static const Type ref_payload = {8, 8, 0, 1, 0, 0, {1}};
static const CooperType *const sum_variants[2] = {NULL, (const CooperType *)&ref_payload};
static const CooperSite sum_site = {0, 8, 2, sum_variants};
static const struct { uint64_t size, layout_size, flags, words, nsites; const CooperSite *sites; uint64_t bits[1]; }
    sum_type = {16, 16, 0, 2, 1, &sum_site, {0}};

static void test_sum_type_scans_only_the_live_variant(void) {
    scrub_stack();
    Sum *volatile holder = cooper_alloc(2 * sizeof(Sum), (const CooperType *)&sum_type);
    holder[0].tag = 1;                      /* Ref: a pointer, which keeps its target */
    uintptr_t hidden_kept = allocate_hidden(100, 0x11, &holder[0].payload);
    holder[1].tag = 0;                      /* Num: an integer that happens to be an address */
    uintptr_t hidden_dropped = allocate_hidden(100, 0x22, &holder[1].payload);
    scrub_stack();
    cooper_gc_collect();
    CHECK(reveal(hidden_kept)[50] == 0x11);
    CHECK(reveal(hidden_dropped)[50] == 0xDB);
}

/* While the barrier is on, a store shades the pointers it overwrites and those it stores: their objects stay
   marked until the next collection ends, whatever refers to them. Here nothing does once the stores are done, so
   the objects shaded survive that collection and the others are freed. */
enum { SHADED = 6, NOT_SHADED = 3 };

static __attribute__((noinline)) void barrier_stores(uintptr_t shaded[SHADED], uintptr_t not_shaded[NOT_SHADED]) {
    uint64_t word = 0;
    shaded[0] = allocate_hidden(100, 0x31, &word);
    shaded[1] = allocate_hidden(100, 0x32, NULL);
    cooper_barrier_pointer((void *)word, reveal(shaded[1]));

    /* A sum type's pointers are those of the variant it holds: before the store, and in the value stored. */
    Sum old = {1, 0}, new = {0, 0};
    shaded[2] = allocate_hidden(100, 0x33, &old.payload);
    not_shaded[0] = allocate_hidden(100, 0x41, &new.payload);
    cooper_barrier_typed(&old, &new, (const CooperType *)&sum_type);

    Sum from[2] = {{1, 0}, {0, 0}}, to[2] = {{1, 0}, {1, 0}};
    shaded[3] = allocate_hidden(100, 0x34, &from[0].payload);
    not_shaded[1] = allocate_hidden(100, 0x42, &from[1].payload);
    shaded[4] = allocate_hidden(100, 0x35, &to[1].payload);
    cooper_barrier_on = 1;
    cooper_typed_copy(to, from, sizeof to, (const CooperType *)&sum_type);
    CHECK(memcmp(to, from, sizeof to) == 0);

    /* C's store tests the flag itself. */
    uint64_t slot = 0;
    shaded[5] = allocate_hidden(100, 0x36, NULL);
    cooper_store_pointer((void **)&slot, reveal(shaded[5]));
    CHECK(slot == (uintptr_t)reveal(shaded[5]));
    cooper_barrier_on = 0;
    not_shaded[2] = allocate_hidden(100, 0x43, &slot);
    cooper_store_pointer((void **)&slot, NULL);
    CHECK(slot == 0);
}

static void test_barrier_shades_old_and_new_pointers(void) {
    uintptr_t shaded[SHADED], not_shaded[NOT_SHADED];
    barrier_stores(shaded, not_shaded);
    scrub_stack();
    cooper_gc_collect();
    for (int i = 0; i < SHADED; i++) CHECK(reveal(shaded[i])[50] == 0x31 + i);
    for (int i = 0; i < NOT_SHADED; i++) CHECK(reveal(not_shaded[i])[50] == 0xDB);
}

/* Threads on several Ps allocate from their own caches while collections stop them: each builds a list
   among garbage of every kind and checks it, and nothing a thread holds may be freed or handed out twice. */
enum { WORKERS = 8, NODES = 30000 / SCALE };
static int64_t finished, worker_failures;

static void worker(void *env) {
    uint64_t seed = (uint64_t)(uintptr_t)env;
    Node *volatile head = NULL;
    for (uint64_t i = 0; i < NODES; i++) {
        Node *node = cooper_alloc(16, (CooperType *)&node_type);
        node->value = i ^ seed;
        node->next = head;
        head = node;
        for (int k = 0; k < 4; k++) {
            size_t size = 16 + (size_t)((i * 7 + (uint64_t)k * 13) % 100) * 16;
            volatile char *garbage = cooper_alloc(size, k & 1 ? NULL : (CooperType *)&node_type);
            garbage[0] = 1;
            garbage[size - 1] = 1;
        }
        if (i % 5000 == 0) {
            volatile char *large = cooper_alloc(100000, NULL);
            large[99999] = 1;
        }
    }
    uint64_t expect = NODES - 1;
    for (Node *node = head; node; node = node->next, expect--) {
        if (node->value != (expect ^ seed)) { __atomic_add_fetch(&worker_failures, 1, __ATOMIC_SEQ_CST); break; }
    }
    if (expect != (uint64_t)-1) __atomic_add_fetch(&worker_failures, 1, __ATOMIC_SEQ_CST);
    __atomic_add_fetch(&finished, 1, __ATOMIC_SEQ_CST);
}

static void test_threads_allocate_in_parallel(void) {
    for (uintptr_t i = 0; i < WORKERS; i++) cooper_spawn((void *)worker, (void *)(i + 1));
    while (__atomic_load_n(&finished, __ATOMIC_SEQ_CST) < WORKERS) cooper_yield();
    CHECK(worker_failures == 0);
}

/* A live heap big enough that the mark workers must share it: a binary tree (many small objects to hand out
   in blocks and steal), and an array of pointers larger than an oblet (scanned in pieces by different
   workers). Threads that allocate garbage meanwhile force collections, and every mark is checked by
   checkmark. A mark that lost an object shows as a corrupt tree, an overwritten slot, or a checkmark panic. */
typedef struct Tree { struct Tree *left, *right; uint64_t value; } Tree;
static Type tree_type = {24, 24, 0, 3, 0, 0, {0x3, 0}};
enum { DEPTH = 16, ARRAY_LENGTH = 40000 };

static Tree *build(int depth, uint64_t *counter) {
    Tree *node = cooper_alloc(sizeof(Tree), (CooperType *)&tree_type);
    node->value = (*counter)++;
    if (depth > 0) {
        node->left = build(depth - 1, counter);
        node->right = build(depth - 1, counter);
    }
    return node;
}

static int verify(const Tree *node, int depth, uint64_t *counter) {
    if (!node || node->value != (*counter)++) return 0;
    return depth == 0 ? !node->left && !node->right
                      : verify(node->left, depth - 1, counter) && verify(node->right, depth - 1, counter);
}

static int64_t garbage_finished;

static void garbage_worker(void *env) {
    (void)env;
    for (int i = 0; i < 100000; i++) {
        volatile char *p = cooper_alloc(16 + (size_t)(i % 50) * 16, i & 1 ? NULL : (CooperType *)&node_type);
        p[0] = 1;
    }
    __atomic_add_fetch(&garbage_finished, 1, __ATOMIC_SEQ_CST);
}

static void test_parallel_marking_keeps_a_large_shared_heap(void) {
    static Type pointer_type = {8, 8, 0, 1, 0, 0, {1}};
    uint64_t counter = 0;
    Tree *volatile tree = build(DEPTH, &counter);
    Tree **volatile array = cooper_alloc(ARRAY_LENGTH * sizeof(Tree *), (CooperType *)&pointer_type);
    for (uint64_t i = 0; i < ARRAY_LENGTH; i++) {
        array[i] = cooper_alloc(sizeof(Tree), (CooperType *)&tree_type);
        array[i]->value = i;
    }
    for (int i = 0; i < 4; i++) cooper_spawn((void *)garbage_worker, NULL);
    for (int round = 0; round < 3; round++) cooper_gc_collect();
    while (__atomic_load_n(&garbage_finished, __ATOMIC_SEQ_CST) < 4) cooper_yield();
    cooper_gc_collect();
    counter = 0;
    CHECK(verify(tree, DEPTH, &counter));
    for (uint64_t i = 0; i < ARRAY_LENGTH; i++) {
        if (array[i]->value != i) { CHECK(0 && "an array element was lost or reused"); break; }
    }
}

/* Threads move pointers around a shared array while the collector marks, as generated code does: a store into
   the heap goes through the barrier while it is on, and what a thread moves is meanwhile only in its frame,
   which is scanned once in a cycle. Another thread stores from C the objects its Cooper frame holds, while
   the collector scans that frame. The garbage they allocate keeps cycles going, each checked by checkmark. A
   lost object shows as a value overwritten by poisoning. */
enum { SHARED = 512, MOVERS = 4, MOVES = 300000 / SCALE, C_ROUNDS = 2000 / SCALE, MAGIC = 0x600d };
static int64_t movers_finished, moved_while_marking, moved_wrong;

static void store_shared(Tree **slot, Tree *value) {
    if (__atomic_load_n(&cooper_barrier_on, __ATOMIC_RELAXED)) {
        __atomic_add_fetch(&moved_while_marking, 1, __ATOMIC_RELAXED);
        cooper_barrier_pointer(__atomic_load_n(slot, __ATOMIC_RELAXED), value);
    }
    __atomic_store_n(slot, value, __ATOMIC_RELEASE);
}

static Tree *new_pair(void) {
    Tree *node = cooper_alloc(sizeof(Tree), (CooperType *)&tree_type);
    node->left = cooper_alloc(sizeof(Tree), (CooperType *)&tree_type);
    node->value = node->left->value = MAGIC;
    return node;
}

static int intact(const Tree *node) {
    return node->value == MAGIC && node->left->value == MAGIC;
}

static uint64_t next_random(uint64_t *state) {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    return *state;
}

static void mover(void *env) {
    Tree **shared = env;
    uint64_t random = (uint64_t)(uintptr_t)&random | 1;
    for (int i = 0; i < MOVES; i++) {
        if (__atomic_load_n(&cooper_poll_word, __ATOMIC_RELAXED)) cooper_poll_slow();
        size_t a = next_random(&random) % SHARED, b = next_random(&random) % SHARED;
        Tree *held = __atomic_load_n(&shared[a], __ATOMIC_ACQUIRE);
        store_shared(&shared[a], __atomic_load_n(&shared[b], __ATOMIC_ACQUIRE));
        store_shared(&shared[b], i % 16 ? held : new_pair());
        if (!intact(held)) __atomic_add_fetch(&moved_wrong, 1, __ATOMIC_RELAXED);
        volatile char *garbage = cooper_alloc(16 + (size_t)(i % 50) * 16, i & 1 ? NULL : (CooperType *)&node_type);
        garbage[0] = 1;
    }
    __atomic_add_fetch(&movers_finished, 1, __ATOMIC_SEQ_CST);
}

/* In C: store what the caller's frame holds, as C may. The collector reads that frame meanwhile, so C keeps its
   own state in its own frame. */
static __attribute__((noinline)) uint64_t store_from_c(Tree **shared, Tree *const *mine, uint64_t random) {
    for (int i = 0; i < 100; i++) {
        size_t a = next_random(&random) % SHARED;
        cooper_store_pointer((void **)&shared[a], mine[i % 4]);
    }
    return random;
}

static void c_mover(void *env) {
    Tree **shared = env;
    uint64_t random = 0x9e3779b97f4a7c15;
    for (int round = 0; round < C_ROUNDS; round++) {
        if (__atomic_load_n(&cooper_poll_word, __ATOMIC_RELAXED)) cooper_poll_slow();
        Tree *mine[4] = {new_pair(), new_pair(), new_pair(), new_pair()};
        int entered = cooper_enter_c();
        random = store_from_c(shared, mine, random);
        cooper_exit_c(entered);
    }
    __atomic_add_fetch(&movers_finished, 1, __ATOMIC_SEQ_CST);
}

static void test_mutators_move_pointers_while_marking(void) {
    static Type pointer_type = {8, 8, 0, 1, 0, 0, {1}};
    Tree **volatile shared = cooper_alloc(SHARED * sizeof(Tree *), (CooperType *)&pointer_type);
    for (int i = 0; i < SHARED; i++) shared[i] = new_pair();
    for (int i = 0; i < MOVERS; i++) cooper_spawn((void *)mover, shared);
    cooper_spawn((void *)c_mover, shared);
    while (__atomic_load_n(&movers_finished, __ATOMIC_SEQ_CST) < MOVERS + 1) cooper_yield();
    cooper_gc_collect();
    CHECK(moved_while_marking > 0);
    CHECK(moved_wrong == 0);
    for (int i = 0; i < SHARED; i++) {
        if (!intact(shared[i])) { CHECK(0 && "a shared object was lost"); break; }
    }
}

static int run_collector_tests(void) {
    test_parallel_marking_keeps_a_large_shared_heap();
    test_garbage_is_reclaimed();
    test_reachable_objects_survive();
    test_interior_pointer_keeps_an_object();
    test_large_objects_are_reused();
    test_freed_memory_is_poisoned();
    test_sum_type_scans_only_the_live_variant();
    test_barrier_shades_old_and_new_pointers();
    test_threads_allocate_in_parallel();
    test_mutators_move_pointers_while_marking();
    if (failures) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    puts("ok");
    return 0;
}

int main(void) {
    setenv("COOPER_GC", "poison,checkmark", 1);
    /* These keep their objects where the collector does not look, so it must not run yet. */
    test_sizes_and_zeroing();
    test_distinct_objects();
    test_pointer_words();
    test_reuse_of_slots_clears_bits();
    test_foreign_addresses();
    test_many_arenas();
    setenv("COOPER_THREADS", "4", 1);
    cooper_gc_init();
    cooper_run_main(run_collector_tests);
}
