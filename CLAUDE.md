# Working title: Cooper

A project written in the Go programming language for a compiled, statically typed, general purpose programming language.
This is primarily a project for learning programming language design and to study how compilers are implemented, but the
compiler is still being built according to known best practices and industry standards.

## Compiler project structure

* main.go - a thin entry point for the compiler for now
  * The primary focus right now is in validation by tests, so the main doesn't really get executed much currently
  * TODO: Could benefit from some CLI/TUI library once compiler arguments and flags start to matter

* /lexer - the lexer package for performing tokenization
  * Relies heavily on regular expressions, matching in order from longest to shortest
  * Outputs a slice of tokens that can be then used as input for the parser

* /ast - the abstract syntax tree package
  * Very explicit for clarity; see e.g. `IfExpr` vs `IfStmt` , `TypeExpr` instead of `Type` etc
  * Just a package of types with no functionality
  * Interfaces with empty implementations are only to enable the AST as a heterogeneous collection

* /parser - the parser package
  * Primarily works on one-token lookahead -- semicolon inference details aside, effectively a LL(1) grammar
  * Explicit parsing functions for each specific kind of statement, mostly identified by keyword
  * Pratt parsing for most operator-focused expressions
  * Syntax is mostly imperative, with some quality-of-life- features planned, inspired by functional languages

* /typechecker - the post-parsing analysis package
  * The analysis is split into multiple passes to make the source code order insensitive
  * In the first pass, the symbol resolver constructs a mapping between AST nodes to scopes (type environments)
  * In the second pass, type checking is performed, using the symbol table(ish) maps from the resolver
  * In the third pass, semantic analysis is performed

* /codegen - a proof of concept implementation for generating platform specific binaries. To be refined later
  * macOS only for now
  * No IR, the step goes directly from (type checked) AST into assembly and the final binary
  * This package may get discarded later once a reasonable grasp on code generation challenges has developed
  * The purpose of this package is mainly to highlight the tradeoffs between ideal syntax & semantics and reality

## Significant language features (implemented or planned)

### The envisioned project and directory structure

* A **project** is defined by a `project.toml` (name and type subject to change) in a directory
  * The presence of a `main.coop` (file extension still subject to change) indicates an executable program
  * The lack of a `main.coop` indicates a library
  * For distributing library binaries, plaintext interface file(s) are auto-generated (similarly to Swift)
  * Basically default C binary layout despite the language having some more complex features:
    * No inheritance or embedding, a method has an explicit receiver (effectively sugar for functions)
    * No hidden fields in structs such as a per-struct vtable
    * Interfaces are just fat pointers similar to Go: pointing to a concrete type + a separate itable
    * No silent layout optimization like struct member reordering or packing
    * Needs some name mangling (module path), but given that, the result should be C ABI compatible

* Each subdirectory under a project defines a **module** of matching name, e.g. `auth` in `auth/`, `server.auth` in
`server/auth/`
  * With the exception being an implicit module `main` which sits at the project root, regardless of its directory name
  * All `.coop` source files in a module directory (including `main` at project root) belong to that module
  * No need to declare the module name in the source code, as it's unambiguously implied from the directory structure
  * A subdirectory may define a project, recursively -- indicated by the presence of a `project.toml` file
  * Libraries distributed as source are thus their own (sub-)projects with their own dependencies
  * TODO: Does a sub-project require additional rules, or can its modules just be used in the parent project?
    * Note: What if the sub-project is for an executable: has a `main.coop` and files in the `main` module?
    * Note: Should program sub-projects even be allowed? Or only libraries?

* External dependencies are declared in `project.toml`
  * Syntax is something like `http = "github.com/fast/http@v2.1.0"` (notice optional syntax for a specific version)
    * Note: unlike in Go, no raw URLs repeating across sources; only once in the project definition file
    * Note: this also avoids Rust-like name camping in some centralized library repository
    * Note: this also avoids the npm import resolution mess -- explicit per-project dependencies only
  * Source level dependencies are declared with top-of-file use blocks like `use { std.io, http, }`
    * Note: terminology matters -- _importing_ happens at project level; modules _use_ other modules
  * A module must explicitly declare all its individual module dependencies
    * `use { server }` doesn't include `server.auth` even if `server` itself declares `use { server.auth }`
    * in other words, `use` doesn't propagate from the leaf modules towards the root module
  * The scope of a use declaration is the source file, not the module (import applies to the whole project)
    * This is somewhat counterintuitive as a module is essentially the atomic unit of compilation
    * It is better for visibility though, and for IDE/editor UX when following the trail to definition
    * No need to jump across files to determine the origin of some types or functions used
  * A use declaration may define a local alias like `use { io: std.io, http, auth: server.auth }`
    * Similarly to the use declarations proper, this only applies within the scope of a source file
  * Resolution order:
    * Starts with `std` like `std.io`: Standard library
    * Relative module path like `server.auth`: local to project, search `<project root>/server/auth`
    * Otherwise, search `project.toml` for a matching identifier declaring an external dependency

### Some notes on individual considerations and design specifics

* Semicolon acts as a statement separator and/or terminator
    * However, an endline can be automatically converted into a semicolon
    * One reason to type an explicit semicolon is when a function or a block expression is intended to return nothing
        * the return type of a block expression or function body ending with `foo` is the type of `foo` (e.g. i32)
        * the return type of a similar block ending with `foo;` is `()` (Unit type)
* Semicolon inference is based on the Ahnfelt variant of the Scala implementation:
    * If two consecutive tokens `a` and `b` are separated by an `EOL` token (endline), and
    * if `a` is in the `beforeSemicolon` category (if `a` immediately followed by a semicolon is syntactically valid),
    and
    * if `b` is in the `afterSemicolon` category (if a semicolon immediately followed by `b` is syntactically valid),
    then
    * the `EOL` token is converted into a semicolon
* Redundant consecutive endlines are eliminated in the tokenization phase, as are all other kinds of whitespace
* Some other details also apply to semicolon inference, such as endline conversions being disabled within parentheses

* Program sources are UTF-8
  * TODO: Current lexer only tokenizes a narrow ASCII subset
* Strings are by default encoded in UTF-8
  * TODO: `rune` type for representing individual code point -- or grapheme, TBD

## Coding guidelines

**Pause and Report Pattern**: If you find yourself stuck and running in circles due to unexpected or unexplainable
results from tools, or due to some missing configuration in the runtime environment, stop! Report your status to the
user instead of trying to power through. Also, proactively pause after completing logical milestones or subtasks
(typically every few minutes of work). Report what you've accomplished, what you plan to do next, and report any
blocking issues or decisions needed. This checkpoint pattern preserves progress and allows the user to steer the
process.

Follow general Go coding guidelines. Follow established patterns, naming conventions, and the general style of existing
code in the project. Do notify the user however, if there is a significant discrepancy in the project style vs idiomatic
Go.

Do **not** proactively author tests with each coding task; the user will explicitly ask for tests as a separate task,
when new code reaches sufficient maturity to become a permanent addition to the project.

Prefer concise code, modifying existing packages by appending them with new functionality when feasible, as long as the
package remains cohesive. Only establish new packages, functions and structures, when it is worth the added complexity
or "glue code" required in order to make new components communicate with the rest of the system.

Don't export package functions, types or variables by default. Only expose the minimum public API. Prefer white-box
(same-package) tests for verification of package core functionality. Black-box testing across packages should rely on
the public API only. Adding exported (public) functions intended for test-only mocking and cleanup is potentially
dangerous and obfuscates the API proper, so only add such extensions when absolutely necessary.
