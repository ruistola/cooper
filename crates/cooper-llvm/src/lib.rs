//! The Cooper LLVM backend.
//!
//! [`emit`] turns a monomorphized [`Program`] into one textual LLVM IR module; the
//! driver compiles that with the system `clang`, linking in the C runtime
//! [`RUNTIME_C`]. Every function instance becomes an LLVM function named by its
//! mangled [`symbol`](mangle), and the program's `main` is exported to the runtime as
//! `cooper_main`, returning the process exit status.
//!
//! Code generation covers a growing subset of the IR. Anything outside it is reported
//! as [`CodegenError::Unsupported`] rather than miscompiled.

mod mangle;

use std::collections::HashMap;
use std::fmt::{self, Write};

use cooper_frontend::diag::Span;
use cooper_frontend::resolve::Callable;
use cooper_frontend::types::{integer_bits, is_integer_name, Type};
use cooper_ir::{Function, IrExpr, IrExprKind, IrStmt, IrStmtKind, Program};

/// The C runtime every program links against: the process entry point and the
/// operations the IR's intrinsics lower to.
pub const RUNTIME_C: &str = include_str!("../runtime/cooper_rt.c");

/// The module and name of the function a program starts in.
const ENTRY_MODULE: &str = "main";
const ENTRY_NAME: &str = "main";

/// Why a program could not be compiled to LLVM IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodegenError {
    /// The program has no `main` function in its `main` module.
    NoEntry,
    /// `main` takes parameters, or returns something other than `i32` or unit.
    BadEntry { span: Span },
    /// A construct code generation does not handle yet.
    Unsupported { what: String, span: Span },
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::NoEntry => write!(f, "the program has no `main` function in module `main`"),
            CodegenError::BadEntry { .. } => {
                write!(f, "`main` must take no parameters and return `i32` or nothing")
            }
            CodegenError::Unsupported { what, .. } => {
                write!(f, "code generation does not support {what} yet")
            }
        }
    }
}

impl std::error::Error for CodegenError {}

/// Emit `program` as a textual LLVM IR module for the target `triple`.
pub fn emit(program: &Program, triple: &str) -> Result<String, CodegenError> {
    let mut out = format!("target triple = \"{triple}\"\n");
    for function in &program.functions {
        out.push('\n');
        out.push_str(&FunctionEmitter::emit(function)?);
    }
    out.push('\n');
    out.push_str(&entry(&program.functions)?);
    Ok(out)
}

/// The `cooper_main` export the runtime calls: the program's `main`, with a unit
/// result turned into exit status 0.
fn entry(functions: &[Function]) -> Result<String, CodegenError> {
    let main = functions
        .iter()
        .find(|f| {
            matches!(&f.item, Callable::Func { module, name }
                if module == ENTRY_MODULE && name == ENTRY_NAME)
        })
        .ok_or(CodegenError::NoEntry)?;
    let symbol = mangle::symbol(&main.item, &main.type_args);
    let call = match &main.return_type {
        _ if !main.params.is_empty() || main.receiver.is_some() => None,
        Type::Primitive(name) if name == "i32" => Some(format!(
            "  %status = call i32 @{symbol}()\n  ret i32 %status\n"
        )),
        Type::Unit => Some(format!("  call void @{symbol}()\n  ret i32 0\n")),
        _ => None,
    };
    let body = call.ok_or(CodegenError::BadEntry { span: main.span })?;
    Ok(format!("define i32 @cooper_main() {{\n{body}}}\n"))
}

/// An SSA value: its LLVM type and its operand spelling. Unit has no value.
struct Value {
    ty: &'static str,
    operand: String,
}

/// The emission state for one function body.
struct FunctionEmitter {
    body: String,
    /// The next free temporary number.
    next: usize,
    /// Parameter names to the values that hold them.
    params: HashMap<String, Value>,
    /// Whether the current block has ended in a terminator.
    terminated: bool,
}

impl FunctionEmitter {
    fn emit(function: &Function) -> Result<String, CodegenError> {
        if let Some(receiver) = &function.receiver {
            return unsupported("methods", receiver.span);
        }
        let ret = llvm_type(&function.return_type, function.span)?;
        let mut emitter = FunctionEmitter {
            body: String::new(),
            next: 0,
            params: HashMap::new(),
            terminated: false,
        };
        let mut params = Vec::with_capacity(function.params.len());
        for (index, param) in function.params.iter().enumerate() {
            let ty = value_type(&param.ty, param.span)?;
            let operand = format!("%arg{index}");
            params.push(format!("{ty} {operand}"));
            emitter.params.insert(param.name.clone(), Value { ty, operand });
        }
        for stmt in &function.body {
            if emitter.terminated {
                break;
            }
            emitter.stmt(stmt)?;
        }
        if !emitter.terminated {
            // A unit function may fall off its end; a valued one cannot, as the
            // checker requires a return on every path.
            emitter.line(if ret == "void" { "ret void" } else { "unreachable" });
        }
        Ok(format!(
            "define {ret} @{}({}) {{\nentry:\n{}}}\n",
            mangle::symbol(&function.item, &function.type_args),
            params.join(", "),
            emitter.body
        ))
    }

    fn line(&mut self, instruction: &str) {
        writeln!(self.body, "  {instruction}").expect("writing to a String cannot fail");
    }

    fn temp(&mut self) -> String {
        self.next += 1;
        format!("%t{}", self.next)
    }

    fn stmt(&mut self, stmt: &IrStmt) -> Result<(), CodegenError> {
        match &stmt.kind {
            IrStmtKind::Expr(expr) => {
                self.expr(expr)?;
            }
            IrStmtKind::Return(value) => {
                let value = match value {
                    Some(expr) => self.expr(expr)?,
                    None => None,
                };
                match value {
                    Some(Value { ty, operand }) => self.line(&format!("ret {ty} {operand}")),
                    None => self.line("ret void"),
                }
                self.terminated = true;
            }
            IrStmtKind::Var { .. } => return unsupported("local variables", stmt.span),
            IrStmtKind::While { .. }
            | IrStmtKind::ForRange { .. }
            | IrStmtKind::ForEach { .. } => return unsupported("loops", stmt.span),
            IrStmtKind::Block(_) => return unsupported("blocks", stmt.span),
            IrStmtKind::Break | IrStmtKind::Continue => {
                return unsupported("`break` and `continue`", stmt.span)
            }
            IrStmtKind::Match { .. } => return unsupported("`if` and `match`", stmt.span),
        }
        Ok(())
    }

    /// Emit `expr`, returning its value, or `None` for a unit-typed expression.
    fn expr(&mut self, expr: &IrExpr) -> Result<Option<Value>, CodegenError> {
        match &expr.kind {
            IrExprKind::Int(magnitude) => match &expr.ty {
                Type::Primitive(name) if is_integer_name(name) => Ok(Some(Value {
                    ty: value_type(&expr.ty, expr.span)?,
                    operand: int_operand(*magnitude, integer_bits(name)),
                })),
                _ => unsupported("integer literals of non-integer type", expr.span),
            },
            IrExprKind::Bool(b) => Ok(Some(Value {
                ty: "i1",
                operand: b.to_string(),
            })),
            IrExprKind::Unit => Ok(None),
            IrExprKind::Var(name) => match self.params.get(name) {
                Some(Value { ty, operand }) => Ok(Some(Value {
                    ty,
                    operand: operand.clone(),
                })),
                None => unsupported("local variables", expr.span),
            },
            IrExprKind::Call { callee, args } => self.call(callee, args, &expr.ty, expr.span),
            IrExprKind::Float(_) => unsupported("float literals", expr.span),
            IrExprKind::Str(_) => unsupported("strings", expr.span),
            IrExprKind::Nil => unsupported("`nil`", expr.span),
            IrExprKind::FuncRef { .. } | IrExprKind::Method { .. } => {
                unsupported("function values", expr.span)
            }
            IrExprKind::Tuple(_) | IrExprKind::LetTuple { .. } => {
                unsupported("tuples", expr.span)
            }
            IrExprKind::Unary { .. } | IrExprKind::Binary { .. } => {
                unsupported("operators", expr.span)
            }
            IrExprKind::Field { .. } | IrExprKind::StructLiteral { .. } => {
                unsupported("structs", expr.span)
            }
            IrExprKind::Intrinsic { .. } => unsupported("arrays", expr.span),
            IrExprKind::Deref(_) | IrExprKind::AddressOf(_) => {
                unsupported("pointers", expr.span)
            }
            IrExprKind::Assign { .. } | IrExprKind::Let { .. } => {
                unsupported("local variables", expr.span)
            }
            IrExprKind::Block { .. } => unsupported("block expressions", expr.span),
            IrExprKind::Match { .. } => unsupported("`if` and `match` expressions", expr.span),
            IrExprKind::Variant { .. } => unsupported("sum types", expr.span),
            IrExprKind::Convert(_) => unsupported("numeric conversions", expr.span),
        }
    }

    /// Emit a direct call to a named function.
    fn call(
        &mut self,
        callee: &IrExpr,
        args: &[IrExpr],
        result: &Type,
        span: Span,
    ) -> Result<Option<Value>, CodegenError> {
        let IrExprKind::FuncRef { item, type_args } = &callee.kind else {
            return unsupported("calls through function values or methods", span);
        };
        let mut operands = Vec::with_capacity(args.len());
        for arg in args {
            match self.expr(arg)? {
                Some(Value { ty, operand }) => operands.push(format!("{ty} {operand}")),
                None => return unsupported("unit-typed arguments", arg.span),
            }
        }
        let symbol = mangle::symbol(item, type_args);
        let ret = llvm_type(result, span)?;
        let call = format!("call {ret} @{symbol}({})", operands.join(", "));
        if ret == "void" {
            self.line(&call);
            return Ok(None);
        }
        let operand = self.temp();
        self.line(&format!("{operand} = {call}"));
        Ok(Some(Value { ty: ret, operand }))
    }
}

/// The LLVM type of `ty` in a function signature, where unit is `void`.
fn llvm_type(ty: &Type, span: Span) -> Result<&'static str, CodegenError> {
    match ty {
        Type::Unit => Ok("void"),
        _ => value_type(ty, span),
    }
}

/// The LLVM type of a value of type `ty`.
fn value_type(ty: &Type, span: Span) -> Result<&'static str, CodegenError> {
    match ty {
        Type::Primitive(name) => match name.as_str() {
            "bool" => Ok("i1"),
            "i8" | "u8" => Ok("i8"),
            "i16" | "u16" => Ok("i16"),
            "i32" | "u32" => Ok("i32"),
            "i64" | "u64" => Ok("i64"),
            "f32" => Ok("float"),
            "f64" => Ok("double"),
            _ => unsupported(&format!("values of type {ty}"), span),
        },
        _ => unsupported(&format!("values of type {ty}"), span),
    }
}

/// An integer literal's operand at a width of `bits`. LLVM reads integer constants as
/// signed, so a magnitude beyond the signed range of its width (`0xFFFF_FFFF` as a
/// `u32`) is written as the signed value with the same bits.
fn int_operand(magnitude: u128, bits: u32) -> String {
    let shift = 128 - bits;
    (((magnitude << shift) as i128) >> shift).to_string()
}

fn unsupported<T>(what: &str, span: Span) -> Result<T, CodegenError> {
    Err(CodegenError::Unsupported {
        what: what.to_string(),
        span,
    })
}
