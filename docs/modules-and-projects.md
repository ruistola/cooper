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

The final path segment decides what is bound:

* **A module binding** brings the module into scope, and its members are reached qualified:
  `use { std.io }` then `std.io.print("hello")`.
* **A name binding** brings one exported item (a type or function) into scope for bare use:
  `use { std.types.Result }` then `let x: Result i32 string`. Several names from one module
  are grouped: `use { std.types.{ Maybe, Result } }`.

`as` renames any binding, and the alias is its only spelling in that file:

```
use {
  http as web,                                 # web.serve()
  std.types.{ Maybe, Result as CoreResult },
}
```

A bare-bound name shares the file's top-level namespace with the module's declarations. Two
bindings of one name, or a binding colliding with a declaration, is an error reported at the
`use`.

### Rules

* `use` is file-scoped. A name used in a file is either declared in its module (in any of the
  module's files) or listed in that file's `use` block.
* `use` does not propagate: `use { server }` does not bring in `server.auth`, and a module
  never re-exports what it uses.
* A module dependency cycle is an error.
* Aliases and bare names are local spellings only. Every item keeps its full module identity,
  so same-named items from different modules stay distinct.

### Path resolution

1. A path starting with `std` names the standard library. The compiler provides `std.io`:
   `print(s: string)` writes `s` to standard output, and `println(s: string)` writes it
   followed by a newline.
2. A path naming a module of this project resolves under the module root
   (`server.auth` is `<root>/server/auth`).
3. Otherwise the first segment names an external dependency in `project.toml`.
