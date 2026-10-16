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
 * The collector is stop-the-world mark and sweep, single-threaded. The heap is precise: an
 * object's pointers are found from the record above. The roots are the module-level variables
 * (a table the compiler emits, with their types), and the stack and registers, which are scanned
 * conservatively: any word that points into an allocated object keeps it. Nothing moves, so an
 * ambiguous word costs only the retention of what it appears to point at.
 *
 * COOPER_GC=stress collects at every allocation, and COOPER_GC=poison (or stress) fills freed
 * memory with 0xDB, so that a missed root shows up as a crash rather than as silent reuse.
 */

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
    uint32_t freeindex;  /* slots below this are allocated; above it, allocated iff alloc_bits says so */
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
    uint8_t queued;       /* on the collector's span queue */
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

static Arena **arena_table[1 << TABLE_BITS]; /* [address >> 26 >> 11][(address >> 26) & 2047] */
static Arena *current_arena;
static size_t heap_bytes, span_count;
static size_t heap_used;  /* bytes in allocated slots: live as of the last collection, plus allocated since */
static size_t next_gc;    /* collect when `heap_used` reaches this */
static int stress, poison, object_mark; /* object_mark: scan objects as reached, not a span at a time */
static void *stack_base;
static int collecting;
static unsigned growth = 100;  /* the heap may grow to this percent over what survived: COOPER_HEAP_GROWTH */
static int gc_off;
static size_t root_bytes;      /* stack and module-level variables scanned by the last collection */

/* --- measurements: COOPER_GC=trace prints a line per collection, COOPER_GC_STATS=1 a summary at exit --- */

static struct {
    uint64_t collections, bytes_allocated;
    uint64_t mark_ns, sweep_ns, max_pause_ns;
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
        if (!*top) *top = meta_alloc(sizeof(Arena *) << TABLE_BITS);
        (*top)[index & ((1 << TABLE_BITS) - 1)] = arena;
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
   otherwise from fresh memory. */
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
        arena->next_page += npages;
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
    for (size_t page = 0; page < npages; page++) arena->spans[first_page + page] = span;
    span->all_next = all_spans;
    all_spans = span;
    heap_bytes += npages << PAGE_SHIFT;
    span_count++;
    return span;
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
    Arena **row = arena_table[(index >> TABLE_BITS) & ((1 << TABLE_BITS) - 1)];
    if (!row) return NULL;
    Arena *arena = row[index & ((1 << TABLE_BITS) - 1)];
    if (!arena || address < arena->base) return NULL;
    size_t page = (address - arena->base) >> PAGE_SHIFT;
    return page < arena->next_page ? arena->spans[page] : NULL;
}

/* --- spans of small objects --- */

/* The kinds of small-object span. A scannable object up to 512 bytes whose pointer words are the same
   whatever it holds records them in a bitmap at the end of its span; any other scannable object has a
   header word holding its type (which a sum type's variant sites need). */
enum { KIND_BITS = 0, KIND_NOSCAN = 1, KIND_HEADER = 2, KINDS = 3 };

static Span *cache[NUM_CLASSES * KINDS];
static Span *partial[NUM_CLASSES * KINDS];

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
    return span;
}

/* The next free slot of a span, or null when it has none. */
static void *next_free(Span *span) {
    uint32_t index = span->freeindex;
    while (index < span->nelems) {
        uint64_t free = ~span->alloc_bits[index / 64] >> (index % 64);
        if (free == 0) {
            index = (index / 64 + 1) * 64;
            continue;
        }
        index += (uint32_t)__builtin_ctzll(free);
        if (index >= span->nelems) break;
        span->freeindex = index + 1;
        span->alloc_count++;
        return (void *)(span->start + (size_t)index * span->elemsize);
    }
    span->freeindex = span->nelems;
    return NULL;
}

/* --- recording where an object's pointers are --- */

static uint64_t *heap_bits(const Span *span) {
    return (uint64_t *)(span->start + (span->npages << PAGE_SHIFT) - (span->npages << PAGE_SHIFT) / 64);
}

/* Set the heap bits of the slot at `slot` of a small-object span: the descriptor tiled over
   `size` bytes, and clear for the rest of the slot. A slot is at most 64 words, so the bits of
   one object are a single mask that falls in at most two words of the bitmap. */
static void write_heap_bits(const Span *span, uintptr_t slot, const CooperType *type, size_t size) {
    uint64_t *bits = heap_bits(span);
    size_t first = (slot - span->start) / 8;
    size_t slot_words = span->elemsize / 8;
    size_t stride = type->size / 8;
    uint64_t mask = 0;
    for (size_t copy = 0; copy < size / type->size; copy++) mask |= type->bits[0] << (copy * stride);
    uint64_t clear = slot_words >= 64 ? ~(uint64_t)0 : ((uint64_t)1 << slot_words) - 1;
    size_t word = first / 64, shift = first % 64;
    bits[word] = (bits[word] & ~(clear << shift)) | (mask << shift);
    if (shift + slot_words > 64) {
        bits[word + 1] = (bits[word + 1] & ~(clear >> (64 - shift))) | (mask >> (64 - shift));
    }
}

/* --- allocation --- */

static uint64_t zerobase;
static int ready;

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
    if (span->needzero) memset((void *)span->start, 0, size);
    heap_used += span->elemsize;
    return (void *)span->start;
}

static void collect(void);

static void print_stats(void) {
    uint64_t pause = stats.mark_ns + stats.sweep_ns;
    fprintf(stderr,
            "gc: %llu collections, %.1f ms collecting (mark %.1f, sweep %.1f), longest pause %.2f ms\n"
            "gc: %.1f MiB allocated, peak heap %.1f MiB, %.1f MiB live after the last collection, %.1f MiB returned to the system\n",
            (unsigned long long)stats.collections, pause / 1e6, stats.mark_ns / 1e6, stats.sweep_ns / 1e6,
            stats.max_pause_ns / 1e6, stats.bytes_allocated / 1048576.0,
            stats.peak_heap / 1048576.0, stats.last_live / 1048576.0, released_bytes / 1048576.0);
}

static void init_heap(void) {
    init_classes();
    const char *mode = getenv("COOPER_GC");
    trace = mode && strstr(mode, "trace");
    if (getenv("COOPER_GC_STATS")) {
        atexit(print_stats);
        instrument = 1;
    }
    instrument |= trace;
    const char *percent = getenv("COOPER_HEAP_GROWTH");
    if (percent && !strcmp(percent, "off")) gc_off = 1;
    else if (percent && atoi(percent) > 0) growth = (unsigned)atoi(percent);
    stress = mode && strstr(mode, "stress");
    object_mark = mode && strstr(mode, "objects");
    poison = stress || (mode && strstr(mode, "poison"));
    next_gc = MIN_HEAP * growth / 100;
    ready = 1;
}

void *cooper_alloc(size_t size, const CooperType *type) {
    if (type && (type->size != type->layout_size || size % type->size != 0)) {
        cooper_panic("internal error: an allocation's size does not match its type");
    }
    if (size == 0) return &zerobase;
    if (!ready) init_heap();
    if ((stress || (heap_used >= next_gc && !gc_off)) && stack_base && !collecting) collect();
    int noscan = type == NULL;
    int kind = noscan ? KIND_NOSCAN : (type->nsites || size > MIN_SIZE_FOR_HEADER) ? KIND_HEADER : KIND_BITS;
    size_t total = size;
    if (kind == KIND_HEADER) total += HEADER_SIZE;
    if (total < size || total > MAX_SMALL) return allocate_large(size, type);
    int class = size_to_class8[(total + 7) / 8];
    int spanclass = class * KINDS + kind;
    Span *span = cache[spanclass];
    void *slot = span ? next_free(span) : NULL;
    while (!slot) {
        span = partial[spanclass];
        if (span) {
            partial[spanclass] = span->partial_next;
            span->partial_next = NULL;
        } else {
            span = new_small_span(class, kind);
        }
        cache[spanclass] = span;
        slot = next_free(span);
    }
    heap_used += span->elemsize;
    if (span->needzero) memset(slot, 0, span->elemsize);
    if (noscan) return slot;
    if (span->heap_bits_in_span) {
        write_heap_bits(span, (uintptr_t)slot, type, size);
        return slot;
    }
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

typedef struct {
    uintptr_t slot;
    Span *span;
} Work;

static Work *work;
static size_t work_len, work_cap;

static void push_work(uintptr_t slot, Span *span) {
    if (work_len == work_cap) {
        size_t cap = work_cap ? work_cap * 2 : 4096;
        Work *grown = map_zeroed(cap * sizeof *grown);
        if (work) {
            memcpy(grown, work, work_len * sizeof *grown);
            munmap(work, work_cap * sizeof *work);
        }
        work = grown;
        work_cap = cap;
    }
    work[work_len].slot = slot;
    work[work_len].span = span;
    work_len++;
}

/* The spans with objects waiting to be scanned, first in first out: the longer a span waits, the more
   of its objects have been reached by the time it is scanned. */
static Span **queue;
static size_t queue_head, queue_len, queue_cap;

static void queue_span(Span *span) {
    if (queue_len == queue_cap) {
        size_t cap = queue_cap ? queue_cap * 2 : 1024;
        Span **grown = map_zeroed(cap * sizeof *grown);
        for (size_t i = 0; i < queue_len; i++) grown[i] = queue[(queue_head + i) % queue_cap];
        if (queue) munmap(queue, queue_cap * sizeof *queue);
        queue = grown;
        queue_head = 0;
        queue_cap = cap;
    }
    queue[(queue_head + queue_len++) % queue_cap] = span;
}

/* Mark the object `word` points into, if it points into an allocated one. */
static Span *last_span; /* the span the previous word pointed into: the next often does too */

static void mark_word(uintptr_t word) {
    Span *span = last_span;
    if (!span || word - span->start >= span->npages << PAGE_SHIFT || span->state != SPAN_IN_USE) {
        span = span_of(word);
        if (span) last_span = span;
    }
    if (!span || span->state != SPAN_IN_USE || word < span->start) return;
    uint32_t index = span->large ? 0 : (uint32_t)(((word - span->start) * span->div_magic) >> 32);
    if (index >= span->nelems) return;
    uint64_t bit = (uint64_t)1 << (index % 64);
    if (index >= span->freeindex && !(span->alloc_bits[index / 64] & bit)) return;
    if (span->mark_bits[index / 64] & bit) return;
    span->mark_bits[index / 64] |= bit;
    if (span->noscan) return;
    if (span->heap_bits_in_span && !object_mark) {
        /* Green Tea: do not scan the object now. Queue its span, so that the objects reached
           meanwhile in the same span are scanned together, a span at a time. */
        if (!span->queued) {
            span->queued = 1;
            queue_span(span);
        }
        return;
    }
    push_work(span->start + (size_t)index * span->elemsize, span);
}

/* The pointers in one value of `type` at `base`: the words its bitmap names, and the payload of the
   variant each sum type in it holds. */
static void scan_value(const uint64_t *base, const CooperType *type) {
    for (size_t chunk = 0; chunk < (type->words + 63) / 64; chunk++) {
        uint64_t mask = type->bits[chunk];
        while (mask) {
            mark_word(base[chunk * 64 + (size_t)__builtin_ctzll(mask)]);
            mask &= mask - 1;
        }
    }
    for (uint64_t i = 0; i < type->nsites; i++) {
        const CooperSite *site = &type->sites[i];
        uint32_t tag = *(const uint32_t *)((const char *)base + site->tag_offset);
        if (tag < site->variants && site->payloads[tag]) {
            scan_value((const uint64_t *)((const char *)base + site->payload_offset), site->payloads[tag]);
        }
    }
}

/* The pointers among `bytes` bytes at `base`, which hold whole values of `type`. */
static void scan_tiled(const uint64_t *base, size_t bytes, const CooperType *type) {
    size_t stride = type->size / 8;
    for (size_t copy = 0; copy < bytes / type->size; copy++) scan_value(base + copy * stride, type);
}

static void scan_object(uintptr_t slot, const Span *span) {
    if (span->large) {
        scan_tiled((const uint64_t *)slot, span->large_size, span->large_type);
    } else if (span->heap_bits_in_span) {
        /* The slot's pointer words, as one mask taken from the span's bitmap, and only those visited. */
        const uint64_t *bits = heap_bits(span);
        size_t first = (slot - span->start) / 8, slot_words = span->elemsize / 8;
        size_t at = first / 64, shift = first % 64;
        uint64_t mask = bits[at] >> shift;
        if (shift && shift + slot_words > 64) mask |= bits[at + 1] << (64 - shift);
        if (slot_words < 64) mask &= ((uint64_t)1 << slot_words) - 1;
        while (mask) {
            mark_word(((const uint64_t *)slot)[__builtin_ctzll(mask)]);
            mask &= mask - 1;
        }
    } else {
        const CooperType *type = *(const CooperType **)slot;
        scan_tiled((const uint64_t *)(slot + HEADER_SIZE), span->elemsize - HEADER_SIZE, type);
    }
}

/* Scan the objects of a small span that were marked and not yet scanned. Scanning may mark more of
   them (and requeue the span), which a later turn picks up. */
static void scan_span(Span *span) {
    size_t words = (span->nelems + 63) / 64;
    for (size_t word = 0; word < words; word++) {
        uint64_t pending = span->mark_bits[word] & ~span->scan_bits[word];
        span->scan_bits[word] |= pending;
        while (pending) {
            size_t index = word * 64 + (size_t)__builtin_ctzll(pending);
            pending &= pending - 1;
            scan_object(span->start + index * span->elemsize, span);
        }
    }
}

static void drain(void) {
    while (work_len || queue_len) {
        while (work_len) {
            Work item = work[--work_len];
            scan_object(item.slot, item.span);
        }
        if (queue_len) {
            Span *span = queue[queue_head];
            queue_head = (queue_head + 1) % queue_cap;
            queue_len--;
            span->queued = 0;
            /* The span after this one will be scanned soon: start bringing in what that takes. */
            if (queue_len) {
                const Span *next = queue[queue_head];
                __builtin_prefetch(next->mark_bits);
                __builtin_prefetch(next->scan_bits);
                __builtin_prefetch((const void *)next->start);
            }
            scan_span(span);
        }
    }
}

/* The module-level variables the compiler emitted: each one's address and type. */
struct Root {
    const uint64_t *address;
    const CooperType *type;
};
extern const struct Root cooper_roots[];
extern const int64_t cooper_root_count;

static void mark_roots(void) {
    root_bytes = 0;
    last_span = NULL;
    for (int64_t i = 0; i < cooper_root_count; i++) {
        root_bytes += cooper_roots[i].type->size;
        scan_tiled(cooper_roots[i].address, cooper_roots[i].type->size, cooper_roots[i].type);
    }
    drain();
}

/* Every word of the stack from `here` up to its base, as a possible pointer. */
static __attribute__((noinline)) void scan_stack(void) {
    volatile uintptr_t here = 0;
    uintptr_t low = (uintptr_t)&here & ~(uintptr_t)7;
    root_bytes += (size_t)((uintptr_t)stack_base - low);
    for (const uintptr_t *word = (const uintptr_t *)low; (const void *)word < stack_base; word++) {
        mark_word(*word);
    }
    drain();
}

static void fill(void *memory, size_t bytes) {
    if (poison) memset(memory, 0xDB, bytes);
}

/* Free what is unmarked, and make the marked objects the allocated ones. */
static void sweep(void) {
    memset(cache, 0, sizeof cache);
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
            for (size_t page = 0; page < span->npages; page++) span->arena->spans[span->first_page + page] = NULL;
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
            for (size_t page = 0; page < span->npages; page++) span->arena->spans[span->first_page + page] = NULL;
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

static __attribute__((noinline)) void collect_with_registers(void) {
    /* The callee-saved registers are spilled into this frame, which the scan covers. */
    __builtin_unwind_init();
    scan_stack();
}

static void collect(void) {
    collecting = 1;
    if (heap_bytes > stats.peak_heap) stats.peak_heap = heap_bytes;
    size_t before = heap_used;
    uint64_t start = instrument ? now_ns() : 0;
    mark_roots();
    collect_with_registers();
    uint64_t marked = instrument ? now_ns() : 0;
    sweep();
    uint64_t done = instrument ? now_ns() : 0;
    /* Go's goal: what survived, plus a percentage of it and of the roots, but at least a minimum. */
    next_gc = heap_used + (heap_used + root_bytes) / 100 * growth;
    if (next_gc < MIN_HEAP / 100 * growth) next_gc = MIN_HEAP / 100 * growth;
    scavenge();
    stats.collections++;
    stats.bytes_allocated += before > stats.last_live ? before - stats.last_live : 0;
    stats.last_live = heap_used;
    if (instrument) {
        stats.mark_ns += marked - start;
        stats.sweep_ns += done - marked;
        if (done - start > stats.max_pause_ns) stats.max_pause_ns = done - start;
    }
    if (trace) {
        fprintf(stderr, "gc %llu: mark %.2f ms, sweep %.2f ms, live %.1f MiB, next at %.1f MiB, heap %.1f MiB\n",
                (unsigned long long)stats.collections, (marked - start) / 1e6, (done - marked) / 1e6,
                heap_used / 1048576.0, next_gc / 1048576.0, heap_bytes / 1048576.0);
    }
    collecting = 0;
}

/* Tell the collector where the stack begins: the address of a local of `main`. */
void cooper_gc_init(void *base) {
    stack_base = base;
}

/* Collect now. */
void cooper_gc_collect(void) {
    if (!ready) init_heap();
    if (stack_base && !collecting) collect();
}
