# Compiler Architecture

The compiler is a Rust Cargo workspace.

## Crates

* **cooper-frontend** — lexing, parsing, and every semantic pass over an in-memory `Project`.
  It touches neither the filesystem nor the host, so it runs anywhere (including WebAssembly)
  and tests drive it from in-memory sources.
* **cooper-ir** — the typed lowering IR and monomorphization: the seam between the frontend
  and a backend.
* **cooper-llvm** — the backend: emits a monomorphized program as textual LLVM IR, and holds
  the C runtime (`runtime/cooper_rt.c`) every program links against.
* **cooper-cli** (binary `cooper`) — the host driver: the filesystem loader, the build
  pipeline, and the `check`, `build`, and `run` commands. All filesystem, process, and `toml`
  use lives here.

## Frontend

* **lib.rs** — Pipeline entry points. [`analyze_project`] runs the frontend over a `Project` and
  returns every diagnostic. `check_project` does the same and, for a clean project, returns its
  checked form (each file's declarations, symbol table, and checker tables, plus all type
  definitions) for lowering. [`analyze`] wraps one source snippet as a program.
* **project.rs** — The input model: a `Project` is a manifest plus `Module`s, and a module is
  one or more `SourceFile`s whose top-level declarations share one namespace.
* **modules.rs** — Module-graph analysis. It parses every file, resolves `use` bindings,
  orders modules topologically (rejecting cycles), and analyzes each module against its
  dependencies' exported interfaces, never their bodies. Imports are file-scoped and
  stripped from the interface a module exports.
* **lexer.rs** — Tokenization with [`logos`].
* **parser.rs** — Hand-written recursive descent with Pratt expression parsing. It recovers at
  statement boundaries to report many errors per run.
* **ast.rs** — Every node is a `{ kind, span }` pair with a closed `*Kind` enum, so every
  `match` is exhaustiveness-checked.
* **types.rs** — The resolved `Type` enum, with equality, substitution, unification, and the
  `TypeDefs` table.
* **builtins.rs** — The fixed method sets of built-in types (array `length`, `get`, `set`,
  `push`).
* **diag.rs** — Diagnostics, each with the [`Span`] and file it refers to.
* The semantic passes run per module, each only if the previous one reported no errors:
  1. **resolve.rs** — Builds the symbol table (`Globals`) and checks declaration signatures.
     It runs in phases across all of a module's files: register every type name, resolve
     member and variant types, reject types that contain themselves by value, then resolve
     functions, methods, and variables. Declaration order therefore never matters.
  2. **typecheck.rs** — Types every expression and infers the type arguments of generic
     constructions and references. It records two per-file tables: `span → Type` for every
     expression, and `span → ItemRef` naming the declaration and type arguments of every
     function or method reference.
  3. **semantic.rs** — Control-flow checks: every path returns a value where the function
     needs one, and no code follows a `return`.

**Types are nominal references.** A struct or sum `Type` holds only its identity (`TypeId`:
defining module and name) and type arguments. Members and variants live once in `TypeDefs`
and are read substituted by those arguments. This allows self-reference through pointers,
keeps same-named types from different modules distinct, and makes equality cheap. Callables
have identities too (`Callable`: module and name, or receiver type and name).

## Lowering IR

`cooper-ir` lowers a checked program from the frontend's tables, so nothing is re-resolved or
re-inferred. Every node carries its resolved type, and every callee names its declaration.
Lowering also desugars: `if` to `match`, `match` to a decision tree, and index syntax and array
methods to intrinsics.

**Monomorphization** (`mono.rs`) is an IR-to-IR pass. Lowering keeps a generic function as a
template. Starting from the non-generic functions, the pass instantiates each referenced
declaration once per distinct set of type arguments and follows the references each instance
makes. Unreached templates are dropped, and polymorphic recursion is an error once type
arguments exceed a size bound. `lower_program` runs lowering and monomorphization over a
whole project, producing a `Program` of concrete functions plus the `TypeDefs` a backend needs
for layout.

**The runtime stays behind the IR boundary.** Runtime-touching operations (allocation, array
growth, copies) are abstract typed *intrinsics*, not runtime calls. A backend realises them
under whatever memory and concurrency model it adopts, so those choices need no IR change.

## Backend and build

`cooper build <file | project-dir>` checks, lowers, and monomorphizes the program, emits one
LLVM IR module, and compiles it with the runtime using the system `clang` (overridable with
`COOPER_CC`). The IR, runtime source, and executable are written to a `build/` directory beside
the file or in the project. `cooper run` builds and runs, exiting with the program's status.

The runtime owns the process entry point: its C `main` calls `cooper_main`, which the backend
exports as a wrapper around the program's `main` (returning `i32`, or unit for status 0).
Structs are named LLVM struct types laid out in declaration order, and tuples are literal
struct types; both are first-class values. Locals live in stack slots, except a local whose
address is taken (with `&`, or as the receiver of a pointer-receiver method), which is
allocated on the heap where it is bound. This stands in for Go-style escape analysis, which
will keep such locals on the stack whenever their address does not outlive the frame. Heap
memory is not yet reclaimed. Inert nil costs no
branch: an access through a pointer that is null is redirected to a zero-filled buffer for
loads and to a discard buffer for stores.

A function value is a `{ code, env }` pointer pair, and a call through one passes `env` as a
hidden first argument. A function used as a value gets a private thunk with that convention
and a null `env`; a bound method's `env` is its receiver pointer, or a heap copy of a value
receiver taken when it is bound. Calling a function value that was never assigned is a
runtime error.

A string is a `{ data, length }` pair of immutable bytes, and an array a `{ data, length,
capacity }` triple whose copies share elements. Concatenation, string equality, and array
growth are runtime functions; growth doubles the capacity (at least 4).

A runtime error (integer overflow, division by zero, an index out of bounds) calls the runtime's `cooper_panic`, which
prints `panic: <error> at <file>:<line>:<column>` to standard error and exits with status 101.

Function symbols are mangled from the declaration's identity and type arguments in a
length-prefixed scheme (see `cooper-llvm/src/mangle.rs`), so instances and same-named
functions from different modules never collide. Constructs the backend does not handle yet
are reported, never miscompiled.

Golden programs in `cooper-cli/tests/programs/` are built and run by `cargo test`, each
checked against the exit status declared on its first line (`# exit: N`) and any text its
`# stderr:` lines require.

## AST, not CST

The parser builds an AST. Every node keeps its full source span and the source text is
retained, so a formatter or language server recovers comments and exact token extents by
re-lexing a span.

[`analyze`]: ../crates/cooper-frontend/src/lib.rs
[`analyze_project`]: ../crates/cooper-frontend/src/lib.rs
[`logos`]: https://crates.io/crates/logos
[`Span`]: ../crates/cooper-frontend/src/diag.rs
