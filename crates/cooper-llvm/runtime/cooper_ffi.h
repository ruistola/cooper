/* What the Cooper runtime offers the C code a project compiles with its program (its `[c]` sources). */

#ifndef COOPER_FFI_H
#define COOPER_FFI_H

/* Store the Cooper pointer `value` (or NULL) at `slot` in Cooper memory: a heap object, a module-level variable,
   or a Cooper frame. C code stores every Cooper pointer it writes into Cooper memory this way, so that the
   collector sees the store. */
void cooper_store_pointer(void **slot, void *value);

#endif
