# Working title: Cooper

A compiler, written in the Rust programming language, for a compiled, statically typed, general purpose programming language.
This is primarily a project for learning programming language design and to study how compilers are implemented, but the
compiler is still being built according to known best practices and industry standards.

## Design documentation

Detailed language design notes live in `docs/`:

* [Compiler Architecture](docs/compiler-architecture.md) — Rust module structure and pipeline stages
* [Modules and Projects](docs/modules-and-projects.md) — project layout, module system, dependency management
* [Syntax Fundamentals](docs/syntax-fundamentals.md) — semicolon inference, naming conventions, type expressions
* [Polymorphism and Interfaces](docs/polymorphism-and-interfaces.md) — why Cooper has no interface construct
* [Methods and Functions](docs/methods-and-functions.md) — method declaration, receivers, struct declarations
* [Memory and the Object Model](docs/memory-and-object-model.md) — value layout, storage, pointers, inert nil
* [Generics](docs/generics.md) — juxtaposition type application, type-parameter binders, generic methods
* [Sum Types](docs/sum-types.md) — `oneof` declarations, variant payloads, type-qualified construction, inference
* [Type System](docs/type-system.md) — nominal/structural, no subtyping/variance, constraints as sugar over passing

## Coding guidelines

Do **not** proactively author tests with each coding task; the user will explicitly ask for tests as a separate task,
when new code reaches sufficient maturity to become a permanent addition to the project.

Prefer concise code, modifying existing modules by appending them with new functionality when feasible, as long as the
module remains cohesive. Only establish new modules, functions and structures, when it is worth the added complexity
or "glue code" required in order to make new components communicate with the rest of the system.

Do not leave breadcrumbs in comments or docs; always describe the implementation in absolute terms instead of as a
delta. By default, when existing behavior changes, do not leave the old implementation in the codebase for "backwards
compatibility" unless explicitly requested.

Don't expose module items (`pub`) by default. Only expose the minimum public API. Prefer white-box (same-module) unit
tests for verification of a module's core functionality. Black-box testing across the crate boundary (in `tests/`) should
rely on the public API only. Adding public functions intended for test-only mocking and cleanup is potentially dangerous
and obfuscates the API proper, so only add such extensions when absolutely necessary.
