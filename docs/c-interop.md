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
| a struct of these, nested structs included | the struct with the same members in order |
| no return type | `void` |

A Cooper struct is laid out in declaration order with C alignment, so it matches the C struct
declaring the same member types in the same order, by value or through a pointer. The member
names need not match. A struct may hold a static array (`rgb: u8[3]`), which is the C array
member. Strings, dynamic arrays, tuples, sum types, and function values cannot cross, nor can a
struct containing one, and neither can a static array on its own, since C passes an array as a
pointer: pass `&a[0]`.

```
struct DivT { quot: i32, rem: i32 }
extern func div(a: i32, b: i32): DivT     # the C library's div_t div(int, int)
``` A C buffer is a `u8^`, and `&xs[0]`
points into an array's storage, so C can read or fill a Cooper buffer:

```
let bytes: u8[] = [104, 105, 0]
n := strlen(&bytes[0])            # 2
```

C may use a Cooper pointer for the duration of a call, but must not keep it afterwards.

## Strings and buffers

A Cooper `string` is its bytes and their count, with no terminating NUL, so it does not
cross to C as is. The compiler-provided module `std.ffi` declares the C string type and
copies between the two representations:

```
use { std.ffi.CString }

extern func getenv(name: CString): CString

home := string(getenv(CString("HOME")))
```

* `CString` is a NUL-terminated C string, `struct CString { data: u8^ }`. It crosses to C as
  a `char *`; its zero value is `NULL`.
* `CString(s)` converts a string by copying its bytes and a terminating NUL to fresh
  memory. A NUL inside the string ends the C string there. It is a conversion like any
  other, spelled with the type's name however it is in scope (`ffi.CString(s)` after
  `use { std.ffi as ffi }`).
* `string(c)` converts back, copying the bytes before the NUL. A `NULL` C string yields
  `""`.
* `copyBytes(p: u8^, n: i64): u8[]` copies `n` bytes into a new array; a negative `n` stops
  the program. Reading through `nil` yields zeros, as any load through nil does.

Each copy is independent of its source: C may free or reuse its memory afterwards, and a
`CString` stays valid however the Cooper string it came from is used.

## Linking

The C standard library is always linked, so a single file can call it. A project names
further libraries and its own C sources in a `[c]` table in `project.toml`:

```toml
name = "demo"

[c]
libraries = ["m", "sqlite3"]   # linked as -lm -lsqlite3
sources = ["native/shim.c"]    # relative to the project root, compiled with the program
```

## Variadic functions

A final `...` declares a C function that takes further arguments, as `printf` does. A call
passes any number of extra arguments, each as its own type under C's default promotions:
integers narrower than `int` and `bool` widen to `int`, and `f32` to `double`. Only integers,
floats, `bool`, pointers, and `CString` can be passed this way, and a variadic function cannot
take or return a struct by value. Number the arguments carefully: an integer literal is an
`i32`, and `%ld` wants an `i64`.

```
extern func snprintf(buf: u8^, n: u64, fmt: CString, ...): i32
n := snprintf(&buf[0], 64, CString("%d-%s"), 42, CString("hi"))
```

## Callbacks

A parameter of a function type is a C function pointer. The argument must be the name of a
function (a module function, or a C function), whose parameters are integers, floats, `bool`,
or pointers and whose result is one of these or nothing. A function literal, a closure, a
nested function, a method, or a variable holding a function value cannot be passed, since a C
function pointer has nowhere to keep an environment: C APIs carry state in a `void *` argument,
which here is a pointer the callback casts back.

```
extern func qsort(base: i32^, n: u64, size: u64, cmp: func(i32^, i32^): i32)

func descending(a: i32^, b: i32^): i32 { … }
qsort(&nums[0], 5, 4, descending)
```

A callback runs on the caller's thread during the C call, or later if C keeps the pointer
(`atexit`). Deferred code and panics behave as anywhere else.

## Not supported yet
* A C function returning a function pointer.
* A Cooper function exported under its own name for C to call, and C global variables.
