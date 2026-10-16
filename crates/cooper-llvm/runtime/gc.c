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
 * This file is single-threaded; a per-thread cache sits where `cache` is when threads arrive.
 * Memory is not reclaimed yet.
 */

#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

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

enum { SPAN_FREE = 0, SPAN_IN_USE = 1 };

typedef struct Span {
    uintptr_t start;
    size_t npages;
    size_t elemsize;     /* the slot size, or for a large object its size rounded up to a page */
    uint32_t nelems;
    uint32_t freeindex;  /* slots below this are allocated; above it, allocated iff alloc_bits says so */
    uint32_t alloc_count;
    uint16_t spanclass;  /* size class << 1 | noscan, or 0 for a large object */
    uint8_t state;
    uint8_t noscan;
    uint8_t large;
    uint8_t heap_bits_in_span;
    uint8_t needzero;    /* the span's memory may be dirty: zero a slot when it is handed out */
    uint64_t *alloc_bits; /* allocated as of the last collection */
    uint64_t *mark_bits;
    const CooperType *large_type;
    size_t large_size;
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

/* A new span of `npages` pages from fresh memory. */
static Span *new_span(size_t npages) {
    if (!current_arena || current_arena->npages - current_arena->next_page < npages) {
        current_arena = new_arena(npages > ARENA_PAGES ? npages : ARENA_PAGES);
    }
    Arena *arena = current_arena;
    Span *span = meta_alloc(sizeof(Span));
    span->start = arena->base + (arena->next_page << PAGE_SHIFT);
    span->npages = npages;
    span->state = SPAN_IN_USE;
    for (size_t page = 0; page < npages; page++) arena->spans[arena->next_page + page] = span;
    arena->next_page += npages;
    heap_bytes += npages << PAGE_SHIFT;
    span_count++;
    return span;
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

static Span *cache[NUM_CLASSES * 2];

static Span *new_small_span(int class, int noscan) {
    Span *span = new_span(class_pages[class]);
    size_t bytes = span->npages << PAGE_SHIFT;
    span->elemsize = class_size[class];
    span->spanclass = (uint16_t)(class << 1 | noscan);
    span->noscan = (uint8_t)noscan;
    span->heap_bits_in_span = !noscan && span->elemsize <= MIN_SIZE_FOR_HEADER;
    /* A bitmap of one bit per word takes the end of the span. */
    size_t bitmap = span->heap_bits_in_span ? bytes / 64 : 0;
    span->nelems = (uint32_t)((bytes - bitmap) / span->elemsize);
    size_t words = (span->nelems + 63) / 64;
    span->alloc_bits = meta_alloc(words * 8);
    span->mark_bits = meta_alloc(words * 8);
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
    span->alloc_bits = meta_alloc(8);
    span->mark_bits = meta_alloc(8);
    return (void *)span->start;
}

void *cooper_alloc(size_t size, const CooperType *type) {
    if (type && (type->size != type->layout_size || size % type->size != 0)) {
        cooper_panic("internal error: an allocation's size does not match its type");
    }
    if (size == 0) return &zerobase;
    if (!ready) {
        init_classes();
        ready = 1;
    }
    int noscan = type == NULL;
    size_t total = size;
    int header = !noscan && size > MIN_SIZE_FOR_HEADER;
    if (header) total += HEADER_SIZE;
    if (total < size || total > MAX_SMALL) return allocate_large(size, type);
    int class = size_to_class8[(total + 7) / 8];
    Span *span = cache[class << 1 | noscan];
    void *slot = span ? next_free(span) : NULL;
    while (!slot) {
        span = cache[class << 1 | noscan] = new_small_span(class, noscan);
        slot = next_free(span);
    }
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
