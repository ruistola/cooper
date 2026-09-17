//! The in-memory project model: the compiler's input.
//!
//! Cooper compiles a [`Project`] — a self-contained description of a program or
//! library — rather than reading a filesystem directly. A future utility will walk
//! a physical source tree to construct one of these, but keeping the compiler's
//! input abstract lets it run anywhere (e.g. a browser playground) and lets tests
//! build projects from in-memory sources.
//!
//! A project owns 1..n [`Module`]s (the atomic compilation units), each spanning
//! 1..n [`SourceFile`]s. `std`, external dependencies, sub-projects, and caching
//! are not modelled yet; the shape here is deliberately minimal but leaves room for
//! them.

/// A source file: a name for diagnostics and its raw text. Parsing happens inside
/// the compiler, so the input carries text rather than an AST.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub name: String,
    pub source: String,
}

impl SourceFile {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        SourceFile {
            name: name.into(),
            source: source.into(),
        }
    }
}

/// A dotted module path such as `server.auth`. The root program module is `main`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath(pub Vec<String>);

impl ModulePath {
    /// Parse a dotted path (`"server.auth"`) into its segments.
    pub fn parse(dotted: &str) -> Self {
        ModulePath(dotted.split('.').map(str::to_string).collect())
    }
}

impl std::fmt::Display for ModulePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.join("."))
    }
}

/// A module: the atomic compilation unit, identified by its path and made of one
/// or more source files whose top-level declarations unite into one namespace.
#[derive(Debug, Clone)]
pub struct Module {
    pub path: ModulePath,
    pub files: Vec<SourceFile>,
}

impl Module {
    pub fn new(path: &str, files: Vec<SourceFile>) -> Self {
        Module {
            path: ModulePath::parse(path),
            files,
        }
    }
}

/// Whether a project builds an executable (has a `main` module) or a library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    Program,
    Library,
}

/// The not-in-source project configuration: its name and kind. Dependency bindings
/// (external packages, sub-projects, `std`) will live here once modelled.
#[derive(Debug, Clone)]
pub struct ProjectManifest {
    pub name: String,
    pub kind: ProjectKind,
}

/// A whole project: its manifest plus every module it owns.
#[derive(Debug, Clone)]
pub struct Project {
    pub manifest: ProjectManifest,
    pub modules: Vec<Module>,
}

impl Project {
    pub fn new(name: impl Into<String>, kind: ProjectKind, modules: Vec<Module>) -> Self {
        Project {
            manifest: ProjectManifest {
                name: name.into(),
                kind,
            },
            modules,
        }
    }

    /// Wrap a single source snippet as a one-file `main` module of a program. Used
    /// by the string-in entry point and by focused tests.
    pub fn snippet(source: &str) -> Self {
        Project::new(
            "snippet",
            ProjectKind::Program,
            vec![Module::new("main", vec![SourceFile::new("main", source)])],
        )
    }
}
