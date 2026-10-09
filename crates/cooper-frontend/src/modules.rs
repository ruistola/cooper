//! Module-graph analysis: interfaces, dependency ordering, and cross-module `use`.
//!
//! A project's modules do not compile in isolation. A module may `use` names
//! exported by other modules, so before checking any module's bodies the compiler
//! resolves the module dependency graph, orders it topologically, and builds each
//! module's **interface** — its exported signatures — in that order. Every module
//! is then resolved and checked against its own declarations plus the interfaces of
//! the modules it depends on, never their bodies. This interface-only rule is what
//! keeps modules separable (a future cache can supply an interface without source).
//!
//! `use` bindings are resolved here against the set of the project's modules: a path
//! whose final segment names a module is a *module binding*, made available for
//! qualified access (`other.func()`, `other.Type`), while a path whose prefix names a
//! module and whose final segment names one of that module's exported items is a
//! *name binding*, bringing the item into scope for bare use. Imports are file-scoped:
//! each file is resolved and checked against the module's united declarations plus its
//! own `use` bindings, so a name imported in one file is invisible in its siblings.

use std::collections::{HashMap, HashSet};

use crate::ast::{Stmt, StmtKind, UseSpec};
use crate::diag::{Diagnostic, Span};
use crate::lexer;
use crate::parser;
use crate::project::{Module, ModulePath, Project};
use crate::resolve::{self, Globals, Signature};
use crate::types::{TypeDefs, TypeId};
use crate::{semantic, typecheck};

/// The dependency edges of the module graph: for each module, the set of modules it
/// depends on, keyed by their index and tagged with the span and file of a `use` that
/// induced the edge so a cycle can be reported at the source that closes it.
type Edges = Vec<HashMap<usize, (Span, String)>>;

/// Analyze every module of `project`, returning all diagnostics. Modules are parsed
/// first; if any file fails to lex or parse the analysis stops there, since a
/// missing declaration would cascade into spurious cross-module errors. Otherwise
/// the dependency graph is ordered and each module resolved and checked in turn.
pub fn analyze(project: &Project) -> Vec<Diagnostic> {
    check(project).err().unwrap_or_default()
}

/// One type-checked source file: its module, name, and declarations, the symbol table
/// its bodies were checked against (the module's declarations plus the file's
/// imports), and the checker's span tables.
pub struct CheckedFile {
    pub module: ModulePath,
    pub name: String,
    pub decls: Vec<Stmt>,
    pub globals: Globals,
    pub typed: typecheck::Typed,
}

/// A whole type-checked project: every file, its modules in dependency order, and
/// the definitions of every struct and sum type the project declares.
pub struct CheckedProject {
    pub files: Vec<CheckedFile>,
    pub defs: TypeDefs,
}

/// Analyze `project` as [`analyze`] does, returning its checked form when it is free
/// of diagnostics — the input to lowering — and every diagnostic otherwise.
pub fn check(project: &Project) -> Result<CheckedProject, Vec<Diagnostic>> {
    let mut parsed = parse_modules(project)?;

    let standard = crate::stdlib::interfaces();
    let known: HashSet<ModulePath> = parsed
        .iter()
        .map(|m| m.path.clone())
        .chain(standard.iter().map(|(path, _)| path.clone()))
        .collect();

    let (edges, mut diags) = build_edges(&parsed, &known);
    let order = match topological_order(&parsed, &edges) {
        Ok(order) => order,
        Err(cycle_diags) => {
            diags.extend(cycle_diags);
            return Err(diags);
        }
    };

    // Type definitions accumulate project-wide: a dependency's interface may expose
    // a type whose members name types the dependent never imports, so every module
    // is analyzed against all definitions resolved before it.
    let mut interfaces: HashMap<ModulePath, Globals> = standard.into_iter().collect();
    let mut defs = TypeDefs::default();
    let mut checked = Vec::new();
    for index in order {
        let module = &parsed[index];
        let (module_diags, interface, files) = analyze_module(module, &known, &interfaces, &defs);
        diags.extend(module_diags);
        defs = interface.defs.clone();
        interfaces.insert(module.path.clone(), interface);
        checked.push((index, files));
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    let files = checked
        .into_iter()
        .flat_map(|(index, files)| {
            let module = &mut parsed[index];
            let path = module.path.clone();
            std::mem::take(&mut module.files)
                .into_iter()
                .zip(files)
                .map(move |(file, (globals, typed))| CheckedFile {
                    module: path.clone(),
                    name: file.name,
                    decls: file.decls,
                    globals,
                    typed,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    Ok(CheckedProject { files, defs })
}

/// A module whose files have been parsed. Top-level declarations unite across files
/// into the module's namespace, but each file keeps its own `use` bindings, which are
/// visible only within that file.
struct ParsedModule {
    path: ModulePath,
    files: Vec<ParsedFile>,
}

/// One parsed source file: its file-scoped `use` bindings and its declarations,
/// tagged with the file name so later passes can label their diagnostics.
struct ParsedFile {
    name: String,
    uses: Vec<UseSpec>,
    decls: Vec<Stmt>,
}

/// Parse every file of every module. Returns the parsed modules, or every lex and
/// parse diagnostic (each tagged with its file) if any file was malformed.
fn parse_modules(project: &Project) -> Result<Vec<ParsedModule>, Vec<Diagnostic>> {
    let mut parsed = Vec::with_capacity(project.modules.len());
    let mut errors = Vec::new();
    for module in &project.modules {
        let (files, module_errors) = parse_module_files(module);
        errors.extend(module_errors);
        parsed.push(ParsedModule {
            path: module.path.clone(),
            files,
        });
    }
    if errors.is_empty() {
        Ok(parsed)
    } else {
        Err(errors)
    }
}

/// Lex and parse each source file of `module`, keeping each file's `use` bindings and
/// declarations separate. Lex and parse diagnostics are tagged with the originating
/// file.
fn parse_module_files(module: &Module) -> (Vec<ParsedFile>, Vec<Diagnostic>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for file in &module.files {
        let tokens = match lexer::tokenize(&file.source) {
            Ok(tokens) => tokens,
            Err(diag) => {
                errors.push(diag.in_file(&file.name));
                continue;
            }
        };
        let parsed = parser::parse(tokens);
        errors.extend(parsed.errors.into_iter().map(|d| d.in_file(&file.name)));
        files.push(ParsedFile {
            name: file.name.clone(),
            uses: parsed.uses,
            decls: parsed.decls,
        });
    }
    (files, errors)
}

/// What a `use` binding refers to, once resolved against the project's modules.
enum UseTarget {
    /// The final segment names a module; its members are reached qualified.
    Module(ModulePath),
    /// The prefix names a module and the final segment one of its exported items.
    Name { module: ModulePath, item: String },
}

/// Classify a `use` path against the set of known modules. The longest interpretation
/// wins: a path that is itself a module is a module binding; otherwise a path whose
/// prefix is a module is a name binding for its final segment. A path matching
/// neither is unresolvable.
fn classify(spec: &UseSpec, known: &HashSet<ModulePath>) -> Option<UseTarget> {
    let full = ModulePath(spec.path.clone());
    if known.contains(&full) {
        return Some(UseTarget::Module(full));
    }
    if spec.path.len() >= 2 {
        let prefix = ModulePath(spec.path[..spec.path.len() - 1].to_vec());
        if known.contains(&prefix) {
            return Some(UseTarget::Name {
                module: prefix,
                item: spec.path.last().cloned().unwrap(),
            });
        }
    }
    None
}

/// Build the dependency edges from every module's `use` bindings, reporting any
/// binding that names no known module. Each edge (module index -> depended-on module
/// index) carries the span and file of a `use` that induced it, so a dependency cycle
/// can be reported at the source that closes it.
fn build_edges(
    parsed: &[ParsedModule],
    known: &HashSet<ModulePath>,
) -> (Edges, Vec<Diagnostic>) {
    let index_of: HashMap<&ModulePath, usize> =
        parsed.iter().enumerate().map(|(i, m)| (&m.path, i)).collect();
    let mut edges = vec![HashMap::new(); parsed.len()];
    let mut diags = Vec::new();
    for (i, module) in parsed.iter().enumerate() {
        for file in &module.files {
            for spec in &file.uses {
                let target = match classify(spec, known) {
                    Some(UseTarget::Module(path))
                    | Some(UseTarget::Name { module: path, .. }) => path,
                    None => {
                        diags.push(
                            Diagnostic::error(
                                spec.span,
                                format!("no module `{}` in this project", spec.path.join(".")),
                            )
                            .in_file(&file.name),
                        );
                        continue;
                    }
                };
                if let Some(&j) = index_of.get(&target) {
                    if j != i {
                        edges[i]
                            .entry(j)
                            .or_insert_with(|| (spec.span, file.name.clone()));
                    }
                }
            }
        }
    }
    (edges, diags)
}

/// Order modules so every module follows those it depends on, or report the modules
/// forming a dependency cycle. Cycles are forbidden: interface construction and
/// module initialization order both require an acyclic graph.
fn topological_order(
    parsed: &[ParsedModule],
    edges: &Edges,
) -> Result<Vec<usize>, Vec<Diagnostic>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unvisited,
        InProgress,
        Done,
    }
    let mut marks = vec![Mark::Unvisited; parsed.len()];
    let mut order = Vec::with_capacity(parsed.len());

    fn visit(
        node: usize,
        parsed: &[ParsedModule],
        edges: &Edges,
        marks: &mut [Mark],
        order: &mut Vec<usize>,
    ) -> Result<(), Vec<Diagnostic>> {
        marks[node] = Mark::InProgress;
        for (&next, (span, file)) in &edges[node] {
            match marks[next] {
                Mark::Done => {}
                Mark::InProgress => {
                    return Err(vec![Diagnostic::error(
                        *span,
                        format!(
                            "module `{}` is part of a dependency cycle with `{}`",
                            parsed[node].path, parsed[next].path
                        ),
                    )
                    .in_file(file)]);
                }
                Mark::Unvisited => visit(next, parsed, edges, marks, order)?,
            }
        }
        marks[node] = Mark::Done;
        order.push(node);
        Ok(())
    }

    for node in 0..parsed.len() {
        if marks[node] == Mark::Unvisited {
            visit(node, parsed, edges, &mut marks, &mut order)?;
        }
    }
    Ok(order)
}

/// The imported names a module brings into scope, keyed by local surface name (its
/// alias, or the item's own name). Types and functions are kept apart so they seed
/// the right symbol tables; an imported struct's methods travel with it.
#[derive(Default)]
struct Imports {
    structs: HashMap<String, TypeId>,
    oneofs: HashMap<String, TypeId>,
    funcs: HashMap<String, Signature>,
    methods: HashMap<TypeId, HashMap<String, Signature>>,
    /// Module bindings keyed by local spelling (`["std", "io"]`, or `["web"]` when
    /// aliased), each mapped to that module's interface for qualified access.
    modules: HashMap<Vec<String>, Globals>,
    local_names: HashSet<String>,
}

impl Imports {
    /// This file's bare imports overlaid on `base` — the module's own declarations —
    /// yielding the symbol table a file's declarations and bodies are checked against.
    /// Imported structs, sum types, and functions join by local name; an imported
    /// struct's methods merge under its identity.
    fn overlay(&self, base: &Globals) -> Globals {
        let mut globals = base.clone();
        globals.structs.extend(self.structs.clone());
        globals.oneofs.extend(self.oneofs.clone());
        globals.funcs.extend(self.funcs.clone());
        for (owner, methods) in &self.methods {
            globals
                .methods
                .entry(owner.clone())
                .or_default()
                .extend(methods.clone());
        }
        globals
    }
}

/// Resolve, type-check, and semantically analyze one module against its dependencies'
/// interfaces, returning its diagnostics, its own exported interface for later
/// modules, and each file's symbol table and checker tables (complete only when
/// there are no diagnostics). Each file is resolved and checked against the module's declarations plus
/// that file's own imports, so a `use` is visible only in the file that wrote it.
/// Diagnostics short-circuit per phase so failures do not cascade within a file.
fn analyze_module(
    module: &ParsedModule,
    known: &HashSet<ModulePath>,
    interfaces: &HashMap<ModulePath, Globals>,
    defs: &TypeDefs,
) -> (Vec<Diagnostic>, Globals, Vec<(Globals, typecheck::Typed)>) {
    let mut diags = Vec::new();
    let declared = declared_names(&module.files);
    let file_imports: Vec<Imports> = module
        .files
        .iter()
        .map(|file| build_file_imports(file, &declared, known, interfaces, &mut diags))
        .collect();

    // Resolve in phases, each across every file, accumulating one module-wide table
    // of the module's own declarations: all type names first, so any file's types
    // and signatures may name a type declared later or in another file. Each file's
    // phase sees that file's imports, stripped back out before the next file so
    // they never leak across files.
    let module_name = module.path.to_string();
    let mut interface = Globals::default();
    interface.module = module_name.clone();
    interface.defs = defs.clone();
    for file in &module.files {
        let mut file_diags = Vec::new();
        resolve::declare_types(&mut interface, &file.decls, &mut file_diags);
        diags.extend(file_diags.into_iter().map(|d| d.in_file(&file.name)));
    }
    type Phase = fn(&mut Globals, &[Stmt], &mut Vec<Diagnostic>);
    let size_check: Phase = |globals, decls, diags| resolve::check_type_sizes(globals, decls, diags);
    let phases: [Phase; 3] = [resolve::define_types, size_check, resolve::resolve_signatures];
    for phase in phases {
        for (file, imports) in module.files.iter().zip(&file_imports) {
            let mut globals = imports.overlay(&interface);
            let mut file_diags = Vec::new();
            phase(&mut globals, &file.decls, &mut file_diags);
            diags.extend(file_diags.into_iter().map(|d| d.in_file(&file.name)));
            interface = strip_imports(globals, imports);
        }
    }

    let mut checked = Vec::new();
    if diags.is_empty() {
        // Bodies are checked per file against the module's declarations plus that
        // file's imports, so type and semantic diagnostics carry their file too.
        for (file, imports) in module.files.iter().zip(&file_imports) {
            let globals = imports.overlay(&interface);
            let mut typed = typecheck::check(&file.decls, &globals, &imports.modules);
            let type_diags = std::mem::take(&mut typed.diags);
            if type_diags.is_empty() {
                diags.extend(
                    semantic::analyze(&file.decls, &globals)
                        .into_iter()
                        .map(|d| d.in_file(&file.name)),
                );
            } else {
                diags.extend(type_diags.into_iter().map(|d| d.in_file(&file.name)));
            }
            checked.push((globals, typed));
        }
    }
    (diags, interface, checked)
}

/// The names a module declares across all its files: its top-level structs, sum
/// types, free functions, and module-level variables. Methods are keyed by receiver,
/// not by a bare name, so they never collide with an import. Used to reject an import
/// whose local name would shadow one of the module's own declarations.
fn declared_names(files: &[ParsedFile]) -> HashSet<String> {
    let mut names = HashSet::new();
    for file in files {
        for stmt in &file.decls {
            match &stmt.kind {
                StmtKind::StructDecl { name, .. }
                | StmtKind::OneofDecl { name, .. }
                | StmtKind::VarDecl { name, .. }
                | StmtKind::ExternFunc { name, .. } => {
                    names.insert(name.clone());
                }
                StmtKind::FuncDecl(func) if func.receiver.is_none() => {
                    names.insert(func.name.clone());
                }
                _ => {}
            }
        }
    }
    names
}

/// Resolve one file's `use` bindings against its dependencies' interfaces, collecting
/// the imported symbols. Name bindings seed bare names; module bindings record the
/// dependency's interface under its local spelling for qualified access. An import is
/// rejected — reported at its own `use` — when it binds an unknown item, repeats a
/// local name already bound in this file, or collides with one of `declared`, the
/// module's own declaration names.
fn build_file_imports(
    file: &ParsedFile,
    declared: &HashSet<String>,
    known: &HashSet<ModulePath>,
    interfaces: &HashMap<ModulePath, Globals>,
    diags: &mut Vec<Diagnostic>,
) -> Imports {
    let mut imports = Imports::default();
    for spec in &file.uses {
        let Some(target) = classify(spec, known) else {
            continue;
        };
        let local = spec.local_name().to_string();
        if declared.contains(&local) {
            diags.push(
                Diagnostic::error(
                    spec.span,
                    format!("import `{local}` collides with a declaration of the same name in this module"),
                )
                .in_file(&file.name),
            );
            continue;
        }
        if !imports.local_names.insert(local.clone()) {
            diags.push(
                Diagnostic::error(
                    spec.span,
                    format!("`{local}` is bound by more than one use in this file"),
                )
                .in_file(&file.name),
            );
            continue;
        }
        match target {
            UseTarget::Module(dep) => {
                let Some(interface) = interfaces.get(&dep) else {
                    continue;
                };
                // A module is spelled by its full local path, or by its alias alone.
                let spelling = match &spec.alias {
                    Some(alias) => vec![alias.clone()],
                    None => spec.path.clone(),
                };
                imports.modules.insert(spelling, interface.clone());
            }
            UseTarget::Name { module: dep, item } => {
                let Some(interface) = interfaces.get(&dep) else {
                    continue;
                };
                if let Some(id) = interface.structs.get(&item) {
                    imports.structs.insert(local, id.clone());
                    if let Some(methods) = interface.methods.get(id) {
                        imports.methods.insert(id.clone(), methods.clone());
                    }
                } else if let Some(id) = interface.oneofs.get(&item) {
                    imports.oneofs.insert(local, id.clone());
                } else if let Some(sig) = interface.lookup_func(&item) {
                    imports.funcs.insert(local, sig.clone());
                } else {
                    diags.push(
                        Diagnostic::error(
                            spec.span,
                            format!("module `{dep}` exports no item named `{item}`"),
                        )
                        .in_file(&file.name),
                    );
                }
            }
        }
    }
    imports
}

/// Remove imported names from a resolved symbol table, leaving only the module's own
/// declarations — its exported interface. Imports are not re-exported: a name reaches
/// a module only through that module's own `use`.
fn strip_imports(mut globals: Globals, imports: &Imports) -> Globals {
    for name in imports.structs.keys() {
        globals.structs.remove(name);
    }
    for name in imports.oneofs.keys() {
        globals.oneofs.remove(name);
    }
    for name in imports.funcs.keys() {
        globals.funcs.remove(name);
    }
    for owner in imports.methods.keys() {
        globals.methods.remove(owner);
    }
    globals
}
