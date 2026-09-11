use std::collections::HashMap;

use crate::ast::{Expr, FuncDecl, Pattern, Stmt, TypeExpr, TypedIdent};
use crate::diag::{Diagnostic, Span};
use crate::types::Type;

/// A lexical scope. Scopes are held on a stack in the `Resolver`; a lookup walks
/// the stack from innermost to outermost rather than following parent pointers.
struct Scope {
    vars: HashMap<String, Type>,
    struct_types: HashMap<String, Type>,
    oneof_types: HashMap<String, Type>,
    funcs: HashMap<String, Type>,
    /// Receiver struct type name -> that type's method set (method name -> bound
    /// signature, i.e. parameters excluding the receiver).
    methods: HashMap<String, HashMap<String, Type>>,
}

impl Scope {
    fn new() -> Self {
        Scope {
            vars: HashMap::new(),
            struct_types: HashMap::new(),
            oneof_types: HashMap::new(),
            funcs: HashMap::new(),
            methods: HashMap::new(),
        }
    }
}

/// The result of symbol resolution. In this iteration only the collected
/// diagnostics are surfaced; the scope tree feeds later phases once they are
/// ported.
pub struct ResolvedModule {
    pub errors: Vec<Diagnostic>,
}

struct Resolver {
    errors: Vec<Diagnostic>,
    scopes: Vec<Scope>,
    primitives: HashMap<String, Type>,
    /// Type-parameter names in scope for the declaration currently being resolved
    /// (a struct's, sum type's, or function's binders). A `Named` type expression
    /// matching one of these resolves to a `TypeParam` rather than a concrete type.
    type_params: HashMap<String, ()>,
}

impl Resolver {
    fn new() -> Self {
        let mut primitives = HashMap::new();
        for name in ["bool", "string", "i8", "i32", "i64", "f32", "f64"] {
            primitives.insert(name.to_string(), Type::Primitive(name.to_string()));
        }
        Resolver {
            errors: Vec::new(),
            scopes: vec![Scope::new()],
            primitives,
            type_params: HashMap::new(),
        }
    }

    fn err(&mut self, msg: impl Into<String>) {
        self.errors
            .push(Diagnostic::error(Span::new(0, 0), msg.into()));
    }

    // --- scope stack helpers ---

    fn push_scope(&mut self) {
        self.scopes.push(Scope::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define_var(&mut self, name: &str, ty: Type) {
        self.scopes
            .last_mut()
            .unwrap()
            .vars
            .insert(name.to_string(), ty);
    }

    fn lookup_var(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|s| s.vars.contains_key(name))
    }

    fn define_struct(&mut self, name: &str, ty: Type) {
        self.scopes
            .last_mut()
            .unwrap()
            .struct_types
            .insert(name.to_string(), ty);
    }

    fn lookup_struct(&self, name: &str) -> Option<&Type> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.struct_types.get(name))
    }

    fn define_oneof(&mut self, name: &str, ty: Type) {
        self.scopes
            .last_mut()
            .unwrap()
            .oneof_types
            .insert(name.to_string(), ty);
    }

    fn lookup_oneof(&self, name: &str) -> Option<&Type> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.oneof_types.get(name))
    }

    fn define_func(&mut self, name: &str, ty: Type) {
        self.scopes
            .last_mut()
            .unwrap()
            .funcs
            .insert(name.to_string(), ty);
    }

    fn lookup_func(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|s| s.funcs.contains_key(name))
    }

    fn define_method(&mut self, recv_type: &str, name: &str, ty: Type) {
        self.scopes
            .last_mut()
            .unwrap()
            .methods
            .entry(recv_type.to_string())
            .or_default()
            .insert(name.to_string(), ty);
    }

    fn lookup_method(&self, recv_type: &str, name: &str) -> bool {
        self.scopes
            .iter()
            .rev()
            .any(|s| s.methods.get(recv_type).is_some_and(|m| m.contains_key(name)))
    }

    // --- type resolution ---

    /// Convert an AST type expression to a concrete `Type`, or `None` on error
    /// (after recording a diagnostic), mirroring the Go resolver's nil return.
    fn resolve_type(&mut self, type_expr: &TypeExpr) -> Option<Type> {
        match type_expr {
            TypeExpr::Named(name) => {
                if self.type_params.contains_key(name) {
                    return Some(Type::TypeParam(name.clone()));
                }
                if let Some(prim) = self.primitives.get(name) {
                    return Some(prim.clone());
                }
                if let Some(st) = self.lookup_struct(name) {
                    if let Type::Struct { type_params, .. } = st {
                        if !type_params.is_empty() {
                            let n = type_params.len();
                            self.err(format!(
                                "generic type {} requires {} type argument(s)",
                                name, n
                            ));
                            return None;
                        }
                    }
                    return Some(st.clone());
                }
                if let Some(ot) = self.lookup_oneof(name) {
                    if let Type::Oneof { type_params, .. } = ot {
                        if !type_params.is_empty() {
                            let n = type_params.len();
                            self.err(format!(
                                "generic type {} requires {} type argument(s)",
                                name, n
                            ));
                            return None;
                        }
                    }
                    return Some(ot.clone());
                }
                self.err(format!("undefined type: {}", name));
                None
            }
            TypeExpr::Application { constructor, args } => {
                self.resolve_type_application(constructor, args)
            }
            TypeExpr::Array(elem) => Some(Type::Array(Box::new(self.resolve_type(elem)?))),
            TypeExpr::Pointer(elem) => Some(Type::Pointer(Box::new(self.resolve_type(elem)?))),
            TypeExpr::Tuple(elems) => {
                let mut resolved = Vec::with_capacity(elems.len());
                for e in elems {
                    resolved.push(self.resolve_type(e)?);
                }
                Some(Type::Tuple(resolved))
            }
            TypeExpr::Unit => Some(Type::Unit),
            TypeExpr::Func {
                return_type,
                param_types,
            } => {
                let mut params = Vec::new();
                for p in param_types {
                    if let Some(pt) = self.resolve_type(p) {
                        params.push(pt);
                    }
                }
                let ret = self.resolve_type(return_type)?;
                Some(Type::Func {
                    return_type: Box::new(ret),
                    param_types: params,
                })
            }
        }
    }

    /// Resolve a juxtaposition type application such as `Box i32` or `Map string
    /// i32`. The constructor must name a generic struct or sum type whose declared
    /// arity matches the argument count; the result is an instantiation with every
    /// type parameter substituted for its concrete argument.
    fn resolve_type_application(
        &mut self,
        constructor: &TypeExpr,
        args: &[TypeExpr],
    ) -> Option<Type> {
        let name = match constructor {
            TypeExpr::Named(n) => n.clone(),
            _ => {
                self.err("type application requires a named type constructor");
                return None;
            }
        };
        let (type_params, is_struct) = if let Some(Type::Struct { type_params, .. }) =
            self.lookup_struct(&name)
        {
            (type_params.clone(), true)
        } else if let Some(Type::Oneof { type_params, .. }) = self.lookup_oneof(&name) {
            (type_params.clone(), false)
        } else {
            self.err(format!("undefined generic type: {}", name));
            return None;
        };
        if type_params.is_empty() {
            self.err(format!(
                "type {} is not generic and takes no type arguments",
                name
            ));
            return None;
        }
        if args.len() != type_params.len() {
            self.err(format!(
                "generic type {} expects {} type argument(s), got {}",
                name,
                type_params.len(),
                args.len()
            ));
            return None;
        }
        let mut arg_types = Vec::with_capacity(args.len());
        for arg in args {
            arg_types.push(self.resolve_type(arg)?);
        }
        let mut subst = HashMap::new();
        for (param, arg) in type_params.iter().zip(&arg_types) {
            subst.insert(param.clone(), arg.clone());
        }
        if is_struct {
            let Some(Type::Struct { members, .. }) = self.lookup_struct(&name) else {
                return None;
            };
            let members: HashMap<String, Type> = members
                .iter()
                .map(|(k, v)| (k.clone(), v.substitute(&subst)))
                .collect();
            Some(Type::Struct {
                name,
                members,
                type_params,
                type_args: arg_types,
            })
        } else {
            let Some(Type::Oneof {
                variants,
                variant_order,
                ..
            }) = self.lookup_oneof(&name)
            else {
                return None;
            };
            let variant_order = variant_order.clone();
            let variants: HashMap<String, Vec<Type>> = variants
                .iter()
                .map(|(k, slots)| {
                    (k.clone(), slots.iter().map(|s| s.substitute(&subst)).collect())
                })
                .collect();
            Some(Type::Oneof {
                name,
                variants,
                variant_order,
                type_params,
                type_args: arg_types,
            })
        }
    }

    // --- statement resolution ---

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Block(stmts) => {
                self.push_scope();
                for s in stmts {
                    self.resolve_stmt(s);
                }
                self.pop_scope();
            }
            Stmt::VarDecl { name, ty, init } => self.resolve_var_decl(name, ty, init),
            Stmt::StructDecl {
                name,
                type_params,
                members,
            } => self.resolve_struct_decl(name, type_params, members),
            Stmt::OneofDecl {
                name,
                type_params,
                variants,
            } => self.resolve_oneof_decl(name, type_params, variants),
            Stmt::FuncDecl(func) => self.resolve_func_decl(func),
            Stmt::If { cond, then, els } => {
                self.resolve_expr(cond);
                self.resolve_stmt(then);
                if let Some(els) = els {
                    self.resolve_stmt(els);
                }
            }
            Stmt::Match { scrutinee, arms } => {
                self.resolve_expr(scrutinee);
                for arm in arms {
                    self.push_scope();
                    self.resolve_pattern_binders(&arm.pattern);
                    self.resolve_stmt(&arm.body);
                    self.pop_scope();
                }
            }
            Stmt::For {
                init,
                cond,
                iter,
                body,
            } => {
                self.resolve_stmt(init);
                self.resolve_expr(cond);
                self.resolve_expr(iter);
                self.push_scope();
                for s in body {
                    self.resolve_stmt(s);
                }
                self.pop_scope();
            }
            Stmt::Return(expr) => {
                if let Some(expr) = expr {
                    self.resolve_expr(expr);
                }
            }
            Stmt::Expression { expr, .. } => self.resolve_expr(expr),
            Stmt::Use(_) => {}
        }
    }

    fn resolve_var_decl(&mut self, name: &str, ty: &Option<TypeExpr>, init: &Option<Expr>) {
        // An explicit annotation fixes the type; without one the checker infers it
        // later, so the binding is defined with a placeholder for this pass.
        let declared = match ty {
            Some(ty) => match self.resolve_type(ty) {
                Some(t) => t,
                None => return,
            },
            None => Type::Unknown,
        };
        if let Some(init) = init {
            self.resolve_expr(init);
        }
        self.define_var(name, declared);
    }

    fn resolve_struct_decl(&mut self, name: &str, type_params: &[String], members: &[TypedIdent]) {
        if self.lookup_struct(name).is_some() {
            self.err(format!("redeclared struct {} in the same scope", name));
            return;
        }
        // The struct's binders are in scope while resolving its members, so a member
        // typed `T` resolves to a `TypeParam` rather than an undefined type.
        self.type_params = type_params.iter().map(|tp| (tp.clone(), ())).collect();
        let mut resolved = HashMap::new();
        for member in members {
            if resolved.contains_key(&member.name) {
                self.err(format!(
                    "duplicate member {} in struct {}",
                    member.name, name
                ));
                continue;
            }
            if let Some(t) = self.resolve_type(&member.ty) {
                resolved.insert(member.name.clone(), t);
            }
        }
        self.type_params.clear();
        self.define_struct(
            name,
            Type::Struct {
                name: name.to_string(),
                members: resolved,
                type_params: type_params.to_vec(),
                type_args: Vec::new(),
            },
        );
    }

    fn resolve_oneof_decl(
        &mut self,
        name: &str,
        type_params: &[String],
        variants: &[crate::ast::VariantDef],
    ) {
        if self.lookup_oneof(name).is_some() {
            self.err(format!("redeclared sum type {} in the same scope", name));
            return;
        }
        if self.lookup_struct(name).is_some() {
            self.err(format!(
                "sum type {} collides with a struct of the same name",
                name
            ));
            return;
        }
        self.type_params = type_params.iter().map(|tp| (tp.clone(), ())).collect();
        let mut resolved: HashMap<String, Vec<Type>> = HashMap::new();
        let mut order = Vec::new();
        for variant in variants {
            if resolved.contains_key(&variant.name) {
                self.err(format!(
                    "duplicate variant {} in sum type {}",
                    variant.name, name
                ));
                continue;
            }
            let mut slots = Vec::new();
            for slot_expr in &variant.payload {
                if let Some(t) = self.resolve_type(slot_expr) {
                    slots.push(t);
                }
            }
            resolved.insert(variant.name.clone(), slots);
            order.push(variant.name.clone());
        }
        self.type_params.clear();
        self.define_oneof(
            name,
            Type::Oneof {
                name: name.to_string(),
                variants: resolved,
                variant_order: order,
                type_params: type_params.to_vec(),
                type_args: Vec::new(),
            },
        );
    }

    fn resolve_func_decl(&mut self, func: &FuncDecl) {
        // The declaration's type parameters are in scope for its receiver,
        // parameter, and return types. A method additionally binds receiver-pattern
        // parameters (the bare-identifier arguments of a generic receiver).
        self.type_params = func.type_params.iter().map(|tp| (tp.clone(), ())).collect();
        if let Some(receiver) = &func.receiver {
            for name in receiver_pattern_params(&receiver.ty) {
                self.type_params.insert(name, ());
            }
        }

        let return_type = match &func.return_type {
            Some(rt) => match self.resolve_type(rt) {
                Some(t) => t,
                None => {
                    self.type_params.clear();
                    return;
                }
            },
            None => Type::Unit,
        };

        // Resolve receiver and parameter types before opening the function scope so
        // they see the declaration's binders; collect the bindings to install.
        let mut bindings: Vec<(String, Type)> = Vec::new();
        let mut param_types = Vec::new();

        let mut receiver_struct: Option<Type> = None;
        if let Some(receiver) = &func.receiver {
            let recv_type = match self.resolve_type(&receiver.ty) {
                Some(t) => t,
                None => {
                    self.type_params.clear();
                    return;
                }
            };
            match underlying_struct(&recv_type) {
                Some(st) => receiver_struct = Some(st.clone()),
                None => {
                    self.err(format!(
                        "method receiver {} must be a struct or pointer-to-struct type, found {}",
                        receiver.name, recv_type
                    ));
                    self.type_params.clear();
                    return;
                }
            }
            bindings.push((receiver.name.clone(), recv_type));
        }

        for param in &func.params {
            let param_type = match self.resolve_type(&param.ty) {
                Some(t) => t,
                None => {
                    self.type_params.clear();
                    return;
                }
            };
            param_types.push(param_type.clone());
            bindings.push((param.name.clone(), param_type));
        }

        self.type_params.clear();

        // The bound signature excludes the receiver: `product.isAffordable` has type
        // func(i32): bool, with the receiver captured as the closure environment.
        let func_type = Type::Func {
            return_type: Box::new(return_type),
            param_types,
        };

        if let Some(Type::Struct { name, members, .. }) = &receiver_struct {
            if members.contains_key(&func.name) {
                self.err(format!(
                    "method {} collides with a field of struct {}",
                    func.name, name
                ));
                return;
            }
            if self.lookup_method(name, &func.name) {
                self.err(format!(
                    "redeclared method {} on struct {}",
                    func.name, name
                ));
                return;
            }
            let name = name.clone();
            self.define_method(&name, &func.name, func_type);
        } else {
            if self.lookup_func(&func.name) {
                self.err(format!(
                    "redeclared function {} in the same scope",
                    func.name
                ));
                return;
            }
            self.define_func(&func.name, func_type);
        }

        // Open the function scope, install receiver/parameter bindings, and resolve
        // the body directly within it.
        self.push_scope();
        for (name, ty) in bindings {
            self.define_var(&name, ty);
        }
        for stmt in &func.body {
            self.resolve_stmt(stmt);
        }
        self.pop_scope();
    }

    /// Define every payload binder a variant pattern introduces in the current
    /// scope. Binder types are unknown until type checking, so each is bound to a
    /// placeholder; the wildcard binder `_` introduces nothing.
    fn resolve_pattern_binders(&mut self, pattern: &Pattern) {
        if let Pattern::Variant { binders, .. } = pattern {
            for binder in binders {
                if binder != "_" {
                    self.define_var(binder, Type::Unknown);
                }
            }
        }
    }

    // --- expression resolution ---

    fn resolve_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Number(_)
            | Expr::Str(_)
            | Expr::Bool(_)
            | Expr::Nil
            | Expr::Unit => {}
            Expr::AddressOf(operand) | Expr::Deref(operand) => self.resolve_expr(operand),
            Expr::Ident(name) => {
                if !self.lookup_var(name)
                    && self.lookup_struct(name).is_none()
                    && self.lookup_oneof(name).is_none()
                    && !self.lookup_func(name)
                {
                    self.err(format!("undefined identifier: {}", name));
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.resolve_expr(lhs);
                self.resolve_expr(rhs);
            }
            Expr::Unary { rhs, .. } => self.resolve_expr(rhs),
            Expr::Group(inner) => self.resolve_expr(inner),
            Expr::Tuple(elems) => {
                for elem in elems {
                    self.resolve_expr(elem);
                }
            }
            Expr::FuncCall { func, args } => {
                self.resolve_expr(func);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            Expr::StructLiteral { target, members } => {
                self.resolve_expr(target);
                for member in members {
                    self.resolve_expr(&member.value);
                }
            }
            Expr::StructMember { target, .. } => self.resolve_expr(target),
            Expr::ArrayIndex { array, index } => {
                self.resolve_expr(array);
                self.resolve_expr(index);
            }
            Expr::Assign { target, value, .. } => {
                self.resolve_expr(target);
                self.resolve_expr(value);
            }
            Expr::VarDeclAssign { name, value } => {
                self.resolve_expr(value);
                // The binding's type is inferred later; define it now with a
                // placeholder so subsequent references resolve during this pass.
                self.define_var(name, Type::Unknown);
            }
            Expr::TupleDeclAssign { names, ty, value } => {
                self.resolve_expr(value);
                // A type annotation fixes each element type up front; otherwise the
                // checker infers them from the initializer.
                let declared_elems = match ty {
                    Some(ty) => match self.resolve_type(ty) {
                        Some(Type::Tuple(elems)) => elems,
                        _ => Vec::new(),
                    },
                    None => Vec::new(),
                };
                for (i, name) in names.iter().enumerate() {
                    let ty = declared_elems.get(i).cloned().unwrap_or(Type::Unknown);
                    self.define_var(name, ty);
                }
            }
            Expr::Match { scrutinee, arms } => {
                self.resolve_expr(scrutinee);
                for arm in arms {
                    self.push_scope();
                    self.resolve_pattern_binders(&arm.pattern);
                    self.resolve_expr(&arm.body);
                    self.pop_scope();
                }
            }
            Expr::If { cond, then, els } => {
                self.resolve_expr(cond);
                self.resolve_expr(then);
                self.resolve_expr(els);
            }
            Expr::Block(block) => {
                self.push_scope();
                for stmt in &block.statements {
                    self.resolve_stmt(stmt);
                }
                self.resolve_expr(&block.result);
                self.pop_scope();
            }
        }
    }
}

/// The type-parameter names a method receiver binds by the receiver-pattern rule:
/// the bare-identifier arguments of a generic receiver such as `(m: Map K V)`. A
/// non-generic receiver binds none. A pointer receiver is unwrapped first.
fn receiver_pattern_params(type_expr: &TypeExpr) -> Vec<String> {
    let inner = match type_expr {
        TypeExpr::Pointer(inner) => inner.as_ref(),
        other => other,
    };
    match inner {
        TypeExpr::Application { args, .. } => args
            .iter()
            .filter_map(|arg| match arg {
                TypeExpr::Named(name) => Some(name.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Unwrap a single level of pointer indirection and report the struct type a
/// receiver keys on, so both `Product` and `Product^` receivers resolve to the
/// Product method set. Returns `None` for non-struct receivers.
fn underlying_struct(t: &Type) -> Option<&Type> {
    let inner = match t {
        Type::Pointer(elem) => elem.as_ref(),
        other => other,
    };
    match inner {
        Type::Struct { .. } => Some(inner),
        _ => None,
    }
}

/// Perform symbol resolution on the module. Module statements are processed
/// directly in the root scope.
pub fn resolve(module: &[Stmt]) -> ResolvedModule {
    let mut resolver = Resolver::new();
    for stmt in module {
        resolver.resolve_stmt(stmt);
    }
    ResolvedModule {
        errors: resolver.errors,
    }
}
