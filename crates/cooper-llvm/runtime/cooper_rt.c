/*
 * The Cooper runtime, linked into every program.
 *
 * It owns the process entry point: the C `main` runs any runtime setup and then the
 * program's own `main`, which the compiler exports as `cooper_main` returning the
 * process exit status. It also implements the operations generated code calls into.
 */

#include <stdio.h>
#include <stdlib.h>

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
