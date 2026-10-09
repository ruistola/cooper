//! Filesystem loading: constructing an in-memory [`Project`] from a source tree.
//!
//! The compiler consumes an abstract [`Project`]; this module is the bridge from a
//! physical directory to that model. A project is a directory containing a
//! `project.toml` manifest. Each subdirectory of source files becomes a module named
//! by its path relative to the module root (`server/auth/` → `server.auth`), while
//! `.coop` files directly under the root form the implicit `main` module. A project
//! is a program when a `main.coop` file sits at the module root, otherwise a library.
//!
//! Caching, checksums, external dependencies, and sub-projects are out of scope: a
//! nested directory carrying its own `project.toml` is a sub-project and is skipped
//! rather than absorbed, leaving that seam for later without misreading the tree.

use std::fmt;
use std::path::{Path, PathBuf};

use cooper_frontend::{Module, Project, ProjectKind, SourceFile};

const MANIFEST: &str = "project.toml";
const SOURCE_EXT: &str = "coop";
const MAIN_FILE: &str = "main.coop";
/// The directory, beside the project or file, that build outputs are written to.
const BUILD_DIR: &str = "build";

/// Why a project could not be loaded from disk.
#[derive(Debug)]
pub enum LoadError {
    /// A file or directory could not be read.
    Io { path: PathBuf, source: std::io::Error },
    /// The manifest was absent, unparseable, or missing a required field.
    Manifest { path: PathBuf, message: String },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            LoadError::Manifest { path, message } => {
                write!(f, "invalid manifest {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Load the project rooted at `root`, reading its manifest and every module's source.
pub fn load_project(root: &Path) -> Result<Project, LoadError> {
    load_with_native(root).map(|(project, _)| project)
}

/// The C code a program links with, from its manifest's `[c]` table: libraries
/// (`libraries = ["m"]`, linked as `-lm`) and C source files (`sources`, relative to
/// the project root), compiled into the program.
#[derive(Debug, Clone, Default)]
pub struct Native {
    pub libraries: Vec<String>,
    pub sources: Vec<PathBuf>,
}

/// Load the project rooted at `root` and the C code its manifest names.
fn load_with_native(root: &Path) -> Result<(Project, Native), LoadError> {
    let manifest_path = root.join(MANIFEST);
    let manifest = read(&manifest_path)?;
    let (name, module_root, mut native) = parse_manifest(&manifest_path, &manifest)?;
    native.sources = native.sources.iter().map(|source| root.join(source)).collect();

    let module_root_dir = match module_root {
        Some(rel) => root.join(rel),
        None => root.to_path_buf(),
    };

    let kind = if module_root_dir.join(MAIN_FILE).is_file() {
        ProjectKind::Program
    } else {
        ProjectKind::Library
    };

    let mut modules = Vec::new();
    collect_modules(&module_root_dir, &[], &mut modules)?;
    Ok((Project::new(name, kind, modules), native))
}

/// A project loaded from the command line, the C code it links with, and the directory
/// its build outputs go to.
pub struct Loaded {
    pub project: Project,
    pub native: Native,
    pub build_dir: PathBuf,
}

/// Load `path`: a directory as a project, its outputs in `<dir>/build/`; or a single
/// source file as a one-module program named after the file, its outputs in a
/// `build/` directory beside it.
pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    if path.is_dir() {
        let (project, native) = load_with_native(path)?;
        return Ok(Loaded {
            project,
            native,
            build_dir: path.join(BUILD_DIR),
        });
    }
    let source = read(path)?;
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main".to_string());
    let module = Module::new("main", vec![SourceFile::new(path.to_string_lossy(), source)]);
    Ok(Loaded {
        project: Project::new(name, ProjectKind::Program, vec![module]),
        native: Native::default(),
        build_dir: path.parent().unwrap_or(Path::new(".")).join(BUILD_DIR),
    })
}

/// The manifest's `name` (required), optional `module_root`, and optional `[c]` table.
fn parse_manifest(path: &Path, source: &str) -> Result<(String, Option<String>, Native), LoadError> {
    let table: toml::Table = source.parse().map_err(|e: toml::de::Error| LoadError::Manifest {
        path: path.to_path_buf(),
        message: e.message().to_string(),
    })?;
    let manifest_err = |message: &str| LoadError::Manifest {
        path: path.to_path_buf(),
        message: message.to_string(),
    };
    let name = table
        .get("name")
        .ok_or_else(|| manifest_err("missing required key `name`"))?
        .as_str()
        .ok_or_else(|| manifest_err("`name` must be a string"))?
        .to_string();
    let module_root = match table.get("module_root") {
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| manifest_err("`module_root` must be a string"))?
                .to_string(),
        ),
        None => None,
    };
    let mut native = Native::default();
    if let Some(c) = table.get("c") {
        let c = c.as_table().ok_or_else(|| manifest_err("`c` must be a table"))?;
        let strings = |key: &str| -> Result<Vec<String>, LoadError> {
            let Some(value) = c.get(key) else {
                return Ok(Vec::new());
            };
            let error = || manifest_err(&format!("`c.{key}` must be an array of strings"));
            value
                .as_array()
                .ok_or_else(error)?
                .iter()
                .map(|item| item.as_str().map(str::to_string).ok_or_else(error))
                .collect()
        };
        native.libraries = strings("libraries")?;
        native.sources = strings("sources")?.into_iter().map(PathBuf::from).collect();
        if let Some(key) = c.keys().find(|key| !["libraries", "sources"].contains(&key.as_str())) {
            return Err(manifest_err(&format!("unknown key `c.{key}`")));
        }
    }
    Ok((name, module_root, native))
}

/// Walk `dir`, whose module-path segments so far are `segments`, appending a module
/// for its `.coop` files (named `main` at the root) and recursing into subdirectories
/// that are not themselves sub-projects.
fn collect_modules(
    dir: &Path,
    segments: &[String],
    modules: &mut Vec<Module>,
) -> Result<(), LoadError> {
    let mut source_files = Vec::new();
    let mut subdirs = Vec::new();
    for entry in read_dir(dir)? {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.extension().is_some_and(|ext| ext == SOURCE_EXT) {
            source_files.push(path);
        }
    }

    if !source_files.is_empty() {
        source_files.sort();
        let mut files = Vec::with_capacity(source_files.len());
        for path in &source_files {
            files.push(SourceFile::new(relative_name(dir, path), read(path)?));
        }
        let module_path = if segments.is_empty() {
            "main".to_string()
        } else {
            segments.join(".")
        };
        modules.push(Module::new(&module_path, files));
    }

    subdirs.sort();
    for subdir in subdirs {
        // A subdirectory with its own manifest is a sub-project (deferred): its
        // modules belong to it, not to this project, so it is not walked here.
        if subdir.join(MANIFEST).is_file() {
            continue;
        }
        let name = subdir
            .file_name()
            .expect("a directory entry always has a final component")
            .to_string_lossy()
            .into_owned();
        let mut child_segments = segments.to_vec();
        child_segments.push(name);
        collect_modules(&subdir, &child_segments, modules)?;
    }
    Ok(())
}

/// A source file's name for diagnostics: its path relative to `dir`, or just the file
/// name. The directory prefix keeps names unique across a multi-module project.
fn relative_name(dir: &Path, path: &Path) -> String {
    let file = path
        .file_name()
        .expect("a source path always has a file name")
        .to_string_lossy();
    match dir.file_name() {
        Some(parent) => format!("{}/{file}", parent.to_string_lossy()),
        None => file.into_owned(),
    }
}

fn read(path: &Path) -> Result<String, LoadError> {
    std::fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn read_dir(path: &Path) -> Result<Vec<std::fs::DirEntry>, LoadError> {
    let entries = std::fs::read_dir(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut collected = Vec::new();
    for entry in entries {
        collected.push(entry.map_err(|source| LoadError::Io {
            path: path.to_path_buf(),
            source,
        })?);
    }
    Ok(collected)
}
