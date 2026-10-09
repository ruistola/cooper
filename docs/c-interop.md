# C Interop

A Cooper program calls C functions directly, under the C calling convention of its target.

## Declaring a C function

```
extern func abs(v: i32): i32
extern func strlen(s: u8^): u64
extern func moveBy(p: Point^, dx: i32)
```

An `extern func` declares a C function at module level, with no body. It is linked under its
name exactly as written, not mangled. Its signature cannot be generic. Otherwise an extern is
an ordinary module function: it is called, imported by other modules, and used as a function
value like any other. Two modules may declare the same C function, but only with the same
signature.

`extern` is the whole boundary. There is no `unsafe` block: a C function can do anything,
and its declaration is the visible marker of that.

## Types at the boundary

| Cooper | C |
|---|---|
| `i8` … `i64`, `u8` … `u64` | `int8_t` … `int64_t`, `uint8_t` … `uint64_t` |
| `f32`, `f64` | `float`, `double` |
| `bool` | `bool` (`_Bool`) |
| `T^` (`nil` is `NULL`) | `T *` |
| a struct of one such member | the same struct, passed as its member |
| no return type | `void` |

A Cooper struct is laid out in declaration order with C alignment, so a pointer to one is a
pointer to the matching C struct. Strings, arrays, tuples, sum types, function values, and
structs of more than one member cannot cross by value. A C buffer is a `u8^`, and `&xs[0]`
points into an array's storage, so C can read or fill a Cooper buffer:

```
let bytes: u8[] = [104, 105, 0]
n := strlen(&bytes[0])            # 2
```

C may use a Cooper pointer for the duration of a call, but must not keep it afterwards.

## Linking

The C standard library is always linked, so a single file can call it. A project names
further libraries and its own C sources in a `[c]` table in `project.toml`:

```toml
name = "demo"

[c]
libraries = ["m", "sqlite3"]   # linked as -lm -lsqlite3
sources = ["native/shim.c"]    # relative to the project root, compiled with the program
```

## Not supported yet

* Passing a Cooper function to C as a function pointer (callbacks).
* Variadic C functions such as `printf`.
* A Cooper function exported under its own name for C to call, and C global variables.
