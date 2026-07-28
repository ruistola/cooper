# Working title: Cooper

A project written in the Go programming language for a compiled, statically typed, general purpose programming language.
This is primarily a project for learning programming language design and to study how compilers are implemented, but the
compiler is still being built according to known best practices and industry standards.

## Design documentation

Detailed language design notes live in `docs/`:

* [Compiler Architecture](docs/compiler-architecture.md) — Go package structure and pipeline stages
* [Modules and Projects](docs/modules-and-projects.md) — project layout, module system, dependency management
* [Syntax Fundamentals](docs/syntax-fundamentals.md) — semicolon inference, naming conventions, type expressions
* [Polymorphism and Interfaces](docs/polymorphism-and-interfaces.md) — why Cooper has no interface construct
* [Methods and Functions](docs/methods-and-functions.md) — method declaration, receivers, struct declarations

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
