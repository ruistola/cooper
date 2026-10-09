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
   Generated formatters expand a pointer's target at most once per formatting, and at
   most FORMAT_DEPTH pointers deep, printing the address otherwise, so that cyclic and
   shared structures terminate. */

#define FORMAT_DEPTH 64

static const void **format_seen;
static uint64_t format_seen_capacity;
static uint64_t format_seen_count;
static int format_depth;

static uint64_t format_slot(const void *p) {
    uint64_t h = (uint64_t)(uintptr_t)p * 0x9E3779B97F4A7C15ull;
    return (h >> 32) & (format_seen_capacity - 1);
}

/* Record `p` as expanded, returning 0 if it already was. */
static int format_insert(const void *p) {
    if (format_seen_count * 2 >= format_seen_capacity) {
        const void **old = format_seen;
        uint64_t old_capacity = format_seen_capacity;
        format_seen_capacity = old_capacity ? old_capacity * 2 : 64;
        format_seen = calloc(format_seen_capacity, sizeof *format_seen);
        if (format_seen == NULL) cooper_panic("out of memory");
        format_seen_count = 0;
        for (uint64_t i = 0; i < old_capacity; i++) {
            if (old[i]) format_insert(old[i]);
        }
        free(old);
    }
    uint64_t i = format_slot(p);
    while (format_seen[i]) {
        if (format_seen[i] == p) return 0;
        i = (i + 1) & (format_seen_capacity - 1);
    }
    format_seen[i] = p;
    format_seen_count++;
    return 1;
}

/* Start formatting a value: no pointer has been expanded yet. */
void cooper_format_begin(void) {
    if (format_seen_count) memset(format_seen, 0, format_seen_capacity * sizeof *format_seen);
    format_seen_count = 0;
    format_depth = 0;
}

/* Whether to expand the target of the non-null pointer `p`; if so, the expansion ends
   with cooper_format_leave. */
int32_t cooper_format_enter(const void *p) {
    if (format_depth >= FORMAT_DEPTH || !format_insert(p)) return 0;
    format_depth++;
    return 1;
}

void cooper_format_leave(void) {
    format_depth--;
}

/* The address `p` as text. */
void cooper_format_address(const void *p, CooperString *out) {
    char buffer[32];
    int len = snprintf(buffer, sizeof buffer, "%p", p);
    string_of(buffer, len, out);
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
