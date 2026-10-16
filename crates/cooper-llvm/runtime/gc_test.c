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
    for (uint64_t i = 0; i < 100000; i++) {
        Node *node = cooper_alloc(16, (CooperType *)&node_type);
        node->value = i;
        node->next = head;
        head = node;
    }
    for (int i = 0; i < 2000000; i++) {
        volatile char *p = cooper_alloc(48, NULL);
        p[0] = 1;
    }
    cooper_gc_collect();
    uint64_t expect = 99999;
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

static void test_freed_memory_is_poisoned(void) {
    volatile unsigned char *address;
    {
        address = cooper_alloc(256, NULL);
        address[0] = 1;
    }
    for (int i = 0; i < 10; i++) cooper_alloc(256, NULL);
    uintptr_t hidden = ~(uintptr_t)address;  /* not a pointer to the collector */
    address = NULL;
    scrub_stack();
    cooper_gc_collect();
    /* Nothing refers to it any more; the poison is what a stale pointer would find. */
    unsigned char first = *(volatile unsigned char *)~hidden;
    CHECK(first == 0xDB);
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
    unsigned char *kept = cooper_alloc(100, NULL);
    unsigned char *dropped = cooper_alloc(100, NULL);
    memset(kept, 0x11, 100);
    memset(dropped, 0x22, 100);
    holder[0].tag = 1;                      /* Ref: a pointer, which keeps its target */
    holder[0].payload = (uintptr_t)kept;
    holder[1].tag = 0;                      /* Num: an integer that happens to be an address */
    holder[1].payload = (uintptr_t)dropped;
    uintptr_t hidden_kept = ~(uintptr_t)kept, hidden_dropped = ~(uintptr_t)dropped;
    kept = dropped = NULL;
    scrub_stack();
    cooper_gc_collect();
    CHECK(((unsigned char *)~hidden_kept)[50] == 0x11);
    CHECK(((unsigned char *)~hidden_dropped)[50] == 0xDB);
}

int main(void) {
    int stack_base;
    setenv("COOPER_GC", "poison", 1);
    /* These keep their objects where the collector does not look, so it must not run yet. */
    test_sizes_and_zeroing();
    test_distinct_objects();
    test_pointer_words();
    test_reuse_of_slots_clears_bits();
    test_foreign_addresses();
    test_many_arenas();
    cooper_gc_init(&stack_base);
    test_garbage_is_reclaimed();
    test_reachable_objects_survive();
    test_interior_pointer_keeps_an_object();
    test_large_objects_are_reused();
    test_freed_memory_is_poisoned();
    test_sum_type_scans_only_the_live_variant();
    if (failures) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    puts("ok");
    return 0;
}
