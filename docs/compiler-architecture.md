# Compiler Architecture

Cooper is written in Rust. This document describes the compiler's internal module structure.

## Module overview

* **src/main.rs** — Thin CLI entry point. It reads a source file, runs the frontend, and either
  reports "no errors" or renders every collected diagnostic with [`ariadne`]. The primary focus is
  currently on validation by tests, so the driver stays minimal.

* **src/lib.rs** — The library crate root and pipeline orchestrator. [`analyze`] runs the whole
  frontend and returns every diagnostic; each semantic phase runs only when the previous produced no
  errors, so diagnostics stay meaningful rather than cascading.

* **src/lexer.rs** — Tokenization, built on the [`logos`] derive lexer. Outputs a `Vec<Token>`
  consumed by the parser, or a single diagnostic on an unexpected character.

* **src/ast.rs** — Abstract syntax tree types. Every node is a `{ kind, span }` pair: a closed
  `*Kind` enum for the shape plus the source [`Span`] it was parsed from, so later passes can attach
  precise diagnostics and every `match` is exhaustiveness-checked. Operators are their own
  `BinaryOp`/`UnaryOp`/`AssignOp` enums, decoupling later phases from token kinds.

* **src/parser.rs** — Hand-written recursive-descent parsing with Pratt expression parsing. It
  collects diagnostics and recovers at statement boundaries instead of bailing on the first error,
  so one run reports as many problems as it can.

* **src/types.rs** — The resolved `Type` model. A single closed `enum` replaces an interface
  hierarchy, so every `match` over a type is checked for exhaustiveness at compile time. Carries
  structural equality (with Cooper's pointer/`nil` compatibility rules), substitution, and
  unification for generics.

* **src/diag.rs** — Source spans and diagnostics. Every diagnostic carries the [`Span`] of the
  offending range so the whole pipeline can surface many precisely located errors from one run.

* Post-parsing analysis is split into three passes for source-order insensitivity:
  1. **src/resolve.rs** — Declaration resolution. Collects every top-level struct, sum type,
     function, and method into a global symbol table (`Globals`) and validates each declaration's
     signature in isolation (duplicate names, duplicate members/variants, undefined types, generic
     arity, receiver/method well-formedness). Bodies are not walked here.
  2. **src/typecheck.rs** — Type checking. Walks function and module bodies against `Globals`,
     assigning a type to every expression, with its own stack of block-scoped variable bindings.
     Undefined-variable detection falls out of identifier lookup here.
  3. **src/semantic.rs** — Semantic analysis. Control-flow validation only: every function with a
     non-unit return type must return on all paths, and code made unreachable by a preceding return
     is reported.

Code generation is not yet part of the frontend; the current focus is a correct, well-diagnosed
front end from source text through semantic analysis.

[`analyze`]: ../src/lib.rs
[`ariadne`]: https://crates.io/crates/ariadne
[`logos`]: https://crates.io/crates/logos
[`Span`]: ../src/diag.rs
