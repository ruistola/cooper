# The Cooper programming language

A project written in the Go programming language for a compiled, statically typed, general purpose programming language.
This is primarily a project for learning programming language design and to study how compilers are implemented, but
the compiler is still built according to known best practices and industry standards.

No decision has been made on the compiler backend yet. Potentially could use an off-the-shelf backend like LLVM or QBE,
but for the purpose of learning, the project may end up at least experimenting with a proprietary backend.

The development environment is macOS, git for version control, Neovim as the IDE. Code is mostly typed in by hand, but
LLMs are heavily utilized in validating ideas and designs, applying code refactorings, and test code generation.

## About the user

The user (the lone author of Cooper) is a software professional with a degree in Computer Science but no formal
studies or working experience specifically on programming language design and compiler architecture.

## Language design philosophy

The project is partly motivated by frustrations in existing languages the user has been in contact with:

### Rust

Initial impression good, and the emphasis on _correctness_ in the sense how the compiler helps the user (programmer)
spot and fix errors is nice. However, the emphasis on _safety_ above all makes Rust's tradeoffs feel excessive for most
use cases other than the most extreme: O/S kernel development, medical device software, aerospace etc.

When applied in moderation, some functional-style programming patterns that get optimized by the compiler into simple
constructs, such as regular for-loops, are something the user would like to replicate in Cooper; but not if the only way
to achieve such would be to adopt Rust's degree of rigidity, making some common and valid constructs unrepresentable.

Rust enums (tagged unions) and pattern matching are features the user is considering for Cooper as well.

### Go

The user's current favorite language, considering all the pros and cons. Garbage collection unfortunately seems to move
Go out of the "well-suited for systems programming" category -- whether this is an objective fact or just the mainstream
public opinion, is of course debatable. But it acts as a data point for the user that choosing garbage collection as the
main memory management strategy would undeniably alienate a certain programmer demographic.

Of course Go is very opinionated, and the user doesn't exactly share the Go team's opinions in all matters (such as
hijacking capitalization for access control, for example), and this is probably the only reason for the Cooper project
to exist in the first place. On the conceptual level, Cooper's goals are philosophically very much aligned with Go's.

### C#

Like Go, but with "Microsoft-isms" applied. That is, like an enterprise version of Go which insists on having unique,
Microsoft- specific terminology for common concepts, and suffers from "kitchen sink" feature bloat such as classes vs
structs vs tuples vs valuetuples vs records vs record classes etc. These are failings to be avoided with Cooper.

### C++

Coding enterprise application C++ almost made the user quit programming altogether. Enough said.

### Typescript

The type system is incredibly expressive, but that also introduces the risk of being misapplied by over-eager users who
engage in type system acrobatics. Admiring complexity for its own sake is something that the user is specifically trying
to avoid with Cooper. That said, features such as union types are very convenient, and resolve issues like Go has with
its multiple return values that completely rely on the user doing the right thing.

Occasionally, unions can also be more convenient than Rust enums that must be explicitly declared before use. It is
still being considered which approach will be adopted by Cooper, but one or the other certainly will, as there is no
inheritance model. Union types are therefore needed, if only to enable heterogeneous collections.

## Significant language features (implemented or planned)

### The envisioned structure of projects written in Cooper

* A project is defined by a `project.toml` (name and type subject to change) in a directory
  * The presence of a `main.coop` (file extension still subject to change) indicates an executable program
  * The lack of a `main.coop` indicates a library
  * For distributing library binaries, plaintext interface file(s) are auto-generated (similarly to Swift)
  * Basically default C binary layout despite the language having some more complex features:
    * No inheritance or embedding, a method has an explicit receiver (effectively sugar for functions)
    * No hidden fields in structs such as a per-struct vtable
    * Interfaces are just fat pointers similar to Go: pointing to a concrete type + a separate itable
    * No silent layout optimization like struct member reordering or packing
    * Needs some name mangling (module path), but given that, the result should be C ABI compatible

* Each subdirectory under a project defines a module of matching name, e.g. `auth` in `auth/`, `server.auth` in
`server/auth/`
  * With the exception being an implicit module `main` which is at the project root, regardless of its directory name
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
    * Note: terminology matters -- importing happens at project level; modules _use_ other modules
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
    * Starts with `std` like `std.io`: Cooper standard library
    * Relative module path like `server.auth`: local to project, search `<project root>/server/auth`
    * Otherwise, search `project.toml` for a matching identifier declaring an external dependency

### Some notes on individual considerations and design specifics

* Cooper program sources are UTF-8
  * TODO: Current lexer only tokenizes a narrow ASCII subset
* Strings are by default encoded in UTF-8
  * TODO: `rune` type for representing individual code point -- or grapheme, TBD

## Compiler project structure

* main.go - a thin entry point for the compiler for now.
  * The primary focus right now is in validation by tests, so the main doesn't really get executed much currently.
  * TODO: Could benefit from some CLI/TUI toolkit or library once compiler arguments and flags start to matter.

* /lexer - the lexer package for performing tokenization.
  * Relies heavily on regular expressions, matching in order from longest to shortest.
  * Outputs a slice of tokens that can be then used as input for the parser.

* /ast - the abstract syntax tree package.
  * Very explicit for clarity; see e.g. `IfExpr` vs `IfStmt` , `TypeExpr` instead of `Type` etc.
  * Just a package of types with no functionality.
  * Interfaces with empty implementations are only to enable the AST as a heterogeneous collection.

* /parser - the parser package.
  * Primarily works on one-token lookahead -- semicolon inference details aside, effectively a LL(1) grammar.
  * Explicit and specific parsing functions for individual statements, mostly identified by keyword.
  * Pratt parsing for most operator-focused expressions.
  * Syntax is mostly imperative, with some quality-of-life- features planned, inspired by functional languages.

* /typechecker - the symbol resolver, type checker, and semantic analyzer package.

## Chat and overall conversational guidelines

Be concise. Not to the point of being blunt or impolite, but excessive sycophancy is unnecessary. Adopt the
conversational tone of a senior software professional addressing a similarly experienced (if with different domain
expertise) colleague.

Do not trail your responses with questions priming the anticipated next step, such as whether the user would like to "do
X". The user will proactively ask you. This may feel like a conflicting instruction that is in tension with the overall
desired tone of the chat, but the goal is to be wary of idle chatter that may negatively impact the signal/noise ratio
of the conversation, and eat up precious context window "headspace".

When explicitly asked to "update memory" or "summarize progress", provide a concise summary that captures:

- Key decisions made and rationale
- Lessons learned or gotchas discovered  
- Planned next steps or open questions
- Any important context for resuming this task

Format it as a ready-to-paste brief that would get a future LLM up to speed quickly.

## Coding guidelines

Follow general Go coding guidelines. Follow established patterns, naming conventions, and the general style of existing
code in the project. Do **not** proactively author tests with each coding task; the user will explicitly ask for tests
as a separate task, when new code reaches sufficient maturity to become a permanent addition to the project.

If you find yourself running in circles due to unexpected or unexplainable results from tools, or due to some missing configuration in the runtime environment, stop! Rather than forcing the user to manually interrupt the process when they see that, for example, creating a simple file has already taken you 15 steps for unknown reasons, it is **much** preferred that you rather break proactively, report what you have and what you intended to do next, and at a high level, why it failed (at least your perception of why it may have failed). The user can often help you cross those chasms by e.g. adjusting allowed directories, creating or adjusting file permissions, or even installing some new command-line tool.

**Pause and Report Pattern**: Beyond stopping only when stuck, proactively pause after completing logical milestones or subtasks (typically every few minutes of work). Report what you've accomplished, what you plan to do next, and any blocking issues or decisions needed. This checkpoint pattern preserves progress, allows the user to steer the process, and enables graceful recovery if the AI application itself catastrophically fails. It's better to pause at 80% complete and report status, than to lose everything -- including the memory of what was achieved -- attempting to reach 100% in one leap.

There are details that you are probably able to discover independently by examining the project files. But when facing an "executive decision" which the user hasn't been able to foresee and provide for in their prompts, stop! Ask questions, and only when you're confident that you have all the major design specifications and constraints, proceed with the task itself. It is always preferred that you ask rather than assume! Of course there is some threshold where the user prefers that you attempt to make the right choice based on the materials provided, so don't become overly cautious and ask for approval on every detail, but for decisions that drive high-level design, architecture, and implementation, err on the side of confirming first to avoid reverting later.
