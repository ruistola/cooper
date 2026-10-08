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
//!
//! This module holds the emitter's state, values and places, and the expression
//! dispatcher. Its submodules extend `Emitter` by responsibility: `control` (statements,
//! loops, `match`), `operators` (unary, binary, equality, conversions), `calls` (function
//! values and methods), and `arrays` (strings, arrays, intrinsics).

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use cooper_frontend::ast::{AssignOp, BinaryOp, UnaryOp};
use cooper_frontend::diag::Span;
use cooper_frontend::types::Type;
use cooper_ir::{
    Access, Binder, Decision, Function, Intrinsic, IrExpr, IrExprKind, IrStmt, IrStmtKind,
    MatchBinding, Test,
};

use crate::layout::{Scalar, ARRAY, FUNC_VALUE, STRING};
use crate::{mangle, CodegenError, Module};

mod arrays;
mod calls;
mod control;
mod operators;

const PANIC_DECL: &str = "declare void @cooper_panic(ptr) noreturn";
const ALLOC_DECL: &str = "declare ptr @cooper_alloc(i64)";
const CONCAT_DECL: &str = "declare void @cooper_string_concat(ptr, i64, ptr, i64, ptr)";
const STRING_EQ_DECL: &str = "declare i32 @cooper_string_eq(ptr, i64, ptr, i64)";
const GROW_DECL: &str = "declare ptr @cooper_array_grow(ptr, i64, i64, i64)";
const PRINT_DECL: &str = "declare void @cooper_print(ptr, i64, i32)";
const FORMAT_INT_DECL: &str = "declare void @cooper_format_int(i64, i32, ptr)";
const FORMAT_FLOAT_DECL: &str = "declare void @cooper_format_float(double, i32, ptr)";

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

    /// The place `expr` denotes, if it is one: a variable, a field of a place, an array
    /// element, or a dereference. `None` for any other expression, which is then left unevaluated.
    fn place(&mut self, expr: &IrExpr) -> Result<Option<Place>, CodegenError> {
        match &expr.kind {
            IrExprKind::Var(name) => Ok(Some(self.lookup(name))),
            IrExprKind::Deref(pointer) => self.pointee(pointer).map(Some),
            IrExprKind::Intrinsic {
                op: Intrinsic::ArrayGet,
                args,
            } => {
                let array = self.expr(&args[0])?.expect("an array has a value");
                let index = self.expr(&args[1])?.expect("an index has a value");
                let ptr = self.element(&array, &index, &args[1].ty, &expr.ty, expr.span)?;
                Ok(Some(Place {
                    ptr,
                    llvm: self.value_type(&expr.ty, expr.span)?,
                    ty: expr.ty.clone(),
                    nullable: false,
                }))
            }
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

    // --- match ---

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
            IrExprKind::Str(text) => {
                let bytes = self.module.string(text);
                Ok(Some(Value {
                    ty: STRING.to_string(),
                    operand: format!("{{ ptr {bytes}, i64 {} }}", text.len()),
                }))
            }
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
            IrExprKind::Intrinsic { op, args } => self.intrinsic(*op, args, &expr.ty, span),
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
        let place = self.place(target)?.expect("a checked assignment target is a place");
        let current = self.load(&place).expect("a compound target has a value");
        let value = value.expect("a compound operand has a value");
        let stored = if is_string(&target.ty) {
            self.concat(&current, &value, span)?
        } else {
            let Some(scalar) = Scalar::of(&target.ty) else {
                return self.unsupported(&format!("`{}` on {}", op.symbol(), target.ty), span);
            };
            let operand = self.arithmetic(binary, scalar, &current, &value, span);
            Value {
                ty: current.ty,
                operand,
            }
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

}

/// Whether `ty` is the built-in `string`.
fn is_string(ty: &Type) -> bool {
    matches!(ty, Type::Primitive(name) if name == "string")
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
