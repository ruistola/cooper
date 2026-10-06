//! Code generation for one function body.
//!
//! Every local — parameters included — lives in an `alloca` in the entry block, which
//! an optimizing build promotes to registers. Control flow becomes explicit basic
//! blocks. Integer arithmetic is checked: overflow and division by zero branch to a
//! call of the runtime's `cooper_panic`.

use std::collections::HashMap;
use std::fmt::Write;

use cooper_frontend::ast::{AssignOp, BinaryOp, UnaryOp};
use cooper_frontend::diag::Span;
use cooper_frontend::types::{integer_bits, is_float_name, is_integer_name, Type};
use cooper_ir::{
    Access, Decision, Function, IrExpr, IrExprKind, IrStmt, IrStmtKind, MatchBinding, Test,
};

use crate::{mangle, CodegenError, Module};

const PANIC_DECL: &str = "declare void @cooper_panic(ptr) noreturn";

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
    };
    if let Some(receiver) = &function.receiver {
        return emitter.unsupported("methods", receiver.span);
    }
    let ret = emitter.signature_type(&function.return_type, function.span)?;
    let mut params = Vec::new();
    for (index, param) in function.params.iter().enumerate() {
        let slot = emitter.bind(&param.name, &param.ty, param.span)?;
        if let Some(ty) = &slot.ty {
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

/// How a scalar type computes: it selects the LLVM type and instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    Bool,
    Int { bits: u32, signed: bool },
    Float { bits: u32 },
}

impl Scalar {
    fn of(ty: &Type) -> Option<Scalar> {
        let Type::Primitive(name) = ty else {
            return None;
        };
        if name == "bool" {
            Some(Scalar::Bool)
        } else if is_integer_name(name) {
            Some(Scalar::Int {
                bits: integer_bits(name),
                signed: name.starts_with('i'),
            })
        } else if is_float_name(name) {
            Some(Scalar::Float {
                bits: if name == "f32" { 32 } else { 64 },
            })
        } else {
            None
        }
    }

    fn llvm(self) -> String {
        match self {
            Scalar::Bool => "i1".to_string(),
            Scalar::Int { bits, .. } => format!("i{bits}"),
            Scalar::Float { bits: 32 } => "float".to_string(),
            Scalar::Float { .. } => "double".to_string(),
        }
    }
}

/// An SSA value: its LLVM type and operand spelling. Unit-typed expressions have none.
#[derive(Debug, Clone)]
struct Value {
    ty: String,
    operand: String,
}

/// A local variable's storage: an `alloca` of its LLVM type, or nothing for unit.
#[derive(Debug, Clone)]
struct Slot {
    ptr: String,
    ty: Option<String>,
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
    scopes: Vec<HashMap<String, Slot>>,
    loops: Vec<Loop>,
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
    fn value_type(&self, ty: &Type, span: Span) -> Result<Option<String>, CodegenError> {
        match ty {
            Type::Unit => Ok(None),
            _ => match Scalar::of(ty) {
                Some(scalar) => Ok(Some(scalar.llvm())),
                None => self.unsupported(&format!("values of type {ty}"), span),
            },
        }
    }

    /// The LLVM type of `ty` as a function's return type, where unit is `void`.
    fn signature_type(&self, ty: &Type, span: Span) -> Result<String, CodegenError> {
        Ok(self.value_type(ty, span)?.unwrap_or_else(|| "void".to_string()))
    }

    /// Storage for a value of type `ty`, not yet bound to any name.
    fn slot(&mut self, ty: &Type, span: Span) -> Result<Slot, CodegenError> {
        let llvm = self.value_type(ty, span)?;
        let ptr = match &llvm {
            Some(llvm) => {
                let ptr = self.fresh("%v");
                writeln!(self.allocas, "  {ptr} = alloca {llvm}")
                    .expect("writing to a String cannot fail");
                ptr
            }
            None => String::new(),
        };
        Ok(Slot { ptr, ty: llvm })
    }

    /// Declare a new local named `name` in the innermost scope, shadowing any outer one.
    fn bind(&mut self, name: &str, ty: &Type, span: Span) -> Result<Slot, CodegenError> {
        let slot = self.slot(ty, span)?;
        self.scopes
            .last_mut()
            .expect("a function body has a scope")
            .insert(name.to_string(), slot.clone());
        Ok(slot)
    }

    fn lookup(&self, name: &str) -> Slot {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .cloned()
            .unwrap_or_else(|| unreachable!("a checked program binds `{name}` before use"))
    }

    fn load(&mut self, slot: &Slot) -> Option<Value> {
        let ty = slot.ty.clone()?;
        let operand = self.assign(&format!("load {ty}, ptr {}", slot.ptr));
        Some(Value { ty, operand })
    }

    fn store(&mut self, slot: &Slot, value: Option<&Value>) {
        if let Some(value) = value {
            self.inst(&format!("store {} {}, ptr {}", value.ty, value.operand, slot.ptr));
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

    /// The zero value of `ty`, which an uninitialized `let` takes.
    fn zero(&self, ty: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        let operand = match Scalar::of(ty) {
            Some(Scalar::Bool) => "false",
            Some(Scalar::Int { .. }) => "0",
            Some(Scalar::Float { .. }) => "0.0",
            None if matches!(ty, Type::Unit) => return Ok(None),
            None => return self.unsupported(&format!("values of type {ty}"), span),
        };
        Ok(Some(Value {
            ty: self.value_type(ty, span)?.expect("a scalar has an LLVM type"),
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
        let mut slots: Vec<HashMap<String, Slot>> = Vec::with_capacity(count);
        for action_bindings in &bindings {
            let mut action_slots = HashMap::new();
            for binding in action_bindings {
                if !action_slots.contains_key(&binding.name) {
                    let slot = self.slot(&binding.ty, scrutinee.span)?;
                    action_slots.insert(binding.name.clone(), slot);
                }
            }
            slots.push(action_slots);
        }
        let result_slot = match actions {
            Actions::Exprs(_) if !matches!(result, Type::Unit) => {
                Some(self.slot(result, scrutinee.span)?)
            }
            _ => None,
        };
        let labels: Vec<String> = (0..count).map(|_| self.fresh("arm")).collect();
        let join = self.fresh("join");
        self.decision(tree, value.as_ref(), &labels, &slots, scrutinee.span)?;
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
        scrutinee: Option<&Value>,
        labels: &[String],
        slots: &[HashMap<String, Slot>],
        span: Span,
    ) -> Result<(), CodegenError> {
        match tree {
            Decision::Leaf { bindings, action } => {
                for binding in bindings {
                    let value = self.access(&binding.access, scrutinee, span)?;
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
                    _ => return self.unsupported("matching on sum types", span),
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

    /// The subvalue of the scrutinee at `access`.
    fn access(
        &self,
        access: &Access,
        scrutinee: Option<&Value>,
        span: Span,
    ) -> Result<Option<Value>, CodegenError> {
        match access {
            Access::Root => Ok(scrutinee.cloned()),
            _ => self.unsupported("tuple, struct, and variant patterns", span),
        }
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
            IrExprKind::Var(name) => {
                let slot = self.lookup(name);
                Ok(self.load(&slot))
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
            IrExprKind::Nil => self.unsupported("`nil`", span),
            IrExprKind::FuncRef { .. } | IrExprKind::Method { .. } => {
                self.unsupported("function values", span)
            }
            IrExprKind::Tuple(_) | IrExprKind::LetTuple { .. } => self.unsupported("tuples", span),
            IrExprKind::Field { .. } | IrExprKind::StructLiteral { .. } => {
                self.unsupported("structs", span)
            }
            IrExprKind::Intrinsic { .. } => self.unsupported("arrays", span),
            IrExprKind::Deref(_) | IrExprKind::AddressOf(_) => self.unsupported("pointers", span),
            IrExprKind::Variant { .. } => self.unsupported("sum types", span),
        }
    }

    /// A numeric literal of type `ty`: `int` when it is an integer type, `float` when
    /// it is a float type (an integer literal may take a float type).
    fn literal(&self, ty: &Type, float: f64, int: i128, span: Span) -> Result<Option<Value>, CodegenError> {
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
            return self.unsupported(&format!("`{}` on {}", op.symbol(), lhs.ty), span);
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
    fn assignment(
        &mut self,
        op: AssignOp,
        target: &IrExpr,
        value: &IrExpr,
        span: Span,
    ) -> Result<Option<Value>, CodegenError> {
        let IrExprKind::Var(name) = &target.kind else {
            return self.unsupported("assignment through fields, pointers, or tuples", span);
        };
        let slot = self.lookup(name);
        let value = self.expr(value)?;
        let stored = match op {
            AssignOp::Assign => value,
            _ => {
                let binary = match op {
                    AssignOp::Add => BinaryOp::Add,
                    AssignOp::Sub => BinaryOp::Sub,
                    AssignOp::Mul => BinaryOp::Mul,
                    _ => BinaryOp::Div,
                };
                let Some(scalar) = Scalar::of(&target.ty) else {
                    return self.unsupported(&format!("`{}` on {}", op.symbol(), target.ty), span);
                };
                let current = self.load(&slot).expect("a compound target is a number");
                let value = value.expect("a compound operand is a number");
                let operand = self.arithmetic(binary, scalar, &current, &value, span);
                Some(Value {
                    ty: current.ty,
                    operand,
                })
            }
        };
        self.store(&slot, stored.as_ref());
        Ok(stored)
    }

    /// A direct call to a named function.
    fn call(&mut self, callee: &IrExpr, args: &[IrExpr], result: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        let IrExprKind::FuncRef { item, type_args } = &callee.kind else {
            return self.unsupported("calls through function values or methods", span);
        };
        let mut operands = Vec::with_capacity(args.len());
        for arg in args {
            // A unit argument is evaluated for its effects and passes nothing.
            if let Some(Value { ty, operand }) = self.expr(arg)? {
                operands.push(format!("{ty} {operand}"));
            }
        }
        let symbol = mangle::symbol(item, type_args);
        let ret = self.signature_type(result, span)?;
        let call = format!("call {ret} @{symbol}({})", operands.join(", "));
        if ret == "void" {
            self.inst(&call);
            return Ok(None);
        }
        let operand = self.assign(&call);
        Ok(Some(Value { ty: ret, operand }))
    }
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
