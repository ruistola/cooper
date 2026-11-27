# Modules and Projects

## Projects

A project is a directory with a `project.toml` (name and format provisional). A `main.coop`
at the root makes it an executable program; without one it is a library. A distributed
library binary ships auto-generated plaintext interface files.

Struct layout follows declaration order, with no hidden fields and no reordering or packing.
With module-path name mangling, compiled code is C ABI compatible.

## Modules

Each subdirectory is a module named by its path: `auth/` is `auth`, and `server/auth/` is
`server.auth`. Files at the project root form the module `main`. A `module_root` entry in
`project.toml` (e.g. `module_root = "src"`) moves the root that module paths are relative to.
Every `.coop` file in a directory belongs to its module, and sources never declare their
module.

A subdirectory with its own `project.toml` is a sub-project, not a module of the parent.
How a parent uses a sub-project's modules, and whether a sub-project may be a program, is
not yet decided.

## Dependencies

External dependencies are declared once, in `project.toml`, by source location and version.
There is no central registry, and resolution is limited to each project's explicit
dependencies.

```toml
http = "github.com/fast/http@v2.1.0"
```

C libraries and C source files a program links with are named in a `[c]` table (see
[C Interop](./c-interop.md#linking)).

## `use`

A file declares the modules it uses in a `use` block at its top. Entries are comma-separated,
and a trailing comma is optional:

```
use {
  std.io,
  http,
}
```

A `use` names a module, never an item of one. The module is bound under the last segment of
its path, and everything the file takes from it, types and constants included, is written
qualified with that name:

```
use {
  std.io,
  std.sync,
}

func main() {
  let wg: sync.WaitGroup
  io.println("waiting")
  wg.wait()
}
```

The full path is not a spelling: after `use { std.io }` the module is `io`, and `std.io.println`
is an error. A qualified name is written the same way wherever a name may appear: in an
expression (`io.println`, `math.Pi`, `geo.Shape.Circle(1)`, `geo.Point { x: 1, y: 2 }`), a type
(`sync.WaitGroup`, `sync.Channel i32`), and a pattern (`geo.Shape.Circle(r)`,
`geo.Point { x: x }`). Methods are not qualified: they are reached through a value
(`wg.wait()`). A variant whose sum type inference fixes is written bare (`Circle(1)`) whatever
module declares it.

Since a module's names always appear behind its name, they are chosen to read well that way and
not repeat it: `strings.Builder`, not `strings.StringBuilder`.

`as` renames a module, and the alias is its only spelling in that file. It is needed only when two
modules end in the same segment, or when a module's name collides with one of the file's own
module's declarations:

```
use {
  http as web,                                 # web.serve()
  std.strings,
  text.strings as textStrings,
}
```

`Maybe` and `Result` need no `use`; see [Sum Types](./sum-types.md#maybe-and-result).

A module's name shares the file's top-level namespace with the module's declarations. Two
bindings of one name, or a binding colliding with a declaration, is an error reported at the
`use`. A local variable may shadow a module's name.

### Rules

* `use` is file-scoped. A name used in a file is either declared in its module (in any of the
  module's files) or qualified with the name of a module listed in that file's `use` block.
* `use` does not propagate: `use { server }` does not bring in `server.auth`, and a module
  never re-exports what it uses.
* A module dependency cycle is an error.
* Module names and aliases are local spellings only. Every item keeps its full module identity,
  so same-named items from different modules stay distinct.

## Visibility

Everything a module declares is visible to the modules that `use` it, except what is declared in
a file that starts with the directive `#private`, before its `use` block:

```
#private

struct Builder {
    bytes: u8[],
}

func isSpace(c: u8): bool { … }
```

A private file's declarations are visible to every file of its module, tests included, and to
no other module. Its functions, methods, variables, constants, and aliases do not exist outside the module.
Its structs and sum types are **abstract** there: other modules can name them, hold, copy, pass,
compare, and format their values, take their zero values, and call the methods the module's other
files declare on them, but cannot read or write fields, write struct literals, construct variants,
or match them with patterns. A public type can hold a field of an abstract type, whose contents
stay hidden.

### Path resolution

1. A path starting with `std` names the standard library. `std.io` has `print(s: string)`,
   which writes `s` to standard output, and `println(s: string)`, which adds a newline;
   `std.fmt` has `format`, `list`, `quote`, and `fixed`; `std.strings`, `std.arrays`, and `std.runes` have the methods of
   `string`, the arrays, and `rune`, and `std.math` the math functions and constants and the checked conversions of the numeric types. They are Cooper source
   the compiler embeds and compiles with a program that uses them.
2. A path naming a module of this project resolves under the module root
   (`server.auth` is `<root>/server/auth`).
3. Otherwise the first segment names an external dependency in `project.toml`.
