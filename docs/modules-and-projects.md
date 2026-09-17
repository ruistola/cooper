# Modules and Projects

## Project structure

A **project** is defined by a `project.toml` file (name and format subject to change) in a directory.

* Presence of `main.coop` (file extension TBD) indicates an executable program.
* Absence of `main.coop` indicates a library.
* For distributing library binaries, plaintext interface files are auto-generated (similarly to Swift).

### Binary layout

The goal is a default C binary layout despite some higher-level language features:

* No inheritance or embedding; methods have explicit receivers (sugar for functions).
* No hidden fields in structs (no per-struct vtable).
* No silent layout optimization (no struct member reordering or packing).
* Needs some name mangling (module path), but given that, the result should be C ABI compatible.

## Modules

Each subdirectory under a project defines a **module** with a matching name:

* `auth/` → module `auth`
* `server/auth/` → module `server.auth`
* Exception: an implicit module `main` sits at the project root, regardless of directory name.
* Exception: attribute `module_root` in `project.toml` can be defined, example: `module_root = "src"`

All `.coop` source files in a module directory (including `main` at project root) belong to that module.
No need to declare the module name in source code — it's unambiguously implied from the directory structure.

### Sub-projects

A subdirectory may itself define a project (indicated by the presence of a `project.toml`).
Libraries distributed as source are their own sub-projects with their own dependencies.

Open questions:

* Does a sub-project require additional rules, or can its modules just be used in the parent project?
* What if the sub-project is for an executable (has `main.coop` and files in the `main` module)?
* Should program sub-projects be allowed, or only libraries?

## Dependencies and imports

External dependencies are declared in `project.toml`:

```toml
http = "github.com/fast/http@v2.1.0"
```

Key principles:

* Unlike Go, no raw URLs repeating across sources — only once in the project definition file.
* Unlike Rust, no centralized library repository with name-squatting concerns.
* Unlike npm, explicit per-project dependencies only — no complex resolution.

## The `use` declaration

Source-level dependencies are declared with top-of-file use blocks (entries are comma-separated; a trailing comma is optional):

```
use {
  std.io,
  http
}
```

Terminology matters: _importing_ happens at project level; modules _use_ other modules.

### Binding granularity

A `use` clause binds one of two things, decided by the **kind of its final path segment**:

* **Module binding** — the final segment names a module. The module name enters
  local scope and its members are reached through it, qualified:

  ```
  use { std.io }        // usage: std.io.print("hello")
  ```

* **Name binding** — the final segment names an item *exported by* a module (a type,
  a function). That name enters local scope directly and is used **bare**, exactly as
  if it were declared in this file:

  ```
  use { std.types.Result }   // usage: let x: Result i32
  ```

  Several names from one module are grouped with braces on the trailing position:

  ```
  use { std.types.{ Maybe, Result } }   // both Maybe and Result usable bare
  ```

A bare-bound name occupies a top-level name in this file's namespace, sharing that
namespace with local `struct` and `oneof` declarations. Two things claiming the same
name — two `use` bindings, or a `use` binding and a local declaration — are a
**name collision**, reported at the `use` site and resolved with an alias.

### Aliasing

Any binding — module or name, single or grouped — may be renamed with the **`as`**
keyword. The alias is the only local spelling and applies solely within this file:

```
use {
  http as web,                              // module alias: web.serve()
  std.types.Result as CoreResult,           // name alias
  std.types.{ Maybe, Result as CoreResult },// alias inside a group
}
```

### Rules

* A module must explicitly declare all its individual module dependencies.
  * `use { server }` does **not** include `server.auth` — use doesn't propagate transitively.
* The scope of a `use` declaration is the **source file**, not the module.
  * Better for visibility and IDE/editor UX when tracing a name back to another module.
  * Every name that crosses a **module** boundary appears in this file's `use` block, so
    a bare name is either listed there or declared somewhere in *this* module — it is
    never silently pulled from a module the file does not name. Recovering *which module*
    a name comes from therefore needs only the top of the same file, no cross-module
    jumping and no editor tooling.
  * This does not extend to files: like Go's packages, a module's top-level
    declarations share one namespace across all its files, so a bare name may be defined
    in a sibling file (and a name declared twice across sibling files is a redeclaration
    error). Provenance is recoverable to the *module*, not to the file.
* Bare-bound names and aliases affect only the local surface spelling. Resolution maps
  them back to the full module path for name mangling and the C ABI.

### Resolution order

1. Starts with `std` → Standard library (e.g., `std.io`).
2. Relative module path → local to project (e.g., `server.auth` → `<project root>/<module_root>/server/auth`).
3. Otherwise → search `project.toml` for a matching identifier declaring an external dependency.
