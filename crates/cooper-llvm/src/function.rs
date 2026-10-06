//! Code generation for one function body.
//!
//! Every local — parameters included — lives in an `alloca` in the entry block, which
//! an optimizing build promotes to registers, unless its address is taken: such a
//! local may outlive its frame, so it is allocated on the heap where it is bound.
//! Structs and tuples are first-class aggregate values; a *place* (a variable, a field
//! of a place, or a dereference) is addressed by pointer, so fields can be assigned
//! and addressed. Control flow becomes explicit basic blocks. Integer arithmetic is
//! checked: overflow and division by zero branch to the runtime's `cooper_panic`.
//!
//! Nil is inert. A place reached through a pointer is *nullable*: field offsets keep a
//! null base null, and only the final access substitutes a target — a load reads the
//! all-zero `@cooper.zero`, so it yields the zero value, and a store writes the
//! never-read `@cooper.sink`, so it is discarded.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use cooper_frontend::ast::{AssignOp, BinaryOp, UnaryOp};
use cooper_frontend::diag::Span;
use cooper_frontend::types::Type;
use cooper_ir::{
    Access, Decision, Function, IrExpr, IrExprKind, IrStmt, IrStmtKind, MatchBinding, Test,
};

use crate::layout::{Scalar, FUNC_VALUE};
use crate::{mangle, CodegenError, Module};

const PANIC_DECL: &str = "declare void @cooper_panic(ptr) noreturn";
const ALLOC_DECL: &str = "declare ptr @cooper_alloc(i64)";

/// Emit `function` as an LLVM function definition.
pub(crate) fn emit(module: &mut Module, function: &Function) -> Result<String, CodegenError> {
    let mut emitter = Emitter {
        module,
        file: function.file.clone(),
        allocas: String::new(),
        body: String::new(),
        next: 0,
        terminated: false,
        current: "entry".to_string(),
        scopes: vec![HashMap::new()],
        loops: Vec::new(),
        heap: HashSet::new(),
    };
    let mut heap = HashSet::new();
    function
        .body
        .iter()
        .for_each(|s| walk_stmt(s, &mut |e| emitter.note_escape(e, &mut heap)));
    emitter.heap = heap;
    let ret = emitter.signature_type(&function.return_type, function.span)?;
    let mut params = Vec::new();
    for (index, param) in function.receiver.iter().chain(&function.params).enumerate() {
        let slot = emitter.bind(&param.name, &param.ty, param.span)?;
        if let Some(ty) = &slot.llvm {
            let operand = format!("%arg{index}");
            params.push(format!("{ty} {operand}"));
            emitter.inst(&format!("store {ty} {operand}, ptr {}", slot.ptr));
        }
    }
    for stmt in &function.body {
        emitter.stmt(stmt)?;
    }
    if !emitter.terminated {
        // A unit function may fall off its end; a valued one cannot, as the checker
        // requires a return on every path.
        emitter.terminate(if ret == "void" { "ret void" } else { "unreachable" });
    }
    Ok(format!(
        "define {ret} @{}({}) {{\nentry:\n{}{}}}\n",
        mangle::symbol(&function.item, &function.type_args),
        params.join(", "),
        emitter.allocas,
        emitter.body
    ))
}

/// An SSA value: its LLVM type and operand spelling. Unit-typed expressions have none.
#[derive(Debug, Clone)]
struct Value {
    ty: String,
    operand: String,
}

/// A storage location: a pointer to a value of type `ty` (whose LLVM type is `llvm`,
/// `None` for unit, which has no storage). A local's place is never null; a place
/// reached through a pointer is `nullable`, and accessing it when null is inert.
#[derive(Debug, Clone)]
struct Place {
    ptr: String,
    ty: Type,
    llvm: Option<String>,
    nullable: bool,
}

/// The targets of `continue` and `break` in an enclosing loop.
struct Loop {
    continue_to: String,
    break_to: String,
}

/// The two kinds of `match` action: statements run for effect, or expressions whose
/// value is the match's.
#[derive(Clone, Copy)]
enum Actions<'a> {
    Stmts(&'a [IrStmt]),
    Exprs(&'a [IrExpr]),
}

struct Emitter<'m, 'p> {
    module: &'m mut Module<'p>,
    file: String,
    /// Entry-block `alloca`s, emitted ahead of the body.
    allocas: String,
    body: String,
    /// The counter that keeps temporaries and labels unique.
    next: usize,
    /// Whether the current block has ended in a terminator.
    terminated: bool,
    /// The label of the block being emitted, for `phi` predecessors.
    current: String,
    scopes: Vec<HashMap<String, Place>>,
    loops: Vec<Loop>,
    /// Names of locals whose address is taken, allocated on the heap.
    heap: HashSet<String>,
}

impl Emitter<'_, '_> {
    // --- emission primitives ---

    fn fresh(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    /// Emit an instruction, opening a fresh (unreachable) block first if the current
    /// one has already terminated, so code after `return` or `break` stays valid.
    fn inst(&mut self, instruction: &str) {
        if self.terminated {
            let label = self.fresh("dead");
            writeln!(self.body, "{label}:").expect("writing to a String cannot fail");
            self.current = label;
            self.terminated = false;
        }
        writeln!(self.body, "  {instruction}").expect("writing to a String cannot fail");
    }

    /// Emit an instruction yielding a value, returning the temporary holding it.
    fn assign(&mut self, rhs: &str) -> String {
        let temp = self.fresh("%t");
        self.inst(&format!("{temp} = {rhs}"));
        temp
    }

    fn terminate(&mut self, instruction: &str) {
        self.inst(instruction);
        self.terminated = true;
    }

    /// Begin the block `label`, falling through into it from an open current block.
    fn start_block(&mut self, label: &str) {
        if !self.terminated {
            writeln!(self.body, "  br label %{label}").expect("writing to a String cannot fail");
        }
        writeln!(self.body, "{label}:").expect("writing to a String cannot fail");
        self.current = label.to_string();
        self.terminated = false;
    }

    fn unsupported<T>(&self, what: &str, span: Span) -> Result<T, CodegenError> {
        Err(CodegenError::Unsupported {
            what: what.to_string(),
            file: self.file.clone(),
            span,
        })
    }

    /// Branch to a runtime panic reporting `what` at `span` when `condition` holds,
    /// continuing in a fresh block otherwise.
    fn panic_if(&mut self, condition: &str, what: &str, span: Span) {
        let message = format!("{what} at {}", self.module.locate(&self.file, span));
        let text = self.module.string(&message);
        self.module.declare(PANIC_DECL);
        let fail = self.fresh("panic");
        let ok = self.fresh("ok");
        self.terminate(&format!("br i1 {condition}, label %{fail}, label %{ok}"));
        self.start_block(&fail);
        self.inst(&format!("call void @cooper_panic(ptr {text})"));
        self.terminate("unreachable");
        self.start_block(&ok);
    }

    // --- types and variables ---

    /// The LLVM type of a value of type `ty`, or `None` for unit.
    fn value_type(&mut self, ty: &Type, span: Span) -> Result<Option<String>, CodegenError> {
        self.module
            .llvm_type(ty)
            .or_else(|what| self.unsupported(&what, span))
    }

    /// The LLVM type of `ty` as a function's return type, where unit is `void`.
    fn signature_type(&mut self, ty: &Type, span: Span) -> Result<String, CodegenError> {
        Ok(self.value_type(ty, span)?.unwrap_or_else(|| "void".to_string()))
    }

    /// Fresh storage for a value of type `ty`, for the local `name` if it has one: on
    /// the heap when that local's address is taken, in the frame otherwise.
    fn slot(&mut self, name: Option<&str>, ty: &Type, span: Span) -> Result<Place, CodegenError> {
        let llvm = self.value_type(ty, span)?;
        let ptr = match &llvm {
            None => String::new(),
            Some(llvm) if name.is_some_and(|n| self.heap.contains(n)) => self.heap_alloc(llvm),
            Some(llvm) => {
                let ptr = self.fresh("%v");
                writeln!(self.allocas, "  {ptr} = alloca {llvm}")
                    .expect("writing to a String cannot fail");
                ptr
            }
        };
        Ok(Place {
            ptr,
            ty: ty.clone(),
            llvm,
            nullable: false,
        })
    }

    /// Declare a new local named `name` in the innermost scope, shadowing any outer one.
    fn bind(&mut self, name: &str, ty: &Type, span: Span) -> Result<Place, CodegenError> {
        let slot = self.slot(Some(name), ty, span)?;
        self.scopes
            .last_mut()
            .expect("a function body has a scope")
            .insert(name.to_string(), slot.clone());
        Ok(slot)
    }

    fn lookup(&self, name: &str) -> Place {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .cloned()
            .unwrap_or_else(|| unreachable!("a checked program binds `{name}` before use"))
    }

    /// The pointer an access to `place` uses: its own, or `nil_target` when a nullable
    /// place is null. Records the size the nil buffers must cover.
    fn access_ptr(&mut self, place: &Place, nil_target: &str) -> String {
        if !place.nullable {
            return place.ptr.clone();
        }
        let size = self.module.size_bound(&place.ty);
        self.module.nil_size = self.module.nil_size.max(size);
        let is_nil = self.assign(&format!("icmp eq ptr {}, null", place.ptr));
        self.assign(&format!("select i1 {is_nil}, ptr {nil_target}, ptr {}", place.ptr))
    }

    fn load(&mut self, place: &Place) -> Option<Value> {
        let ty = place.llvm.clone()?;
        let ptr = self.access_ptr(place, "@cooper.zero");
        let operand = self.assign(&format!("load {ty}, ptr {ptr}"));
        Some(Value { ty, operand })
    }

    fn store(&mut self, place: &Place, value: Option<&Value>) {
        let Some(value) = value else { return };
        let ptr = self.access_ptr(place, "@cooper.sink");
        self.inst(&format!("store {} {}, ptr {ptr}", value.ty, value.operand));
    }

    /// The place `expr` denotes, if it is one: a variable, a field of a place, or a
    /// dereference. `None` for any other expression, which is then left unevaluated.
    fn place(&mut self, expr: &IrExpr) -> Result<Option<Place>, CodegenError> {
        match &expr.kind {
            IrExprKind::Var(name) => Ok(Some(self.lookup(name))),
            IrExprKind::Deref(pointer) => self.pointee(pointer).map(Some),
            IrExprKind::Field { target, name } => {
                let base = match &target.ty {
                    Type::Pointer(_) => self.pointee(target)?,
                    _ => match self.place(target)? {
                        Some(base) => base,
                        None => return Ok(None),
                    },
                };
                self.field_place(&base, name, expr.span).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// The nullable place `pointer` (an expression of pointer type) points to.
    fn pointee(&mut self, pointer: &IrExpr) -> Result<Place, CodegenError> {
        let Type::Pointer(target) = &pointer.ty else {
            unreachable!("a dereference is of a pointer");
        };
        let ptr = self.expr(pointer)?.expect("a pointer has a value").operand;
        let llvm = self.value_type(target, pointer.span)?;
        Ok(Place {
            ptr,
            ty: (**target).clone(),
            llvm,
            nullable: true,
        })
    }

    /// The place of field `name` within the struct at `base`. A null base yields a
    /// null field place.
    fn field_place(&mut self, base: &Place, name: &str, span: Span) -> Result<Place, CodegenError> {
        let (index, ty) = self.module.field(&base.ty, name);
        let llvm = self.value_type(&ty, span)?;
        let aggregate = base.llvm.clone().expect("a struct has an LLVM type");
        let mut ptr = self.assign(&format!(
            "getelementptr {aggregate}, ptr {}, i32 0, i32 {index}",
            base.ptr
        ));
        if base.nullable {
            let is_nil = self.assign(&format!("icmp eq ptr {}, null", base.ptr));
            ptr = self.assign(&format!("select i1 {is_nil}, ptr null, ptr {ptr}"));
        }
        Ok(Place {
            ptr,
            ty,
            llvm,
            nullable: base.nullable,
        })
    }

    /// Component `index` (of type `ty`) of the aggregate `value`.
    fn extract(&mut self, value: &Value, index: usize, ty: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        let Some(llvm) = self.value_type(ty, span)? else {
            return Ok(None);
        };
        let operand = self.assign(&format!("extractvalue {} {}, {index}", value.ty, value.operand));
        Ok(Some(Value { ty: llvm, operand }))
    }

    /// An aggregate of LLVM type `ty` holding `fields` at their positions; a missing
    /// (unit) field keeps the zero-initialized empty struct.
    fn aggregate(&mut self, ty: &str, fields: Vec<Option<Value>>) -> Value {
        let mut operand = "zeroinitializer".to_string();
        for (index, field) in fields.into_iter().enumerate() {
            if let Some(field) = field {
                operand = self.assign(&format!(
                    "insertvalue {ty} {operand}, {} {}, {index}",
                    field.ty, field.operand
                ));
            }
        }
        Value {
            ty: ty.to_string(),
            operand,
        }
    }

    /// Record in `heap` the local whose address `expr` takes, explicitly with `&` or
    /// implicitly as the value receiver of a pointer-receiver method.
    fn note_escape(&self, expr: &IrExpr, heap: &mut HashSet<String>) {
        let addressed = match &expr.kind {
            IrExprKind::AddressOf(operand) => Some(operand.as_ref()),
            IrExprKind::Method {
                receiver,
                item,
                type_args,
            } => {
                let symbol = mangle::symbol(item, type_args);
                let wants_pointer =
                    matches!(self.module.receivers.get(&symbol), Some(Type::Pointer(_)));
                (wants_pointer && !matches!(receiver.ty, Type::Pointer(_))).then_some(receiver.as_ref())
            }
            _ => None,
        };
        if let Some(name) = addressed.and_then(root_local) {
            heap.insert(name.to_string());
        }
    }

    /// Run `body` in a nested variable scope.
    fn scoped<T>(&mut self, body: impl FnOnce(&mut Self) -> T) -> T {
        self.scopes.push(HashMap::new());
        let result = body(self);
        self.scopes.pop();
        result
    }

    // --- statements ---

    fn stmts(&mut self, stmts: &[IrStmt]) -> Result<(), CodegenError> {
        stmts.iter().try_for_each(|s| self.stmt(s))
    }

    fn stmt(&mut self, stmt: &IrStmt) -> Result<(), CodegenError> {
        match &stmt.kind {
            IrStmtKind::Var { name, ty, init } => {
                let value = match init {
                    Some(init) => self.expr(init)?,
                    None => self.zero(ty, stmt.span)?,
                };
                let slot = self.bind(name, ty, stmt.span)?;
                self.store(&slot, value.as_ref());
            }
            IrStmtKind::Expr(expr) => {
                self.expr(expr)?;
            }
            IrStmtKind::Return(value) => {
                let value = match value {
                    Some(expr) => self.expr(expr)?,
                    None => None,
                };
                match value {
                    Some(Value { ty, operand }) => self.terminate(&format!("ret {ty} {operand}")),
                    None => self.terminate("ret void"),
                }
            }
            IrStmtKind::While {
                cond,
                body,
                post_test,
                until,
            } => self.while_loop(cond, body, *post_test, *until)?,
            IrStmtKind::ForRange {
                var,
                ty,
                start,
                end,
                inclusive,
                body,
            } => self.for_range(var, ty, start, end, *inclusive, body, stmt.span)?,
            IrStmtKind::ForEach { .. } => return self.unsupported("array iteration", stmt.span),
            IrStmtKind::Block(stmts) => self.scoped(|e| e.stmts(stmts))?,
            IrStmtKind::Break | IrStmtKind::Continue => {
                let target = self.loops.last().expect("a checked break or continue is in a loop");
                let label = match stmt.kind {
                    IrStmtKind::Break => target.break_to.clone(),
                    _ => target.continue_to.clone(),
                };
                self.terminate(&format!("br label %{label}"));
            }
            IrStmtKind::Match {
                scrutinee,
                actions,
                tree,
            } => {
                self.matching(scrutinee, tree, Actions::Stmts(actions), &Type::Unit)?;
            }
        }
        Ok(())
    }

    /// The zero value of `ty`, which an uninitialized `let` takes: all bits zero.
    fn zero(&mut self, ty: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        let Some(llvm) = self.value_type(ty, span)? else {
            return Ok(None);
        };
        let operand = match Scalar::of(ty) {
            Some(Scalar::Bool) => "false",
            Some(Scalar::Int { .. }) => "0",
            Some(Scalar::Float { .. }) => "0.0",
            None if llvm == "ptr" => "null",
            None => "zeroinitializer",
        };
        Ok(Some(Value {
            ty: llvm,
            operand: operand.to_string(),
        }))
    }

    /// The four conditional loops. A pre-test loop checks before each pass; a post-test
    /// loop runs its body first. An `until` loop continues while its condition is false.
    fn while_loop(
        &mut self,
        cond: &IrExpr,
        body: &[IrStmt],
        post_test: bool,
        until: bool,
    ) -> Result<(), CodegenError> {
        let test = self.fresh("cond");
        let pass = self.fresh("body");
        let exit = self.fresh("exit");
        self.start_block(if post_test { &pass } else { &test });
        if !post_test {
            self.branch_on(cond, until, &pass, &exit)?;
            self.start_block(&pass);
        }
        self.loops.push(Loop {
            continue_to: test.clone(),
            break_to: exit.clone(),
        });
        let result = self.scoped(|e| e.stmts(body));
        self.loops.pop();
        result?;
        if post_test {
            self.start_block(&test);
            self.branch_on(cond, until, &pass, &exit)?;
        } else {
            self.terminate(&format!("br label %{test}"));
        }
        self.start_block(&exit);
        Ok(())
    }

    /// Branch to `on_true` when `cond` holds (or, if `negate`, when it does not).
    fn branch_on(
        &mut self,
        cond: &IrExpr,
        negate: bool,
        on_true: &str,
        on_false: &str,
    ) -> Result<(), CodegenError> {
        let value = self.expr(cond)?.expect("a condition is a bool");
        let (yes, no) = if negate { (on_false, on_true) } else { (on_true, on_false) };
        self.terminate(&format!("br i1 {}, label %{yes}, label %{no}", value.operand));
        Ok(())
    }

    /// `for var in start..end` (or `..=`). Both bounds are evaluated once. An exclusive
    /// range tests before each pass; an inclusive one tests for the last value before
    /// stepping, so a range ending at the type's maximum never overflows its counter.
    #[allow(clippy::too_many_arguments)]
    fn for_range(
        &mut self,
        var: &str,
        ty: &Type,
        start: &IrExpr,
        end: &IrExpr,
        inclusive: bool,
        body: &[IrStmt],
        span: Span,
    ) -> Result<(), CodegenError> {
        let Some(Scalar::Int { bits, signed }) = Scalar::of(ty) else {
            return self.unsupported(&format!("ranges over {ty}"), span);
        };
        let int = format!("i{bits}");
        let (less, less_eq) = if signed { ("slt", "sle") } else { ("ult", "ule") };
        let first = self.expr(start)?.expect("a range bound is an integer");
        let last = self.expr(end)?.expect("a range bound is an integer");
        let head = self.fresh("head");
        let pass = self.fresh("body");
        let step = self.fresh("step");
        let exit = self.fresh("exit");
        self.scoped(|e| -> Result<(), CodegenError> {
            let counter = e.bind(var, ty, span)?;
            e.store(&counter, Some(&first));
            if inclusive {
                let enter = e.assign(&format!("icmp {less_eq} {int} {}, {}", first.operand, last.operand));
                e.terminate(&format!("br i1 {enter}, label %{pass}, label %{exit}"));
            } else {
                e.start_block(&head);
                let i = e.load(&counter).expect("a counter has a value");
                let more = e.assign(&format!("icmp {less} {int} {}, {}", i.operand, last.operand));
                e.terminate(&format!("br i1 {more}, label %{pass}, label %{exit}"));
            }
            e.start_block(&pass);
            e.loops.push(Loop {
                continue_to: step.clone(),
                break_to: exit.clone(),
            });
            let result = e.scoped(|e| e.stmts(body));
            e.loops.pop();
            result?;
            e.start_block(&step);
            let i = e.load(&counter).expect("a counter has a value");
            if inclusive {
                let done = e.assign(&format!("icmp eq {int} {}, {}", i.operand, last.operand));
                let next = e.fresh("next");
                e.terminate(&format!("br i1 {done}, label %{exit}, label %{next}"));
                e.start_block(&next);
            }
            let flag = if signed { "nsw" } else { "nuw" };
            let stepped = e.assign(&format!("add {flag} {int} {}, 1", i.operand));
            e.store(&counter, Some(&Value { ty: int.clone(), operand: stepped }));
            e.terminate(&format!("br label %{}", if inclusive { &pass } else { &head }));
            Ok(())
        })?;
        self.start_block(&exit);
        Ok(())
    }

    // --- match ---

    /// Evaluate `scrutinee` once and run the action its decision tree selects. An
    /// expression `match` of non-unit `result` type yields the selected action's value.
    fn matching(
        &mut self,
        scrutinee: &IrExpr,
        tree: &Decision,
        actions: Actions,
        result: &Type,
    ) -> Result<Option<Value>, CodegenError> {
        let value = self.expr(scrutinee)?;
        let count = match actions {
            Actions::Stmts(stmts) => stmts.len(),
            Actions::Exprs(exprs) => exprs.len(),
        };
        // Each action's bindings get one slot, shared by every leaf that reaches it.
        let mut bindings: Vec<Vec<&MatchBinding>> = vec![Vec::new(); count];
        collect_bindings(tree, &mut bindings);
        let mut slots: Vec<HashMap<String, Place>> = Vec::with_capacity(count);
        for action_bindings in &bindings {
            let mut action_slots = HashMap::new();
            for binding in action_bindings {
                if !action_slots.contains_key(&binding.name) {
                    let slot = self.slot(Some(&binding.name), &binding.ty, scrutinee.span)?;
                    action_slots.insert(binding.name.clone(), slot);
                }
            }
            slots.push(action_slots);
        }
        let result_slot = match actions {
            Actions::Exprs(_) if !matches!(result, Type::Unit) => {
                Some(self.slot(None, result, scrutinee.span)?)
            }
            _ => None,
        };
        let labels: Vec<String> = (0..count).map(|_| self.fresh("arm")).collect();
        let join = self.fresh("join");
        let root = (value, scrutinee.ty.clone());
        self.decision(tree, &root, &labels, &slots, scrutinee.span)?;
        for (index, label) in labels.iter().enumerate() {
            self.start_block(label);
            self.scopes.push(slots[index].clone());
            let outcome = match actions {
                Actions::Stmts(stmts) => self.stmt(&stmts[index]).map(|()| None),
                Actions::Exprs(exprs) => self.expr(&exprs[index]),
            };
            self.scopes.pop();
            let outcome = outcome?;
            if let Some(slot) = &result_slot {
                self.store(slot, outcome.as_ref());
            }
            self.terminate(&format!("br label %{join}"));
        }
        self.start_block(&join);
        Ok(result_slot.and_then(|slot| self.load(&slot)))
    }

    fn decision(
        &mut self,
        tree: &Decision,
        scrutinee: &(Option<Value>, Type),
        labels: &[String],
        slots: &[HashMap<String, Place>],
        span: Span,
    ) -> Result<(), CodegenError> {
        match tree {
            Decision::Leaf { bindings, action } => {
                for binding in bindings {
                    let (value, _) = self.access(&binding.access, scrutinee, span)?;
                    let slot = slots[*action][&binding.name].clone();
                    self.store(&slot, value.as_ref());
                }
                self.terminate(&format!("br label %{}", labels[*action]));
            }
            Decision::Fail => self.terminate("unreachable"),
            Decision::Switch {
                access,
                ty,
                cases,
                default,
            } => {
                let value = self
                    .access(access, scrutinee, span)?
                    .0
                    .expect("a switched value is a scalar");
                let case_labels: Vec<String> = cases.iter().map(|_| self.fresh("case")).collect();
                let default_label = self.fresh("default");
                match Scalar::of(ty) {
                    Some(Scalar::Bool) => {
                        let target = |want: bool| {
                            cases
                                .iter()
                                .position(|c| c.test == Test::Bool(want))
                                .map_or(default_label.clone(), |i| case_labels[i].clone())
                        };
                        let (yes, no) = (target(true), target(false));
                        self.terminate(&format!("br i1 {}, label %{yes}, label %{no}", value.operand));
                    }
                    Some(Scalar::Int { bits, .. }) => {
                        let mut arms = String::new();
                        for (case, label) in cases.iter().zip(&case_labels) {
                            let Test::Int { value: magnitude, negative } = case.test else {
                                unreachable!("an integer switch tests integers");
                            };
                            let constant = if negative {
                                -(magnitude as i128)
                            } else {
                                magnitude as i128
                            };
                            write!(arms, " i{bits} {}, label %{label}", int_constant(constant, bits))
                                .expect("writing to a String cannot fail");
                        }
                        self.terminate(&format!(
                            "switch {} {}, label %{default_label} [{arms} ]",
                            value.ty, value.operand
                        ));
                    }
                    None if matches!(ty, Type::Oneof { .. }) => {
                        let tag = self.tag(&value);
                        let mut arms = String::new();
                        for (case, label) in cases.iter().zip(&case_labels) {
                            let Test::Variant(variant) = &case.test else {
                                unreachable!("a sum-type switch tests variants");
                            };
                            let (index, _) = self.module.variant(ty, variant);
                            write!(arms, " i32 {index}, label %{label}")
                                .expect("writing to a String cannot fail");
                        }
                        self.terminate(&format!("switch i32 {tag}, label %{default_label} [{arms} ]"));
                    }
                    _ => unreachable!("a switch tests a bool, an integer, or a sum type"),
                }
                for (case, label) in cases.iter().zip(&case_labels) {
                    self.start_block(label);
                    self.decision(&case.tree, scrutinee, labels, slots, span)?;
                }
                self.start_block(&default_label);
                match default {
                    Some(default) => self.decision(default, scrutinee, labels, slots, span)?,
                    None => self.terminate("unreachable"),
                }
            }
        }
        Ok(())
    }

    /// The subvalue of the scrutinee at `access`, with its type.
    fn access(
        &mut self,
        access: &Access,
        scrutinee: &(Option<Value>, Type),
        span: Span,
    ) -> Result<(Option<Value>, Type), CodegenError> {
        let (parent, index, ty) = match access {
            Access::Root => return Ok(scrutinee.clone()),
            Access::Field { parent, name } => {
                let (parent, parent_ty) = self.access(parent, scrutinee, span)?;
                let (index, ty) = self.module.field(&parent_ty, name);
                (parent, index, ty)
            }
            Access::Elem { parent, index } => {
                let (parent, parent_ty) = self.access(parent, scrutinee, span)?;
                let Type::Tuple(elems) = parent_ty else {
                    unreachable!("an element access is into a tuple");
                };
                (parent, *index, elems[*index].clone())
            }
            Access::Payload {
                parent,
                variant,
                index,
            } => {
                let (parent, parent_ty) = self.access(parent, scrutinee, span)?;
                let parent = parent.expect("a sum type has a value");
                return self.payload_slot(&parent, &parent_ty, variant, *index, span);
            }
        };
        let parent = parent.expect("an aggregate has a value");
        Ok((self.extract(&parent, index, &ty, span)?, ty))
    }

    // --- expressions ---

    /// Emit `expr`, returning its value, or `None` for a unit-typed expression.
    fn expr(&mut self, expr: &IrExpr) -> Result<Option<Value>, CodegenError> {
        let span = expr.span;
        match &expr.kind {
            IrExprKind::Int(magnitude) => self.literal(&expr.ty, *magnitude as f64, *magnitude as i128, span),
            IrExprKind::Float(value) => self.literal(&expr.ty, *value, 0, span),
            IrExprKind::Bool(b) => Ok(Some(Value {
                ty: "i1".to_string(),
                operand: b.to_string(),
            })),
            IrExprKind::Unit => Ok(None),
            IrExprKind::Var(_) | IrExprKind::Deref(_) => {
                let place = self.place(expr)?.expect("a variable or dereference is a place");
                Ok(self.load(&place))
            }
            IrExprKind::Field { target, name } => match self.place(expr)? {
                Some(place) => Ok(self.load(&place)),
                None => {
                    let aggregate = self.expr(target)?.expect("a struct has a value");
                    let (index, ty) = self.module.field(&target.ty, name);
                    self.extract(&aggregate, index, &ty, span)
                }
            },
            IrExprKind::AddressOf(operand) => match self.place(operand)? {
                Some(place) if place.llvm.is_some() => Ok(Some(Value {
                    ty: "ptr".to_string(),
                    operand: place.ptr,
                })),
                _ => self.unsupported("taking this address", span),
            },
            IrExprKind::Nil => Ok(Some(Value {
                ty: "ptr".to_string(),
                operand: "null".to_string(),
            })),
            IrExprKind::StructLiteral { members, .. } => {
                let llvm = self.value_type(&expr.ty, span)?.expect("a struct has an LLVM type");
                // Members are evaluated in source order, then placed in layout order.
                let mut values = HashMap::new();
                for (name, value) in members {
                    values.insert(name.as_str(), self.expr(value)?);
                }
                let order = self.module.defs.struct_members(&expr.ty).expect("a defined struct");
                let fields = order
                    .iter()
                    .map(|(name, _)| values.remove(name.as_str()).flatten())
                    .collect();
                Ok(Some(self.aggregate(&llvm, fields)))
            }
            IrExprKind::Tuple(elems) => {
                let llvm = self.value_type(&expr.ty, span)?.expect("a tuple has an LLVM type");
                let fields = elems.iter().map(|e| self.expr(e)).collect::<Result<_, _>>()?;
                Ok(Some(self.aggregate(&llvm, fields)))
            }
            IrExprKind::LetTuple { bindings, value } => {
                let tuple = self.expr(value)?.expect("a tuple has a value");
                for (index, binder) in bindings.iter().enumerate() {
                    let component = self.extract(&tuple, index, &binder.ty, span)?;
                    let slot = self.bind(&binder.name, &binder.ty, span)?;
                    self.store(&slot, component.as_ref());
                }
                Ok(Some(tuple))
            }
            IrExprKind::Unary { op, operand } => self.unary(*op, operand, &expr.ty, span),
            IrExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, span),
            IrExprKind::Call { callee, args } => self.call(callee, args, &expr.ty, span),
            IrExprKind::Convert(operand) => self.convert(operand, &expr.ty, span),
            IrExprKind::Assign { op, target, value } => self.assignment(*op, target, value, span),
            IrExprKind::Let { name, value } => {
                let value_ir = value;
                let value = self.expr(value_ir)?;
                let slot = self.bind(name, &value_ir.ty, span)?;
                self.store(&slot, value.as_ref());
                Ok(value)
            }
            IrExprKind::Block { stmts, result } => self.scoped(|e| {
                e.stmts(stmts)?;
                e.expr(result)
            }),
            IrExprKind::Match {
                scrutinee,
                actions,
                tree,
            } => self.matching(scrutinee, tree, Actions::Exprs(actions), &expr.ty),
            IrExprKind::Str(_) => self.unsupported("strings", span),
            IrExprKind::FuncRef { item, type_args } => {
                let symbol = mangle::symbol(item, type_args);
                self.function_value(&symbol, &expr.ty, span).map(Some)
            }
            IrExprKind::Method {
                receiver,
                item,
                type_args,
            } => {
                let symbol = mangle::symbol(item, type_args);
                self.bound_method(receiver, &symbol, &expr.ty, span).map(Some)
            }
            IrExprKind::Intrinsic { .. } => self.unsupported("arrays", span),
            IrExprKind::Variant { variant, args } => {
                let values = args.iter().map(|a| self.expr(a)).collect::<Result<Vec<_>, _>>()?;
                self.construct(&expr.ty, variant, values, span).map(Some)
            }
        }
    }

    /// A numeric literal of type `ty`: `int` when it is an integer type, `float` when
    /// it is a float type (an integer literal may take a float type).
    fn literal(&mut self, ty: &Type, float: f64, int: i128, span: Span) -> Result<Option<Value>, CodegenError> {
        let operand = match Scalar::of(ty) {
            Some(Scalar::Int { bits, .. }) => int_constant(int, bits),
            Some(Scalar::Float { bits }) => float_constant(float, bits),
            _ => return self.unsupported(&format!("literals of type {ty}"), span),
        };
        Ok(Some(Value {
            ty: Scalar::of(ty).expect("checked above").llvm(),
            operand,
        }))
    }

    fn unary(
        &mut self,
        op: UnaryOp,
        operand: &IrExpr,
        ty: &Type,
        span: Span,
    ) -> Result<Option<Value>, CodegenError> {
        match op {
            UnaryOp::Pos => self.expr(operand),
            UnaryOp::Not => {
                let value = self.expr(operand)?.expect("`!` applies to a bool");
                let operand = self.assign(&format!("xor i1 {}, true", value.operand));
                Ok(Some(Value { ty: value.ty, operand }))
            }
            UnaryOp::Neg => {
                // A negated literal is a constant: `-128` as an `i8` is in range, though
                // `128` alone is not.
                match &operand.kind {
                    IrExprKind::Int(magnitude) => {
                        return self.literal(ty, -(*magnitude as f64), -(*magnitude as i128), span)
                    }
                    IrExprKind::Float(value) => return self.literal(ty, -value, 0, span),
                    _ => {}
                }
                let value = self.expr(operand)?.expect("`-` applies to a number");
                match Scalar::of(ty) {
                    Some(Scalar::Float { .. }) => {
                        let operand = self.assign(&format!("fneg {} {}", value.ty, value.operand));
                        Ok(Some(Value { ty: value.ty, operand }))
                    }
                    Some(scalar @ Scalar::Int { .. }) => {
                        let zero = Value {
                            ty: value.ty.clone(),
                            operand: "0".to_string(),
                        };
                        let operand = self.arithmetic(BinaryOp::Sub, scalar, &zero, &value, span);
                        Ok(Some(Value { ty: value.ty, operand }))
                    }
                    _ => self.unsupported(&format!("negating {ty}"), span),
                }
            }
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        lhs: &IrExpr,
        rhs: &IrExpr,
        span: Span,
    ) -> Result<Option<Value>, CodegenError> {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            return self.short_circuit(op, lhs, rhs).map(Some);
        }
        let Some(scalar) = Scalar::of(&lhs.ty) else {
            if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
                return self.unsupported(&format!("`{}` on {}", op.symbol(), lhs.ty), span);
            }
            let left = self.expr(lhs)?;
            let right = self.expr(rhs)?;
            let mut operand = self.equal(&lhs.ty, left.as_ref(), right.as_ref(), span)?;
            if op == BinaryOp::Ne {
                operand = self.assign(&format!("xor i1 {operand}, true"));
            }
            return Ok(Some(Value {
                ty: "i1".to_string(),
                operand,
            }));
        };
        let left = self.expr(lhs)?.expect("a scalar operand has a value");
        let right = self.expr(rhs)?.expect("a scalar operand has a value");
        let comparison = match op {
            BinaryOp::Eq => Some(("eq", "eq", "oeq")),
            BinaryOp::Ne => Some(("ne", "ne", "une")),
            BinaryOp::Lt => Some(("slt", "ult", "olt")),
            BinaryOp::Le => Some(("sle", "ule", "ole")),
            BinaryOp::Gt => Some(("sgt", "ugt", "ogt")),
            BinaryOp::Ge => Some(("sge", "uge", "oge")),
            _ => None,
        };
        if let Some((signed_cmp, unsigned_cmp, float_cmp)) = comparison {
            let instruction = match scalar {
                Scalar::Int { signed: true, .. } => format!("icmp {signed_cmp}"),
                Scalar::Int { signed: false, .. } | Scalar::Bool => format!("icmp {unsigned_cmp}"),
                Scalar::Float { .. } => format!("fcmp {float_cmp}"),
            };
            let operand = self.assign(&format!(
                "{instruction} {} {}, {}",
                left.ty, left.operand, right.operand
            ));
            return Ok(Some(Value {
                ty: "i1".to_string(),
                operand,
            }));
        }
        let operand = self.arithmetic(op, scalar, &left, &right, span);
        Ok(Some(Value {
            ty: left.ty,
            operand,
        }))
    }

    /// Whether two values of comparable type `ty` are equal: scalars by value (floats by
    /// IEEE), pointers by address, and aggregates field by field.
    fn equal(
        &mut self,
        ty: &Type,
        left: Option<&Value>,
        right: Option<&Value>,
        span: Span,
    ) -> Result<String, CodegenError> {
        let (Some(left), Some(right)) = (left, right) else {
            return Ok("true".to_string()); // unit
        };
        let (l, r) = (&left.operand, &right.operand);
        let components: Vec<Type> = match ty {
            Type::Tuple(elems) => elems.clone(),
            Type::Struct { .. } => self
                .module
                .defs
                .struct_members(ty)
                .expect("a defined struct")
                .into_iter()
                .map(|(_, member)| member)
                .collect(),
            Type::Pointer(_) | Type::Nil => return Ok(self.assign(&format!("icmp eq ptr {l}, {r}"))),
            Type::Oneof { .. } => return self.equal_variants(ty, left, right, span),
            _ => match Scalar::of(ty) {
                Some(Scalar::Float { .. }) => return Ok(self.assign(&format!("fcmp oeq {} {l}, {r}", left.ty))),
                Some(_) => return Ok(self.assign(&format!("icmp eq {} {l}, {r}", left.ty))),
                None => return self.unsupported(&format!("comparing values of type {ty}"), span),
            },
        };
        let mut all = "true".to_string();
        for (index, component) in components.iter().enumerate() {
            let a = self.extract(left, index, component, span)?;
            let b = self.extract(right, index, component, span)?;
            let same = self.equal(component, a.as_ref(), b.as_ref(), span)?;
            all = self.assign(&format!("and i1 {all}, {same}"));
        }
        Ok(all)
    }

    /// Whether two sum-type values are equal: the same variant, with equal payloads.
    fn equal_variants(&mut self, ty: &Type, left: &Value, right: &Value, span: Span) -> Result<String, CodegenError> {
        let left_tag = self.tag(left);
        let right_tag = self.tag(right);
        let same_tag = self.assign(&format!("icmp eq i32 {left_tag}, {right_tag}"));
        let differ_from = self.current.clone();
        let dispatch = self.fresh("eqtag");
        let join = self.fresh("eqjoin");
        self.terminate(&format!("br i1 {same_tag}, label %{dispatch}, label %{join}"));
        self.start_block(&dispatch);
        let variants = self.module.defs.variant_order(ty).to_vec();
        let labels: Vec<String> = variants.iter().map(|_| self.fresh("eqvariant")).collect();
        let arms: String = labels
            .iter()
            .enumerate()
            .map(|(index, label)| format!(" i32 {index}, label %{label}"))
            .collect();
        let none = self.fresh("eqnone");
        self.terminate(&format!("switch i32 {left_tag}, label %{none} [{arms} ]"));
        self.start_block(&none);
        self.terminate("unreachable");
        let mut incoming = vec![format!("[ false, %{differ_from} ]")];
        for (variant, label) in variants.iter().zip(&labels) {
            self.start_block(label);
            let (_, slots) = self.module.variant(ty, variant);
            let mut all = "true".to_string();
            for (index, slot) in slots.iter().enumerate() {
                let (a, _) = self.payload_slot(left, ty, variant, index, span)?;
                let (b, _) = self.payload_slot(right, ty, variant, index, span)?;
                let same = self.equal(slot, a.as_ref(), b.as_ref(), span)?;
                all = self.assign(&format!("and i1 {all}, {same}"));
            }
            incoming.push(format!("[ {all}, %{} ]", self.current));
            self.terminate(&format!("br label %{join}"));
        }
        self.start_block(&join);
        Ok(self.assign(&format!("phi i1 {}", incoming.join(", "))))
    }

    /// The variant tag of a sum-type value.
    fn tag(&mut self, value: &Value) -> String {
        self.assign(&format!("extractvalue {} {}, 0", value.ty, value.operand))
    }

    /// A value of sum type `ty`: variant `variant` carrying `payload`. The tag and
    /// payload are written into a zeroed temporary, which reinterprets the payload area
    /// as the variant's payload struct.
    fn construct(&mut self, ty: &Type, variant: &str, payload: Vec<Option<Value>>, span: Span) -> Result<Value, CodegenError> {
        let temp = self.slot(None, ty, span)?;
        let llvm = temp.llvm.clone().expect("a sum type has an LLVM type");
        let (tag, slots) = self.module.variant(ty, variant);
        self.inst(&format!("store {llvm} zeroinitializer, ptr {}", temp.ptr));
        let tag_ptr = self.assign(&format!("getelementptr {llvm}, ptr {}, i32 0, i32 0", temp.ptr));
        self.inst(&format!("store i32 {tag}, ptr {tag_ptr}"));
        if !slots.is_empty() {
            let payload_ty = self.module.payload_type(&slots).or_else(|what| self.unsupported(&what, span))?;
            let fields = self.aggregate(&payload_ty, payload);
            let area = self.assign(&format!("getelementptr {llvm}, ptr {}, i32 0, i32 1", temp.ptr));
            self.inst(&format!("store {payload_ty} {}, ptr {area}", fields.operand));
        }
        Ok(self.load(&temp).expect("a sum type has a value"))
    }

    /// Payload slot `index` of `variant` in the sum-type value `value`, with its type.
    fn payload_slot(
        &mut self,
        value: &Value,
        ty: &Type,
        variant: &str,
        index: usize,
        span: Span,
    ) -> Result<(Option<Value>, Type), CodegenError> {
        let (_, slots) = self.module.variant(ty, variant);
        let slot_ty = slots[index].clone();
        let Some(slot_llvm) = self.value_type(&slot_ty, span)? else {
            return Ok((None, slot_ty));
        };
        let payload_ty = self.module.payload_type(&slots).or_else(|what| self.unsupported(&what, span))?;
        let temp = self.slot(None, ty, span)?;
        self.store(&temp, Some(value));
        let area = self.assign(&format!("getelementptr {}, ptr {}, i32 0, i32 1", value.ty, temp.ptr));
        let field = self.assign(&format!("getelementptr {payload_ty}, ptr {area}, i32 0, i32 {index}"));
        let operand = self.assign(&format!("load {slot_llvm}, ptr {field}"));
        Ok((
            Some(Value {
                ty: slot_llvm,
                operand,
            }),
            slot_ty,
        ))
    }

    /// Arithmetic `op` on two values of one scalar type. Integer overflow, division by
    /// zero, and the overflowing signed division `MIN / -1` stop the program.
    fn arithmetic(&mut self, op: BinaryOp, scalar: Scalar, left: &Value, right: &Value, span: Span) -> String {
        let (l, r, ty) = (&left.operand, &right.operand, &left.ty);
        match scalar {
            Scalar::Float { .. } => {
                let instruction = match op {
                    BinaryOp::Add => "fadd",
                    BinaryOp::Sub => "fsub",
                    BinaryOp::Mul => "fmul",
                    BinaryOp::Div => "fdiv",
                    BinaryOp::Rem => "frem",
                    _ => unreachable!("not an arithmetic operator: {op:?}"),
                };
                self.assign(&format!("{instruction} {ty} {l}, {r}"))
            }
            Scalar::Int { bits, signed } => match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => {
                    let name = format!(
                        "llvm.{}{}.with.overflow.{ty}",
                        if signed { 's' } else { 'u' },
                        match op {
                            BinaryOp::Add => "add",
                            BinaryOp::Sub => "sub",
                            _ => "mul",
                        }
                    );
                    self.module
                        .declare(&format!("declare {{{ty}, i1}} @{name}({ty}, {ty})"));
                    let pair = self.assign(&format!("call {{{ty}, i1}} @{name}({ty} {l}, {ty} {r})"));
                    let result = self.assign(&format!("extractvalue {{{ty}, i1}} {pair}, 0"));
                    let overflow = self.assign(&format!("extractvalue {{{ty}, i1}} {pair}, 1"));
                    self.panic_if(&overflow, "integer overflow", span);
                    result
                }
                BinaryOp::Div | BinaryOp::Rem => {
                    let zero = self.assign(&format!("icmp eq {ty} {r}, 0"));
                    self.panic_if(&zero, "division by zero", span);
                    if signed {
                        let minus_one = self.assign(&format!("icmp eq {ty} {r}, -1"));
                        let min = int_constant(i128::MIN >> (128 - bits), bits);
                        let is_min = self.assign(&format!("icmp eq {ty} {l}, {min}"));
                        let overflow = self.assign(&format!("and i1 {minus_one}, {is_min}"));
                        self.panic_if(&overflow, "integer overflow", span);
                    }
                    let instruction = match (op, signed) {
                        (BinaryOp::Div, true) => "sdiv",
                        (BinaryOp::Div, false) => "udiv",
                        (_, true) => "srem",
                        (_, false) => "urem",
                    };
                    self.assign(&format!("{instruction} {ty} {l}, {r}"))
                }
                _ => unreachable!("not an arithmetic operator: {op:?}"),
            },
            Scalar::Bool => unreachable!("the checker allows no arithmetic on bool"),
        }
    }

    /// `and` / `or`, evaluating `rhs` only when `lhs` does not decide the result.
    fn short_circuit(&mut self, op: BinaryOp, lhs: &IrExpr, rhs: &IrExpr) -> Result<Value, CodegenError> {
        let left = self.expr(lhs)?.expect("a logical operand is a bool");
        let from_left = self.current.clone();
        let right_block = self.fresh("rhs");
        let end = self.fresh("logic");
        let (decided, branch) = match op {
            BinaryOp::And => ("false", format!("br i1 {}, label %{right_block}, label %{end}", left.operand)),
            _ => ("true", format!("br i1 {}, label %{end}, label %{right_block}", left.operand)),
        };
        self.terminate(&branch);
        self.start_block(&right_block);
        let right = self.expr(rhs)?.expect("a logical operand is a bool");
        let from_right = self.current.clone();
        self.start_block(&end);
        let operand = self.assign(&format!(
            "phi i1 [ {decided}, %{from_left} ], [ {}, %{from_right} ]",
            right.operand
        ));
        Ok(Value {
            ty: "i1".to_string(),
            operand,
        })
    }

    /// A numeric conversion `T(x)`. Integer conversions truncate or extend by the
    /// source's signedness; float-to-integer conversions saturate at the target's range
    /// (NaN becomes 0), so no conversion is undefined.
    fn convert(&mut self, operand: &IrExpr, target: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        let (Some(from), Some(to)) = (Scalar::of(&operand.ty), Scalar::of(target)) else {
            return self.unsupported(&format!("converting {} to {target}", operand.ty), span);
        };
        let value = self.expr(operand)?.expect("a converted value is a number");
        let (source, dest) = (from.llvm(), to.llvm());
        let instruction = match (from, to) {
            _ if from == to => return Ok(Some(value)),
            (Scalar::Int { bits: a, signed }, Scalar::Int { bits: b, .. }) => match a.cmp(&b) {
                std::cmp::Ordering::Equal => return Ok(Some(Value { ty: dest, operand: value.operand })),
                std::cmp::Ordering::Greater => format!("trunc {source} {} to {dest}", value.operand),
                std::cmp::Ordering::Less if signed => format!("sext {source} {} to {dest}", value.operand),
                std::cmp::Ordering::Less => format!("zext {source} {} to {dest}", value.operand),
            },
            (Scalar::Int { signed, .. }, Scalar::Float { .. }) => {
                let op = if signed { "sitofp" } else { "uitofp" };
                format!("{op} {source} {} to {dest}", value.operand)
            }
            (Scalar::Float { bits: from_bits }, Scalar::Int { signed, .. }) => {
                let name = format!(
                    "llvm.{}.sat.{dest}.f{from_bits}",
                    if signed { "fptosi" } else { "fptoui" }
                );
                self.module.declare(&format!("declare {dest} @{name}({source})"));
                format!("call {dest} @{name}({source} {})", value.operand)
            }
            (Scalar::Float { bits: a }, Scalar::Float { bits: b }) => {
                let op = if a < b { "fpext" } else { "fptrunc" };
                format!("{op} {source} {} to {dest}", value.operand)
            }
            _ => return self.unsupported(&format!("converting {} to {target}", operand.ty), span),
        };
        let operand = self.assign(&instruction);
        Ok(Some(Value { ty: dest, operand }))
    }

    /// `target = value` or a compound `target op= value`, yielding the stored value.
    /// The value is evaluated before the target's place. A tuple target assigns each
    /// component to its own place.
    fn assignment(
        &mut self,
        op: AssignOp,
        target: &IrExpr,
        value: &IrExpr,
        span: Span,
    ) -> Result<Option<Value>, CodegenError> {
        let value = self.expr(value)?;
        if op == AssignOp::Assign {
            self.assign_to(target, value.as_ref(), span)?;
            return Ok(value);
        }
        let binary = match op {
            AssignOp::Add => BinaryOp::Add,
            AssignOp::Sub => BinaryOp::Sub,
            AssignOp::Mul => BinaryOp::Mul,
            _ => BinaryOp::Div,
        };
        let Some(scalar) = Scalar::of(&target.ty) else {
            return self.unsupported(&format!("`{}` on {}", op.symbol(), target.ty), span);
        };
        let place = self.place(target)?.expect("a checked assignment target is a place");
        let current = self.load(&place).expect("a compound target is a number");
        let value = value.expect("a compound operand is a number");
        let operand = self.arithmetic(binary, scalar, &current, &value, span);
        let stored = Value {
            ty: current.ty,
            operand,
        };
        self.store(&place, Some(&stored));
        Ok(Some(stored))
    }

    /// Store `value` into the place `target` denotes, or component-wise into a tuple of
    /// places.
    fn assign_to(&mut self, target: &IrExpr, value: Option<&Value>, span: Span) -> Result<(), CodegenError> {
        if let IrExprKind::Tuple(targets) = &target.kind {
            let value = value.expect("a tuple has a value");
            for (index, element) in targets.iter().enumerate() {
                let component = self.extract(value, index, &element.ty, span)?;
                self.assign_to(element, component.as_ref(), span)?;
            }
            return Ok(());
        }
        let place = self.place(target)?.expect("a checked assignment target is a place");
        self.store(&place, value);
        Ok(())
    }

    /// A call: direct to a named function or method, or indirect through a function
    /// value, which passes the value's environment as a hidden first argument.
    fn call(&mut self, callee: &IrExpr, args: &[IrExpr], result: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        let mut operands = Vec::with_capacity(args.len() + 1);
        let target = match &callee.kind {
            IrExprKind::FuncRef { item, type_args } => format!("@{}", mangle::symbol(item, type_args)),
            IrExprKind::Method {
                receiver,
                item,
                type_args,
            } => {
                let symbol = mangle::symbol(item, type_args);
                let wanted = self.module.receivers[&symbol].clone();
                let value = self.receiver(receiver, &wanted)?;
                operands.push(format!("{} {}", value.ty, value.operand));
                format!("@{symbol}")
            }
            _ => {
                let function = self.expr(callee)?.expect("a function value has a value");
                let code = self.assign(&format!("extractvalue {FUNC_VALUE} {}, 0", function.operand));
                let env = self.assign(&format!("extractvalue {FUNC_VALUE} {}, 1", function.operand));
                let missing = self.assign(&format!("icmp eq ptr {code}, null"));
                self.panic_if(&missing, "call of a function value that was never assigned", span);
                operands.push(format!("ptr {env}"));
                code
            }
        };
        for arg in args {
            // A unit argument is evaluated for its effects and passes nothing.
            if let Some(Value { ty, operand }) = self.expr(arg)? {
                operands.push(format!("{ty} {operand}"));
            }
        }
        let ret = self.signature_type(result, span)?;
        let call = format!("call {ret} {target}({})", operands.join(", "));
        if ret == "void" {
            self.inst(&call);
            return Ok(None);
        }
        let operand = self.assign(&call);
        Ok(Some(Value { ty: ret, operand }))
    }

    /// A function value for the function instance `symbol`, of function type `ty`: its
    /// thunk, with no environment.
    fn function_value(&mut self, symbol: &str, ty: &Type, span: Span) -> Result<Value, CodegenError> {
        let thunk = self.thunk(symbol, ty, None, span)?;
        Ok(Value {
            ty: FUNC_VALUE.to_string(),
            operand: format!("{{ ptr {thunk}, ptr null }}"),
        })
    }

    /// A method bound to `receiver`: the method's thunk, with the receiver as its
    /// environment. A pointer receiver is the environment itself; a value receiver is
    /// copied to the heap when bound, so later changes to the original do not reach it.
    fn bound_method(&mut self, receiver: &IrExpr, symbol: &str, ty: &Type, span: Span) -> Result<Value, CodegenError> {
        let wanted = self.module.receivers[symbol].clone();
        let value = self.receiver(receiver, &wanted)?;
        let env = if matches!(wanted, Type::Pointer(_)) {
            value.operand
        } else {
            let copy = self.heap_alloc(&value.ty);
            self.inst(&format!("store {} {}, ptr {copy}", value.ty, value.operand));
            copy
        };
        let thunk = self.thunk(symbol, ty, Some(&wanted), span)?;
        let pointer = |operand: String| Some(Value {
            ty: "ptr".to_string(),
            operand,
        });
        Ok(self.aggregate(FUNC_VALUE, vec![pointer(thunk), pointer(env)]))
    }

    /// The thunk giving `symbol` the function-value calling convention, defined on
    /// first use: it takes the hidden environment and calls `symbol` with the given
    /// arguments, first loading the receiver from the environment for a value-receiver
    /// method (`receiver`), or passing the environment for a pointer-receiver one.
    fn thunk(&mut self, symbol: &str, ty: &Type, receiver: Option<&Type>, span: Span) -> Result<String, CodegenError> {
        let Type::Func {
            param_types,
            return_type,
        } = ty
        else {
            unreachable!("a function value has a function type");
        };
        let name = format!("@{symbol}.{}", if receiver.is_some() { "bound" } else { "fn" });
        if self.module.thunks.contains_key(&name) {
            return Ok(name);
        }
        let ret = self.signature_type(return_type, span)?;
        let mut params = vec!["ptr %env".to_string()];
        let mut args = Vec::new();
        let mut body = String::new();
        match receiver {
            Some(Type::Pointer(_)) => args.push("ptr %env".to_string()),
            Some(value_receiver) => {
                let llvm = self.value_type(value_receiver, span)?.expect("a receiver has a value");
                body.push_str(&format!("  %receiver = load {llvm}, ptr %env\n"));
                args.push(format!("{llvm} %receiver"));
            }
            None => {}
        }
        for (index, param) in param_types.iter().enumerate() {
            if let Some(llvm) = self.value_type(param, span)? {
                params.push(format!("{llvm} %a{index}"));
                args.push(format!("{llvm} %a{index}"));
            }
        }
        let call = format!("call {ret} @{symbol}({})", args.join(", "));
        if ret == "void" {
            body.push_str(&format!("  {call}\n  ret void\n"));
        } else {
            body.push_str(&format!("  %result = {call}\n  ret {ret} %result\n"));
        }
        self.module.thunks.insert(
            name.clone(),
            format!("define private {ret} {name}({}) {{\n{body}}}\n", params.join(", ")),
        );
        Ok(name)
    }

    /// A zeroed heap allocation for one value of LLVM type `llvm`.
    fn heap_alloc(&mut self, llvm: &str) -> String {
        self.module.declare(ALLOC_DECL);
        let end = self.assign(&format!("getelementptr {llvm}, ptr null, i32 1"));
        let size = self.assign(&format!("ptrtoint ptr {end} to i64"));
        self.assign(&format!("call ptr @cooper_alloc(i64 {size})"))
    }
}

impl Emitter<'_, '_> {
    /// The receiver argument a method declared with receiver type `wanted` takes from
    /// the receiver expression `receiver`: its address for a pointer receiver called on
    /// a value (a temporary's, if the value is no place), the pointee for a value
    /// receiver called through a pointer, and the value itself otherwise.
    fn receiver(&mut self, receiver: &IrExpr, wanted: &Type) -> Result<Value, CodegenError> {
        let pointer = |operand: String| Value {
            ty: "ptr".to_string(),
            operand,
        };
        match (matches!(wanted, Type::Pointer(_)), matches!(receiver.ty, Type::Pointer(_))) {
            (true, false) => match self.place(receiver)? {
                Some(place) => Ok(pointer(place.ptr)),
                None => {
                    let value = self.expr(receiver)?;
                    let temp = self.slot(None, &receiver.ty, receiver.span)?;
                    self.store(&temp, value.as_ref());
                    Ok(pointer(temp.ptr))
                }
            },
            (false, true) => {
                let place = self.pointee(receiver)?;
                Ok(self.load(&place).expect("a receiver has a value"))
            }
            _ => Ok(self.expr(receiver)?.expect("a receiver has a value")),
        }
    }
}

/// The local variable a place expression is rooted in, if the place is stored in that
/// local's own storage (not reached through a pointer).
fn root_local(expr: &IrExpr) -> Option<&str> {
    match &expr.kind {
        IrExprKind::Var(name) => Some(name),
        IrExprKind::Field { target, .. } if !matches!(target.ty, Type::Pointer(_)) => {
            root_local(target)
        }
        _ => None,
    }
}

/// Visit every expression in `stmt`, outermost first.
fn walk_stmt(stmt: &IrStmt, visit: &mut impl FnMut(&IrExpr)) {
    match &stmt.kind {
        IrStmtKind::Var { init, .. } => init.iter().for_each(|e| walk_expr(e, visit)),
        IrStmtKind::Expr(expr) => walk_expr(expr, visit),
        IrStmtKind::Return(value) => value.iter().for_each(|e| walk_expr(e, visit)),
        IrStmtKind::While { cond, body, .. } => {
            walk_expr(cond, visit);
            body.iter().for_each(|s| walk_stmt(s, visit));
        }
        IrStmtKind::ForRange {
            start, end, body, ..
        } => {
            walk_expr(start, visit);
            walk_expr(end, visit);
            body.iter().for_each(|s| walk_stmt(s, visit));
        }
        IrStmtKind::ForEach { array, body, .. } => {
            walk_expr(array, visit);
            body.iter().for_each(|s| walk_stmt(s, visit));
        }
        IrStmtKind::Block(stmts) => stmts.iter().for_each(|s| walk_stmt(s, visit)),
        IrStmtKind::Break | IrStmtKind::Continue => {}
        IrStmtKind::Match {
            scrutinee, actions, ..
        } => {
            walk_expr(scrutinee, visit);
            actions.iter().for_each(|s| walk_stmt(s, visit));
        }
    }
}

/// Visit `expr` and every expression within it, outermost first.
fn walk_expr(expr: &IrExpr, visit: &mut impl FnMut(&IrExpr)) {
    visit(expr);
    match &expr.kind {
        IrExprKind::Int(_)
        | IrExprKind::Float(_)
        | IrExprKind::Bool(_)
        | IrExprKind::Str(_)
        | IrExprKind::Nil
        | IrExprKind::Unit
        | IrExprKind::Var(_)
        | IrExprKind::FuncRef { .. } => {}
        IrExprKind::Method { receiver, .. } => walk_expr(receiver, visit),
        IrExprKind::Tuple(elems) | IrExprKind::Intrinsic { args: elems, .. } => walk_all(elems, visit),
        IrExprKind::Variant { args, .. } => walk_all(args, visit),
        IrExprKind::Unary { operand, .. }
        | IrExprKind::Deref(operand)
        | IrExprKind::AddressOf(operand)
        | IrExprKind::Convert(operand)
        | IrExprKind::Field {
            target: operand, ..
        }
        | IrExprKind::Let { value: operand, .. }
        | IrExprKind::LetTuple { value: operand, .. } => walk_expr(operand, visit),
        IrExprKind::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, visit);
            walk_expr(rhs, visit);
        }
        IrExprKind::Assign { target, value, .. } => {
            walk_expr(target, visit);
            walk_expr(value, visit);
        }
        IrExprKind::Call { callee, args } => {
            walk_expr(callee, visit);
            walk_all(args, visit);
        }
        IrExprKind::Block { stmts, result } => {
            stmts.iter().for_each(|s| walk_stmt(s, visit));
            walk_expr(result, visit);
        }
        IrExprKind::StructLiteral { members, .. } => {
            members.iter().for_each(|(_, value)| walk_expr(value, visit));
        }
        IrExprKind::Match {
            scrutinee, actions, ..
        } => {
            walk_expr(scrutinee, visit);
            walk_all(actions, visit);
        }
    }
}

fn walk_all(exprs: &[IrExpr], visit: &mut impl FnMut(&IrExpr)) {
    exprs.iter().for_each(|e| walk_expr(e, visit));
}

/// Each action's bindings, from every leaf of `tree` that reaches it.
fn collect_bindings<'t>(tree: &'t Decision, out: &mut [Vec<&'t MatchBinding>]) {
    match tree {
        Decision::Leaf { bindings, action } => out[*action].extend(bindings),
        Decision::Switch { cases, default, .. } => {
            cases.iter().for_each(|c| collect_bindings(&c.tree, out));
            if let Some(default) = default {
                collect_bindings(default, out);
            }
        }
        Decision::Fail => {}
    }
}

/// An integer constant at a width of `bits`. LLVM reads integer constants as signed, so
/// the value is written as the signed number with the same low `bits` bits: a `u32`
/// `0xFFFF_FFFF` becomes `-1`.
fn int_constant(value: i128, bits: u32) -> String {
    let shift = 128 - bits;
    ((value << shift) >> shift).to_string()
}

/// A float constant, rounded to the precision of `bits` and written as the hex bits of
/// the equivalent `double`, the exact form LLVM requires for any non-decimal value.
fn float_constant(value: f64, bits: u32) -> String {
    let rounded = if bits == 32 { value as f32 as f64 } else { value };
    format!("0x{:016X}", rounded.to_bits())
}
