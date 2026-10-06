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
