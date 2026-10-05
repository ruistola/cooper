//! Declaration resolution: the first semantic pass.
//!
//! This pass collects every top-level struct, sum type, function, method, and
//! variable into a global symbol table ([`Globals`]) and validates each declaration's
//! signature in isolation — duplicate names, duplicate members or variants, undefined
//! types in signatures, generic arity, value-recursive (infinitely sized) types, and
//! receiver/method well-formedness. Function *bodies* are not walked here; the type
//! checker does that against the finished [`Globals`].
//!
//! Resolution is order-insensitive. It runs in phases over a module's files:
//! [`declare_types`] registers every struct and sum type name first, so
//! [`define_types`] can resolve members and variants that refer to any type of the
//! module — forward, mutually, or self-referentially. [`check_type_sizes`] then
//! rejects a type containing itself by value, and [`resolve_signatures`] resolves
//! functions, methods, and variables against the complete type table.

use std::collections::{HashMap, HashSet};

use crate::ast::{FuncDecl, Stmt, StmtKind, TypeExpr, TypeExprKind, TypedIdent, VariantDef};
use crate::diag::{Diagnostic, Span};
use crate::types::{is_primitive_name, OneofDef, StructDef, Type, TypeDefs, TypeId};

/// The module-wide symbol table produced by resolution. Structs, sum types, and
/// functions are global (only variables are block-scoped), so later passes read
/// declarations directly from here rather than from a scope tree.
#[derive(Debug, Default, Clone)]
pub struct Globals {
    /// Struct names in scope, by local spelling, to their identities.
    pub structs: HashMap<String, TypeId>,
    /// Sum type names in scope, by local spelling, to their identities.
    pub oneofs: HashMap<String, TypeId>,
    /// The definitions of every type reachable from this table, including those
    /// only reachable through another type's members.
    pub defs: TypeDefs,
    pub funcs: HashMap<String, Type>,
    /// Receiver struct identity -> method name -> bound signature (receiver excluded).
    pub methods: HashMap<TypeId, HashMap<String, Type>>,
    /// Module-level variable names. Top-level `let` bindings occupy the module
    /// namespace alongside structs, sum types, and functions; unlike a block-scoped
    /// `let` — an ordered sequence where a re-binding shadows its predecessor — the
    /// module namespace is an unordered union across a module's files, so a repeated
    /// name is a redeclaration, never a shadow. Only the name is recorded: a
    /// binding's type is inferred later and top-level variables are not yet consumed
    /// as values by downstream passes, so this table exists purely to enforce the
    /// namespace rule.
    pub vars: HashSet<String>,
    /// Types registered by [`declare_types`] whose bodies [`define_types`] has not yet
    /// resolved. A redeclared name is never registered, so its body is skipped.
    pending: HashSet<TypeId>,
}

impl Globals {
    /// The struct named `name` in scope, as its type (a generic struct's template).
    pub fn lookup_struct(&self, name: &str) -> Option<Type> {
        self.structs.get(name).map(|id| Type::Struct {
            id: id.clone(),
            type_args: Vec::new(),
        })
    }

    /// The sum type named `name` in scope, as its type (a generic one's template).
    pub fn lookup_oneof(&self, name: &str) -> Option<Type> {
        self.oneofs.get(name).map(|id| Type::Oneof {
            id: id.clone(),
            type_args: Vec::new(),
        })
    }

    pub fn lookup_func(&self, name: &str) -> Option<&Type> {
        self.funcs.get(name)
    }

    pub fn lookup_method(&self, recv: &TypeId, name: &str) -> Option<&Type> {
        self.methods.get(recv).and_then(|m| m.get(name))
    }

    fn type_name_taken(&self, name: &str) -> bool {
        self.structs.contains_key(name) || self.oneofs.contains_key(name)
    }
}

/// Resolve one file's declarations — every phase in turn — into a symbol table
/// already seeded with imported names, minting identities under `module`. A
/// multi-file module runs the phases itself, each across all its files.
pub fn resolve_into(mut globals: Globals, module: &str, decls: &[Stmt]) -> (Globals, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    declare_types(&mut globals, module, decls, &mut diags);
    define_types(&mut globals, decls, &mut diags);
    check_type_sizes(&globals, decls, &mut diags);
    resolve_signatures(&mut globals, decls, &mut diags);
    (globals, diags)
}

/// Phase one: register every struct and sum type `decls` declares, under an
/// identity in `module`, with its type parameters but no body yet. A name already
/// taken by a type is a redeclaration. Seeded imports never collide here: an import
/// shadowing one of the module's own declarations is rejected at its `use`.
pub fn declare_types(globals: &mut Globals, module: &str, decls: &[Stmt], diags: &mut Vec<Diagnostic>) {
    for stmt in decls {
        let (name, type_params, is_struct) = match &stmt.kind {
            StmtKind::StructDecl { name, type_params, .. } => (name, type_params, true),
            StmtKind::OneofDecl { name, type_params, .. } => (name, type_params, false),
            _ => continue,
        };
        if globals.type_name_taken(name) {
            diags.push(Diagnostic::error(
                stmt.span,
                format!("redeclared type {name} in the same scope"),
            ));
            continue;
        }
        let id = TypeId {
            module: module.to_string(),
            name: name.clone(),
        };
        let type_params = type_params.clone();
        if is_struct {
            globals.structs.insert(name.clone(), id.clone());
            globals.defs.structs.insert(id.clone(), StructDef { type_params, ..Default::default() });
        } else {
            globals.oneofs.insert(name.clone(), id.clone());
            globals.defs.oneofs.insert(id.clone(), OneofDef { type_params, ..Default::default() });
        }
        globals.pending.insert(id);
    }
}

/// Phase two: resolve the members and variants of the types `decls` declares. Every
/// type of the module is already registered, so a body may name any of them.
pub fn define_types(globals: &mut Globals, decls: &[Stmt], diags: &mut Vec<Diagnostic>) {
    for stmt in decls {
        match &stmt.kind {
            StmtKind::StructDecl {
                name,
                type_params,
                members,
            } => define_struct(globals, diags, name, type_params, members, stmt.span),
            StmtKind::OneofDecl {
                name,
                type_params,
                variants,
            } => define_oneof(globals, diags, name, type_params, variants, stmt.span),
            _ => {}
        }
    }
}

/// Phase four: resolve the functions, methods, and module-level variables `decls`
/// declares, against the complete type table.
pub fn resolve_signatures(globals: &mut Globals, decls: &[Stmt], diags: &mut Vec<Diagnostic>) {
    for stmt in decls {
        match &stmt.kind {
            StmtKind::FuncDecl(func) => resolve_func_decl(globals, diags, func),
            StmtKind::VarDecl { name, .. } => resolve_var_decl(globals, diags, name, stmt.span),
            _ => {}
        }
    }
}

/// Phase three: reject every type `decls` declares that contains itself by value —
/// through members, tuple elements, or variant payloads, possibly via other types
/// or type arguments — since such a value would be infinitely large. Recursion is
/// legal only through a pointer or an array, which store their target out of line.
pub fn check_type_sizes(globals: &Globals, decls: &[Stmt], diags: &mut Vec<Diagnostic>) {
    for stmt in decls {
        let ty = match &stmt.kind {
            StmtKind::StructDecl { name, .. } => globals.lookup_struct(name),
            StmtKind::OneofDecl { name, .. } => globals.lookup_oneof(name),
            _ => continue,
        };
        let Some(ty) = ty else { continue };
        let mut path = Vec::new();
        if contains_by_value(&ty, &ty, &globals.defs, &mut path) {
            // Name the type as written at a use site: a generic one applied to its own
            // parameters, parenthesised since juxtaposition binds looser than `^`/`[]`.
            let params = globals.defs.type_params(&ty);
            let spelled = if params.is_empty() {
                ty.to_string()
            } else {
                format!("({ty} {})", params.join(" "))
            };
            diags.push(Diagnostic::error(
                stmt.span,
                format!(
                    "type {ty} contains itself by value and would be infinitely large; \
                     recurse through a pointer `{spelled}^` or an array `{spelled}[]`"
                ),
            ));
        }
    }
}

/// How deep [`contains_by_value`] follows nested instantiations before treating the
/// nesting as unbounded: a generic type instantiating itself by value with ever
/// larger arguments (`struct S T { s: S (T, T) }`) never repeats an instantiation.
const MAX_VALUE_NESTING: usize = 64;

/// Whether `ty`'s by-value layout reaches the declaration `root` names. `path`
/// holds the instantiations being expanded, so an unrelated cycle elsewhere ends the
/// walk rather than looping (it is reported at its own declaration).
fn contains_by_value(ty: &Type, root: &Type, defs: &TypeDefs, path: &mut Vec<Type>) -> bool {
    let components: Vec<Type> = match ty {
        Type::Tuple(elems) => elems.clone(),
        Type::Struct { .. } => defs.struct_members(ty).unwrap_or_default().into_values().collect(),
        Type::Oneof { .. } => defs
            .variant_order(ty)
            .iter()
            .flat_map(|v| defs.payload(ty, v).unwrap_or_default())
            .collect(),
        _ => return false,
    };
    if path.iter().any(|seen| seen.equals(ty)) || path.len() > MAX_VALUE_NESTING {
        return path.len() > MAX_VALUE_NESTING;
    }
    path.push(ty.clone());
    let found = components.iter().any(|c| {
        same_declaration(c, root) || contains_by_value(c, root, defs, path)
    });
    path.pop();
    found
}

/// Whether two struct or sum types name the same declaration, whatever their
/// type arguments.
fn same_declaration(a: &Type, b: &Type) -> bool {
    match (a, b) {
        (Type::Struct { id: x, .. }, Type::Struct { id: y, .. })
        | (Type::Oneof { id: x, .. }, Type::Oneof { id: y, .. }) => x == y,
        _ => false,
    }
}

/// Convert an AST type expression to a concrete [`Type`], recording a diagnostic
/// and returning `None` on any undefined type or generic-arity error. A named type
/// matching one of `type_params` resolves to a [`Type::TypeParam`] rather than a
/// concrete declaration. Shared by this pass (for signatures) and the type checker
/// (for local annotations).
pub fn resolve_type(
    type_expr: &TypeExpr,
    type_params: &HashSet<String>,
    globals: &Globals,
    diags: &mut Vec<Diagnostic>,
) -> Option<Type> {
    match &type_expr.kind {
        TypeExprKind::Named(name) => {
            if type_params.contains(name) {
                return Some(Type::TypeParam(name.clone()));
            }
            if is_primitive_name(name) {
                return Some(Type::Primitive(name.clone()));
            }
            if let Some(ty) = globals.lookup_struct(name).or_else(|| globals.lookup_oneof(name)) {
                let arity = globals.defs.type_params(&ty).len();
                if arity > 0 {
                    diags.push(Diagnostic::error(
                        type_expr.span,
                        format!("generic type {name} requires {arity} type argument(s)"),
                    ));
                    return None;
                }
                return Some(ty);
            }
            diags.push(Diagnostic::error(
                type_expr.span,
                format!("undefined type: {name}"),
            ));
            None
        }
        TypeExprKind::Application { constructor, args } => {
            resolve_type_application(constructor, args, type_expr.span, type_params, globals, diags)
        }
        TypeExprKind::Array(elem) => Some(Type::Array(Box::new(resolve_type(
            elem,
            type_params,
            globals,
            diags,
        )?))),
        TypeExprKind::Pointer(elem) => Some(Type::Pointer(Box::new(resolve_type(
            elem,
            type_params,
            globals,
            diags,
        )?))),
        TypeExprKind::Tuple(elems) => {
            let mut resolved = Vec::with_capacity(elems.len());
            for e in elems {
                resolved.push(resolve_type(e, type_params, globals, diags)?);
            }
            Some(Type::Tuple(resolved))
        }
        TypeExprKind::Unit => Some(Type::Unit),
        TypeExprKind::Func { params, ret } => {
            let mut param_types = Vec::with_capacity(params.len());
            for p in params {
                param_types.push(resolve_type(p, type_params, globals, diags)?);
            }
            let ret = resolve_type(ret, type_params, globals, diags)?;
            Some(Type::Func {
                return_type: Box::new(ret),
                param_types,
            })
        }
    }
}

/// Resolve a juxtaposition type application such as `Box i32` or `Map string i32`
/// into an instantiation of the named generic declaration.
fn resolve_type_application(
    constructor: &TypeExpr,
    args: &[TypeExpr],
    span: Span,
    type_params: &HashSet<String>,
    globals: &Globals,
    diags: &mut Vec<Diagnostic>,
) -> Option<Type> {
    let TypeExprKind::Named(name) = &constructor.kind else {
        diags.push(Diagnostic::error(
            span,
            "type application requires a named type constructor",
        ));
        return None;
    };

    let Some(template) = globals.lookup_struct(name).or_else(|| globals.lookup_oneof(name)) else {
        diags.push(Diagnostic::error(
            span,
            format!("undefined generic type: {name}"),
        ));
        return None;
    };

    let arity = globals.defs.type_params(&template).len();
    if arity == 0 {
        diags.push(Diagnostic::error(
            span,
            format!("type {name} is not generic and takes no type arguments"),
        ));
        return None;
    }
    if args.len() != arity {
        diags.push(Diagnostic::error(
            span,
            format!(
                "generic type {name} expects {arity} type argument(s), got {}",
                args.len()
            ),
        ));
        return None;
    }

    let mut arg_types = Vec::with_capacity(args.len());
    for arg in args {
        arg_types.push(resolve_type(arg, type_params, globals, diags)?);
    }
    Some(match template {
        Type::Struct { id, .. } => Type::Struct { id, type_args: arg_types },
        Type::Oneof { id, .. } => Type::Oneof { id, type_args: arg_types },
        other => other,
    })
}

/// The identity `name` was registered under by [`declare_types`], claimed for body
/// resolution. `None` for a redeclaration, whose body is not resolved.
fn claim_pending(globals: &mut Globals, id: Option<TypeId>) -> Option<TypeId> {
    let id = id?;
    globals.pending.remove(&id).then_some(id)
}

fn define_struct(
    globals: &mut Globals,
    diags: &mut Vec<Diagnostic>,
    name: &str,
    type_params: &[String],
    members: &[TypedIdent],
    span: Span,
) {
    let Some(id) = claim_pending(globals, globals.structs.get(name).cloned()) else {
        return;
    };
    check_type_param_binders(type_params, span, globals, diags);
    let params: HashSet<String> = type_params.iter().cloned().collect();
    let mut resolved = HashMap::new();
    for member in members {
        if resolved.contains_key(&member.name) {
            diags.push(Diagnostic::error(
                member.span,
                format!("duplicate member {} in struct {name}", member.name),
            ));
            continue;
        }
        if let Some(t) = resolve_type(&member.ty, &params, globals, diags) {
            resolved.insert(member.name.clone(), t);
        }
    }
    if let Some(def) = globals.defs.structs.get_mut(&id) {
        def.members = resolved;
    }
}

fn define_oneof(
    globals: &mut Globals,
    diags: &mut Vec<Diagnostic>,
    name: &str,
    type_params: &[String],
    variants: &[VariantDef],
    span: Span,
) {
    let Some(id) = claim_pending(globals, globals.oneofs.get(name).cloned()) else {
        return;
    };
    check_type_param_binders(type_params, span, globals, diags);
    let params: HashSet<String> = type_params.iter().cloned().collect();
    let mut resolved: HashMap<String, Vec<Type>> = HashMap::new();
    let mut order = Vec::new();
    for variant in variants {
        if resolved.contains_key(&variant.name) {
            diags.push(Diagnostic::error(
                variant.span,
                format!("duplicate variant {} in sum type {name}", variant.name),
            ));
            continue;
        }
        let mut slots = Vec::new();
        for slot_expr in &variant.payload {
            if let Some(t) = resolve_type(slot_expr, &params, globals, diags) {
                slots.push(t);
            }
        }
        resolved.insert(variant.name.clone(), slots);
        order.push(variant.name.clone());
    }
    if let Some(def) = globals.defs.oneofs.get_mut(&id) {
        def.variants = resolved;
        def.variant_order = order;
    }
}

/// Record a module-level variable declaration in the global namespace. A top-level
/// `let` shares the module namespace with structs, sum types, and functions across
/// all of a module's files, so a name already claimed there — by an earlier variable,
/// a function, a struct, or a sum type — is a redeclaration, not a shadow. Block-local
/// shadowing does not apply here: that is an ordered-sequence rule, whereas the module
/// namespace is an unordered union.
fn resolve_var_decl(globals: &mut Globals, diags: &mut Vec<Diagnostic>, name: &str, span: Span) {
    if globals.vars.contains(name)
        || globals.funcs.contains_key(name)
        || globals.structs.contains_key(name)
        || globals.oneofs.contains_key(name)
    {
        diags.push(Diagnostic::error(
            span,
            format!("redeclared name {name} in the same scope"),
        ));
        return;
    }
    globals.vars.insert(name.to_string());
}

fn resolve_func_decl(globals: &mut Globals, diags: &mut Vec<Diagnostic>, func: &FuncDecl) {
    // The declaration's binders — its own type parameters plus any bound by a
    // generic receiver pattern — are in scope for the receiver, parameter, and
    // return types. Each must be a fresh name (checked together so a receiver and
    // method-local binder cannot clash either).
    let mut binders = func.type_params.clone();
    if let Some(receiver) = &func.receiver {
        binders.extend(receiver_pattern_params(&receiver.ty));
    }
    check_type_param_binders(&binders, func.span, globals, diags);
    let params: HashSet<String> = binders.into_iter().collect();

    let return_type = match &func.return_type {
        Some(rt) => match resolve_type(rt, &params, globals, diags) {
            Some(t) => t,
            None => return,
        },
        None => Type::Unit,
    };

    let mut param_types = Vec::with_capacity(func.params.len());
    for param in &func.params {
        match resolve_type(&param.ty, &params, globals, diags) {
            Some(t) => param_types.push(t),
            None => return,
        }
    }

    // The bound signature excludes the receiver: it is captured as the method's
    // environment, so `product.isAffordable` has a plain function type.
    let func_type = Type::Func {
        return_type: Box::new(return_type),
        param_types,
    };

    let Some(receiver) = &func.receiver else {
        if globals.funcs.contains_key(&func.name)
            || globals.vars.contains(&func.name)
            || globals.type_name_taken(&func.name)
        {
            diags.push(Diagnostic::error(
                func.span,
                format!("redeclared function {} in the same scope", func.name),
            ));
            return;
        }
        globals.funcs.insert(func.name.clone(), func_type);
        return;
    };

    let recv_type = match resolve_type(&receiver.ty, &params, globals, diags) {
        Some(t) => t,
        None => return,
    };
    let Some(Type::Struct { id, .. }) = underlying_struct(&recv_type) else {
        diags.push(Diagnostic::error(
            receiver.span,
            format!(
                "method receiver {} must be a struct or pointer-to-struct type, found {recv_type}",
                receiver.name
            ),
        ));
        return;
    };
    let id = id.clone();
    let struct_name = &id.name;
    if globals
        .defs
        .structs
        .get(&id)
        .is_some_and(|def| def.members.contains_key(&func.name))
    {
        diags.push(Diagnostic::error(
            func.span,
            format!("method {} collides with a field of struct {struct_name}", func.name),
        ));
        return;
    }
    if globals.lookup_method(&id, &func.name).is_some() {
        diags.push(Diagnostic::error(
            func.span,
            format!("redeclared method {} on struct {struct_name}", func.name),
        ));
        return;
    }
    globals
        .methods
        .entry(id)
        .or_default()
        .insert(func.name.clone(), func_type);
}

/// The type-parameter names a method receiver binds through the receiver-pattern
/// rule: the bare-identifier arguments of a generic receiver such as `(m: Map K
/// V)`. A pointer receiver is unwrapped first; a non-generic receiver binds none.
pub fn receiver_pattern_params(type_expr: &TypeExpr) -> Vec<String> {
    let inner = match &type_expr.kind {
        TypeExprKind::Pointer(inner) => inner.as_ref(),
        _ => type_expr,
    };
    match &inner.kind {
        TypeExprKind::Application { args, .. } => args
            .iter()
            .filter_map(|arg| match &arg.kind {
                TypeExprKind::Named(name) => Some(name.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Validate a declaration's type-parameter binders: each must introduce a *fresh*
/// name — distinct from its siblings and from any type already in scope (a primitive
/// or any declared type, wherever it is declared). This keeps every name in a signature unambiguously
/// either a type parameter or a concrete type, never both, so the distinction rests
/// on the binder rather than on identifier casing. A receiver pattern's slots are
/// binders too, so `(m: Map string i32)` — which would otherwise bind phantom
/// parameters named after concrete types — is rejected here. Binders carry no
/// individual span, so diagnostics point at the declaration (or receiver).
fn check_type_param_binders(
    names: &[String],
    span: Span,
    globals: &Globals,
    diags: &mut Vec<Diagnostic>,
) {
    let mut seen: HashSet<&str> = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            diags.push(Diagnostic::error(
                span,
                format!("duplicate type parameter {name}"),
            ));
            continue;
        }
        if is_primitive_name(name) || globals.type_name_taken(name) {
            diags.push(Diagnostic::error(
                span,
                format!(
                    "type parameter {name} collides with a type of the same name; \
                     a type parameter must introduce a fresh name"
                ),
            ));
        }
    }
}

/// Unwrap a single level of pointer indirection and report the struct type a
/// receiver keys on, so both `Product` and `Product^` receivers resolve to the
/// same method set. Returns `None` for non-struct receivers.
pub fn underlying_struct(t: &Type) -> Option<&Type> {
    let inner = match t {
        Type::Pointer(elem) => elem.as_ref(),
        other => other,
    };
    matches!(inner, Type::Struct { .. }).then_some(inner)
}

/// The struct name a receiver type expression keys on, unwrapping a pointer and a
/// generic application. Used to look a method up in the global table by receiver.
pub fn underlying_struct_name(type_expr: &TypeExpr) -> Option<&str> {
    let inner = match &type_expr.kind {
        TypeExprKind::Pointer(inner) => inner.as_ref(),
        _ => type_expr,
    };
    let named = match &inner.kind {
        TypeExprKind::Application { constructor, .. } => &constructor.kind,
        other => other,
    };
    match named {
        TypeExprKind::Named(name) => Some(name),
        _ => None,
    }
}
