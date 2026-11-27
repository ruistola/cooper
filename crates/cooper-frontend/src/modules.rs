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
//! `use` bindings are resolved here against the set of the project's modules. Each names
//! a module, bound under its last path segment or its `as` alias, and everything the
//! file uses from it is qualified with that name (`io.println()`, `sync.WaitGroup`).
//! Imports are file-scoped: each file is resolved and checked against the module's united
//! declarations plus its own `use` bindings, so a module bound in one file is invisible
//! in its siblings.

use std::collections::{HashMap, HashSet};

use crate::ast::{Stmt, StmtKind, TypeExpr, TypeExprKind, UseSpec};
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
    /// The documentation comment of each declaration, member, and variant that has one,
    /// by the byte offset where it starts.
    pub docs: crate::lexer::Docs,
}

/// A whole type-checked project: every file, its modules in dependency order, and
/// the definitions of every struct and sum type the project declares.
pub struct CheckedProject {
    pub files: Vec<CheckedFile>,
    pub defs: TypeDefs,
}

/// Analyze `project` as [`analyze`] does, returning its checked form when it is free
/// of diagnostics — the input to lowering — and every diagnostic otherwise. A file that
/// fails to lex or parse stops the analysis there, since a missing declaration would
/// cascade into spurious errors elsewhere.
pub fn check(project: &Project) -> Result<CheckedProject, Vec<Diagnostic>> {
    let project = &crate::stdlib::with_source_modules(project);
    let (parsed, errors) = parse_modules(project);
    if !errors.is_empty() {
        return Err(errors);
    }
    let (diagnostics, checked) = analyze_parsed(parsed);
    if diagnostics.is_empty() {
        Ok(checked)
    } else {
        Err(diagnostics)
    }
}

/// The result of [`analyze_lenient`]: every diagnostic, and whatever was checked.
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    /// The files of every module that resolved, with the checker's tables for what type
    /// checked, even when other code has errors. A file that failed to parse is present
    /// with the declarations that were recovered from it.
    pub project: CheckedProject,
}

/// Analyze `project` for a tool that works on code as it is being written: parse errors
/// do not stop the analysis, and the checked form is returned however many diagnostics
/// there are, so that hover, go-to-definition, and documentation keep working in a file
/// with an error in it. A module whose declarations do not resolve has no checked files.
pub fn analyze_lenient(project: &Project) -> Analysis {
    let project = &crate::stdlib::with_source_modules(project);
    let (parsed, mut diagnostics) = parse_modules(project);
    let (more, checked) = analyze_parsed(parsed);
    diagnostics.extend(more);
    Analysis { diagnostics, project: checked }
}

/// Analyze parsed modules: the checked files of every module that resolved, and the
/// diagnostics of the dependency graph and each module.
fn analyze_parsed(mut parsed: Vec<ParsedModule>) -> (Vec<Diagnostic>, CheckedProject) {
    let standard = crate::stdlib::interfaces();
    let known: HashSet<ModulePath> = parsed
        .iter()
        .map(|m| m.path.clone())
        .chain(standard.iter().map(|(path, _)| path.clone()))
        .collect();

    let (mut edges, mut diags) = build_edges(&parsed, &known);
    // Every module sees the methods of the built-in types, so follows their home modules,
    // except that a home follows only the homes listed before it.
    let homes: Vec<Option<usize>> =
        crate::stdlib::HOMES.iter().map(|home| parsed.iter().position(|m| m.path.to_string() == *home)).collect();
    for (rank, home) in homes.iter().enumerate() {
        let Some(home) = *home else { continue };
        for (i, module) in edges.iter_mut().enumerate() {
            if !homes[..=rank].contains(&Some(i)) {
                module.entry(home).or_insert((Span::new(0, 0), parsed[home].files[0].name.clone()));
            }
        }
    }
    let order = match topological_order(&parsed, &edges) {
        Ok(order) => order,
        Err(cycle_diags) => {
            diags.extend(cycle_diags);
            return (diags, CheckedProject { files: Vec::new(), defs: TypeDefs::default() });
        }
    };

    // Type definitions accumulate project-wide: a dependency's interface may expose
    // a type whose members name types the dependent never imports, so every module
    // is analyzed against all definitions resolved before it.
    let mut defs = TypeDefs::default();
    let prelude = crate::stdlib::prelude();
    defs.unions.extend(prelude.defs.unions);
    defs.structs.extend(prelude.defs.structs);
    for (_, interface) in &standard {
        defs.structs.extend(interface.defs.structs.clone());
        defs.unions.extend(interface.defs.unions.clone());
    }
    let mut interfaces: HashMap<ModulePath, Globals> = standard.into_iter().collect();
    let mut checked = Vec::new();
    for index in order {
        let module = &parsed[index];
        let (module_diags, interface, files) = analyze_module(module, &known, &interfaces, &defs);
        diags.extend(module_diags);
        defs = interface.defs.clone();
        interfaces.insert(module.path.clone(), exported(interface, &module.files));
        checked.push((index, files));
    }
    let counts: Vec<usize> = parsed.iter().map(|m| m.files.len()).collect();
    let files = checked
        .into_iter()
        .filter(|(index, files)| files.len() == counts[*index])
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
                    docs: file.docs,
                    globals,
                    typed,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    (diags, CheckedProject { files, defs })
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
    source: String,
    /// Whether the file is marked `#private`.
    private: bool,
    uses: Vec<UseSpec>,
    decls: Vec<Stmt>,
    docs: crate::lexer::Docs,
}

/// Parse every file of every module, returning the parsed modules and every lex and parse
/// diagnostic (each tagged with its file). A file that fails to lex is an empty one.
fn parse_modules(project: &Project) -> (Vec<ParsedModule>, Vec<Diagnostic>) {
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
    (parsed, errors)
}

/// Lex and parse each source file of `module`, keeping each file's `use` bindings and
/// declarations separate. Lex and parse diagnostics are tagged with the originating
/// file.
fn parse_module_files(module: &Module) -> (Vec<ParsedFile>, Vec<Diagnostic>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for file in &module.files {
        let (tokens, docs) = match lexer::tokenize_with_docs(&file.source) {
            Ok(lexed) => lexed,
            Err(diag) => {
                errors.push(diag.in_file(&file.name));
                files.push(ParsedFile {
                    name: file.name.clone(),
                    source: file.source.clone(),
                    private: false,
                    uses: Vec::new(),
                    decls: Vec::new(),
                    docs: crate::lexer::Docs::new(),
                });
                continue;
            }
        };
        let parsed = parser::parse_with_docs(tokens, docs);
        errors.extend(parsed.errors.into_iter().map(|d| d.in_file(&file.name)));
        files.push(ParsedFile {
            name: file.name.clone(),
            source: file.source.clone(),
            private: parsed.private,
            uses: parsed.uses,
            decls: parsed.decls,
            docs: parsed.docs,
        });
    }
    (files, errors)
}

/// The module a `use` path names, if it names one.
fn named_module(spec: &UseSpec, known: &HashSet<ModulePath>) -> Option<ModulePath> {
    Some(ModulePath(spec.path.clone())).filter(|path| known.contains(path))
}

/// The diagnostic for a `use` path that names no module: an item of a module, which is
/// reached through the module instead, or nothing.
fn unknown_use(spec: &UseSpec, known: &HashSet<ModulePath>) -> Diagnostic {
    let (item, module) = spec.path.split_last().expect("a use path has at least one segment");
    let module = ModulePath(module.to_vec());
    if known.contains(&module) {
        let name = module.0.last().expect("a module path has at least one segment");
        return Diagnostic::error(spec.span, format!("`use` names modules, not their items: use `{module}`, then write `{name}.{item}`"));
    }
    Diagnostic::error(spec.span, format!("no module `{}` in this project", spec.path.join(".")))
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
                let Some(target) = named_module(spec, known) else {
                    diags.push(unknown_use(spec, known).in_file(&file.name));
                    continue;
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

/// What a file's `use` bindings bring into scope. The bound modules' types are kept
/// under their qualified spelling (`sync.WaitGroup`) so they seed the symbol tables type
/// expressions resolve against; their methods travel with them.
#[derive(Default)]
struct Imports {
    structs: HashMap<String, TypeId>,
    unions: HashMap<String, TypeId>,
    aliases: HashMap<String, crate::types::Type>,
    methods: HashMap<TypeId, HashMap<String, Signature>>,
    /// Bound modules by local name (`io`, or `web` for `http as web`), each mapped to
    /// its interface for qualified access.
    modules: HashMap<String, Globals>,
}

impl Imports {
    /// This file's imported types overlaid on `base` — the module's own declarations —
    /// yielding the symbol table a file's declarations and bodies are checked against.
    /// Imported types join by qualified name; their methods merge under their identity.
    fn overlay(&self, base: &Globals) -> Globals {
        let mut globals = base.clone();
        let prelude = crate::stdlib::prelude();
        for (name, id) in prelude.unions {
            globals.unions.entry(name).or_insert(id);
        }
        for (name, id) in prelude.structs {
            globals.structs.entry(name).or_insert(id);
        }
        globals.structs.extend(self.structs.clone());
        globals.unions.extend(self.unions.clone());
        globals.aliases.extend(self.aliases.clone());
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
    let mut file_imports: Vec<Imports> = module
        .files
        .iter()
        .map(|file| build_file_imports(file, &declared, known, interfaces, &mut diags))
        .collect();
    // The methods of the built-in types come from their home modules.
    for home in crate::stdlib::HOMES {
        let Some(interface) = interfaces.get(&ModulePath::parse(home)).filter(|_| module.path.to_string() != *home)
        else {
            continue;
        };
        for imports in &mut file_imports {
            for (owner, methods) in &interface.methods {
                if owner.module == crate::stdlib::PRELUDE_MODULE {
                    imports.methods.entry(owner.clone()).or_default().extend(methods.clone());
                }
            }
        }
    }

    // Resolve in phases, each across every file, accumulating one module-wide table
    // of the module's own declarations: all type names first, so any file's types
    // and signatures may name a type declared later or in another file. Each file's
    // phase sees that file's imports, stripped back out before the next file so
    // they never leak across files.
    let module_name = module.path.to_string();
    let mut interface = Globals::default();
    interface.module = module_name.clone();
    interface.defs = defs.clone();
    crate::stdlib::provide(&mut interface);
    for file in &module.files {
        let mut file_diags = Vec::new();
        resolve::declare_types(&mut interface, &file.decls, file.private, &mut file_diags);
        diags.extend(file_diags.into_iter().map(|d| d.in_file(&file.name)));
    }
    let overlays: Vec<_> = file_imports.iter().map(|imports| (imports.overlay(&interface), &imports.modules)).collect();
    let consts: Vec<_> = (module.files.iter().enumerate())
        .flat_map(|(i, file)| file.decls.iter().filter(|d| matches!(d.kind, StmtKind::ConstDecl { .. })).map(move |d| (i, d)))
        .collect();
    let overlays: Vec<_> = overlays.iter().map(|(globals, modules)| (globals, *modules)).collect();
    let (consts, const_diags) = crate::consts::evaluate(&consts, &overlays);
    interface.consts = consts;
    diags.extend(const_diags.into_iter().map(|(i, d)| d.in_file(&module.files[i].name)));
    type Phase = fn(&mut Globals, &[Stmt], &mut Vec<Diagnostic>);
    let size_check: Phase = |globals, decls, diags| resolve::check_type_sizes(globals, decls, diags);
    let phases: [Phase; 5] = [
        resolve::resolve_known_aliases,
        resolve::resolve_aliases,
        resolve::define_types,
        size_check,
        resolve::resolve_signatures,
    ];
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
        let files: Vec<_> = module.files.iter().map(|file| (file.name.as_str(), file.source.as_str())).collect();
        let typed: Vec<_> = checked.iter().map(|(_, typed)| typed).collect();
        diags.extend(typecheck::settle_requirements(&files, &typed, &mut interface, interfaces.values()));
    }
    (diags, interface, checked)
}

/// The interface other modules see of a module whose declarations resolved into
/// `interface`: without the functions, variables, constants, and aliases its `#private` files
/// declare, and without their methods. The private files' types stay, abstract (see
/// [`TypeDefs::is_abstract_in`]).
fn exported(mut interface: Globals, files: &[ParsedFile]) -> Globals {
    for stmt in files.iter().filter(|file| file.private).flat_map(|file| &file.decls) {
        match &stmt.kind {
            StmtKind::FuncDecl(func) => match &func.receiver {
                None => {
                    interface.funcs.remove(&func.name);
                }
                Some(receiver) => {
                    if let Some(methods) = receiver_owner(&receiver.ty, &interface).and_then(|owner| interface.methods.get_mut(&owner)) {
                        methods.remove(&func.name);
                    }
                }
            },
            StmtKind::ExternFunc { name, .. } => {
                interface.funcs.remove(name);
            }
            StmtKind::VarDecl { name, .. } => {
                interface.vars.remove(name);
            }
            StmtKind::ConstDecl { name, .. } => {
                interface.consts.remove(name);
            }
            StmtKind::TypeAlias { name, .. } => {
                interface.aliases.remove(name);
            }
            _ => {}
        }
    }
    interface
}

/// The identity of the type a method with receiver type `ty` is declared on: a struct or
/// sum type of the module, through a pointer or type arguments, or a built-in type.
pub(crate) fn receiver_owner(ty: &TypeExpr, interface: &Globals) -> Option<TypeId> {
    match &ty.kind {
        TypeExprKind::Pointer(inner) => receiver_owner(inner, interface),
        TypeExprKind::Application { constructor, .. } => receiver_owner(constructor, interface),
        TypeExprKind::Named(name) => interface
            .structs
            .get(name)
            .or_else(|| interface.unions.get(name))
            .cloned()
            .or_else(|| crate::stdlib::builtin_receiver_owner(ty)),
        _ => crate::stdlib::builtin_receiver_owner(ty),
    }
}

/// The names a module declares across all its files: its top-level structs, sum
/// types, free functions, module-level variables, and constants. Methods are keyed by receiver,
/// not by a bare name, so they never collide with a module's name. Used to reject a `use`
/// whose module name would shadow one of the module's own declarations.
fn declared_names(files: &[ParsedFile]) -> HashSet<String> {
    let mut names = HashSet::new();
    for file in files {
        for stmt in &file.decls {
            match &stmt.kind {
                StmtKind::StructDecl { name, .. }
                | StmtKind::UnionDecl { name, .. }
                | StmtKind::VarDecl { name, .. }
                | StmtKind::ConstDecl { name, .. }
                | StmtKind::TypeAlias { name, .. }
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

/// Resolve one file's `use` bindings against its dependencies' interfaces. Each binds a
/// module under its last path segment or its alias, for qualified access only: its
/// types join the file's table as `name.Type`, which is how type expressions and patterns
/// spell them, and its interface is recorded for qualified expressions (`name.func()`).
/// A binding is rejected — reported at its own `use` — when its name is already bound in
/// this file or collides with one of `declared`, the module's own declaration names.
fn build_file_imports(
    file: &ParsedFile,
    declared: &HashSet<String>,
    known: &HashSet<ModulePath>,
    interfaces: &HashMap<ModulePath, Globals>,
    diags: &mut Vec<Diagnostic>,
) -> Imports {
    let mut imports = Imports::default();
    for spec in &file.uses {
        let Some(interface) = named_module(spec, known).and_then(|dep| interfaces.get(&dep)) else {
            continue;
        };
        let local = spec.local_name().to_string();
        if declared.contains(&local) {
            diags.push(
                Diagnostic::error(
                    spec.span,
                    format!("module name `{local}` collides with a declaration of the same name in this module; rename the module with `as`"),
                )
                .in_file(&file.name),
            );
            continue;
        }
        if imports.modules.contains_key(&local) {
            diags.push(
                Diagnostic::error(
                    spec.span,
                    format!("`{local}` is bound by more than one use in this file; rename one with `as`"),
                )
                .in_file(&file.name),
            );
            continue;
        }
        let qualified = |name: &String| format!("{local}.{name}");
        for (name, id) in &interface.structs {
            imports.structs.insert(qualified(name), id.clone());
        }
        for (name, id) in &interface.unions {
            imports.unions.insert(qualified(name), id.clone());
        }
        for (name, ty) in &interface.aliases {
            imports.aliases.insert(qualified(name), ty.clone());
        }
        for (owner, methods) in &interface.methods {
            if owner.module != crate::stdlib::PRELUDE_MODULE {
                imports.methods.insert(owner.clone(), methods.clone());
            }
        }
        imports.modules.insert(local, interface.clone());
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
    for name in imports.unions.keys() {
        globals.unions.remove(name);
    }
    for name in imports.aliases.keys() {
        globals.aliases.remove(name);
    }
    globals.structs.retain(|_, id| id.module != crate::stdlib::PRELUDE_MODULE);
    globals.unions.retain(|_, id| id.module != crate::stdlib::PRELUDE_MODULE);
    for owner in imports.methods.keys() {
        globals.methods.remove(owner);
    }
    globals
}
