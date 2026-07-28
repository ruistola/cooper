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

Source-level dependencies are declared with top-of-file use blocks:

```
use { std.io, http }
```

Terminology matters: _importing_ happens at project level; modules _use_ other modules.

### Rules

* A module must explicitly declare all its individual module dependencies.
  * `use { server }` does **not** include `server.auth` — use doesn't propagate transitively.
* The scope of a `use` declaration is the **source file**, not the module.
  * Better for visibility and IDE/editor UX when following the trail to a definition.
  * No jumping across files to determine origin of types or functions.
* A use declaration may define a local alias: `use { io: std.io, http, auth: server.auth }`
  * Aliases only apply within the scope of the source file.

### Resolution order

1. Starts with `std` → Standard library (e.g., `std.io`).
2. Relative module path → local to project (e.g., `server.auth` → `<project root>/server/auth`).
3. Otherwise → search `project.toml` for a matching identifier declaring an external dependency.
