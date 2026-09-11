//! Declaration resolution: the first semantic pass.
//!
//! This pass collects every top-level struct, sum type, function, and method into
//! a global symbol table ([`Globals`]) and validates each declaration's signature
//! in isolation — duplicate names, duplicate members or variants, undefined types
//! in signatures, generic arity, and receiver/method well-formedness. Function
//! *bodies* are not walked here; the type checker does that against the finished
//! [`Globals`]. Types must be declared before use, so the table is built top-down
//! and each declaration sees only those preceding it.

use std::collections::{HashMap, HashSet};

use crate::ast::{FuncDecl, Stmt, StmtKind, TypeExpr, TypeExprKind, TypedIdent, VariantDef};
use crate::diag::{Diagnostic, Span};
use crate::types::Type;

/// The primitive type names recognised without declaration.
const PRIMITIVES: [&str; 7] = ["bool", "string", "i8", "i32", "i64", "f32", "f64"];

/// The module-wide symbol table produced by resolution. Structs, sum types, and
/// functions are global (only variables are block-scoped), so later passes read
/// declarations directly from here rather than from a scope tree.
#[derive(Debug, Default)]
pub struct Globals {
    pub structs: HashMap<String, Type>,
    pub oneofs: HashMap<String, Type>,
    pub funcs: HashMap<String, Type>,
    /// Receiver struct name -> method name -> bound signature (receiver excluded).
    pub methods: HashMap<String, HashMap<String, Type>>,
}

impl Globals {
    pub fn lookup_struct(&self, name: &str) -> Option<&Type> {
        self.structs.get(name)
    }

    pub fn lookup_oneof(&self, name: &str) -> Option<&Type> {
        self.oneofs.get(name)
    }

    pub fn lookup_func(&self, name: &str) -> Option<&Type> {
        self.funcs.get(name)
    }

    pub fn lookup_method(&self, recv_type: &str, name: &str) -> Option<&Type> {
        self.methods.get(recv_type).and_then(|m| m.get(name))
    }
}

/// Resolve every top-level declaration in `module`, returning the global symbol
/// table alongside any signature diagnostics.
pub fn resolve(module: &[Stmt]) -> (Globals, Vec<Diagnostic>) {
    let mut globals = Globals::default();
    let mut diags = Vec::new();
    for stmt in module {
        match &stmt.kind {
            StmtKind::StructDecl {
                name,
                type_params,
                members,
            } => resolve_struct_decl(&mut globals, &mut diags, name, type_params, members, stmt.span),
            StmtKind::OneofDecl {
                name,
                type_params,
                variants,
            } => resolve_oneof_decl(&mut globals, &mut diags, name, type_params, variants, stmt.span),
            StmtKind::FuncDecl(func) => resolve_func_decl(&mut globals, &mut diags, func),
            _ => {}
        }
    }
    (globals, diags)
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
            if PRIMITIVES.contains(&name.as_str()) {
                return Some(Type::Primitive(name.clone()));
            }
            if let Some(ty) = globals.lookup_struct(name).or_else(|| globals.lookup_oneof(name)) {
                let arity = match ty {
                    Type::Struct { type_params, .. } | Type::Oneof { type_params, .. } => {
                        type_params.len()
                    }
                    _ => 0,
                };
                if arity > 0 {
                    diags.push(Diagnostic::error(
                        type_expr.span,
                        format!("generic type {name} requires {arity} type argument(s)"),
                    ));
                    return None;
                }
                return Some(ty.clone());
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
/// into a fully substituted instantiation of the named generic declaration.
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

    let template = globals
        .lookup_struct(name)
        .or_else(|| globals.lookup_oneof(name))
        .cloned();
    let template = match template {
        Some(t) => t,
        None => {
            diags.push(Diagnostic::error(
                span,
                format!("undefined generic type: {name}"),
            ));
            return None;
        }
    };

    let declared_params = match &template {
        Type::Struct { type_params, .. } | Type::Oneof { type_params, .. } => type_params.clone(),
        _ => Vec::new(),
    };
    if declared_params.is_empty() {
        diags.push(Diagnostic::error(
            span,
            format!("type {name} is not generic and takes no type arguments"),
        ));
        return None;
    }
    if args.len() != declared_params.len() {
        diags.push(Diagnostic::error(
            span,
            format!(
                "generic type {name} expects {} type argument(s), got {}",
                declared_params.len(),
                args.len()
            ),
        ));
        return None;
    }

    let mut arg_types = Vec::with_capacity(args.len());
    for arg in args {
        arg_types.push(resolve_type(arg, type_params, globals, diags)?);
    }
    let subst: HashMap<String, Type> = declared_params
        .iter()
        .cloned()
        .zip(arg_types.iter().cloned())
        .collect();

    Some(instantiate(&template, &subst, arg_types))
}

/// Build an instantiation of a generic struct or sum-type template by substituting
/// its members/variants and recording the concrete type arguments.
fn instantiate(template: &Type, subst: &HashMap<String, Type>, type_args: Vec<Type>) -> Type {
    match template {
        Type::Struct {
            name,
            members,
            type_params,
            ..
        } => Type::Struct {
            name: name.clone(),
            members: members
                .iter()
                .map(|(k, v)| (k.clone(), v.substitute(subst)))
                .collect(),
            type_params: type_params.clone(),
            type_args,
        },
        Type::Oneof {
            name,
            variants,
            variant_order,
            type_params,
            ..
        } => Type::Oneof {
            name: name.clone(),
            variants: variants
                .iter()
                .map(|(k, slots)| (k.clone(), slots.iter().map(|s| s.substitute(subst)).collect()))
                .collect(),
            variant_order: variant_order.clone(),
            type_params: type_params.clone(),
            type_args,
        },
        other => other.clone(),
    }
}

fn resolve_struct_decl(
    globals: &mut Globals,
    diags: &mut Vec<Diagnostic>,
    name: &str,
    type_params: &[String],
    members: &[TypedIdent],
    span: Span,
) {
    if globals.structs.contains_key(name) || globals.oneofs.contains_key(name) {
        diags.push(Diagnostic::error(
            span,
            format!("redeclared type {name} in the same scope"),
        ));
        return;
    }
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
    globals.structs.insert(
        name.to_string(),
        Type::Struct {
            name: name.to_string(),
            members: resolved,
            type_params: type_params.to_vec(),
            type_args: Vec::new(),
        },
    );
}

fn resolve_oneof_decl(
    globals: &mut Globals,
    diags: &mut Vec<Diagnostic>,
    name: &str,
    type_params: &[String],
    variants: &[VariantDef],
    span: Span,
) {
    if globals.oneofs.contains_key(name) || globals.structs.contains_key(name) {
        diags.push(Diagnostic::error(
            span,
            format!("redeclared type {name} in the same scope"),
        ));
        return;
    }
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
    globals.oneofs.insert(
        name.to_string(),
        Type::Oneof {
            name: name.to_string(),
            variants: resolved,
            variant_order: order,
            type_params: type_params.to_vec(),
            type_args: Vec::new(),
        },
    );
}

fn resolve_func_decl(globals: &mut Globals, diags: &mut Vec<Diagnostic>, func: &FuncDecl) {
    // The declaration's binders — its own type parameters plus any bound by a
    // generic receiver pattern — are in scope for the receiver, parameter, and
    // return types.
    let mut params: HashSet<String> = func.type_params.iter().cloned().collect();
    if let Some(receiver) = &func.receiver {
        params.extend(receiver_pattern_params(&receiver.ty));
    }

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
        if globals.funcs.contains_key(&func.name) {
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
    let Some(Type::Struct {
        name: struct_name,
        members,
        ..
    }) = underlying_struct(&recv_type)
    else {
        diags.push(Diagnostic::error(
            receiver.span,
            format!(
                "method receiver {} must be a struct or pointer-to-struct type, found {recv_type}",
                receiver.name
            ),
        ));
        return;
    };
    if members.contains_key(&func.name) {
        diags.push(Diagnostic::error(
            func.span,
            format!("method {} collides with a field of struct {struct_name}", func.name),
        ));
        return;
    }
    if globals.lookup_method(struct_name, &func.name).is_some() {
        diags.push(Diagnostic::error(
            func.span,
            format!("redeclared method {} on struct {struct_name}", func.name),
        ));
        return;
    }
    let struct_name = struct_name.clone();
    globals
        .methods
        .entry(struct_name)
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
