//! Escape analysis: which allocations may live in a stack frame.
//!
//! The backend places a local variable, a closure's environment, and an array literal's
//! storage in the frame unless the analysis cannot show that nothing keeps their address
//! past the frame (or past the loop iteration that made them), and puts those that may be
//! kept on the heap. It is a flow-insensitive points-to analysis over the typed IR, in the
//! manner of Go's: every value is the set of *symbols* whose address it may hold, an address
//! that reaches a place that can outlive the frame makes its symbol escape, and what
//! functions do with their pointer parameters is summarized so that a call need not make
//! its arguments escape.
//!
//! Symbols: `Local(name)` is the storage of a variable (by name, so shadowing merges, which
//! is conservative), `Env(site)` a closure's environment, `Backing(site)` an array
//! literal's elements, and `Param(i)` the value a function was passed, standing for whatever
//! the caller passed.
//!
//! * A value's address reaching a `return`, a global, memory written through a pointer or
//!   into a heap array, an argument the callee may keep, or an unknown callee, escapes it. An
//!   escaping symbol's contents escape with it, since what it points to may then be reached.
//! * Reading through a pointer gives what the pointed-to variables hold; through a parameter,
//!   the parameter itself.
//! * A symbol that flows into a variable declared outside the loop that made it escapes too:
//!   a frame slot is reused by every iteration, while each iteration's variable is its own.
//! * A function's summary says, per parameter, whether it escapes and whether it may come
//!   back in the result. Summaries are computed to a fixpoint over the whole program, from
//!   the optimistic start, so recursion is handled.
//! * A C function keeps no pointer it is passed (see the C interop rules), but may return one
//!   derived from an argument.

use std::collections::{BTreeSet, HashMap, HashSet};

use cooper_frontend::diag::Span;
use cooper_frontend::resolve::Callable;
use cooper_frontend::types::{Type, TypeDefs};

use crate::{Access, Decision, Function, Intrinsic, IrExpr, IrExprKind, IrStmt, IrStmtKind};

/// What the backend does with a function's allocations: the names of the variables that
/// live on the heap, and the closure environments and array literals (by their span) that
/// live in the frame.
#[derive(Debug, Clone, Default)]
pub struct Escapes {
    pub heap_locals: HashSet<String>,
    pub stack_envs: HashSet<Span>,
    pub stack_arrays: HashSet<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Sym {
    Local(String),
    Param(usize),
    Env(Span),
    Backing(Span),
}

type Set = BTreeSet<Sym>;

/// What a function does with each parameter (the receiver first).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Summary {
    escapes: Vec<bool>,
    to_result: Vec<bool>,
    /// The captured variables a closure's body lets escape, by name.
    captured_escapes: BTreeSet<String>,
}

fn key(item: &Callable, type_args: &[Type]) -> String {
    format!("{item:?}/{type_args:?}")
}

/// Run the analysis over a whole monomorphized program, recording the result on each function.
pub(crate) fn analyze(functions: &mut [Function], defs: &TypeDefs) {
    let by_key: HashMap<String, usize> = functions
        .iter()
        .enumerate()
        .map(|(i, f)| (key(&f.item, &f.type_args), i))
        .collect();
    let mut summaries: HashMap<String, Summary> = HashMap::new();
    let mut results: Vec<Result_> = Vec::new();
    // The summaries only grow, so this ends; the bound is a backstop.
    for _ in 0..50 {
        results.clear();
        let mut changed = false;
        for function in functions.iter() {
            let mut pass = Pass::new(function, &summaries, &by_key, functions, defs);
            let result = pass.run();
            let summary = result.summary.clone();
            if summaries.get(&key(&function.item, &function.type_args)) != Some(&summary) {
                summaries.insert(key(&function.item, &function.type_args), summary);
                changed = true;
            }
            results.push(result);
        }
        if !changed {
            break;
        }
    }
    for (function, result) in functions.iter_mut().zip(results) {
        function.escapes = result.escapes;
    }
}

struct Result_ {
    summary: Summary,
    escapes: Escapes,
}

struct Pass<'a> {
    function: &'a Function,
    summaries: &'a HashMap<String, Summary>,
    by_key: &'a HashMap<String, usize>,
    all: &'a [Function],
    defs: &'a TypeDefs,
    /// What each variable may hold.
    holds: HashMap<String, Set>,
    escaped: Set,
    returned: Set,
    /// The loop nesting (a path of loop ids) each variable is declared at.
    declared: HashMap<String, HashSet<Vec<u32>>>,
    /// Where each environment and array literal is made.
    made: HashMap<Sym, HashSet<Vec<u32>>>,
    /// The captured variables of this function, if it is a lifted closure.
    captured: HashSet<String>,
    path: Vec<u32>,
    loops: u32,
    changed: bool,
    pointer_receivers: HashSet<String>,
}

impl<'a> Pass<'a> {
    fn new(
        function: &'a Function,
        summaries: &'a HashMap<String, Summary>,
        by_key: &'a HashMap<String, usize>,
        all: &'a [Function],
        defs: &'a TypeDefs,
    ) -> Self {
        let pointer_receivers = all
            .iter()
            .filter(|f| matches!(f.receiver.as_ref().map(|r| &r.ty), Some(Type::Pointer(_))))
            .map(|f| key(&f.item, &f.type_args))
            .collect();
        Pass {
            function,
            summaries,
            by_key,
            all,
            defs,
            holds: HashMap::new(),
            escaped: Set::new(),
            returned: Set::new(),
            declared: HashMap::new(),
            made: HashMap::new(),
            captured: function.env.iter().flatten().map(|p| p.name.clone()).collect(),
            path: Vec::new(),
            loops: 0,
            changed: false,
            pointer_receivers,
        }
    }

    fn run(&mut self) -> Result_ {
        // Each walk can add to what variables hold, which later code reads, so walk again
        // until nothing is added.
        for _ in 0..100 {
            self.changed = false;
            self.loops = 0;
            self.path.clear();
            let function = self.function;
            for (index, param) in function.receiver.iter().chain(&function.params).enumerate() {
                self.declare(&param.name);
                self.add(&param.name, Set::from([Sym::Param(index)]));
            }
            for captured in function.env.iter().flatten() {
                self.declare(&captured.name);
            }
            self.stmts(&function.body);
            if !self.changed {
                break;
            }
        }
        self.finish()
    }

    // --- state ---

    fn declare(&mut self, name: &str) {
        let path = self.path.clone();
        if self.declared.entry(name.to_string()).or_default().insert(path) {
            self.changed = true;
        }
    }

    fn add(&mut self, name: &str, set: Set) {
        let held = self.holds.entry(name.to_string()).or_default();
        let before = held.len();
        held.extend(set);
        self.changed |= held.len() != before;
    }

    /// What a variable may hold, and what those variables hold in turn.
    fn deep(&self, set: &Set) -> Set {
        let mut all = set.clone();
        let mut pending: Vec<Sym> = set.iter().cloned().collect();
        while let Some(sym) = pending.pop() {
            if let Sym::Local(name) = sym {
                for held in self.holds.get(&name).into_iter().flatten() {
                    if all.insert(held.clone()) {
                        pending.push(held.clone());
                    }
                }
            }
        }
        all
    }

    /// The set a read through a pointer holding `set` yields.
    fn load(&self, set: &Set) -> Set {
        let mut out = Set::new();
        for sym in set {
            match sym {
                Sym::Local(name) => out.extend(self.holds.get(name).cloned().unwrap_or_default()),
                Sym::Param(_) => {
                    out.insert(sym.clone());
                }
                Sym::Env(_) | Sym::Backing(_) => {}
            }
        }
        out
    }

    fn sink(&mut self, set: &Set) {
        let reach = self.deep(set);
        let before = self.escaped.len();
        self.escaped.extend(reach);
        self.changed |= self.escaped.len() != before;
    }

    fn send_back(&mut self, set: &Set) {
        let reach = self.deep(set);
        let before = self.returned.len();
        self.returned.extend(reach);
        self.changed |= self.returned.len() != before;
    }

    // --- statements ---

    fn stmts(&mut self, stmts: &[IrStmt]) {
        stmts.iter().for_each(|s| self.stmt(s));
    }

    fn looped(&mut self, body: impl FnOnce(&mut Self)) {
        self.loops += 1;
        self.path.push(self.loops);
        body(self);
        self.path.pop();
    }

    fn stmt(&mut self, stmt: &IrStmt) {
        match &stmt.kind {
            IrStmtKind::Var { name, init, .. } => {
                self.declare(name);
                if let Some(init) = init {
                    let value = self.eval(init);
                    self.add(name, value);
                }
            }
            IrStmtKind::Expr(expr) => {
                self.eval(expr);
            }
            IrStmtKind::Return(value) => {
                if let Some(value) = value {
                    let set = self.eval(value);
                    self.send_back(&set);
                }
            }
            IrStmtKind::While { cond, body, .. } => self.looped(|this| {
                this.eval(cond);
                this.stmts(body);
            }),
            IrStmtKind::ForRange { var, start, end, body, .. } => {
                self.eval(start);
                self.eval(end);
                self.looped(|this| {
                    this.declare(var);
                    this.stmts(body);
                });
            }
            IrStmtKind::ForEach { array, index, elem, body } => {
                let held = self.eval(array);
                let items = self.union(&held, &self.load(&held));
                self.looped(|this| {
                    this.declare(&elem.name);
                    this.add(&elem.name, items);
                    if let Some(index) = index {
                        this.declare(&index.name);
                    }
                    this.stmts(body);
                });
            }
            IrStmtKind::Block(stmts) => self.stmts(stmts),
            IrStmtKind::Break | IrStmtKind::Continue => {}
            IrStmtKind::Match { scrutinee, actions, tree } => {
                let held = self.eval(scrutinee);
                self.bindings(tree, &held);
                self.stmts(actions);
            }
        }
    }

    fn union(&self, a: &Set, b: &Set) -> Set {
        a.union(b).cloned().collect()
    }

    /// Declare the names a decision tree binds, each holding what the scrutinee does.
    fn bindings(&mut self, tree: &Decision, held: &Set) {
        match tree {
            Decision::Leaf { bindings, .. } => {
                for binding in bindings {
                    self.declare(&binding.name);
                    self.add(&binding.name, held.clone());
                }
            }
            Decision::Switch { cases, default, .. } => {
                cases.iter().for_each(|c| self.bindings(&c.tree, held));
                if let Some(default) = default {
                    self.bindings(default, held);
                }
            }
            Decision::Fail => {}
        }
    }

    // --- expressions ---

    /// The variable whose own storage a place expression is, if it is one: a variable, a
    /// field of one, or an element of a static array in one.
    fn root(&self, expr: &IrExpr) -> Option<String> {
        match &expr.kind {
            IrExprKind::Var(name) => Some(name.clone()),
            IrExprKind::Field { target, .. } if !matches!(target.ty, Type::Pointer(_)) => self.root(target),
            IrExprKind::Intrinsic { op: Intrinsic::ArrayGet, args }
                if matches!(args[0].ty, Type::StaticArray(..)) =>
            {
                self.root(&args[0])
            }
            _ => None,
        }
    }

    /// The set an address-of `place` yields.
    fn address_of(&mut self, place: &IrExpr) -> Set {
        if let Some(root) = self.root(place) {
            // Evaluate subexpressions (an index) for their effects.
            self.eval_children_of_place(place);
            return Set::from([Sym::Local(root)]);
        }
        match &place.kind {
            IrExprKind::Deref(pointer) => self.eval(pointer),
            IrExprKind::Field { target, .. } => self.eval(target),
            IrExprKind::Intrinsic { op: Intrinsic::ArrayGet, args } => {
                let held = self.eval(&args[0]);
                self.eval(&args[1]);
                held
            }
            _ => {
                self.eval(place);
                Set::new()
            }
        }
    }

    fn eval_children_of_place(&mut self, place: &IrExpr) {
        match &place.kind {
            IrExprKind::Field { target, .. } => self.eval_children_of_place(target),
            IrExprKind::Intrinsic { op: Intrinsic::ArrayGet, args } => {
                self.eval_children_of_place(&args[0]);
                self.eval(&args[1]);
            }
            _ => {}
        }
    }

    /// Store `value` into the place `target`: into a variable's own storage it is held there,
    /// anywhere else it escapes.
    fn store(&mut self, target: &IrExpr, value: &Set) {
        match self.root(target) {
            Some(root) if !self.captured.contains(&root) => {
                self.eval_children_of_place(target);
                self.add(&root, value.clone());
            }
            _ => {
                // The pointer or array stored through, and the index, are evaluated.
                match &target.kind {
                    IrExprKind::Deref(pointer) => {
                        self.eval(pointer);
                    }
                    IrExprKind::Field { target, .. } => {
                        self.eval(target);
                    }
                    IrExprKind::Intrinsic { op: Intrinsic::ArrayGet, args } => {
                        args.iter().for_each(|a| {
                            self.eval(a);
                        });
                    }
                    _ => {}
                }
                self.sink(value);
            }
        }
    }

    fn eval_all(&mut self, exprs: &[IrExpr]) -> Vec<Set> {
        exprs.iter().map(|e| self.eval(e)).collect()
    }

    /// Whether a value of type `ty` can hold an address: only a pointer, function value, array,
    /// or aggregate of those can.
    fn can_hold(&self, ty: &Type, seen: &mut Vec<Type>) -> bool {
        match ty {
            Type::Pointer(_) | Type::Array(_) | Type::Func { .. } | Type::Nil => true,
            Type::StaticArray(elem, _) => self.can_hold(elem, seen),
            Type::Tuple(elems) => elems.iter().any(|e| self.can_hold(e, seen)),
            Type::Struct { .. } | Type::Oneof { .. } => {
                if seen.iter().any(|s| s.equals(ty)) {
                    return false;
                }
                seen.push(ty.clone());
                let parts: Vec<Type> = match ty {
                    Type::Struct { .. } => self.defs.struct_members(ty).unwrap_or_default().into_iter().map(|(_, t)| t).collect(),
                    _ => self.defs.variant_order(ty).iter().flat_map(|v| self.defs.payload(ty, v).unwrap_or_default()).collect(),
                };
                parts.iter().any(|p| self.can_hold(p, seen))
            }
            Type::Primitive(_) | Type::Unit => false,
            // Anything not yet resolved is treated as able to hold one.
            _ => true,
        }
    }

    fn eval(&mut self, expr: &IrExpr) -> Set {
        let set = self.eval_kind(expr);
        if self.can_hold(&expr.ty, &mut Vec::new()) { set } else { Set::new() }
    }

    fn eval_kind(&mut self, expr: &IrExpr) -> Set {
        match &expr.kind {
            IrExprKind::Int(_)
            | IrExprKind::Float(_)
            | IrExprKind::Bool(_)
            | IrExprKind::Str(_)
            | IrExprKind::Nil
            | IrExprKind::Unit
            | IrExprKind::Unreachable
            | IrExprKind::Global { .. }
            | IrExprKind::FuncRef { .. } => Set::new(),
            IrExprKind::Var(name) => self.holds.get(name).cloned().unwrap_or_default(),
            IrExprKind::Closure { captures, .. } | IrExprKind::GenericClosure { captures } => {
                let env = Sym::Env(expr.span);
                self.made.entry(env.clone()).or_default().insert(self.path.clone());
                let mut set: Set = captures.iter().map(|c| Sym::Local(c.clone())).collect();
                set.insert(env);
                // The environment points at each captured variable, so reaching the closure
                // reaches them (a variable it captures that its body lets escape escapes at once).
                if let IrExprKind::Closure { item, type_args, .. } = &expr.kind {
                    if let Some(summary) = self.summaries.get(&key(item, type_args)) {
                        let escaping: Set = summary.captured_escapes.iter().map(|c| Sym::Local(c.clone())).collect();
                        self.sink(&escaping);
                    }
                }
                set
            }
            IrExprKind::Instantiate { closure, .. } => self.eval(closure),
            IrExprKind::Method { receiver, item, type_args } => {
                let held = self.method_receiver(receiver, &key(item, type_args));
                held
            }
            IrExprKind::Tuple(elems) | IrExprKind::Variant { args: elems, .. } => {
                self.eval_all(elems).into_iter().flatten().collect()
            }
            IrExprKind::StructLiteral { members, .. } => {
                members.iter().flat_map(|(_, v)| self.eval(v)).collect()
            }
            IrExprKind::Unary { operand, .. } | IrExprKind::Convert(operand) => {
                self.eval(operand);
                Set::new()
            }
            IrExprKind::Binary { lhs, rhs, .. } => {
                self.eval(lhs);
                self.eval(rhs);
                Set::new()
            }
            IrExprKind::Field { target, .. } => {
                let held = self.eval(target);
                if matches!(target.ty, Type::Pointer(_)) { self.load(&held) } else { held }
            }
            IrExprKind::Deref(pointer) => {
                let held = self.eval(pointer);
                self.load(&held)
            }
            IrExprKind::AddressOf(place) => self.address_of(place),
            IrExprKind::Assign { target, value, .. } => {
                let held = self.eval(value);
                self.store(target, &held);
                held
            }
            IrExprKind::Let { name, value } => {
                let held = self.eval(value);
                self.declare(name);
                self.add(name, held.clone());
                held
            }
            IrExprKind::LetTuple { bindings, value } => {
                let held = self.eval(value);
                for binding in bindings {
                    self.declare(&binding.name);
                    self.add(&binding.name, held.clone());
                }
                held
            }
            IrExprKind::Block { stmts, result } => {
                self.stmts(stmts);
                self.eval(result)
            }
            IrExprKind::Match { scrutinee, actions, tree } => {
                let held = self.eval(scrutinee);
                self.bindings(tree, &held);
                self.eval_all(actions).into_iter().flatten().collect()
            }
            IrExprKind::Call { callee, args } => self.call(callee, args),
            IrExprKind::Intrinsic { op, args } => self.intrinsic(*op, args, expr),
        }
    }

    /// The set a method bound to `receiver` holds, and the one passed as its receiver: the
    /// receiver's own address when the method takes a pointer and `receiver` is a variable.
    fn method_receiver(&mut self, receiver: &IrExpr, method: &str) -> Set {
        if self.pointer_receivers.contains(method) && !matches!(receiver.ty, Type::Pointer(_)) {
            return self.address_of(receiver);
        }
        self.eval(receiver)
    }

    fn call(&mut self, callee: &IrExpr, args: &[IrExpr]) -> Set {
        let mut passed: Vec<Set> = Vec::new();
        let summary = match &callee.kind {
            IrExprKind::FuncRef { item, type_args } => {
                if matches!(item, Callable::Extern { .. }) {
                    // C keeps no pointer it is given, but may return one derived from them.
                    let sets = self.eval_all(args);
                    return sets.iter().flat_map(|s| self.deep(s)).collect();
                }
                self.summaries.get(&key(item, type_args)).cloned().or_else(|| {
                    // Not summarized yet: optimistic, as the fixpoint expects.
                    self.by_key.get(&key(item, type_args)).map(|_| Summary::default())
                })
            }
            IrExprKind::Method { receiver, item, type_args } => {
                let method = key(item, type_args);
                passed.push(self.method_receiver(receiver, &method));
                self.summaries.get(&method).cloned().or_else(|| self.by_key.get(&method).map(|_| Summary::default()))
            }
            _ => None,
        };
        passed.extend(self.eval_all(args));
        let Some(summary) = summary else {
            // An unknown callee may keep what it is passed, and return that or anything it
            // holds (a closure's captured variables).
            let held = self.eval(callee);
            let mut passed_all: Set = Set::new();
            passed_all.extend(passed.iter().flatten().cloned());
            self.sink(&passed_all);
            let mut all = held;
            all.extend(passed_all);
            return self.deep(&all);
        };
        let _ = self.all;
        let mut result = Set::new();
        for (index, set) in passed.iter().enumerate() {
            if summary.escapes.get(index).copied().unwrap_or(false) {
                self.sink(set);
            }
            if summary.to_result.get(index).copied().unwrap_or(false) {
                result.extend(self.deep(set));
            }
        }
        result
    }

    fn intrinsic(&mut self, op: Intrinsic, args: &[IrExpr], expr: &IrExpr) -> Set {
        use Intrinsic::*;
        let is_static = |e: &IrExpr| matches!(e.ty, Type::StaticArray(..));
        match op {
            ArrayLiteral => {
                let sets = self.eval_all(args);
                if matches!(expr.ty, Type::StaticArray(..)) {
                    return sets.into_iter().flatten().collect();
                }
                // The elements are stored in the literal's storage, which may be kept.
                for set in &sets {
                    self.sink(set);
                }
                let site = Sym::Backing(expr.span);
                self.made.entry(site.clone()).or_default().insert(self.path.clone());
                Set::from([site])
            }
            ArrayGet => {
                let held = self.eval(&args[0]);
                self.eval(&args[1]);
                if is_static(&args[0]) { held } else { self.load(&held) }
            }
            ArraySet => {
                let array = self.eval(&args[0]);
                self.eval(&args[1]);
                let value = self.eval(&args[2]);
                match (is_static(&args[0]), self.root(&args[0])) {
                    (true, Some(root)) if !self.captured.contains(&root) => self.add(&root, value),
                    _ => self.sink(&value),
                }
                let _ = array;
                Set::new()
            }
            ArrayPush => {
                let array = self.eval(&args[0]);
                let value = self.eval(&args[1]);
                self.sink(&value);
                array
            }
            ArrayReserve => {
                let array = self.eval(&args[0]);
                self.eval(&args[1]);
                array
            }
            ArrayElementPtr => {
                if is_static(&args[0]) {
                    if let Some(root) = self.root(&args[0]) {
                        self.eval_children_of_place(&args[0]);
                        self.eval(&args[1]);
                        return Set::from([Sym::Local(root)]);
                    }
                }
                let held = self.eval(&args[0]);
                self.eval(&args[1]);
                held
            }
            ArraySlice { .. } => {
                if is_static(&args[0]) {
                    if let Some(root) = self.root(&args[0]) {
                        self.eval_children_of_place(&args[0]);
                        args[1..].iter().for_each(|a| {
                            self.eval(a);
                        });
                        return Set::from([Sym::Local(root)]);
                    }
                }
                let held = self.eval(&args[0]);
                args[1..].iter().for_each(|a| {
                    self.eval(a);
                });
                held
            }
            ArrayLength | ArrayCopy | ArrayUtf8 | StringLength | StringData | StringByteAt | StringSlice { .. }
            | SourceLocation | ToCString | FromCString | CopyBytes => {
                self.eval_all(args);
                Set::new()
            }
            // Formatting reads what it is given and keeps nothing.
            _ if format!("{op:?}").starts_with("Format") || format!("{op:?}").starts_with("Pad") => {
                self.eval_all(args);
                Set::new()
            }
            _ => {
                let sets = self.eval_all(args);
                for set in &sets {
                    self.sink(set);
                }
                Set::new()
            }
        }
    }

    // --- results ---

    fn finish(&mut self) -> Result_ {
        let function = self.function;
        let params = function.receiver.iter().chain(&function.params).count();
        let summary_escapes: Vec<bool> = (0..params).map(|i| self.escaped.contains(&Sym::Param(i))).collect();
        let summary_result: Vec<bool> = (0..params).map(|i| self.returned.contains(&Sym::Param(i))).collect();

        // A symbol held by a variable declared outside the loop that made it cannot live in
        // the frame: the frame slot is shared by every iteration.
        let mut outlived: HashSet<Sym> = HashSet::new();
        for (name, held) in &self.holds {
            let Some(declared) = self.declared.get(name) else { continue };
            for sym in held {
                let made: HashSet<Vec<u32>> = match sym {
                    Sym::Local(local) => self.declared.get(local).cloned().unwrap_or_default(),
                    Sym::Env(_) | Sym::Backing(_) => self.made.get(sym).cloned().unwrap_or_default(),
                    Sym::Param(_) => continue,
                };
                let unsafe_ = made.iter().any(|at| declared.iter().any(|holder| !holder.starts_with(at)));
                if unsafe_ {
                    outlived.insert(sym.clone());
                }
            }
        }
        let escapes = |sym: &Sym| self.escaped.contains(sym) || self.returned.contains(sym) || outlived.contains(sym);

        let mut result = Escapes::default();
        for name in self.declared.keys() {
            if self.captured.contains(name) {
                continue;
            }
            if escapes(&Sym::Local(name.clone())) {
                result.heap_locals.insert(name.clone());
            }
        }
        for sym in self.made.keys() {
            if escapes(sym) {
                continue;
            }
            match sym {
                Sym::Env(span) => {
                    result.stack_envs.insert(*span);
                }
                Sym::Backing(span) => {
                    result.stack_arrays.insert(*span);
                }
                _ => {}
            }
        }
        // A site made by several nodes of one span is on the stack only if all are.
        for sym in self.made.keys() {
            if escapes(sym) {
                match sym {
                    Sym::Env(span) => {
                        result.stack_envs.remove(span);
                    }
                    Sym::Backing(span) => {
                        result.stack_arrays.remove(span);
                    }
                    _ => {}
                }
            }
        }
        let captured_escapes = self
            .captured
            .iter()
            .filter(|name| escapes(&Sym::Local((*name).clone())))
            .cloned()
            .collect();
        Result_ {
            summary: Summary { escapes: summary_escapes, to_result: summary_result, captured_escapes },
            escapes: result,
        }
    }
}

#[allow(dead_code)]
fn unused(_: Access) {}
