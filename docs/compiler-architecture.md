# Compiler Architecture

Cooper is written in Rust and organized as a Cargo workspace. This document describes the workspace
layout and the compiler's internal module structure.

## Workspace layout

The compiler is split across crates so that the filesystem- and host-bound pieces stay out of the
analysis core, which keeps the frontend portable (it compiles to WebAssembly cleanly) and isolates
heavy or native backend dependencies behind a crate boundary.

* **crates/cooper-frontend** (library `cooper_frontend`) — The analysis core: lexing, parsing, and
  all semantic passes over an in-memory `Project`. It touches neither the filesystem nor the host
  environment, so it runs anywhere and is driven entirely from in-memory sources.

* **crates/cooper-cli** (binary `cooper`, library `cooper_cli`) — The host-facing driver. It owns the
  filesystem loader that turns a source tree into a `Project` and the command-line entry point, and
  depends on `cooper-frontend` to analyze what it loads. All `std::fs`/`std::path`/`toml` use lives
  here.

## Module overview

* **cooper-cli/src/main.rs** — Thin CLI entry point. It reads a source file, runs the frontend, and
  either reports "no errors" or renders every collected diagnostic with [`ariadne`]. The primary
  focus is currently on validation by tests, so the driver stays minimal.

* **cooper-frontend/src/lib.rs** — The frontend crate root and pipeline orchestrator.
  [`analyze_project`] runs the whole frontend over an in-memory [`Project`] and returns every
  diagnostic; each module is analyzed independently, and within a module each semantic phase runs
  only when the previous produced no errors, so diagnostics stay meaningful rather than cascading.
  [`analyze`] is a thin convenience that wraps a single source snippet as a one-module program.

* **cooper-frontend/src/project.rs** — The in-memory project model, the compiler's input. A `Project`
  (manifest plus modules) is compiled directly rather than reading a filesystem, so the frontend can
  run anywhere and tests can build programs from in-memory sources. A `Module` is the atomic
  compilation unit, spanning one or more `SourceFile`s whose top-level declarations unite into one
  namespace.

* **cooper-frontend/src/modules.rs** — Module-graph analysis: the driver behind `analyze_project`. It parses every
  module's files, resolves each `use` binding against the project's modules, orders the module
  dependency graph topologically (rejecting cycles), and builds each module's interface — its
  exported signatures — in that order. Every module is checked against its own declarations plus its
  dependencies' interfaces, never their bodies. `use` is file-scoped: each file is resolved and
  checked against the module's united declarations plus that file's own imports, so an import is
  invisible in sibling files and is stripped from the interface the module exports — `use` neither
  leaks across files nor re-exports transitively.

* **cooper-cli/src/loader.rs** — Filesystem loading: constructs a `Project` from a source tree rooted at a
  `project.toml`. Each subdirectory of `.coop` files becomes a module named by its path relative to
  the module root (`server/auth/` → `server.auth`); root files form the implicit `main` module, and
  a `main.coop` there makes the project a program. A nested directory with its own `project.toml` is
  a sub-project and is skipped rather than absorbed. This is the only filesystem-aware component; the
  rest of the compiler consumes the abstract `Project`.

* **cooper-frontend/src/lexer.rs** — Tokenization, built on the [`logos`] derive lexer. Outputs a `Vec<Token>`
  consumed by the parser, or a single diagnostic on an unexpected character.

* **cooper-frontend/src/ast.rs** — Abstract syntax tree types. Every node is a `{ kind, span }` pair: a closed
  `*Kind` enum for the shape plus the source [`Span`] it was parsed from, so later passes can attach
  precise diagnostics and every `match` is exhaustiveness-checked. Operators are their own
  `BinaryOp`/`UnaryOp`/`AssignOp` enums, decoupling later phases from token kinds.

* **cooper-frontend/src/parser.rs** — Hand-written recursive-descent parsing with Pratt expression parsing. It
  collects diagnostics and recovers at statement boundaries instead of bailing on the first error,
  so one run reports as many problems as it can.

* **cooper-frontend/src/types.rs** — The resolved `Type` model. A single closed `enum` replaces an interface
  hierarchy, so every `match` over a type is checked for exhaustiveness at compile time. Carries
  structural equality (with Cooper's pointer/`nil` compatibility rules), substitution, and
  unification for generics.

* **cooper-frontend/src/diag.rs** — Source spans and diagnostics. Every diagnostic carries the [`Span`] of the
  offending range so the whole pipeline can surface many precisely located errors from one run, plus
  the source file that span indexes into (attributed as diagnostics leave the per-file passes) so a
  multi-file project renders each error against the right source.

* Post-parsing analysis is split into three passes for source-order insensitivity:
  1. **cooper-frontend/src/resolve.rs** — Declaration resolution. Collects every top-level struct, sum type,
     function, and method into a global symbol table (`Globals`) and validates each declaration's
     signature in isolation (duplicate names, duplicate members/variants, undefined types, generic
     arity, receiver/method well-formedness). Bodies are not walked here.
  2. **cooper-frontend/src/typecheck.rs** — Type checking. Walks function and module bodies against `Globals`,
     assigning a type to every expression, with its own stack of block-scoped variable bindings.
     Undefined-variable detection falls out of identifier lookup here.
  3. **cooper-frontend/src/semantic.rs** — Semantic analysis. Control-flow validation only: every function with a
     non-unit return type must return on all paths, and code made unreachable by a preceding return
     is reported.

## Toward code generation: the typed lowering IR

The seam between the frontend and any backend is a typed intermediate representation, built by the
`cooper-ir` crate. Lowering consumes the types the frontend already computes: type checking records a
per-file `span → Type` table as it checks rather than discarding each expression's type, and lowering
reads it to produce a typed, span-carrying IR where every node knows its resolved type and no name
resolution or inference happens downstream. The IR is also the home for desugaring (`if` to `match`,
index syntax and array methods to intrinsics) and for monomorphization, Cooper's generics model,
which instantiates each generic function and method per concrete type-argument set.

**Runtime stays behind the IR→backend boundary.** The IR models every runtime-touching operation —
allocation, array growth, copies — as an abstract, typed *intrinsic*, never a concrete runtime call
or committed ABI. A backend lowers those intrinsics into real allocations, object headers, write
barriers, and safepoints according to the memory and concurrency model chosen at that stage. This
keeps the IR neutral about garbage collection, green-thread scheduling, how the runtime is delivered,
and whether a runtime library crate ever exists: those are backend-epoch decisions that require no IR
rework, because the IR commits to none of them.

## Concrete syntax: AST, not a full CST

Cooper's parser produces an abstract syntax tree, not a lossless concrete syntax tree (red-green
tree). Every AST node already carries the full source [`Span`] it was parsed from, and the original
source text is retained alongside, so trivia a formatter or language server needs — comments,
whitespace, exact token extents — is recovered by re-lexing the span on demand rather than being
threaded through every node. This keeps the tree small and every pass's `match` focused on meaning
rather than layout. A full CST earns its place only once incremental reparse performance is a
measured requirement; retrofitting one is a bounded change, because span-complete nodes over retained
source already pin down where every construct lives.

[`analyze`]: ../crates/cooper-frontend/src/lib.rs
[`analyze_project`]: ../crates/cooper-frontend/src/lib.rs
[`Project`]: ../crates/cooper-frontend/src/project.rs
[`ariadne`]: https://crates.io/crates/ariadne
[`logos`]: https://crates.io/crates/logos
[`Span`]: ../crates/cooper-frontend/src/diag.rs
