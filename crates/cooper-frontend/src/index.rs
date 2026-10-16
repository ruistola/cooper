//! A position index over a checked project, for tools: what a name at a source position
//! refers to, where it is declared, and what its documentation says.
//!
//! It answers from what the checker already records: the declarations of every file, and the
//! function, method, and module-level variable references it resolved. Local variables,
//! parameters, field accesses, and the names inside type annotations are not indexed yet.

use std::collections::HashMap;

use crate::ast::{Stmt, StmtKind};
use crate::diag::Span;
use crate::modules::{CheckedFile, CheckedProject};
use crate::resolve::{underlying_struct_name, Callable};

/// What kind of thing a declaration declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Function,
    Method,
    Extern,
    Struct,
    Oneof,
    Alias,
    Variable,
    Field,
    Variant,
}

/// A declaration: where its name is, what it is called, and what documents it.
#[derive(Debug, Clone)]
pub struct Declaration {
    pub name: String,
    pub kind: DeclKind,
    pub module: String,
    pub file: String,
    /// The span of the name in the declaration.
    pub name_span: Span,
    /// The span of the whole declaration.
    pub span: Span,
    /// The documentation comment above it, if any.
    pub doc: Option<String>,
    /// Its type, for a function, method, variable, or alias.
    pub ty: Option<String>,
    /// The struct or sum type a field, variant, or method belongs to.
    pub owner: Option<String>,
}

/// What a declaration is looked up by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Func(String, String),
    Method(String, String, String),
    Extern(String),
    Var(String, String),
}

/// The index of a checked project.
pub struct Index<'p> {
    project: &'p CheckedProject,
    declarations: Vec<Declaration>,
    by_key: HashMap<Key, usize>,
}

impl<'p> Index<'p> {
    pub fn new(project: &'p CheckedProject) -> Self {
        let mut index = Index { project, declarations: Vec::new(), by_key: HashMap::new() };
        for file in &project.files {
            index.add_file(file);
        }
        index
    }

    /// Every declaration of the project's files, in source order within each file.
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    /// The declaration the name at byte `offset` of `file` is, or refers to.
    pub fn definition_at(&self, file: &str, offset: usize) -> Option<&Declaration> {
        if let Some(declaration) = self.declarations.iter().find(|d| d.file == file && contains(d.name_span, offset)) {
            return Some(declaration);
        }
        let checked = self.project.files.iter().find(|f| f.name == file)?;
        // A reference's span ends with the name it uses (`strings.join`).
        let named = |span: Span, name: &str| {
            let start = span.end.saturating_sub(name.len()).max(span.start);
            contains(Span::new(start, span.end), offset)
        };
        for (span, reference) in &checked.typed.refs {
            let key = match &reference.item {
                Callable::Func { module, name } => Some((Key::Func(module.clone(), name.clone()), name)),
                Callable::Method { receiver, name } => {
                    Some((Key::Method(receiver.module.clone(), receiver.name.clone(), name.clone()), name))
                }
                Callable::Extern { name } => Some((Key::Extern(name.clone()), name)),
                _ => None,
            };
            if let Some((key, name)) = key {
                if named(*span, name) {
                    return self.by_key.get(&key).map(|&i| &self.declarations[i]);
                }
            }
        }
        for (span, var) in &checked.typed.variables {
            if named(*span, &var.name) {
                return self.by_key.get(&Key::Var(var.module.clone(), var.name.clone())).map(|&i| &self.declarations[i]);
            }
        }
        None
    }

    fn add_file(&mut self, file: &CheckedFile) {
        let module = file.module.to_string();
        for stmt in &file.decls {
            self.add_stmt(file, &module, stmt);
        }
    }

    fn add(&mut self, key: Option<Key>, declaration: Declaration) {
        if let Some(key) = key {
            self.by_key.insert(key, self.declarations.len());
        }
        self.declarations.push(declaration);
    }

    fn add_stmt(&mut self, file: &CheckedFile, module: &str, stmt: &Stmt) {
        let doc = |start: usize| file.docs.get(&start).cloned();
        let make = |name: &str, kind, name_span, span: Span, ty: Option<String>, owner: Option<String>| Declaration {
            name: name.to_string(),
            kind,
            module: module.to_string(),
            file: file.name.clone(),
            name_span,
            span,
            doc: doc(span.start),
            ty,
            owner,
        };
        match &stmt.kind {
            StmtKind::FuncDecl(func) => {
                let (kind, key, ty, owner) = match &func.receiver {
                    None => (
                        DeclKind::Function,
                        Key::Func(module.to_string(), func.name.clone()),
                        file.globals.lookup_func(&func.name).map(|s| s.ty.to_string()),
                        None,
                    ),
                    Some(receiver) => {
                        let owner = underlying_struct_name(&receiver.ty).unwrap_or("").to_string();
                        // A built-in type's methods are keyed under the prelude.
                        let home = if crate::stdlib::HOMES.iter().any(|(t, _)| *t == owner) {
                            crate::stdlib::PRELUDE_MODULE.to_string()
                        } else {
                            module.to_string()
                        };
                        let ty = file
                            .globals
                            .methods
                            .iter()
                            .filter(|(id, _)| id.name == owner && id.module == home)
                            .find_map(|(_, methods)| methods.get(&func.name))
                            .map(|s| s.ty.to_string());
                        (DeclKind::Method, Key::Method(home, owner.clone(), func.name.clone()), ty, Some(owner))
                    }
                };
                self.add(Some(key), make(&func.name, kind, func.name_span, stmt.span, ty, owner));
            }
            StmtKind::ExternFunc { name, name_span, .. } => {
                let ty = file.globals.lookup_func(name).map(|s| s.ty.to_string());
                self.add(Some(Key::Extern(name.clone())), make(name, DeclKind::Extern, *name_span, stmt.span, ty, None));
            }
            StmtKind::VarDecl { name, name_span, .. } => {
                let ty = file.globals.vars.get(name).map(|v| v.ty.to_string());
                self.add(
                    Some(Key::Var(module.to_string(), name.clone())),
                    make(name, DeclKind::Variable, *name_span, stmt.span, ty, None),
                );
            }
            StmtKind::TypeAlias { name, name_span, .. } => {
                let ty = file.globals.aliases.get(name).map(|t| t.to_string());
                self.add(None, make(name, DeclKind::Alias, *name_span, stmt.span, ty, None));
            }
            StmtKind::StructDecl { name, name_span, members, .. } => {
                self.add(None, make(name, DeclKind::Struct, *name_span, stmt.span, None, None));
                let id = crate::types::TypeId { module: module.to_string(), name: name.clone() };
                for member in members {
                    let ty = file
                        .globals
                        .defs
                        .structs
                        .get(&id)
                        .and_then(|def| def.members.iter().find(|(n, _)| *n == member.name))
                        .map(|(_, ty)| ty.to_string());
                    self.add(None, make(&member.name, DeclKind::Field, member.name_span, member.span, ty, Some(name.clone())));
                }
            }
            StmtKind::OneofDecl { name, name_span, variants, .. } => {
                self.add(None, make(name, DeclKind::Oneof, *name_span, stmt.span, None, None));
                for variant in variants {
                    self.add(None, make(&variant.name, DeclKind::Variant, variant.name_span, variant.span, None, Some(name.clone())));
                }
            }
            _ => {}
        }
    }
}

fn contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset < span.end
}
