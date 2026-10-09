//! Monomorphization: stamping out one concrete instance of each generic function per
//! set of type arguments a program uses.
//!
//! Lowering leaves a generic function as a template whose types name its binders, and
//! every function or method reference carries the type arguments the checker inferred
//! for it. Starting from the non-generic functions, the pass instantiates each referenced
//! (declaration, type arguments) pair once — substituting the arguments through the
//! template's every type — and follows the references the instance makes in turn. The
//! result holds no type parameter anywhere, and no template that nothing reaches.

use std::collections::{HashMap, VecDeque};
use std::fmt;

use cooper_frontend::diag::Span;
use cooper_frontend::resolve::Callable;
use cooper_frontend::types::Type;

use crate::{
    Binder, Case, Decision, Function, IrExpr, IrExprKind, IrStmt, IrStmtKind, MatchBinding, Param,
};

/// Why monomorphization could not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonoError {
    /// A reference names a function or method absent from the program's functions.
    UnknownFunction { item: Callable, file: String, span: Span },
    /// Instantiating `item` keeps demanding ever more deeply nested type arguments —
    /// polymorphic recursion such as `f T` calling `f (T, T)` — so it would never end.
    UnboundedInstantiation { item: Callable, file: String, span: Span },
}

impl MonoError {
    /// The source file and span of the reference that failed.
    pub fn location(&self) -> (&str, Span) {
        match self {
            MonoError::UnknownFunction { file, span, .. }
            | MonoError::UnboundedInstantiation { file, span, .. } => (file, *span),
        }
    }
}

impl fmt::Display for MonoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MonoError::UnknownFunction { item, .. } => {
                write!(f, "internal error: `{}` is not among the program's functions", item_name(item))
            }
            MonoError::UnboundedInstantiation { item, .. } => write!(
                f,
                "instantiating `{}` never ends: each instance calls it with larger type arguments",
                item_name(item)
            ),
        }
    }
}

impl std::error::Error for MonoError {}

fn item_name(item: &Callable) -> String {
    match item {
        Callable::Func { name, .. } | Callable::Method { name, .. } | Callable::Extern { name } => {
            name.clone()
        }
        Callable::Closure { parent, index } => format!("{}.{index}", item_name(parent)),
        Callable::Formatter { index } => format!("formatter {index}"),
    }
}

/// How many type nodes a type argument may hold before instantiation is deemed
/// unbounded. Real programs stay far below it; polymorphic recursion soon passes it,
/// whether its arguments grow deeper (`List T` to `List (List T)`) or wider (`T` to
/// `(T, T)`, which doubles at every step).
const MAX_TYPE_SIZE: usize = 256;

/// Instantiate every generic function `functions` reaches from its non-generic ones,
/// returning the non-generic functions and one instance per distinct instantiation,
/// in the order first reached. `functions` must hold the whole program: a reference
/// to a function outside it is an error.
pub fn monomorphize(functions: &[Function]) -> Result<Vec<Function>, MonoError> {
    let templates: HashMap<&Callable, &Function> =
        functions.iter().map(|f| (&f.item, f)).collect();
    // Each queued reference carries the file and span of the site that made it.
    let mut queue: VecDeque<(Callable, Vec<Type>, String, Span)> = functions
        .iter()
        .filter(|f| f.type_params.is_empty())
        .map(|f| (f.item.clone(), Vec::new(), f.file.clone(), f.span))
        .collect();
    let mut instantiated: HashMap<Callable, Vec<Vec<Type>>> = HashMap::new();
    let mut instances = Vec::new();
    while let Some((item, type_args, file, span)) = queue.pop_front() {
        let seen = instantiated.entry(item.clone()).or_default();
        // An extern C function is declared, never instantiated.
        if matches!(item, Callable::Extern { .. }) || seen.iter().any(|args| same_types(args, &type_args)) {
            continue;
        }
        if type_args.iter().any(|t| size(t) > MAX_TYPE_SIZE) {
            return Err(MonoError::UnboundedInstantiation { item, file, span });
        }
        let Some(template) = templates.get(&item) else {
            return Err(MonoError::UnknownFunction { item, file, span });
        };
        seen.push(type_args.clone());
        let (instance, references) = instantiate(template, type_args);
        queue.extend(
            references
                .into_iter()
                .map(|(item, args, span)| (item, args, instance.file.clone(), span)),
        );
        instances.push(instance);
    }
    Ok(instances)
}

/// Whether two type-argument lists are the same instantiation.
fn same_types(a: &[Type], b: &[Type]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y))
}

/// How many type nodes `ty` holds.
fn size(ty: &Type) -> usize {
    1 + match ty {
        Type::Array(elem) | Type::Pointer(elem) => size(elem),
        Type::Tuple(elems) => elems.iter().map(size).sum(),
        Type::Func {
            return_type,
            param_types,
        } => size(return_type) + param_types.iter().map(size).sum::<usize>(),
        Type::Struct { type_args, .. } | Type::Oneof { type_args, .. } => {
            type_args.iter().map(size).sum()
        }
        Type::Unknown
        | Type::Unit
        | Type::Primitive(_)
        | Type::Nil
        | Type::TypeParam(_)
        | Type::Infer(_)
        | Type::Module(_) => 0,
    }
}

/// The instance of `template` at `type_args`, with every type substituted, and the
/// references the instance makes, each with its now-concrete type arguments.
fn instantiate(
    template: &Function,
    type_args: Vec<Type>,
) -> (Function, Vec<(Callable, Vec<Type>, Span)>) {
    let subst = template
        .type_params
        .iter()
        .cloned()
        .zip(type_args.iter().cloned())
        .collect();
    let mut pass = Substitute {
        subst: &subst,
        references: Vec::new(),
    };
    let mut instance = template.clone();
    instance.type_args = type_args;
    if let Some(receiver) = &mut instance.receiver {
        pass.param(receiver);
    }
    instance.env.iter_mut().flatten().for_each(|p| pass.param(p));
    instance.params.iter_mut().for_each(|p| pass.param(p));
    pass.ty(&mut instance.return_type);
    pass.stmts(&mut instance.body);
    (instance, pass.references)
}

/// An in-place walk substituting type parameters through a function body, collecting
/// the function and method references it passes.
struct Substitute<'a> {
    subst: &'a HashMap<String, Type>,
    references: Vec<(Callable, Vec<Type>, Span)>,
}

impl Substitute<'_> {
    fn ty(&self, ty: &mut Type) {
        *ty = ty.substitute(self.subst);
    }

    fn param(&mut self, param: &mut Param) {
        self.ty(&mut param.ty);
    }

    fn binder(&mut self, binder: &mut Binder) {
        self.ty(&mut binder.ty);
    }

    fn stmts(&mut self, stmts: &mut [IrStmt]) {
        stmts.iter_mut().for_each(|s| self.stmt(s));
    }

    fn exprs(&mut self, exprs: &mut [IrExpr]) {
        exprs.iter_mut().for_each(|e| self.expr(e));
    }

    fn stmt(&mut self, stmt: &mut IrStmt) {
        match &mut stmt.kind {
            IrStmtKind::Var { ty, init, .. } => {
                self.ty(ty);
                if let Some(init) = init {
                    self.expr(init);
                }
            }
            IrStmtKind::Expr(expr) => self.expr(expr),
            IrStmtKind::Return(value) => {
                if let Some(value) = value {
                    self.expr(value);
                }
            }
            IrStmtKind::While { cond, body, .. } => {
                self.expr(cond);
                self.stmts(body);
            }
            IrStmtKind::ForRange {
                ty,
                start,
                end,
                body,
                ..
            } => {
                self.ty(ty);
                self.expr(start);
                self.expr(end);
                self.stmts(body);
            }
            IrStmtKind::ForEach {
                array,
                index,
                elem,
                body,
            } => {
                self.expr(array);
                if let Some(index) = index {
                    self.binder(index);
                }
                self.binder(elem);
                self.stmts(body);
            }
            IrStmtKind::Block(stmts) => self.stmts(stmts),
            IrStmtKind::Break | IrStmtKind::Continue => {}
            IrStmtKind::Match {
                scrutinee,
                actions,
                tree,
            } => {
                self.expr(scrutinee);
                self.stmts(actions);
                self.decision(tree);
            }
        }
    }

    fn expr(&mut self, expr: &mut IrExpr) {
        self.ty(&mut expr.ty);
        match &mut expr.kind {
            IrExprKind::Int(_)
            | IrExprKind::Float(_)
            | IrExprKind::Bool(_)
            | IrExprKind::Str(_)
            | IrExprKind::Nil
            | IrExprKind::Unit
            | IrExprKind::Var(_) => {}
            IrExprKind::FuncRef { item, type_args } | IrExprKind::Closure { item, type_args, .. } => {
                type_args.iter_mut().for_each(|t| self.ty(t));
                self.references
                    .push((item.clone(), type_args.clone(), expr.span));
            }
            IrExprKind::Method {
                receiver,
                item,
                type_args,
            } => {
                self.expr(receiver);
                type_args.iter_mut().for_each(|t| self.ty(t));
                self.references
                    .push((item.clone(), type_args.clone(), expr.span));
            }
            IrExprKind::Tuple(elems) => self.exprs(elems),
            IrExprKind::Unary { operand, .. } => self.expr(operand),
            IrExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            IrExprKind::Call { callee, args } => {
                self.expr(callee);
                self.exprs(args);
            }
            IrExprKind::Field { target, .. } => self.expr(target),
            IrExprKind::Intrinsic { args, .. } => self.exprs(args),
            IrExprKind::Deref(operand)
            | IrExprKind::AddressOf(operand)
            | IrExprKind::Convert(operand) => self.expr(operand),
            IrExprKind::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            IrExprKind::Let { value, .. } => self.expr(value),
            IrExprKind::Block { stmts, result } => {
                self.stmts(stmts);
                self.expr(result);
            }
            IrExprKind::StructLiteral { members, .. } => {
                members.iter_mut().for_each(|(_, value)| self.expr(value));
            }
            IrExprKind::Match {
                scrutinee,
                actions,
                tree,
            } => {
                self.expr(scrutinee);
                self.exprs(actions);
                self.decision(tree);
            }
            IrExprKind::Variant { args, .. } => self.exprs(args),
            IrExprKind::LetTuple { bindings, value } => {
                bindings.iter_mut().for_each(|b| self.binder(b));
                self.expr(value);
            }
        }
    }

    fn decision(&mut self, decision: &mut Decision) {
        match decision {
            Decision::Leaf { bindings, .. } => {
                bindings
                    .iter_mut()
                    .for_each(|MatchBinding { ty, .. }| self.ty(ty));
            }
            Decision::Switch {
                ty, cases, default, ..
            } => {
                self.ty(ty);
                cases
                    .iter_mut()
                    .for_each(|Case { tree, .. }| self.decision(tree));
                if let Some(default) = default {
                    self.decision(default);
                }
            }
            Decision::Fail => {}
        }
    }
}
