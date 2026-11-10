/*
 * The Cooper heap allocator, after Go's: memory comes in arenas, which are cut into pages and
 * the pages grouped into spans. A small object (up to 32 KiB) lives in a span of equal-sized
 * slots of one size class; a larger object has a span of its own. The collector will find the
 * span of any address from the arena table, and the object within it by arithmetic.
 *
 * Which words of an object are pointers is recorded as it is allocated, from the type
 * descriptor the compiler passed (see cooper.h):
 *   - a pointer-free object (null type) is in a "noscan" span and has no such record;
 *   - a small object (up to 512 bytes) has one bit per word in a bitmap at the end of its span;
 *   - a larger object up to 32 KiB has a word before it holding its type, tiled over the object;
 *   - an object with a span of its own has the type in the span.
 *
 * The collector marks while the program runs, after Go's: a short stop of the world begins marking (it turns on
 * the write barrier and scans the module-level variables), mark workers on otherwise idle Ps and allocations that
 * reach the heap goal do the marking, each thread's stack is scanned once as the scheduler arranges (sched.c), and
 * a second short stop ends it (it finishes what the Ps' workers hold, turns off the barrier) and sweeps.
 * The heap is precise: an object's pointers are found from the record above. The roots are the module-level variables
 * (a table the compiler emits, with their types), and the stack and registers, which are scanned
 * conservatively: any word that points into an allocated object keeps it. Nothing moves, so an
 * ambiguous word costs only the retention of what it appears to point at.
 *
 * COOPER_GC=stress collects at every allocation, and COOPER_GC=poison (or stress) fills freed
 * memory with 0xDB, so that a missed root shows up as a crash rather than as silent reuse. COOPER_GC=markstress
 * (which also poisons) keeps marking on almost all the time, a new cycle beginning at the first allocation after
 * one ends, and each allocation marking a little. COOPER_GC=checkmark marks again serially after every mark, and
 * fails on any object the mark missed.
 */

#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>

#include "cooper.h"

#define PAGE_SHIFT 13
#define PAGE_SIZE ((size_t)1 << PAGE_SHIFT)
#define ARENA_SHIFT 26
#define ARENA_BYTES ((size_t)1 << ARENA_SHIFT)
#define ARENA_PAGES (ARENA_BYTES >> PAGE_SHIFT)
#define MAX_SMALL 32768
#define NUM_CLASSES 68
#define MIN_SIZE_FOR_HEADER 512 /* exclusive: a larger scannable object has a header word */
#define HEADER_SIZE 8
#define TABLE_BITS 11
#define MIN_HEAP ((size_t)4 << 20) /* the smallest heap that is worth collecting */

enum { SPAN_FREE = 0, SPAN_IN_USE = 1 };

typedef struct Span {
    uintptr_t start;
    size_t npages;
    size_t elemsize;     /* the slot size, or for a large object its size rounded up to a page */
    uint32_t nelems;
    uint32_t freeindex;  /* slots below this are allocated; above it, allocated iff alloc_bits says so (atomic:
                            mark workers read it while the P allocating from the span changes it) */
    uint32_t alloc_count;
    uint16_t spanclass;  /* size class * 3 + kind, or 0 for a large object */
    uint8_t state;
    uint8_t noscan;
    uint8_t large;
    uint8_t heap_bits_in_span;
    uint8_t needzero;    /* the span's memory may be dirty: zero a slot when it is handed out */
    uint64_t *alloc_bits; /* allocated as of the last collection */
    uint64_t *mark_bits;  /* reached by the collector */
    uint64_t *scan_bits;  /* already scanned (scannable small objects only) */
    uint8_t queued;       /* on a mark worker's span queue */
    uint32_t div_magic;   /* index = offset * div_magic >> 32, as Go's size classes do: no divide */
    size_t bits_words;    /* the capacity of the two bit arrays, in words */
    const CooperType *large_type;
    size_t large_size;
    struct Arena *arena;
    size_t first_page;    /* the span's first page within its arena */
    struct Span *all_next;     /* every span in use */
    struct Span *partial_next; /* spans of a class with free slots */
} Span;

typedef struct Arena {
    uintptr_t base;
    size_t npages;
    size_t next_page;    /* pages before this are in spans */
    Span **spans;        /* the span of each page */
} Arena;

/* --- metadata memory: never freed, never scanned --- */

static char *meta_next, *meta_end;

static void *map_zeroed(size_t bytes) {
    void *memory = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (memory == MAP_FAILED) cooper_panic("out of memory");
    return memory;
}

static void *meta_alloc(size_t bytes) {
    bytes = (bytes + 15) & ~(size_t)15;
    if ((size_t)(meta_end - meta_next) < bytes) {
        size_t chunk = bytes > (1 << 20) ? bytes : (1 << 20);
        meta_next = map_zeroed(chunk);
        meta_end = meta_next + chunk;
    }
    void *out = meta_next;
    meta_next += bytes;
    return out;
}

/* --- size classes (Go's) --- */

static const uint16_t class_size[NUM_CLASSES] = {
    0, 8, 16, 24, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288, 320,
    352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024, 1152, 1280, 1408, 1536, 1792,
    2048, 2304, 2688, 3072, 3200, 3456, 4096, 4864, 5376, 6144, 6528, 6784, 6912, 8192, 9472,
    9728, 10240, 10880, 12288, 13568, 14336, 16384, 18432, 19072, 20480, 21760, 24576, 27264,
    28672, 32768};
static const uint8_t class_pages[NUM_CLASSES] = {
    0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 2, 1, 2, 1, 2, 1, 3, 2, 3, 1, 3, 2, 3, 4, 5, 6, 1, 7, 6, 5, 4, 3, 5, 7, 2, 9, 7,
    5, 8, 3, 10, 7, 4};

static uint8_t size_to_class8[MAX_SMALL / 8 + 1];

static void init_classes(void) {
    for (size_t index = 0; index <= MAX_SMALL / 8; index++) {
        size_t size = index * 8;
        int class = 1;
        while (class < NUM_CLASSES - 1 && class_size[class] < size) class++;
        size_to_class8[index] = (uint8_t)class;
    }
}

/* --- arenas and the page heap --- */

/* Every global of the allocator and collector, by who may touch it:
     immutable after init:  class_size, class_pages, size_to_class8, stress, markstress, poison, checkmark, growth,
                            gc_off, trace, instrument, zerobase, started, ready
     collector only (a stopped world, with the heap lock): stats, and the fields of `mark` not marked
                            otherwise; `phase` and `cycles`, which are also read atomically
     mark workers: `workers` (each P's its own, but rings are shared), and `mark` by its own lock and atomics;
                            mark and scan bits and `Span.queued` atomically
   per-P (no lock): a P's AllocCache, and the spans in it, which only that P allocates from. The collector
                            reads and clears every cache with the world stopped.
   The rest (arena table, current_arena, heap_bytes, span_count, heap_used, next_gc, trigger, pace_at, free_runs,
   spare_runs, span_pool, all_spans, partial, meta_next, meta_end, released_bytes) are the heap lock's: refills,
   large objects and everything that changes the span structure. heap_used and pace_at are also read without it,
   and mark workers find spans through the arena table without it, so a span is published there (atomically)
   once it is complete. */
static int64_t heap_word; /* the heap lock */
static Arena **arena_table[1 << TABLE_BITS]; /* [address >> 26 >> 11][(address >> 26) & 2047] */
static Arena *current_arena;
static size_t heap_bytes, span_count;
static size_t heap_used;  /* bytes in allocated slots: live as of the last collection, plus allocated since */
static size_t next_gc;    /* the heap goal: marking should end before `heap_used` reaches this */
static size_t trigger;    /* begin marking when `heap_used` reaches this */
static size_t pace_at;    /* `trigger` while not marking, `next_gc` while marking: an allocation reaching it acts */
static int stress, markstress, poison, checkmark;
static int started;            /* cooper_gc_init has run: collections may begin */
static unsigned growth = 100;  /* the heap may grow to this percent over what survived: COOPER_HEAP_GROWTH */
static int gc_off;

/* --- measurements: COOPER_GC=trace prints a line per collection, COOPER_GC_STATS=1 a summary at exit --- */

static struct {
    uint64_t collections, bytes_allocated;
    uint64_t mark_ns;        /* from the beginning of marking to its end, while the program ran */
    uint64_t mark_worker_ns; /* spent marking, summed over the workers, assists and the end's helpers */
    uint64_t pause_ns, max_pause_ns, sweep_ns;
    size_t peak_heap, last_live;
} stats;
static int trace, instrument; /* instrument: trace or statistics are on */
static size_t released_bytes; /* memory returned to the operating system, in all */

static uint64_t now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000000u + (uint64_t)t.tv_nsec;
}

static void register_arena(Arena *arena, size_t chunks) {
    for (size_t chunk = 0; chunk < chunks; chunk++) {
        uintptr_t index = (arena->base >> ARENA_SHIFT) + chunk;
        Arena ***top = &arena_table[(index >> TABLE_BITS) & ((1 << TABLE_BITS) - 1)];
        if (!*top) __atomic_store_n(top, meta_alloc(sizeof(Arena *) << TABLE_BITS), __ATOMIC_RELEASE);
        __atomic_store_n(&(*top)[index & ((1 << TABLE_BITS) - 1)], arena, __ATOMIC_RELEASE);
    }
}

static Arena *new_arena(size_t pages_wanted) {
    size_t chunks = (pages_wanted * PAGE_SIZE + ARENA_BYTES - 1) / ARENA_BYTES;
    size_t bytes = chunks * ARENA_BYTES;
    /* Map extra so the arena can start on a multiple of its size. */
    uintptr_t raw = (uintptr_t)map_zeroed(bytes + ARENA_BYTES);
    uintptr_t base = (raw + ARENA_BYTES - 1) & ~(uintptr_t)(ARENA_BYTES - 1);
    if (base > raw) munmap((void *)raw, base - raw);
    uintptr_t tail = raw + bytes + ARENA_BYTES - (base + bytes);
    if (tail) munmap((void *)(base + bytes), tail);
    Arena *arena = meta_alloc(sizeof(Arena));
    arena->base = base;
    arena->npages = bytes >> PAGE_SHIFT;
    arena->spans = meta_alloc(arena->npages * sizeof(Span *));
    register_arena(arena, chunks);
    return arena;
}

/* Free runs of pages, in address order (arenas in the order they were made, then page). */
typedef struct Run {
    Arena *arena;
    size_t first_page, npages;
    int released; /* the memory has been given back to the operating system */
    struct Run *next;
} Run;

static Run *free_runs, *spare_runs;
static Span *span_pool, *all_spans;

static Run *new_run(void) {
    Run *run = spare_runs;
    if (run) spare_runs = run->next;
    else run = meta_alloc(sizeof(Run));
    return run;
}

/* Give a span's pages back to the page heap, merging with free neighbours. */
static void free_pages(Arena *arena, size_t first_page, size_t npages) {
    Run **link = &free_runs;
    while (*link && ((*link)->arena != arena ? (*link)->arena->base < arena->base : (*link)->first_page < first_page)) {
        link = &(*link)->next;
    }
    Run *run = new_run();
    run->arena = arena;
    run->first_page = first_page;
    run->npages = npages;
    run->released = 0;
    run->next = *link;
    *link = run;
    /* Merge with the next run, then the one before. */
    Run *after = run->next;
    if (after && after->arena == arena && run->first_page + run->npages == after->first_page) {
        run->npages += after->npages;
        run->released = 0;
        run->next = after->next;
        after->next = spare_runs;
        spare_runs = after;
    }
    for (Run *before = free_runs; before && before != run; before = before->next) {
        if (before->next == run && before->arena == arena && before->first_page + before->npages == run->first_page) {
            before->npages += run->npages;
            before->released = 0;
            before->next = run->next;
            run->next = spare_runs;
            spare_runs = run;
            break;
        }
    }
}

/* A new span of `npages` pages: from a free run if one is big enough (its memory is dirty),
   otherwise from fresh memory. The caller publishes it once it is complete. */
static Span *new_span(size_t npages) {
    Arena *arena = NULL;
    size_t first_page = 0;
    int dirty = 0;
    for (Run **link = &free_runs; *link; link = &(*link)->next) {
        Run *run = *link;
        if (run->npages < npages) continue;
        arena = run->arena;
        first_page = run->first_page;
        dirty = 1;
#ifdef MADV_FREE_REUSE
        /* Tell the system the pages are in use again (it accounts for them that way). */
        if (run->released) madvise((void *)(arena->base + (first_page << PAGE_SHIFT)), npages << PAGE_SHIFT, MADV_FREE_REUSE);
#endif
        run->first_page += npages;
        run->npages -= npages;
        if (run->npages == 0) {
            *link = run->next;
            run->next = spare_runs;
            spare_runs = run;
        }
        break;
    }
    if (!arena) {
        if (!current_arena || current_arena->npages - current_arena->next_page < npages) {
            current_arena = new_arena(npages > ARENA_PAGES ? npages : ARENA_PAGES);
        }
        arena = current_arena;
        first_page = arena->next_page;
        __atomic_store_n(&arena->next_page, first_page + npages, __ATOMIC_RELAXED);
    }
    Span *span = span_pool;
    if (span) span_pool = span->all_next;
    else span = meta_alloc(sizeof(Span));
    size_t words = span->bits_words;
    uint64_t *alloc_bits = span->alloc_bits, *mark_bits = span->mark_bits, *scan_bits = span->scan_bits;
    memset(span, 0, sizeof *span);
    span->bits_words = words;
    span->alloc_bits = alloc_bits;
    span->mark_bits = mark_bits;
    span->scan_bits = scan_bits;
    span->arena = arena;
    span->first_page = first_page;
    span->start = arena->base + (first_page << PAGE_SHIFT);
    span->npages = npages;
    span->state = SPAN_IN_USE;
    span->needzero = (uint8_t)dirty;
    span->all_next = all_spans;
    all_spans = span;
    heap_bytes += npages << PAGE_SHIFT;
    span_count++;
    return span;
}

/* Make the span's pages lead to `span`, or to nothing (null). */
static void set_span_pages(Span *span, Span *value) {
    for (size_t page = 0; page < span->npages; page++) {
        __atomic_store_n(&span->arena->spans[span->first_page + page], value, __ATOMIC_RELEASE);
    }
}

/* Bit arrays for `words` words, reusing the span's if they are big enough. They start zeroed. */
static void span_bits(Span *span, size_t words) {
    if (span->bits_words >= words) {
        memset(span->alloc_bits, 0, words * 8);
        memset(span->mark_bits, 0, words * 8);
        memset(span->scan_bits, 0, words * 8);
        return;
    }
    span->alloc_bits = meta_alloc(words * 8);
    span->mark_bits = meta_alloc(words * 8);
    span->scan_bits = meta_alloc(words * 8);
    span->bits_words = words;
}

/* The span an address is in, or null if it is not in the heap. */
static Span *span_of(uintptr_t address) {
    uintptr_t index = address >> ARENA_SHIFT;
    Arena **row = __atomic_load_n(&arena_table[(index >> TABLE_BITS) & ((1 << TABLE_BITS) - 1)], __ATOMIC_ACQUIRE);
    if (!row) return NULL;
    Arena *arena = __atomic_load_n(&row[index & ((1 << TABLE_BITS) - 1)], __ATOMIC_ACQUIRE);
    if (!arena || address < arena->base) return NULL;
    size_t page = (address - arena->base) >> PAGE_SHIFT;
    if (page >= __atomic_load_n(&arena->next_page, __ATOMIC_RELAXED)) return NULL;
    return __atomic_load_n(&arena->spans[page], __ATOMIC_ACQUIRE);
}

/* --- spans of small objects --- */

/* The kinds of small-object span. A scannable object up to 512 bytes whose pointer words are the same
   whatever it holds records them in a bitmap at the end of its span; any other scannable object has a
   header word holding its type (which a sum type's variant sites need). */
enum { KIND_BITS = 0, KIND_NOSCAN = 1, KIND_HEADER = 2, KINDS = 3 };

static Span *partial[NUM_CLASSES * KINDS];

/* A P's allocation cache (Go's mcache): the span it is allocating from for each span class, and the bytes it
   has allocated that heap_used does not count yet. Padded so that two Ps' caches never share a cache line. */
typedef struct {
    Span *spans[NUM_CLASSES * KINDS];
    size_t used;
} __attribute__((aligned(128))) AllocCache;

static AllocCache caches[COOPER_MAX_PROCS];

static Span *new_small_span(int class, int kind) {
    int noscan = kind == KIND_NOSCAN;
    Span *span = new_span(class_pages[class]);
    size_t bytes = span->npages << PAGE_SHIFT;
    span->elemsize = class_size[class];
    span->spanclass = (uint16_t)(class * KINDS + kind);
    span->noscan = (uint8_t)noscan;
    span->heap_bits_in_span = kind == KIND_BITS;
    span->div_magic = UINT32_MAX / (uint32_t)span->elemsize + 1;
    /* A bitmap of one bit per word takes the end of the span. */
    size_t bitmap = span->heap_bits_in_span ? bytes / 64 : 0;
    span->nelems = (uint32_t)((bytes - bitmap) / span->elemsize);
    span_bits(span, (span->nelems + 63) / 64);
    set_span_pages(span, span);
    return span;
}

/* The next free slot of a span, or null when it has none. While marking, the slot is allocated black: marked, and
   for a span with a pointer bitmap scanned too, so that no worker scans it while it is initialized. A worker that
   sees the slot allocated (by `freeindex`) sees those bits; one that does not leaves it alone. */
static void *next_free(Span *span, int black) {
    uint32_t index = span->freeindex;
    while (index < span->nelems) {
        uint64_t free = ~span->alloc_bits[index / 64] >> (index % 64);
        if (free == 0) {
            index = (index / 64 + 1) * 64;
            continue;
        }
        index += (uint32_t)__builtin_ctzll(free);
        if (index >= span->nelems) break;
        if (black) {
            /* The scan bit first: a worker that sees the mark bit must not take the object as unscanned. */
            uint64_t bit = (uint64_t)1 << (index % 64);
            if (span->heap_bits_in_span) __atomic_fetch_or(&span->scan_bits[index / 64], bit, __ATOMIC_RELAXED);
            __atomic_fetch_or(&span->mark_bits[index / 64], bit, __ATOMIC_RELEASE);
        }
        __atomic_store_n(&span->freeindex, index + 1, __ATOMIC_RELEASE);
        span->alloc_count++;
        return (void *)(span->start + (size_t)index * span->elemsize);
    }
    __atomic_store_n(&span->freeindex, span->nelems, __ATOMIC_RELEASE);
    return NULL;
}

/* --- recording where an object's pointers are --- */

static uint64_t *heap_bits(const Span *span) {
    return (uint64_t *)(span->start + (span->npages << PAGE_SHIFT) - (span->npages << PAGE_SHIFT) / 64);
}

/* Set the heap bits of the slot at `slot` of a small-object span: the descriptor tiled over
   `size` bytes, and clear for the rest of the slot. A slot is at most 64 words, so the bits of
   one object are a single mask that falls in at most two words of the bitmap. Mark workers may be reading the
   neighbouring slots' bits in the same words. */
static void write_heap_bits(const Span *span, uintptr_t slot, const CooperType *type, size_t size) {
    uint64_t *bits = heap_bits(span);
    size_t first = (slot - span->start) / 8;
    size_t slot_words = span->elemsize / 8;
    size_t stride = type->size / 8;
    uint64_t mask = 0;
    for (size_t copy = 0; copy < size / type->size; copy++) mask |= type->bits[0] << (copy * stride);
    uint64_t clear = slot_words >= 64 ? ~(uint64_t)0 : ((uint64_t)1 << slot_words) - 1;
    size_t word = first / 64, shift = first % 64;
    uint64_t old = __atomic_load_n(&bits[word], __ATOMIC_RELAXED);
    __atomic_store_n(&bits[word], (old & ~(clear << shift)) | (mask << shift), __ATOMIC_RELAXED);
    if (shift + slot_words > 64) {
        old = __atomic_load_n(&bits[word + 1], __ATOMIC_RELAXED);
        __atomic_store_n(&bits[word + 1], (old & ~(clear >> (64 - shift))) | (mask >> (64 - shift)), __ATOMIC_RELAXED);
    }
}

/* --- allocation --- */

static uint64_t zerobase;
static int ready;

static int phase; /* PHASE_OFF or PHASE_MARK */
enum { PHASE_OFF, PHASE_MARK };

/* With the heap lock held. Allocated black while marking, as next_free does. */
static void *allocate_large(size_t size, const CooperType *type) {
    size_t npages = (size + PAGE_SIZE - 1) >> PAGE_SHIFT;
    if (size > SIZE_MAX - PAGE_SIZE) cooper_panic("out of memory");
    Span *span = new_span(npages);
    span->large = 1;
    span->noscan = type == NULL;
    span->elemsize = npages << PAGE_SHIFT;
    span->nelems = 1;
    span->freeindex = 1;
    span->alloc_count = 1;
    span->large_type = type;
    span->large_size = size;
    span_bits(span, 1);
    if (__atomic_load_n(&phase, __ATOMIC_RELAXED) == PHASE_MARK) span->mark_bits[0] = 1;
    __atomic_store_n(&heap_used, heap_used + span->elemsize, __ATOMIC_RELAXED);
    if (span->needzero) memset((void *)span->start, 0, size);
    set_span_pages(span, span);
    return (void *)span->start;
}

static void cpu_relax(void) {
#if defined(__aarch64__)
    __asm__ volatile("yield");
#elif defined(__x86_64__)
    __asm__ volatile("pause");
#endif
}

static void spin_lock(int64_t *word) {
    for (int spins = 0; __atomic_exchange_n(word, 1, __ATOMIC_ACQUIRE); spins++) {
        while (__atomic_load_n(word, __ATOMIC_RELAXED)) {
            if (++spins > 1000) sched_yield();
        }
    }
}

static void spin_unlock(int64_t *word) {
    __atomic_store_n(word, 0, __ATOMIC_RELEASE);
}

static void heap_lock(void) {
    spin_lock(&heap_word);
}

static void heap_unlock(void) {
    spin_unlock(&heap_word);
}

static void print_stats(void) {
    fprintf(stderr,
            "gc: %llu collections, pauses %.1f ms in all (sweeping %.1f), longest pause %.2f ms\n"
            "gc: marking ran %.1f ms, and took %.1f ms on all workers together\n"
            "gc: %.1f MiB allocated, peak heap %.1f MiB, %.1f MiB live after the last collection, %.1f MiB returned to the system\n",
            (unsigned long long)stats.collections, stats.pause_ns / 1e6, stats.sweep_ns / 1e6, stats.max_pause_ns / 1e6,
            stats.mark_ns / 1e6, stats.mark_worker_ns / 1e6, stats.bytes_allocated / 1048576.0,
            stats.peak_heap / 1048576.0, stats.last_live / 1048576.0, released_bytes / 1048576.0);
}

/* Go's goal: what survived, plus a percentage of it and of the roots, but at least a minimum. Marking begins
   seven tenths of the way there. */
static void set_goal(size_t live, size_t root_bytes) {
    next_gc = live + (live + root_bytes) / 100 * growth;
    if (next_gc < MIN_HEAP / 100 * growth) next_gc = MIN_HEAP / 100 * growth;
    trigger = live + (next_gc - live) / 10 * 7;
    __atomic_store_n(&pace_at, gc_off ? SIZE_MAX : trigger, __ATOMIC_RELAXED);
}

/* Whether the comma-separated list `options` has `name`. */
static int has_option(const char *options, const char *name) {
    size_t length = strlen(name);
    for (const char *at = options; at && *at; at = strchr(at, ',') ? strchr(at, ',') + 1 : NULL) {
        if (!strncmp(at, name, length) && (at[length] == ',' || at[length] == 0)) return 1;
    }
    return 0;
}

static void init_heap(void) {
    init_classes();
    const char *mode = getenv("COOPER_GC");
    trace = has_option(mode, "trace");
    if (getenv("COOPER_GC_STATS")) {
        atexit(print_stats);
        instrument = 1;
    }
    instrument |= trace;
    const char *percent = getenv("COOPER_HEAP_GROWTH");
    if (percent && !strcmp(percent, "off")) gc_off = 1;
    else if (percent && atoi(percent) > 0) growth = (unsigned)atoi(percent);
    stress = has_option(mode, "stress");
    markstress = has_option(mode, "markstress");
    checkmark = has_option(mode, "checkmark");
    poison = stress || markstress || has_option(mode, "poison");
    set_goal(0, 0);
    ready = 1;
}

static void *allocate(size_t size, const CooperType *type);
static void pace(void);

static void check_size(size_t size, const CooperType *type) {
    if (type && (type->size != type->layout_size || size % type->size != 0)) {
        cooper_panic("internal error: an allocation's size does not match its type");
    }
}

void *cooper_alloc(size_t size, const CooperType *type) {
    check_size(size, type);
    return allocate(size, type);
}

void *cooper_alloc_with_word(size_t size, const CooperType *type) {
    check_size(size, type);
    if (size > SIZE_MAX - 8) cooper_panic("allocation too large");
    return allocate(size + 8, type);
}

static void *allocate(size_t size, const CooperType *type) {
    if (size == 0) return &zerobase;
    if (!ready) init_heap();
    if (__atomic_load_n(&cooper_poll_word, __ATOMIC_RELAXED)) cooper_poll_slow();
    AllocCache *cache = &caches[cooper_proc_id()];
    size_t used = __atomic_load_n(&heap_used, __ATOMIC_RELAXED) + cache->used;
    if (started && (stress || markstress || used >= __atomic_load_n(&pace_at, __ATOMIC_RELAXED))) {
        pace();
        cache = &caches[cooper_proc_id()]; /* it may have switched */
    }
    int black = __atomic_load_n(&phase, __ATOMIC_RELAXED) == PHASE_MARK;
    int noscan = type == NULL;
    int kind = noscan ? KIND_NOSCAN : (type->nsites || size > MIN_SIZE_FOR_HEADER) ? KIND_HEADER : KIND_BITS;
    size_t total = size;
    if (kind == KIND_HEADER) total += HEADER_SIZE;
    if (total < size || total > MAX_SMALL) {
        heap_lock();
        void *large = allocate_large(size, type);
        heap_unlock();
        return large;
    }
    int class = size_to_class8[(total + 7) / 8];
    int spanclass = class * KINDS + kind;
    Span *span = cache->spans[spanclass];
    void *slot = span ? next_free(span, black) : NULL;
    if (!slot) {
        heap_lock();
        __atomic_store_n(&heap_used, heap_used + cache->used, __ATOMIC_RELAXED);
        cache->used = 0;
        do {
            span = partial[spanclass];
            if (span) {
                partial[spanclass] = span->partial_next;
                span->partial_next = NULL;
            } else {
                span = new_small_span(class, kind);
            }
            cache->spans[spanclass] = span;
            slot = next_free(span, black);
        } while (!slot);
        heap_unlock();
    }
    cache->used += span->elemsize;
    /* The bitmap words are shared with neighbouring slots of the span, which is this P's. */
    if (!noscan && span->heap_bits_in_span) write_heap_bits(span, (uintptr_t)slot, type, size);
    int needzero = span->needzero, header = !noscan && !span->heap_bits_in_span;
    if (needzero) memset(slot, 0, span->elemsize);
    if (!header) return slot;
    *(const CooperType **)slot = type;
    return (char *)slot + HEADER_SIZE;
}

/* --- finding objects, for the collector --- */

/* The object containing `address`: its slot's first byte and size, whether it holds pointers,
   and the byte of the slot where its value starts (after a header word). Returns 0 when the address
   is not in an allocated object. */
int cooper_heap_lookup(const void *address, void **slot, size_t *slot_size, int *noscan, size_t *value_offset) {
    Span *span = span_of((uintptr_t)address);
    if (!span || span->state != SPAN_IN_USE || (uintptr_t)address < span->start) return 0;
    size_t offset = (uintptr_t)address - span->start;
    uint32_t index = span->large ? 0 : (uint32_t)(offset / span->elemsize);
    if (index >= span->nelems) return 0;
    if (index >= span->freeindex && !(span->alloc_bits[index / 64] >> (index % 64) & 1)) return 0;
    *slot = (void *)(span->start + (size_t)index * span->elemsize);
    *slot_size = span->large ? span->large_size : span->elemsize;
    *noscan = span->noscan;
    *value_offset = !span->noscan && !span->large && !span->heap_bits_in_span ? HEADER_SIZE : 0;
    return 1;
}

/* Whether word `word` of the value of the object at `slot` (as cooper_heap_lookup returned it)
   holds a pointer. For tests and the collector's checks. */
int cooper_heap_is_pointer_word(void *slot, size_t word) {
    Span *span = span_of((uintptr_t)slot);
    if (!span || span->noscan) return 0;
    const CooperType *type;
    size_t value_words;
    size_t first = 0;
    if (span->large) {
        type = span->large_type;
        value_words = span->large_size / 8;
    } else if (span->heap_bits_in_span) {
        size_t at = ((uintptr_t)slot - span->start) / 8 + word;
        return (int)(heap_bits(span)[at / 64] >> (at % 64) & 1);
    } else {
        type = *(const CooperType **)slot;
        value_words = (span->elemsize - HEADER_SIZE) / 8;
        first = 0;
    }
    if (word >= value_words) return 0;
    size_t within = (first + word) % (type->size / 8);
    return (int)(type->bits[within / 64] >> (within % 64) & 1);
}

/* Heap statistics: bytes in spans, and the number of spans. */
void cooper_heap_stats(size_t *bytes, size_t *spans) {
    *bytes = heap_bytes;
    *spans = span_count;
}

/* --- the collector --- */

/* A cycle: an allocation that reaches the trigger stops the world to begin marking (begin_mark): it turns on the
   write barrier, scans the module-level variables and the timers, and has the scheduler scan every thread's stack
   once as the program runs (sched.c). Each P has a worker, which whatever runs on the P uses: the barrier of the
   code it runs, an allocation that helps mark, or the P's idle time (mark_running). Each worker has an object
   stack and a span queue of its own, and shares surplus work through global lists of fixed-size blocks; a worker
   that runs out steals from the others' span rings. Mark and scan bits change by atomic OR and a span's `queued`
   flag by exchange, so each object is marked once and scanned once. Objects allocated meanwhile are black.

   A worker that finds no work while no other is marking ends the cycle (end_mark): with the world stopped again it
   hands the work every worker kept to itself (the barrier's, mostly) to the global lists, scans the stacks of any
   threads not yet scanned, and if any work is left, marks it on every P, as a stopped world's helpers join
   (mark_stopped). Then it turns off the barrier and sweeps. An allocation that reaches the heap goal while marking
   helps until the cycle ends.

   The runtime is compiled with the program, unoptimized by default: the functions on the path of every word
   and span are inlined even then. */

typedef struct {
    uintptr_t slot; /* an object to scan; for a large object, the start of one of its oblets */
    Span *span;
} Work;

#define BLOCK_WORK 128
#define RING_SIZE 256
#define OBLET_BYTES ((size_t)128 << 10) /* a large object is scanned in pieces of about this size */

typedef struct Block {
    struct Block *next;
    size_t count;
    union {
        Work objects[BLOCK_WORK];
        Span *spans[RING_SIZE];
    };
} Block;

typedef struct {
    /* Spans to scan, oldest first: only the owner adds (at tail); the owner and thieves take (CAS on head). */
    Span *ring[RING_SIZE];
    uint32_t head, tail;
    Block *objects;  /* objects to scan, a stack */
    Block *fresh;    /* spans queued most recently, until a full block of them goes to the global list */
    Span *last_span; /* the span the previous word pointed into: the next often does too */
    uint64_t random;
    size_t root_bytes; /* stack and register bytes this worker scanned, for the pacer */
    uint64_t items;  /* objects and spans scanned, to tell whether a turn found work */
    int checking;    /* checkmark: verify marks instead of making them */
    uintptr_t from;  /* checkmark: the object being scanned, 0 for a module-level variable, FROM_STACK */
} __attribute__((aligned(128))) Worker;

static Worker workers[COOPER_MAX_PROCS];

/* The state word of marking with the world stopped: the workers that joined, those of them idle, whether marking
   is done, and a sequence number that changes with every update, so a compare-and-swap cannot succeed on a word
   that changed and changed back. Otherwise the word is DONE, and nobody can join. */
#define JOINED(s) ((s) & 0x3ff)
#define IDLE(s) (((s) >> 10) & 0x3ff)
#define IDLE_ONE ((uint64_t)1 << 10)
#define DONE ((uint64_t)1 << 20)
#define SEQ ((uint64_t)1 << 21)

/* Work shared between the workers: under `lock`, except the atomic fields. */
static struct {
    int64_t lock;
    Block *objects;            /* full blocks of objects, a stack */
    Block *spans, *spans_tail; /* full blocks of spans, first in first out */
    Block *empty;
    char *chunk, *chunk_end;   /* memory for new blocks */
    int64_t nobjects, nspans;  /* atomic: the blocks in the two lists */
    uint64_t state;            /* atomic */
    int64_t left;              /* atomic: helpers that have finished */
    uint64_t cycle;            /* each mark's number: a thread's stack is claimed for scanning by setting it */
    int64_t worker_ns;         /* atomic: time spent by all workers, for statistics */
    int64_t working;           /* atomic: workers marking while the program runs */
    int64_t ending;            /* atomic: a worker is ending the cycle */
    uint64_t began;            /* when the cycle began, for statistics */
} mark = {.state = DONE};

static uint64_t cycles; /* atomic: the cycles begun */

/* An empty block, with mark.lock held. */
static Block *new_block(void) {
    Block *block = mark.empty;
    if (block) {
        mark.empty = block->next;
    } else {
        if ((size_t)(mark.chunk_end - mark.chunk) < sizeof *block) {
            mark.chunk = map_zeroed((size_t)1 << 20);
            mark.chunk_end = mark.chunk + ((size_t)1 << 20);
        }
        block = (Block *)mark.chunk;
        mark.chunk += sizeof *block;
    }
    block->next = NULL;
    block->count = 0;
    return block;
}

/* Give a worker its blocks, the first time it is used. */
static void worker_ready(Worker *w) {
    if (w->objects) return;
    spin_lock(&mark.lock);
    w->objects = new_block();
    w->fresh = new_block();
    spin_unlock(&mark.lock);
}

static void push_object(Worker *w, uintptr_t slot, Span *span) {
    Block *block = w->objects;
    if (block->count == BLOCK_WORK) {
        spin_lock(&mark.lock);
        block->next = mark.objects;
        mark.objects = block;
        __atomic_add_fetch(&mark.nobjects, 1, __ATOMIC_SEQ_CST);
        w->objects = block = new_block();
        spin_unlock(&mark.lock);
    }
    block->objects[block->count++] = (Work){slot, span};
}

__attribute__((always_inline)) static inline int pop_object(Worker *w, Work *item) {
    Block *block = w->objects;
    if (block->count == 0) {
        if (!__atomic_load_n(&mark.nobjects, __ATOMIC_RELAXED)) return 0;
        spin_lock(&mark.lock);
        Block *full = mark.objects;
        if (full) {
            mark.objects = full->next;
            __atomic_sub_fetch(&mark.nobjects, 1, __ATOMIC_SEQ_CST);
            block->next = mark.empty;
            mark.empty = block;
            w->objects = block = full;
        }
        spin_unlock(&mark.lock);
        if (!full) return 0;
    }
    *item = block->objects[--block->count];
    return 1;
}

/* Queue a span whose objects were marked: the longer it waits, the more of its objects have been reached by
   the time it is scanned (Green Tea). New spans collect in the worker's fresh block, full blocks join the end
   of the global list, and a worker takes from its ring, then the global list, then its fresh block: first in,
   first out, as a whole. */
static void queue_span(Worker *w, Span *span) {
    Block *block = w->fresh;
    block->spans[block->count++] = span;
    if (block->count < RING_SIZE) return;
    spin_lock(&mark.lock);
    if (mark.spans_tail) mark.spans_tail->next = block;
    else mark.spans = block;
    mark.spans_tail = block;
    __atomic_add_fetch(&mark.nspans, 1, __ATOMIC_SEQ_CST);
    w->fresh = new_block();
    spin_unlock(&mark.lock);
}

__attribute__((always_inline)) static inline Span *ring_take(Worker *w) {
    for (;;) {
        uint32_t head = __atomic_load_n(&w->head, __ATOMIC_ACQUIRE);
        if (head == __atomic_load_n(&w->tail, __ATOMIC_ACQUIRE)) return NULL;
        Span *span = __atomic_load_n(&w->ring[head % RING_SIZE], __ATOMIC_RELAXED);
        if (__atomic_compare_exchange_n(&w->head, &head, head + 1, false, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED)) {
            return span;
        }
    }
}

/* Move a block's spans to the owner's empty ring, emptying the block. */
static void ring_fill(Worker *w, Block *block) {
    uint32_t tail = w->tail;
    for (size_t i = 0; i < block->count; i++) __atomic_store_n(&w->ring[(tail + i) % RING_SIZE], block->spans[i], __ATOMIC_RELAXED);
    __atomic_store_n(&w->tail, tail + (uint32_t)block->count, __ATOMIC_SEQ_CST);
    block->count = 0;
}

static Span *pop_span(Worker *w) {
    Span *span = ring_take(w);
    if (span) return span;
    if (__atomic_load_n(&mark.nspans, __ATOMIC_RELAXED)) {
        spin_lock(&mark.lock);
        Block *block = mark.spans;
        if (block) {
            mark.spans = block->next;
            if (!mark.spans) mark.spans_tail = NULL;
            __atomic_sub_fetch(&mark.nspans, 1, __ATOMIC_SEQ_CST);
            ring_fill(w, block);
            block->next = mark.empty;
            mark.empty = block;
        }
        spin_unlock(&mark.lock);
        if (block) return ring_take(w);
    }
    if (!w->fresh->count) return NULL;
    ring_fill(w, w->fresh);
    return ring_take(w);
}

/* Take half of another worker's ring into this worker's empty one. */
static int steal(Worker *w) {
    int joined = cooper_proc_count();
    if (joined < 2) return 0;
    w->random ^= w->random << 13;
    w->random ^= w->random >> 7;
    w->random ^= w->random << 17;
    int start = (int)(w->random % (uint64_t)joined);
    for (int i = 0; i < joined; i++) {
        Worker *victim = &workers[(start + i) % joined];
        if (victim == w) continue;
        for (;;) {
            uint32_t head = __atomic_load_n(&victim->head, __ATOMIC_ACQUIRE);
            uint32_t tail = __atomic_load_n(&victim->tail, __ATOMIC_ACQUIRE);
            uint32_t n = tail - head;
            n -= n / 2;
            if (n == 0) break;
            if (n > RING_SIZE / 2) continue; /* head and tail read at different moments */
            uint32_t mine = w->tail;
            for (uint32_t k = 0; k < n; k++) {
                Span *span = __atomic_load_n(&victim->ring[(head + k) % RING_SIZE], __ATOMIC_RELAXED);
                __atomic_store_n(&w->ring[(mine + k) % RING_SIZE], span, __ATOMIC_RELAXED);
            }
            if (__atomic_compare_exchange_n(&victim->head, &head, head + n, false, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED)) {
                __atomic_store_n(&w->tail, mine + n, __ATOMIC_SEQ_CST);
                return 1;
            }
        }
    }
    return 0;
}

static __attribute__((noinline, cold)) void check_mark(Worker *w, Span *span, uint32_t index);

/* Mark the object `word` points into, if it points into an allocated one. */
__attribute__((always_inline)) static inline void mark_word(Worker *w, uintptr_t word) {
    Span *span = w->last_span;
    if (!span || word - span->start >= span->npages << PAGE_SHIFT || span->state != SPAN_IN_USE) {
        span = span_of(word);
        if (span) w->last_span = span;
    }
    if (!span || span->state != SPAN_IN_USE || word < span->start) return;
    uint32_t index = span->large ? 0 : (uint32_t)(((word - span->start) * span->div_magic) >> 32);
    if (index >= span->nelems) return;
    uint64_t bit = (uint64_t)1 << (index % 64);
    if (index >= __atomic_load_n(&span->freeindex, __ATOMIC_ACQUIRE) && !(span->alloc_bits[index / 64] & bit)) return;
    if (w->checking) {
        check_mark(w, span, index);
        return;
    }
    uint64_t *marks = &span->mark_bits[index / 64];
    if (__atomic_load_n(marks, __ATOMIC_RELAXED) & bit) return;
    if (__atomic_fetch_or(marks, bit, __ATOMIC_SEQ_CST) & bit) return;
    if (span->noscan) return;
    if (span->heap_bits_in_span) {
        /* Do not scan the object now: queue its span, unless it is queued already. Its scanner clears `queued`
           before it reads the mark bits, and all four accesses are sequentially consistent, so either it sees
           this mark or this load sees the flag clear. The load first keeps markers from writing the flag's
           cache line for every object. */
        if (!__atomic_load_n(&span->queued, __ATOMIC_SEQ_CST) && !__atomic_exchange_n(&span->queued, 1, __ATOMIC_SEQ_CST)) {
            queue_span(w, span);
        }
        return;
    }
    push_object(w, span->start + (size_t)index * span->elemsize, span);
}

/* A word of memory the program may be writing meanwhile (marking while it runs). */
__attribute__((always_inline)) static inline uintptr_t read_word(const uint64_t *word) {
    return __atomic_load_n(word, __ATOMIC_RELAXED);
}

/* The pointers in one value of `type` at `base`: the words its bitmap names, and the payload of the
   variant each sum type in it holds. A tag and payload changing meanwhile may be read torn, but the barrier shaded
   the pointers of both values. */
static void scan_value(Worker *w, const uint64_t *base, const CooperType *type) {
    for (size_t chunk = 0; chunk < (type->words + 63) / 64; chunk++) {
        uint64_t mask = type->bits[chunk];
        while (mask) {
            mark_word(w, read_word(&base[chunk * 64 + (size_t)__builtin_ctzll(mask)]));
            mask &= mask - 1;
        }
    }
    for (uint64_t i = 0; i < type->nsites; i++) {
        const CooperSite *site = &type->sites[i];
        uint32_t tag = __atomic_load_n((const uint32_t *)((const char *)base + site->tag_offset), __ATOMIC_RELAXED);
        if (tag < site->variants && site->payloads[tag]) {
            scan_value(w, (const uint64_t *)((const char *)base + site->payload_offset), site->payloads[tag]);
        }
    }
}

/* The pointers among `bytes` bytes at `base`, which hold whole values of `type`. */
static void scan_tiled(Worker *w, const uint64_t *base, size_t bytes, const CooperType *type) {
    size_t stride = type->size / 8;
    for (size_t copy = 0; copy < bytes / type->size; copy++) scan_value(w, base + copy * stride, type);
}

/* A small object, from its span's bitmap or its header word. */
static void scan_object(Worker *w, uintptr_t slot, const Span *span) {
    if (span->heap_bits_in_span) {
        /* The slot's pointer words, as one mask taken from the span's bitmap, and only those visited. */
        const uint64_t *bits = heap_bits(span);
        size_t first = (slot - span->start) / 8, slot_words = span->elemsize / 8;
        size_t at = first / 64, shift = first % 64;
        uint64_t mask = read_word(&bits[at]) >> shift;
        if (shift && shift + slot_words > 64) mask |= read_word(&bits[at + 1]) << (64 - shift);
        if (slot_words < 64) mask &= ((uint64_t)1 << slot_words) - 1;
        while (mask) {
            mark_word(w, read_word(&((const uint64_t *)slot)[__builtin_ctzll(mask)]));
            mask &= mask - 1;
        }
    } else {
        const CooperType *type = *(const CooperType **)slot;
        scan_tiled(w, (const uint64_t *)(slot + HEADER_SIZE), span->elemsize - HEADER_SIZE, type);
    }
}

/* An object from the object stack. A large object is scanned an oblet (a run of whole values) at a time:
   reaching its first oblet queues the rest, which other workers may take. */
static void scan_work(Worker *w, Work item) {
    w->from = item.slot;
    Span *span = item.span;
    if (!span->large) {
        scan_object(w, item.slot, span);
        return;
    }
    const CooperType *type = span->large_type;
    size_t oblet = OBLET_BYTES / type->size * type->size;
    if (oblet == 0) oblet = type->size;
    uintptr_t end = span->start + span->large_size;
    if (item.slot == span->start) {
        for (uintptr_t next = item.slot + oblet; next < end; next += oblet) push_object(w, next, span);
    }
    size_t bytes = end - item.slot < oblet ? end - item.slot : oblet;
    scan_tiled(w, (const uint64_t *)item.slot, bytes, type);
}

/* Scan the objects of a small span that were marked and not yet scanned. Scanning may mark more of
   them (and requeue the span), which a later turn picks up. */
static void scan_span(Worker *w, Span *span) {
    size_t words = (span->nelems + 63) / 64;
    for (size_t word = 0; word < words; word++) {
        uint64_t pending = __atomic_load_n(&span->mark_bits[word], __ATOMIC_SEQ_CST) &
                           ~__atomic_load_n(&span->scan_bits[word], __ATOMIC_RELAXED);
        if (!pending) continue;
        pending &= ~__atomic_fetch_or(&span->scan_bits[word], pending, __ATOMIC_RELAXED);
        while (pending) {
            size_t index = word * 64 + (size_t)__builtin_ctzll(pending);
            pending &= pending - 1;
            scan_object(w, span->start + index * span->elemsize, span);
        }
    }
}

/* Scan until this worker finds no work: its own, the global lists', or another's to steal. Marking while the
   program runs, it also stops when `stop` says so (asked every few items) or after `budget` items (-1: no limit).
   Gives whether it ran out of work. */
static int drain(Worker *w, bool (*stop)(void), long budget) {
    for (long n = 1;; n++) {
        if (n > budget && budget >= 0) return 0;
        if (stop && n % 64 == 0 && stop()) return 0;
        Work item;
        if (pop_object(w, &item)) {
            w->items++;
            scan_work(w, item);
            continue;
        }
        Span *span = pop_span(w);
        if (span) {
            w->items++;
            __atomic_exchange_n(&span->queued, 0, __ATOMIC_SEQ_CST);
            /* The span after this one will be scanned soon: start bringing in what that takes. */
            uint32_t head = __atomic_load_n(&w->head, __ATOMIC_RELAXED);
            if (head != w->tail) {
                const Span *next = __atomic_load_n(&w->ring[head % RING_SIZE], __ATOMIC_RELAXED);
                __builtin_prefetch(next->mark_bits);
                __builtin_prefetch(next->scan_bits);
                __builtin_prefetch((const void *)next->start);
            }
            scan_span(w, span);
            continue;
        }
        if (!steal(w)) return 1;
    }
}

/* Whether any work is shared: in the global lists or a worker's ring. */
static int work_available(void) {
    if (__atomic_load_n(&mark.nobjects, __ATOMIC_SEQ_CST) || __atomic_load_n(&mark.nspans, __ATOMIC_SEQ_CST)) return 1;
    for (int i = 0; i < cooper_proc_count(); i++) {
        if (__atomic_load_n(&workers[i].head, __ATOMIC_SEQ_CST) != __atomic_load_n(&workers[i].tail, __ATOMIC_SEQ_CST)) return 1;
    }
    return 0;
}

/* Hand the work a worker keeps to itself to the global lists. */
static void flush(Worker *w) {
    if (!w->objects->count && !w->fresh->count) return;
    spin_lock(&mark.lock);
    if (w->objects->count) {
        w->objects->next = mark.objects;
        mark.objects = w->objects;
        __atomic_add_fetch(&mark.nobjects, 1, __ATOMIC_SEQ_CST);
        w->objects = new_block();
    }
    if (w->fresh->count) {
        if (mark.spans_tail) mark.spans_tail->next = w->fresh;
        else mark.spans = w->fresh;
        mark.spans_tail = w->fresh;
        __atomic_add_fetch(&mark.nspans, 1, __ATOMIC_SEQ_CST);
        w->fresh = new_block();
    }
    spin_unlock(&mark.lock);
}

/* Mark with the world stopped until marking is done. An idle worker waits for work to appear; when every worker
   that joined is idle and none is left, no worker can make more, and marking is done. */
static void work(Worker *w) {
    uint64_t s = __atomic_load_n(&mark.state, __ATOMIC_SEQ_CST);
    for (;;) {
        drain(w, NULL, -1);
        s = __atomic_add_fetch(&mark.state, IDLE_ONE + SEQ, __ATOMIC_SEQ_CST);
        for (int spins = 0;; spins++) {
            if (s & DONE) return;
            if (work_available()) {
                if (__atomic_compare_exchange_n(&mark.state, &s, s - IDLE_ONE + SEQ, false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST)) break;
                continue;
            }
            if (IDLE(s) == JOINED(s)) {
                if (__atomic_compare_exchange_n(&mark.state, &s, s | DONE, false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST)) return;
                continue;
            }
            if (spins < 100) cpu_relax();
            else sched_yield();
            s = __atomic_load_n(&mark.state, __ATOMIC_SEQ_CST);
        }
    }
}

/* Every word in [low, high) as a possible pointer. */
static void scan_range(void *context, uintptr_t low, uintptr_t high) {
    Worker *w = context;
    w->root_bytes += high - low;
    for (const uint64_t *word = (const uint64_t *)((low + 7) & ~(uintptr_t)7); (uintptr_t)word + 8 <= high; word++) {
        mark_word(w, read_word(word));
    }
}

/* The module-level variables the compiler emitted: each one's address and type. */
struct Root {
    const uint64_t *address;
    const CooperType *type;
};
extern const struct Root cooper_roots[];
extern const int64_t cooper_root_count;

static void mark_variables(Worker *w) {
    w->from = 0;
    for (int64_t i = 0; i < cooper_root_count; i++) {
        scan_tiled(w, cooper_roots[i].address, cooper_roots[i].type->size, cooper_roots[i].type);
    }
}

/* What a helper M runs while the world is stopped: join the mark, if it has not ended and has room. */
static void help_mark(void) {
    uint64_t s = __atomic_load_n(&mark.state, __ATOMIC_SEQ_CST);
    do {
        if (s & DONE || JOINED(s) >= (uint64_t)cooper_proc_count()) return;
    } while (!__atomic_compare_exchange_n(&mark.state, &s, s + 1 + SEQ, false, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST));
    uint64_t start = instrument ? now_ns() : 0;
    Worker *w = &workers[JOINED(s)];
    w->from = 0;
    work(w);
    if (instrument) __atomic_add_fetch(&mark.worker_ns, (int64_t)(now_ns() - start), __ATOMIC_SEQ_CST);
    __atomic_add_fetch(&mark.left, 1, __ATOMIC_SEQ_CST);
}

/* With the world stopped, mark what is left on every P: the caller's worker 0, and helpers for the others. */
static void mark_stopped(void) {
    mark.left = 0;
    __atomic_store_n(&mark.state, 1 + SEQ, __ATOMIC_SEQ_CST);
    if (cooper_proc_count() > 1) cooper_start_helpers(help_mark);
    uint64_t start = instrument ? now_ns() : 0;
    work(&workers[0]);
    if (instrument) __atomic_add_fetch(&mark.worker_ns, (int64_t)(now_ns() - start), __ATOMIC_SEQ_CST);
    /* The helpers still look at the workers until they see marking done. */
    int64_t helpers = (int64_t)JOINED(__atomic_load_n(&mark.state, __ATOMIC_SEQ_CST)) - 1;
    for (int spins = 0; __atomic_load_n(&mark.left, __ATOMIC_SEQ_CST) < helpers; spins++) {
        if (spins < 100) cpu_relax();
        else sched_yield();
    }
}

/* --- the write barrier ---

   Go's hybrid write barrier: while the collector marks (cooper_barrier_on is set), a store that may write a pointer
   word anywhere but the storing function's own frame first shades the pointers the destination holds and those it
   stores, which marks their objects and queues them to be scanned. Generated code tests the flag inline and before
   such a store passes both values to cooper_barrier_pointer or cooper_barrier_typed; the runtime copies values
   that hold pointers with cooper_typed_copy, and C stores a pointer with cooper_store_pointer. */

int32_t cooper_barrier_on;

/* Code on a P shades into that P's worker, which nothing else on the P uses meanwhile. */
static Worker *barrier_worker(void) {
    Worker *w = &workers[cooper_proc_id()];
    worker_ready(w);
    return w;
}

void cooper_barrier_pointer(void *old, void *value) {
    Worker *w = barrier_worker();
    mark_word(w, (uintptr_t)old);
    mark_word(w, (uintptr_t)value);
}

/* For a store of a value of `type`: copies of the value overwritten and of the one stored. A sum type's pointers
   are found by the variant each holds. */
void cooper_barrier_typed(const void *old, const void *value, const CooperType *type) {
    Worker *w = barrier_worker();
    scan_value(w, old, type);
    scan_value(w, value, type);
}

void cooper_typed_copy(void *dst, const void *src, size_t bytes, const CooperType *type) {
    if (type && __atomic_load_n(&cooper_barrier_on, __ATOMIC_RELAXED)) {
        Worker *w = barrier_worker();
        scan_tiled(w, dst, bytes, type);
        scan_tiled(w, src, bytes, type);
    }
    memmove(dst, src, bytes);
}

/* C may run without a P, so it shades into a worker of its own, under a lock, and hands what that queued to the
   global lists at once. C does not stop for the collector, so a store counts itself in `c_stores` around its test
   of the flag: marking begins only once the stores that may have missed the flag are done. */
static Worker c_worker;
static int64_t c_worker_lock, c_stores;

void cooper_store_pointer(void **slot, void *value) {
    __atomic_add_fetch(&c_stores, 1, __ATOMIC_SEQ_CST);
    if (__atomic_load_n(&cooper_barrier_on, __ATOMIC_SEQ_CST)) {
        spin_lock(&c_worker_lock);
        Worker *w = &c_worker;
        worker_ready(w);
        mark_word(w, (uintptr_t)__atomic_load_n(slot, __ATOMIC_RELAXED));
        mark_word(w, (uintptr_t)value);
        flush(w);
        spin_unlock(&c_worker_lock);
    }
    __atomic_store_n(slot, value, __ATOMIC_RELEASE);
    __atomic_sub_fetch(&c_stores, 1, __ATOMIC_SEQ_CST);
}

/* --- checkmark: COOPER_GC=checkmark marks again, serially and object by object, after every mark, and fails on
   an object reached that way that the mark missed ---

   A stack is scanned once in a cycle, and by the end it may have exposed words its frames left behind before then
   (a slot a later frame does not initialize): such a word can name an object that died before the scan. So an
   object reached only from a stack and not marked is passed over, not reported. Missing a stack shows instead as
   an object freed (and poisoned) while in use. */

#define FROM_STACK UINTPTR_MAX

static __attribute__((noinline, cold)) void check_mark(Worker *w, Span *span, uint32_t index) {
    uint64_t bit = (uint64_t)1 << (index % 64);
    if (span->scan_bits[index / 64] & bit) return;
    uintptr_t slot = span->start + (size_t)index * span->elemsize;
    if (!(span->mark_bits[index / 64] & bit) && w->from == FROM_STACK) return;
    span->scan_bits[index / 64] |= bit;
    if (!(span->mark_bits[index / 64] & bit)) {
        const void *type = span->large ? (const void *)span->large_type
                           : !span->noscan && !span->heap_bits_in_span ? (const void *)*(const CooperType **)slot
                                                                      : NULL;
        char from[48] = "a module-level variable";
        if (w->from) snprintf(from, sizeof from, "the object at %p", (void *)w->from);
        char message[256];
        snprintf(message, sizeof message,
                 "internal error: the collector missed the %zu-byte object at %p (%s, type descriptor %p), reached from %s",
                 span->large ? span->large_size : span->elemsize, (void *)slot,
                 span->noscan ? "pointer-free" : span->large ? "large" : span->heap_bits_in_span ? "bitmap" : "header",
                 type, from);
        cooper_panic(message);
    }
    if (!span->noscan) push_object(w, slot, span);
}

/* With the world still stopped after marking. The scan bits serve as the checkmark bits: marking is done with
   them, and the sweep clears them. The calling thread's stack is scanned from here up, like a stopped thread's:
   its registers are spilled in the caller's frame. Stacks go last, so that what else reaches is reported. */
static __attribute__((noinline)) void check_marks(void) {
    volatile uintptr_t here = 0;
    cooper_save_stack_pointer((void *)&here);
    for (Span *span = all_spans; span; span = span->all_next) {
        memset(span->scan_bits, 0, (span->nelems + 63) / 64 * 8);
    }
    Worker *w = &workers[0];
    w->checking = 1;
    w->last_span = NULL;
    mark.cycle++;
    mark_variables(w);
    Work item;
    while (pop_object(w, &item)) scan_work(w, item);
    w->from = FROM_STACK;
    cooper_scan_threads(scan_range, w, mark.cycle);
    while (pop_object(w, &item)) scan_work(w, item);
    w->checking = 0;
}

static __attribute__((noinline)) void check_marks_with_registers(void) {
    __builtin_unwind_init();
    check_marks();
}

static void fill(void *memory, size_t bytes) {
    if (poison) memset(memory, 0xDB, bytes);
}

/* Free what is unmarked, and make the marked objects the allocated ones. */
static void sweep(void) {
    memset(partial, 0, sizeof partial);
    heap_used = 0;
    Span **link = &all_spans;
    while (*link) {
        Span *span = *link;
        if (span->large) {
            if (span->mark_bits[0] & 1) {
                span->mark_bits[0] = 0;
                heap_used += span->elemsize;
                link = &span->all_next;
                continue;
            }
            fill((void *)span->start, span->elemsize);
            *link = span->all_next;
            span->state = SPAN_FREE;
            set_span_pages(span, NULL);
            free_pages(span->arena, span->first_page, span->npages);
            heap_bytes -= span->npages << PAGE_SHIFT;
            span_count--;
            span->all_next = span_pool;
            span_pool = span;
            continue;
        }
        size_t words = (span->nelems + 63) / 64;
        uint32_t live = 0;
        for (size_t word = 0; word < words; word++) live += (uint32_t)__builtin_popcountll(span->mark_bits[word]);
        if (poison) {
            for (uint32_t index = 0; index < span->nelems; index++) {
                int allocated = index < span->freeindex || (span->alloc_bits[index / 64] >> (index % 64) & 1);
                if (allocated && !(span->mark_bits[index / 64] >> (index % 64) & 1)) {
                    fill((void *)(span->start + (size_t)index * span->elemsize), span->elemsize);
                }
            }
        }
        if (live == 0) {
            *link = span->all_next;
            span->state = SPAN_FREE;
            set_span_pages(span, NULL);
            free_pages(span->arena, span->first_page, span->npages);
            heap_bytes -= span->npages << PAGE_SHIFT;
            span_count--;
            span->all_next = span_pool;
            span_pool = span;
            continue;
        }
        memcpy(span->alloc_bits, span->mark_bits, words * 8);
        memset(span->mark_bits, 0, words * 8);
        memset(span->scan_bits, 0, words * 8);
        span->queued = 0;
        span->freeindex = 0;
        span->alloc_count = live;
        span->needzero = 1;
        heap_used += (size_t)live * span->elemsize;
        if (live < span->nelems) {
            span->partial_next = partial[span->spanclass];
            partial[span->spanclass] = span;
        }
        link = &span->all_next;
    }
}

/* Give the memory of large free runs back to the operating system, once each. The runs are dirty
   anyway (allocation zeroes what it hands out), so whatever the system leaves in them does not matter. */
#define SCAVENGE_MIN_PAGES 16


static void scavenge(void) {
    for (Run *run = free_runs; run; run = run->next) {
        if (run->released || run->npages < SCAVENGE_MIN_PAGES) continue;
        void *start = (void *)(run->arena->base + (run->first_page << PAGE_SHIFT));
        size_t bytes = run->npages << PAGE_SHIFT;
#ifdef MADV_FREE_REUSABLE
        madvise(start, bytes, MADV_FREE_REUSABLE);
#elif defined(MADV_FREE)
        madvise(start, bytes, MADV_FREE);
#else
        madvise(start, bytes, MADV_DONTNEED);
#endif
        run->released = 1;
        released_bytes += bytes;
    }
}

/* --- cycles --- */

/* With the world stopped and the heap lock held: what the Ps allocated since their refills counts, as the
   allocating thread's check counted its own. */
static void count_cached(void) {
    for (int i = 0; i < cooper_proc_count(); i++) {
        heap_used += caches[i].used;
        caches[i].used = 0;
    }
}

static void record_pause(uint64_t ns) {
    stats.pause_ns += ns;
    if (ns > stats.max_pause_ns) stats.max_pause_ns = ns;
}

/* For the scheduler: a range of a thread's stack or of its records, scanned with the running P's worker. What that
   finds goes to the global lists, where idle Ps find it. */
static void scan_for_scheduler(uintptr_t low, uintptr_t high) {
    Worker *w = barrier_worker();
    w->from = 0;
    scan_range(w, low, high);
    flush(w);
}

static bool idle_mark(bool (*stop)(void));

/* With the world stopped and the heap lock held: turn on the barrier, scan the module-level variables, and have
   the scheduler scan the rest. */
static void begin_mark(void) {
    mark.began = instrument ? now_ns() : 0;
    mark.worker_ns = 0;
    mark.cycle++;
    __atomic_add_fetch(&cycles, 1, __ATOMIC_SEQ_CST);
    for (int i = 0; i < cooper_proc_count(); i++) {
        Worker *w = &workers[i];
        w->last_span = NULL;
        w->root_bytes = 0;
        w->random = mark.cycle * (uint64_t)(2 * i + 1) | 1;
        worker_ready(w);
    }
    c_worker.last_span = NULL;
    __atomic_store_n(&phase, PHASE_MARK, __ATOMIC_SEQ_CST);
    __atomic_store_n(&pace_at, next_gc, __ATOMIC_RELAXED);
    __atomic_store_n(&cooper_barrier_on, 1, __ATOMIC_SEQ_CST);
    while (__atomic_load_n(&c_stores, __ATOMIC_SEQ_CST)) cpu_relax();
    Worker *w = barrier_worker();
    mark_variables(w);
    flush(w);
    cooper_mark_begin(mark.cycle, scan_for_scheduler, idle_mark);
}

/* With the world stopped and the heap lock held: finish marking, turn off the barrier, and sweep. `start` is when
   the stop was asked for. */
static void end_mark(uint64_t start) {
    count_cached();
    if (heap_bytes > stats.peak_heap) stats.peak_heap = heap_bytes;
    size_t before = heap_used;
    /* What the workers kept to themselves (the barrier's shading, mostly), and threads no worker has scanned. */
    for (int i = 0; i < cooper_proc_count(); i++) flush(&workers[i]);
    cooper_scan_threads(scan_range, &workers[0], mark.cycle);
    flush(&workers[0]);
    /* C does not stop: the barrier stays on until no work is left while no C store is shading. A store after that
       finds what it shades marked and scanned, unless it is unreachable. */
    for (;;) {
        spin_lock(&c_worker_lock);
        if (c_worker.objects) flush(&c_worker);
        if (!work_available()) {
            __atomic_store_n(&cooper_barrier_on, 0, __ATOMIC_SEQ_CST);
            spin_unlock(&c_worker_lock);
            break;
        }
        spin_unlock(&c_worker_lock);
        mark_stopped();
    }
    /* Leave nothing queued: mark the garbage that stores which saw the barrier on shaded since. */
    while (__atomic_load_n(&c_stores, __ATOMIC_SEQ_CST)) cpu_relax();
    if (c_worker.objects) flush(&c_worker);
    if (work_available()) mark_stopped();
    cooper_mark_end();
    __atomic_store_n(&phase, PHASE_OFF, __ATOMIC_SEQ_CST);
    size_t root_bytes = 0;
    for (int64_t i = 0; i < cooper_root_count; i++) root_bytes += cooper_roots[i].type->size;
    for (int i = 0; i < cooper_proc_count(); i++) root_bytes += workers[i].root_bytes;
    uint64_t marked = instrument ? now_ns() : 0;
    if (checkmark) check_marks_with_registers();
    uint64_t checked = instrument ? now_ns() : 0;
    /* The cached spans are swept like the rest and go back to the lists. */
    memset(caches, 0, (size_t)cooper_proc_count() * sizeof *caches);
    sweep();
    set_goal(heap_used, root_bytes);
    scavenge();
    uint64_t done = instrument ? now_ns() : 0;
    stats.collections++;
    stats.bytes_allocated += before > stats.last_live ? before - stats.last_live : 0;
    stats.last_live = heap_used;
    if (instrument) {
        stats.mark_ns += marked - mark.began;
        stats.mark_worker_ns += (uint64_t)mark.worker_ns;
        stats.sweep_ns += done - checked;
        record_pause(done - start);
    }
    if (trace) {
        fprintf(stderr,
                "gc %llu: marked %.2f ms (%.2f ms on all workers), ending pause %.2f ms (sweep %.2f), live %.1f MiB, "
                "next at %.1f MiB, heap %.1f MiB\n",
                (unsigned long long)stats.collections, (marked - mark.began) / 1e6, mark.worker_ns / 1e6,
                (done - start) / 1e6, (done - checked) / 1e6, heap_used / 1048576.0, next_gc / 1048576.0,
                heap_bytes / 1048576.0);
    }
}

/* Begin a cycle, unless one is on or (unless `forced`) the heap does not need it any more. */
static void begin_cycle(int forced) {
    uint64_t start = instrument ? now_ns() : 0;
    if (!cooper_stop_the_world()) return;
    heap_lock();
    count_cached();
    int begin = phase == PHASE_OFF && (forced || stress || markstress || heap_used >= trigger);
    if (begin) begin_mark();
    heap_unlock();
    if (begin && instrument) record_pause(now_ns() - start);
    cooper_start_the_world();
}

/* End the cycle, unless another worker is ending it. */
static void end_cycle(void) {
    if (__atomic_exchange_n(&mark.ending, 1, __ATOMIC_SEQ_CST)) return;
    uint64_t cycle = __atomic_load_n(&cycles, __ATOMIC_SEQ_CST);
    uint64_t start = instrument ? now_ns() : 0;
    if (cooper_stop_the_world()) {
        heap_lock();
        if (phase == PHASE_MARK && cycles == cycle) end_mark(start);
        heap_unlock();
        cooper_start_the_world();
    }
    __atomic_store_n(&mark.ending, 0, __ATOMIC_SEQ_CST);
}

/* Mark while the program runs, with the running P's worker, until `stop()` is true (or after `budget` items, see
   drain) or no work is left, scanning the stacks of threads that are not running too. A worker that finds no work
   while no other is marking ends the cycle. Gives whether it did any work. */
static bool mark_running(bool (*stop)(void), long budget) {
    if (__atomic_load_n(&phase, __ATOMIC_SEQ_CST) != PHASE_MARK) return false;
    Worker *w = barrier_worker();
    uint64_t items = w->items;
    uint64_t start = instrument ? now_ns() : 0;
    __atomic_add_fetch(&mark.working, 1, __ATOMIC_SEQ_CST);
    int empty;
    bool pending = true;
    for (;;) {
        empty = drain(w, stop, budget);
        if (!empty) break;
        pending = cooper_scan_waiting_threads();
        if (!work_available()) break;
    }
    int64_t others = __atomic_sub_fetch(&mark.working, 1, __ATOMIC_SEQ_CST);
    if (instrument) __atomic_add_fetch(&mark.worker_ns, (int64_t)(now_ns() - start), __ATOMIC_SEQ_CST);
    if (empty && !pending && others == 0 && !work_available()) end_cycle();
    return w->items != items;
}

static bool idle_mark(bool (*stop)(void)) {
    return mark_running(stop, -1);
}

static bool stop_requested(void) {
    return __atomic_load_n(&cooper_poll_word, __ATOMIC_RELAXED) != 0;
}

/* Help mark until the cycle ends, if one is on. */
static void finish_cycle(void) {
    uint64_t cycle = __atomic_load_n(&cycles, __ATOMIC_SEQ_CST);
    for (int spins = 0; __atomic_load_n(&phase, __ATOMIC_SEQ_CST) == PHASE_MARK && __atomic_load_n(&cycles, __ATOMIC_SEQ_CST) == cycle; spins++) {
        if (stop_requested()) cooper_poll_slow();
        else if (mark_running(stop_requested, -1)) spins = 0;
        else if (spins < 100) cpu_relax();
        else sched_yield();
    }
}

/* An allocation reached `pace_at`, or the collector is in a stress mode: begin a cycle, or help one end. Under
   markstress an allocation marks a little until the heap reaches its goal. */
static __attribute__((noinline)) void pace(void) {
    if (__atomic_load_n(&phase, __ATOMIC_RELAXED) == PHASE_OFF) {
        begin_cycle(0);
        if (!stress) return;
    } else if (markstress && __atomic_load_n(&heap_used, __ATOMIC_RELAXED) + caches[cooper_proc_id()].used <
                                 __atomic_load_n(&pace_at, __ATOMIC_RELAXED)) {
        mark_running(NULL, 16);
        return;
    }
    finish_cycle();
}

void cooper_gc_init(void) {
    if (!ready) init_heap();
    started = 1;
    cooper_sched_init();
}

/* Collect now: finish the cycle that is on, if any, and run one more. */
void cooper_gc_collect(void) {
    if (!ready) init_heap();
    if (!started) return;
    uint64_t target = __atomic_load_n(&cycles, __ATOMIC_SEQ_CST) + 1;
    for (;;) {
        uint64_t cycle = __atomic_load_n(&cycles, __ATOMIC_SEQ_CST);
        int marking = __atomic_load_n(&phase, __ATOMIC_SEQ_CST) == PHASE_MARK;
        if (cycle > target || (cycle == target && !marking)) return;
        if (marking) finish_cycle();
        else begin_cycle(1);
    }
}
