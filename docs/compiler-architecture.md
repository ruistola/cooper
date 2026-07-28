# Compiler Architecture

Cooper is written in Go. This document describes the compiler's internal package structure.

## Package overview

* **main.go** — Thin entry point. The primary focus is currently on validation by tests, so
  main doesn't get exercised much yet. May benefit from a CLI/TUI library once compiler
  arguments and flags start to matter.

* **/lexer** — Tokenization.
  * Relies heavily on regular expressions, matching in order from longest to shortest.
  * Outputs a slice of tokens consumed by the parser.

* **/ast** — Abstract syntax tree types.
  * Explicit naming for clarity: `IfExpr` vs `IfStmt`, `TypeExpr` instead of `Type`, etc.
  * Pure data — no functionality, just type definitions.
  * Go interfaces with empty implementations enable the AST as a heterogeneous collection.

* **/parser** — Parsing.
  * Primarily one-token lookahead (semicolon inference details aside, effectively LL(1)).
  * Explicit parsing functions for each statement kind, mostly identified by keyword.
  * Pratt parsing for operator-focused expressions.

* **/typechecker** — Post-parsing analysis, split into multiple passes for source-order
  insensitivity:
  1. **Symbol resolution** — constructs a mapping from AST nodes to scopes (type environments).
  2. **Type checking** — performed using the symbol table maps from the resolver.
  3. **Semantic analysis** — additional correctness checks after types are resolved.

* **/codegen** — Proof-of-concept platform-specific binary generation.
  * macOS only for now.
  * No IR; goes directly from (type-checked) AST to assembly and final binary.
  * May be discarded and rewritten once a reasonable grasp of code generation challenges develops.
  * Primary purpose: highlight tradeoffs between ideal syntax/semantics and real machine constraints.
