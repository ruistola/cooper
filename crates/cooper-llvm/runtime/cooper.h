/* Declarations shared by the Cooper runtime's C files. */

#ifndef COOPER_H
#define COOPER_H

#include <stddef.h>
#include <stdint.h>

typedef struct CooperType CooperType;

/* A sum type value whose pointer words depend on its variant: where its tag (a u32) and payload
   are in the value, and a descriptor for the payload of each variant (null: it holds no pointers). */
typedef struct {
    uint64_t tag_offset;
    uint64_t payload_offset;
    uint64_t variants;
    const CooperType *const *payloads;
} CooperSite;

/* How the collector sees a type: its size in bytes, flags, the number of 8-byte words it covers,
   one bit per word that is set when the word holds a pointer whatever variant it holds, and the
   sum type values in it (sites). */
struct CooperType {
    uint64_t size;
    uint64_t layout_size; /* what the compiler computed; it must equal `size` */
    uint64_t flags;
    uint64_t words;
    uint64_t nsites;
    const CooperSite *sites;
    uint64_t bits[];
};

/* Stop the program on a runtime error. `message` names the error and its source location. */
_Noreturn void cooper_panic(const char *message);

/* Allocate `size` zeroed bytes on the heap. `type` describes one value of the type
   allocated, and the allocation holds a whole number of them (an array's storage holds
   several); null means it holds no pointers. */
void *cooper_alloc(size_t size, const CooperType *type);

/* Tell the collector where the stack begins: the address of a local of `main`. */
void cooper_gc_init(void *stack_base);

/* Collect garbage now. */
void cooper_gc_collect(void);

#endif
