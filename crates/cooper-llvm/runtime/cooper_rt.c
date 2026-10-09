/*
 * The Cooper runtime, linked into every program.
 *
 * It owns the process entry point: the C `main` runs any runtime setup and then the
 * program's own `main`, which the compiler exports as `cooper_main` returning the
 * process exit status. It also implements the operations generated code calls into.
 */

#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* The exit status of a program stopped by a runtime error. */
#define PANIC_STATUS 101

extern int cooper_main(void);

int main(void) {
    return cooper_main();
}

/* Stop the program on a runtime error (overflow, division by zero, ...). `message`
   names the error and its source location. */
_Noreturn void cooper_panic(const char *message) {
    fflush(stdout);
    fprintf(stderr, "panic: %s\n", message);
    exit(PANIC_STATUS);
}

/* Allocate `size` zeroed bytes on the heap. Memory is not yet reclaimed. */
void *cooper_alloc(size_t size) {
    void *memory = calloc(1, size ? size : 1);
    if (memory == NULL) {
        cooper_panic("out of memory");
    }
    return memory;
}

/* A string as generated code lays it out: its bytes and their count. */
typedef struct {
    const char *data;
    uint64_t len;
} CooperString;

/* Concatenate two strings into a fresh one, written to `out`. */
void cooper_string_concat(const char *a, uint64_t a_len, const char *b, uint64_t b_len,
                          CooperString *out) {
    uint64_t len = a_len + b_len;
    if (len < a_len) {
        cooper_panic("string too long");
    }
    char *data = cooper_alloc(len);
    if (a_len) memcpy(data, a, a_len);
    if (b_len) memcpy(data + a_len, b, b_len);
    out->data = data;
    out->len = len;
}

/* Whether two strings hold the same bytes: 1 if so, 0 if not. */
int32_t cooper_string_eq(const char *a, uint64_t a_len, const char *b, uint64_t b_len) {
    return a_len == b_len && (a_len == 0 || memcmp(a, b, a_len) == 0);
}

/* Fresh element storage of `capacity` elements of `size` bytes, holding a copy of the
   `len` elements at `data`. */
void *cooper_array_grow(const void *data, uint64_t len, uint64_t capacity, uint64_t size) {
    if (size && capacity > UINT64_MAX / size) {
        cooper_panic("array too large");
    }
    void *grown = cooper_alloc(capacity * size);
    if (len) memcpy(grown, data, len * size);
    return grown;
}

/* A NUL-terminated copy of a string's bytes, for C. */
char *cooper_to_cstring(const char *data, uint64_t len) {
    if (len == UINT64_MAX) {
        cooper_panic("string too long");
    }
    char *c = cooper_alloc(len + 1);
    if (len) memcpy(c, data, len);
    return c;
}

/* A string copied from the C string `c` up to its NUL, written to `out`; empty when `c`
   is null. */
void cooper_from_cstring(const char *c, CooperString *out) {
    uint64_t len = c ? strlen(c) : 0;
    char *data = cooper_alloc(len);
    if (len) memcpy(data, c, len);
    out->data = data;
    out->len = len;
}

/* A copy of the `n` bytes at `p`, as array storage; generated code rejects a negative
   `n`. Bytes read through a null `p` are zero, as loads through nil are. */
void *cooper_copy_bytes(const void *p, int64_t n) {
    void *bytes = cooper_alloc((size_t)n);
    if (p && n) memcpy(bytes, p, (size_t)n);
    return bytes;
}

/* Write a string to standard output, followed by a newline when `newline` is set. */
void cooper_print(const char *data, uint64_t len, int32_t newline) {
    if (len) fwrite(data, 1, len, stdout);
    if (newline) fputc('\n', stdout);
}

/* A fresh string holding the `len` bytes at `text`. */
static void string_of(const char *text, int len, CooperString *out) {
    char *data = cooper_alloc((size_t)len);
    memcpy(data, text, (size_t)len);
    out->data = data;
    out->len = (uint64_t)len;
}

/* The decimal text of an integer: `value` as signed when `is_signed`, otherwise as the
   unsigned number with the same bits. */
void cooper_format_int(int64_t value, int32_t is_signed, CooperString *out) {
    char text[32];
    int len = is_signed ? snprintf(text, sizeof text, "%" PRId64, value)
                        : snprintf(text, sizeof text, "%" PRIu64, (uint64_t)value);
    string_of(text, len, out);
}

/* The shortest decimal text that reads back as the same float (an `f32` when
   `is_f32`), so 0.1 prints as "0.1". NaN and infinities print as "nan", "inf", "-inf". */
void cooper_format_float(double value, int32_t is_f32, CooperString *out) {
    char text[40];
    int len = 0;
    for (int precision = 1; precision <= 17; precision++) {
        len = snprintf(text, sizeof text, "%.*g", precision, value);
        if (is_f32 ? strtof(text, NULL) == (float)value : strtod(text, NULL) == value) {
            break;
        }
    }
    string_of(text, len, out);
}

/* --- Formatting any value (`string(x)`) ---
   A formatting runs the generated formatters twice over the value. The first pass finds
   the pointer targets reached more than once (through a cycle or sharing); the second,
   whose text is kept, labels each such target where it is expanded (`#1=&...`) and
   shows the label wherever it is reached again (`#1`). Each pass expands a target at
   most once, and at most FORMAT_DEPTH pointers deep (`...` past that), so every value
   formats in time proportional to its size. */

#define FORMAT_DEPTH 64

typedef struct {
    const void *target;
    int32_t shared;
    int32_t label;   /* 0 until labelled in the printing pass */
    int32_t printed; /* expanded in the printing pass */
} FormatEntry;

static FormatEntry *format_entries;
static uint64_t format_capacity;
static uint64_t format_count;
static int format_depth;
static int format_printing;
static int32_t format_labels;

static FormatEntry *format_find(const void *p) {
    if (format_capacity == 0) return NULL;
    uint64_t mask = format_capacity - 1;
    uint64_t i = ((uint64_t)(uintptr_t)p * 0x9E3779B97F4A7C15ull >> 32) & mask;
    while (format_entries[i].target && format_entries[i].target != p) i = (i + 1) & mask;
    return &format_entries[i];
}

/* The entry of `p`, added if absent. */
static FormatEntry *format_entry(const void *p) {
    if (format_count * 2 >= format_capacity) {
        FormatEntry *old = format_entries;
        uint64_t old_capacity = format_capacity;
        format_capacity = old_capacity ? old_capacity * 2 : 64;
        format_entries = calloc(format_capacity, sizeof *format_entries);
        if (format_entries == NULL) cooper_panic("out of memory");
        for (uint64_t i = 0; i < old_capacity; i++) {
            if (old[i].target) *format_find(old[i].target) = old[i];
        }
        free(old);
    }
    FormatEntry *entry = format_find(p);
    if (!entry->target) {
        entry->target = p;
        format_count++;
    }
    return entry;
}

/* Start the first pass of formatting a value. */
void cooper_format_begin(void) {
    if (format_count) memset(format_entries, 0, format_capacity * sizeof *format_entries);
    format_count = 0;
    format_depth = 0;
    format_printing = 0;
    format_labels = 0;
}

/* Start the printing pass. */
void cooper_format_print(void) {
    format_depth = 0;
    format_printing = 1;
}

/* Whether to expand the target of the non-null pointer `p`; if so, the expansion ends
   with cooper_format_leave. */
int32_t cooper_format_enter(const void *p) {
    if (format_depth >= FORMAT_DEPTH) return 0;
    if (format_printing) {
        FormatEntry *entry = format_find(p);
        if (entry && entry->target && entry->printed) return 0;
        if (entry && entry->target) entry->printed = 1;
    } else {
        FormatEntry *entry = format_entry(p);
        if (entry->shared++) return 0;
    }
    format_depth++;
    return 1;
}

void cooper_format_leave(void) {
    format_depth--;
}

/* The label of an expanded target: `#n=` if it is shared, numbered in printing order. */
void cooper_format_label(const void *p, CooperString *out) {
    FormatEntry *entry = format_printing ? format_find(p) : NULL;
    if (!entry || !entry->target || entry->shared < 2) {
        string_of("", 0, out);
        return;
    }
    entry->label = ++format_labels;
    char buffer[16];
    string_of(buffer, snprintf(buffer, sizeof buffer, "#%d=", entry->label), out);
}

/* A target not expanded: its label, or `...` past the depth limit. */
void cooper_format_reference(const void *p, CooperString *out) {
    FormatEntry *entry = format_printing ? format_find(p) : NULL;
    if (!entry || !entry->target || !entry->label) {
        string_of("...", 3, out);
        return;
    }
    char buffer[16];
    string_of(buffer, snprintf(buffer, sizeof buffer, "#%d", entry->label), out);
}

/* The string as a quoted literal: `"` and `\` escaped, and the control characters
   Cooper literals spell (`\n`, `\t`, `\r`, `\0`) written as escapes. */
void cooper_quote(const char *data, uint64_t len, CooperString *out) {
    uint64_t size = 2;
    for (uint64_t i = 0; i < len; i++) {
        char c = data[i];
        size += (c == '"' || c == '\\' || c == '\n' || c == '\t' || c == '\r' || c == '\0') ? 2 : 1;
    }
    char *text = cooper_alloc(size);
    uint64_t at = 0;
    text[at++] = '"';
    for (uint64_t i = 0; i < len; i++) {
        char c = data[i];
        const char *escape = c == '"' ? "\\\"" : c == '\\' ? "\\\\" : c == '\n' ? "\\n"
                           : c == '\t' ? "\\t" : c == '\r' ? "\\r" : c == '\0' ? "\\0" : NULL;
        if (escape) {
            text[at++] = escape[0];
            text[at++] = escape[1];
        } else {
            text[at++] = c;
        }
    }
    text[at++] = '"';
    out->data = text;
    out->len = at;
}

/* --- Format specs in string holes --- */

/* The low `bits` bits of `value` in base `radix` (2, 8, or 16), uppercase when `upper`:
   a negative number shows as its two's complement. */
void cooper_format_radix(uint64_t value, int32_t bits, int32_t radix, int32_t upper, CooperString *out) {
    if (bits < 64) value &= ((uint64_t)1 << bits) - 1;
    const char *digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    char buffer[64];
    int at = 64;
    do {
        buffer[--at] = digits[value % (uint64_t)radix];
        value /= (uint64_t)radix;
    } while (value);
    string_of(buffer + at, 64 - at, out);
}

/* `value` with `precision` digits after the point. */
void cooper_format_fixed(double value, int32_t precision, CooperString *out) {
    int len = snprintf(NULL, 0, "%.*f", precision, value);
    char *text = cooper_alloc((size_t)len + 1);
    snprintf(text, (size_t)len + 1, "%.*f", precision, value);
    out->data = text;
    out->len = (uint64_t)len;
}

/* The UTF-8 encoding of code point `c` into `buffer`, returning its length. */
static int utf8_encode(uint32_t c, char *buffer) {
    if (c < 0x80) { buffer[0] = (char)c; return 1; }
    if (c < 0x800) { buffer[0] = (char)(0xC0 | c >> 6); buffer[1] = (char)(0x80 | (c & 0x3F)); return 2; }
    if (c < 0x10000) {
        buffer[0] = (char)(0xE0 | c >> 12); buffer[1] = (char)(0x80 | (c >> 6 & 0x3F));
        buffer[2] = (char)(0x80 | (c & 0x3F)); return 3;
    }
    buffer[0] = (char)(0xF0 | c >> 18); buffer[1] = (char)(0x80 | (c >> 12 & 0x3F));
    buffer[2] = (char)(0x80 | (c >> 6 & 0x3F)); buffer[3] = (char)(0x80 | (c & 0x3F)); return 4;
}

/* `text` padded to at least `width` characters (code points): with `fill` after it
   (`align` 0), before it (1), or around it, the odd one after (2); or, when `zero`, with
   zeros after a leading sign. */
void cooper_pad(const char *text, uint64_t len, int64_t width, int32_t align, int32_t fill,
                int32_t zero, CooperString *out) {
    int64_t chars = 0;
    for (uint64_t i = 0; i < len; i++) chars += ((unsigned char)text[i] & 0xC0) != 0x80;
    if (chars >= width) {
        out->data = text;
        out->len = len;
        return;
    }
    int64_t missing = width - chars;
    char fill_bytes[4];
    int fill_len = zero ? 1 : utf8_encode((uint32_t)fill, fill_bytes);
    if (zero) fill_bytes[0] = '0';
    char *padded = cooper_alloc(len + (uint64_t)(missing * fill_len));
    uint64_t at = 0;
    int64_t before = zero || align == 1 ? missing : align == 2 ? missing / 2 : 0;
    uint64_t start = 0;
    if (zero && len && (text[0] == '-' || text[0] == '+')) padded[at++] = text[start++];
    for (int64_t i = 0; i < before; i++) for (int b = 0; b < fill_len; b++) padded[at++] = fill_bytes[b];
    memcpy(padded + at, text + start, len - start);
    at += len - start;
    for (int64_t i = before; i < missing; i++) for (int b = 0; b < fill_len; b++) padded[at++] = fill_bytes[b];
    out->data = padded;
    out->len = at;
}
