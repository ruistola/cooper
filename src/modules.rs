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
//! *name binding*, bringing the item into scope for bare use. For now imports are
//! resolved at module granularity — all of a module's files share one import view;
//! per-file scoping is a later refinement that does not affect the interface or graph
//! machinery here.

use std::collections::{HashMap, HashSet};

use crate::ast::{Stmt, UseSpec};
use crate::diag::Diagnostic;
use crate::lexer;
use crate::parser;
use crate::project::{Module, ModulePath, Project};
use crate::resolve::{self, Globals};
use crate::{semantic, typecheck};

/// Analyze every module of `project`, returning all diagnostics. Modules are parsed
/// first; if any file fails to lex or parse the analysis stops there, since a
/// missing declaration would cascade into spurious cross-module errors. Otherwise
/// the dependency graph is ordered and each module resolved and checked in turn.
pub fn analyze(project: &Project) -> Vec<Diagnostic> {
    let parsed = match parse_modules(project) {
        Ok(parsed) => parsed,
        Err(errors) => return errors,
    };

    let known: HashSet<ModulePath> = parsed.iter().map(|m| m.path.clone()).collect();

    let (edges, mut diags) = build_edges(&parsed, &known);
    let order = match topological_order(&parsed, &edges) {
        Ok(order) => order,
        Err(cycle_diags) => {
            diags.extend(cycle_diags);
            return diags;
        }
    };

    let mut interfaces: HashMap<ModulePath, Globals> = HashMap::new();
    for index in order {
        let module = &parsed[index];
        let (module_diags, interface) = analyze_module(module, &known, &interfaces);
        diags.extend(module_diags);
        interfaces.insert(module.path.clone(), interface);
    }
    diags
}

/// A module whose files have been parsed and their declarations united. `uses`
/// gathers every file's flattened `use` bindings (module-granularity for now).
struct ParsedModule {
    path: ModulePath,
    uses: Vec<UseSpec>,
    decls: Vec<Stmt>,
}

/// Parse every file of every module. Returns the parsed modules, or every lex and
/// parse diagnostic if any file was malformed.
fn parse_modules(project: &Project) -> Result<Vec<ParsedModule>, Vec<Diagnostic>> {
    let mut parsed = Vec::with_capacity(project.modules.len());
    let mut errors = Vec::new();
    for module in &project.modules {
        let (uses, decls, module_errors) = parse_module_files(module);
        errors.extend(module_errors);
        parsed.push(ParsedModule {
            path: module.path.clone(),
            uses,
            decls,
        });
    }
    if errors.is_empty() {
        Ok(parsed)
    } else {
        Err(errors)
    }
}

/// Lex and parse each source file of `module`, uniting all files' `use` bindings and
/// top-level declarations.
fn parse_module_files(module: &Module) -> (Vec<UseSpec>, Vec<Stmt>, Vec<Diagnostic>) {
    let mut uses = Vec::new();
    let mut decls = Vec::new();
    let mut errors = Vec::new();
    for file in &module.files {
        let tokens = match lexer::tokenize(&file.source) {
            Ok(tokens) => tokens,
            Err(diag) => {
                errors.push(diag);
                continue;
            }
        };
        let parsed = parser::parse(tokens);
        errors.extend(parsed.errors);
        uses.extend(parsed.uses);
        decls.extend(parsed.decls);
    }
    (uses, decls, errors)
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

/// Build the dependency edges (module index -> depended-on module indices) from every
/// module's `use` bindings, reporting any binding that names no known module.
fn build_edges(
    parsed: &[ParsedModule],
    known: &HashSet<ModulePath>,
) -> (Vec<HashSet<usize>>, Vec<Diagnostic>) {
    let index_of: HashMap<&ModulePath, usize> =
        parsed.iter().enumerate().map(|(i, m)| (&m.path, i)).collect();
    let mut edges = vec![HashSet::new(); parsed.len()];
    let mut diags = Vec::new();
    for (i, module) in parsed.iter().enumerate() {
        for spec in &module.uses {
            let target = match classify(spec, known) {
                Some(UseTarget::Module(path)) | Some(UseTarget::Name { module: path, .. }) => path,
                None => {
                    diags.push(Diagnostic::error(
                        spec.span,
                        format!("no module `{}` in this project", spec.path.join(".")),
                    ));
                    continue;
                }
            };
            if let Some(&j) = index_of.get(&target) {
                if j != i {
                    edges[i].insert(j);
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
    edges: &[HashSet<usize>],
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
        edges: &[HashSet<usize>],
        marks: &mut [Mark],
        order: &mut Vec<usize>,
    ) -> Result<(), Vec<Diagnostic>> {
        match marks[node] {
            Mark::Done => return Ok(()),
            Mark::InProgress => {
                return Err(vec![Diagnostic::error(
                    crate::diag::Span::new(0, 0),
                    format!(
                        "module `{}` is part of a dependency cycle",
                        parsed[node].path
                    ),
                )]);
            }
            Mark::Unvisited => {}
        }
        marks[node] = Mark::InProgress;
        for &next in &edges[node] {
            visit(next, parsed, edges, marks, order)?;
        }
        marks[node] = Mark::Done;
        order.push(node);
        Ok(())
    }

    for node in 0..parsed.len() {
        visit(node, parsed, edges, &mut marks, &mut order)?;
    }
    Ok(order)
}

/// The imported names a module brings into scope, keyed by local surface name (its
/// alias, or the item's own name). Types and functions are kept apart so they seed
/// the right symbol tables; an imported struct's methods travel with it.
#[derive(Default)]
struct Imports {
    structs: HashMap<String, crate::types::Type>,
    oneofs: HashMap<String, crate::types::Type>,
    funcs: HashMap<String, crate::types::Type>,
    methods: HashMap<String, HashMap<String, crate::types::Type>>,
    /// Module bindings keyed by local spelling (`["std", "io"]`, or `["web"]` when
    /// aliased), each mapped to that module's interface for qualified access.
    modules: HashMap<Vec<String>, Globals>,
    local_names: HashSet<String>,
}

impl Imports {
    /// A symbol table seeded with these imports, into which a module's own
    /// declarations are then resolved.
    fn seed(&self) -> Globals {
        Globals {
            structs: self.structs.clone(),
            oneofs: self.oneofs.clone(),
            funcs: self.funcs.clone(),
            methods: self.methods.clone(),
        }
    }
}

/// Resolve, type-check, and semantically analyze one module against its dependencies'
/// interfaces, returning its diagnostics and its own exported interface for later
/// modules. Diagnostics short-circuit per phase so failures do not cascade within a
/// module.
fn analyze_module(
    module: &ParsedModule,
    known: &HashSet<ModulePath>,
    interfaces: &HashMap<ModulePath, Globals>,
) -> (Vec<Diagnostic>, Globals) {
    let mut diags = Vec::new();
    let imports = build_imports(module, known, interfaces, &mut diags);

    let (resolved, resolve_diags) = resolve::resolve_into(imports.seed(), &module.decls);
    diags.extend(resolve_diags);

    let interface = strip_imports(resolved.clone(), &imports);

    if diags.is_empty() {
        let type_diags = typecheck::check(&module.decls, &resolved, &imports.modules);
        if type_diags.is_empty() {
            diags.extend(semantic::analyze(&module.decls, &resolved));
        } else {
            diags.extend(type_diags);
        }
    }
    (diags, interface)
}

/// Resolve a module's `use` bindings against its dependencies' interfaces, collecting
/// the imported symbols. Name bindings seed bare names; module bindings record the
/// dependency's interface under its local spelling for qualified access. Bindings to
/// an unknown item, or two bindings claiming the same local name, are reported.
fn build_imports(
    module: &ParsedModule,
    known: &HashSet<ModulePath>,
    interfaces: &HashMap<ModulePath, Globals>,
    diags: &mut Vec<Diagnostic>,
) -> Imports {
    let mut imports = Imports::default();
    for spec in &module.uses {
        match classify(spec, known) {
            Some(UseTarget::Module(dep)) => {
                let Some(interface) = interfaces.get(&dep) else {
                    continue;
                };
                // A module is spelled by its full local path, or by its alias alone.
                let spelling = match &spec.alias {
                    Some(alias) => vec![alias.clone()],
                    None => spec.path.clone(),
                };
                let local = spec.local_name().to_string();
                if !imports.local_names.insert(local.clone()) {
                    diags.push(Diagnostic::error(
                        spec.span,
                        format!("`{local}` is bound by more than one use in this file"),
                    ));
                    continue;
                }
                imports.modules.insert(spelling, interface.clone());
            }
            Some(UseTarget::Name { module: dep, item }) => {
                let Some(interface) = interfaces.get(&dep) else {
                    continue;
                };
                let local = spec.local_name().to_string();
                if !imports.local_names.insert(local.clone()) {
                    diags.push(Diagnostic::error(
                        spec.span,
                        format!("`{local}` is bound by more than one use in this file"),
                    ));
                    continue;
                }
                if let Some(ty) = interface.lookup_struct(&item) {
                    imports.structs.insert(local, ty.clone());
                    if let Some(methods) = interface.methods.get(&item) {
                        imports.methods.insert(item.clone(), methods.clone());
                    }
                } else if let Some(ty) = interface.lookup_oneof(&item) {
                    imports.oneofs.insert(local, ty.clone());
                } else if let Some(ty) = interface.lookup_func(&item) {
                    imports.funcs.insert(local, ty.clone());
                } else {
                    diags.push(Diagnostic::error(
                        spec.span,
                        format!("module `{dep}` exports no item named `{item}`"),
                    ));
                }
            }
            None => {}
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
