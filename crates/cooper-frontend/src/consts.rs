//! Constants: the values of `const` declarations, computed at compile time.
//!
//! A constant's initializer is a constant expression: literals, operators, conversions
//! between numeric types, and other constants. It is evaluated in the constant's declared
//! type, with the operator rules of run-time code, except that what would stop the program
//! at run time (overflow, division by zero, an out-of-range shift count) is a compile error.
//! A module's constants are evaluated before its type declarations are resolved, so a
//! constant can give a static array's length.

use std::collections::HashMap;

use crate::ast::{BinaryOp, Expr, ExprKind, Stmt, StmtKind, UnaryOp};
use crate::diag::{Diagnostic, Span};
use crate::resolve::{self, Globals};
use crate::typecheck::{decode_number_literal, decode_string_literal, LiteralValue};
use crate::types::{integer_bits, is_float_name, is_integer_name, Type, DEFAULT_FLOAT, DEFAULT_INT, SIGNED_INTS};

/// A constant's value. A rune is an `Int` of its code point.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    Str(String),
}

/// A constant: its declared type and its value.
#[derive(Debug, Clone)]
pub struct Constant {
    pub ty: Type,
    pub value: ConstValue,
}

/// The length a static array type `T[name]` gives with the constant `name`.
pub(crate) fn array_length(name: &str, span: Span, globals: &Globals, diags: &mut Vec<Diagnostic>) -> Option<u64> {
    let Some(constant) = globals.consts.get(name) else {
        diags.push(Diagnostic::error(span, format!("undefined constant {name}")));
        return None;
    };
    match constant.value {
        ConstValue::Int(n) if is_integer(&constant.ty) && n >= 0 => u64::try_from(n).ok(),
        _ => {
            diags.push(Diagnostic::error(span, format!("an array length is a non-negative integer, but {name} is {}", constant.ty)));
            None
        }
    }
}

/// Evaluate a module's constant declarations. `decls` pairs each with the index of its
/// file, whose symbol table (the module's declarations and the file's imports) and module
/// bindings `files` holds. Returns the constants that evaluated, by name, and each error
/// with the index of its file.
pub(crate) fn evaluate(
    decls: &[(usize, &Stmt)],
    files: &[(Globals, &HashMap<Vec<String>, Globals>)],
) -> (HashMap<String, Constant>, Vec<(usize, Diagnostic)>) {
    let mut evaluator = Evaluator { decls: HashMap::new(), files, done: HashMap::new(), active: Vec::new() };
    let mut errors = Vec::new();
    for &(file, stmt) in decls {
        let StmtKind::ConstDecl { name, name_span, .. } = &stmt.kind else { continue };
        let globals = &files[file].0;
        let message = if !name.starts_with(|c: char| c.is_ascii_uppercase()) {
            Some(format!("a constant's name is PascalCase, as in BufSize: {name}"))
        } else if evaluator.decls.contains_key(name.as_str()) || globals.type_name_taken(name) {
            Some(format!("redeclared name {name} in the same scope"))
        } else {
            None
        };
        match message {
            Some(message) => errors.push((file, Diagnostic::error(*name_span, message))),
            None => {
                evaluator.decls.insert(name, (file, stmt));
            }
        }
    }
    for &(_, stmt) in decls {
        let StmtKind::ConstDecl { name, .. } = &stmt.kind else { continue };
        if !evaluator.decls.contains_key(name.as_str()) {
            continue;
        }
        if let Err((file, diag)) = evaluator.constant(name) {
            errors.push((file, diag));
        }
    }
    let constants = evaluator.done.into_iter().filter_map(|(name, c)| Some((name, c?))).collect();
    (constants, errors)
}

type Eval<T> = Result<T, (usize, Diagnostic)>;

struct Evaluator<'a> {
    /// The module's constant declarations, by name, with the index of each one's file.
    decls: HashMap<&'a str, (usize, &'a Stmt)>,
    files: &'a [(Globals, &'a HashMap<Vec<String>, Globals>)],
    /// The constants evaluated so far; `None` for one whose initializer has an error,
    /// reported once.
    done: HashMap<String, Option<Constant>>,
    /// The constants being evaluated, innermost last.
    active: Vec<&'a str>,
}

impl<'a> Evaluator<'a> {
    /// The module's constant `name`, evaluated on first use.
    fn constant(&mut self, name: &'a str) -> Eval<Option<Constant>> {
        if let Some(done) = self.done.get(name) {
            return Ok(done.clone());
        }
        let (file, stmt) = self.decls[name];
        let StmtKind::ConstDecl { ty, value, name_span, .. } = &stmt.kind else {
            unreachable!("only constant declarations are collected");
        };
        if self.active.contains(&name) {
            let cycle = format!("constant {name} is defined in terms of itself");
            return Err((file, Diagnostic::error(*name_span, cycle)));
        }
        self.active.push(name);
        let result = self.initializer(file, ty, value);
        self.active.pop();
        let constant = result.as_ref().ok().cloned();
        self.done.insert(name.to_string(), constant.clone());
        result.map(Some)
    }

    fn initializer(&mut self, file: usize, ty: &crate::ast::TypeExpr, value: &Expr) -> Eval<Constant> {
        let mut diags = Vec::new();
        let Some(declared) = resolve::resolve_type(ty, &Default::default(), &self.files[file].0, &mut diags) else {
            return Err((file, diags.remove(0)));
        };
        if !matches!(&declared, Type::Primitive(_)) {
            let message = format!("a constant is a number, bool, rune, or string, not {declared}");
            return Err((file, Diagnostic::error(ty.span, message)));
        }
        let (found, value) = self.eval(file, value, Some(&declared))?;
        if !found.equals(&declared) {
            let message = format!("cannot assign {found} to a constant of type {declared}");
            return Err((file, Diagnostic::error(ty.span, message)));
        }
        Ok(Constant { ty: declared, value })
    }

    fn eval(&mut self, file: usize, expr: &Expr, want: Option<&Type>) -> Eval<(Type, ConstValue)> {
        let err = |message: String| Err((file, Diagnostic::error(expr.span, message)));
        match &expr.kind {
            ExprKind::Group(inner) => self.eval(file, inner, want),
            ExprKind::Number(text) => literal(text, false, want).or_else(err),
            ExprKind::Unary { op: UnaryOp::Neg, operand } if matches!(&operand.kind, ExprKind::Number(_)) => {
                let ExprKind::Number(text) = &operand.kind else { unreachable!() };
                literal(text, true, want).or_else(err)
            }
            ExprKind::Bool(b) => Ok((primitive("bool"), ConstValue::Bool(*b))),
            ExprKind::Rune(r) => Ok((primitive("rune"), ConstValue::Int(i128::from(u32::from(*r))))),
            ExprKind::Str(text) => match decode_string_literal(text) {
                Ok(text) => Ok((primitive("string"), ConstValue::Str(text))),
                Err(message) => err(message),
            },
            ExprKind::Ident(_) | ExprKind::Field { .. } => match self.reference(file, expr)? {
                Some(constant) => Ok((constant.ty, constant.value)),
                None => err("a constant's initializer uses only literals, operators, conversions, and other constants".to_string()),
            },
            ExprKind::Unary { op, operand } => {
                let (ty, value) = self.eval(file, operand, want)?;
                unary(*op, &ty, value).map(|v| (ty, v)).or_else(err)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(file, *op, lhs, rhs, want).or_else(|m| err(m)),
            ExprKind::Chain { operands, ops } => {
                for (pair, op) in operands.windows(2).zip(ops) {
                    let (_, value) = self.binary(file, *op, &pair[0], &pair[1], None).or_else(|m| err(m))?;
                    if value == ConstValue::Bool(false) {
                        return Ok((primitive("bool"), value));
                    }
                }
                Ok((primitive("bool"), ConstValue::Bool(true)))
            }
            ExprKind::Call { callee, args } => match (&callee.kind, args.as_slice()) {
                (ExprKind::Ident(target), [arg]) if is_integer_name(target) || is_float_name(target) => {
                    let (from, value) = self.eval(file, arg, None)?;
                    convert(&from, value, target).map(|v| (primitive(target), v)).or_else(err)
                }
                _ => err("a constant's initializer cannot call a function".to_string()),
            },
            _ => err("a constant's initializer uses only literals, operators, conversions, and other constants".to_string()),
        }
    }

    /// The constant an identifier or a qualified name `module.Name` refers to, if any:
    /// one of the module's own, or one a `use` brings into the file.
    fn reference(&mut self, file: usize, expr: &Expr) -> Eval<Option<Constant>> {
        let (globals, modules) = &self.files[file];
        match &expr.kind {
            ExprKind::Ident(name) => match self.decls.get_key_value(name.as_str()) {
                Some((&name, _)) => match self.constant(name)? {
                    Some(constant) => Ok(Some(constant)),
                    // The error is reported at the constant itself.
                    None => Err((file, Diagnostic::error(expr.span, format!("constant {name} has an error")))),
                },
                None => Ok(globals.consts.get(name).cloned()),
            },
            ExprKind::Field { target, name, .. } => {
                let mut path = Vec::new();
                let mut head = &**target;
                while let ExprKind::Field { target, name, .. } = &head.kind {
                    path.push(name.clone());
                    head = target;
                }
                let ExprKind::Ident(first) = &head.kind else { return Ok(None) };
                path.push(first.clone());
                path.reverse();
                Ok(modules.get(&path).and_then(|interface| interface.consts.get(name)).cloned())
            }
            _ => Ok(None),
        }
    }

    fn binary(&mut self, file: usize, op: BinaryOp, lhs: &Expr, rhs: &Expr, want: Option<&Type>) -> Result<(Type, ConstValue), String> {
        let comparison = matches!(op, BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge);
        let operand_type = match op {
            BinaryOp::Shl | BinaryOp::Shr => lift(self.static_type(file, lhs))?,
            _ => match lift(self.static_type(file, lhs))? {
                Some(ty) => Some(ty),
                None => lift(self.static_type(file, rhs))?,
            },
        };
        let operand_type = operand_type
            .or_else(|| want.filter(|_| !comparison).cloned())
            .unwrap_or_else(|| primitive(if has_float_literal(lhs) || has_float_literal(rhs) { DEFAULT_FLOAT } else { DEFAULT_INT }));
        let (ty, a) = lift(self.eval(file, lhs, Some(&operand_type)))?;
        if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            let count_type = lift(self.static_type(file, rhs))?;
            let (count_type, b) = lift(self.eval(file, rhs, count_type.as_ref()))?;
            return shift(op, &ty, a, &count_type, b).map(|v| (ty, v));
        }
        let (rhs_type, b) = lift(self.eval(file, rhs, Some(&ty)))?;
        if !rhs_type.equals(&ty) {
            return Err(format!("invalid operands for {}: {ty} and {rhs_type}", op.symbol()));
        }
        if comparison {
            return compare(op, &ty, &a, &b).map(|v| (primitive("bool"), ConstValue::Bool(v)));
        }
        arithmetic(op, &ty, a, b).map(|v| (ty, v))
    }

    /// The type an operand has regardless of context: `None` for a bare literal, whose type
    /// the other operand or the declared type decides.
    fn static_type(&mut self, file: usize, expr: &Expr) -> Eval<Option<Type>> {
        Ok(match &expr.kind {
            ExprKind::Number(_) => None,
            ExprKind::Group(inner) | ExprKind::Unary { operand: inner, .. } => self.static_type(file, inner)?,
            ExprKind::Binary { op: BinaryOp::Shl | BinaryOp::Shr, lhs, .. } => self.static_type(file, lhs)?,
            ExprKind::Binary { op: BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge, .. }
            | ExprKind::Chain { .. } => Some(primitive("bool")),
            ExprKind::Binary { lhs, rhs, .. } => match self.static_type(file, lhs)? {
                Some(ty) => Some(ty),
                None => self.static_type(file, rhs)?,
            },
            _ => Some(self.eval(file, expr, None)?.0),
        })
    }
}

fn lift<T>(result: Eval<T>) -> Result<T, String> {
    result.map_err(|(_, diag)| diag.message)
}

fn primitive(name: &str) -> Type {
    Type::Primitive(name.to_string())
}

fn is_integer(ty: &Type) -> bool {
    matches!(ty, Type::Primitive(name) if is_integer_name(name))
}

fn has_float_literal(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Number(text) => matches!(decode_number_literal(text), Some(LiteralValue::Float(_))),
        ExprKind::Group(inner) | ExprKind::Unary { operand: inner, .. } => has_float_literal(inner),
        ExprKind::Binary { lhs, rhs, .. } => has_float_literal(lhs) || has_float_literal(rhs),
        _ => false,
    }
}

/// The least and greatest values of the integer type `name`.
fn int_range(name: &str) -> (i128, i128) {
    let bits = integer_bits(name);
    if SIGNED_INTS.contains(&name) {
        (-(1 << (bits - 1)), (1 << (bits - 1)) - 1)
    } else {
        (0, (1 << bits) - 1)
    }
}

/// `value` as an integer of type `name`, or the error of it not fitting.
fn checked(value: Option<i128>, name: &str) -> Result<ConstValue, String> {
    let (min, max) = int_range(name);
    match value {
        Some(v) if (min..=max).contains(&v) => Ok(ConstValue::Int(v)),
        _ => Err(format!("constant overflows {name}")),
    }
}

/// `value` rounded to the float type `name`.
fn rounded(value: f64, name: &str) -> ConstValue {
    ConstValue::Float(if name == "f32" { value as f32 as f64 } else { value })
}

/// A numeric literal (negated when `negative`) of the type `want`, or of its class's
/// default type when `want` is no numeric type.
fn literal(text: &str, negative: bool, want: Option<&Type>) -> Result<(Type, ConstValue), String> {
    let decoded = decode_number_literal(text).ok_or_else(|| format!("malformed number {text}"))?;
    let target = match (want, &decoded) {
        (Some(Type::Primitive(name)), LiteralValue::Int(_)) if is_integer_name(name) || is_float_name(name) => name.as_str(),
        (Some(Type::Primitive(name)), LiteralValue::Float(_)) if is_float_name(name) => name.as_str(),
        (_, LiteralValue::Int(_)) => DEFAULT_INT,
        (_, LiteralValue::Float(_)) => DEFAULT_FLOAT,
    };
    let value = match decoded {
        LiteralValue::Float(v) => rounded(if negative { -v } else { v }, target),
        LiteralValue::Int(v) if is_float_name(target) => rounded(if negative { -(v as f64) } else { v as f64 }, target),
        LiteralValue::Int(v) => {
            let v = i128::try_from(v).ok().map(|v| if negative { -v } else { v });
            checked(v, target).map_err(|_| format!("literal out of range for {target}"))?
        }
    };
    Ok((primitive(target), value))
}

fn unary(op: UnaryOp, ty: &Type, value: ConstValue) -> Result<ConstValue, String> {
    let Type::Primitive(name) = ty else { unreachable!("constants are primitives") };
    match (op, value) {
        (UnaryOp::Pos, value @ (ConstValue::Int(_) | ConstValue::Float(_))) if name != "rune" => Ok(value),
        (UnaryOp::Neg, ConstValue::Int(v)) if is_integer_name(name) => checked(v.checked_neg(), name),
        (UnaryOp::Neg, ConstValue::Float(v)) => Ok(ConstValue::Float(-v)),
        (UnaryOp::Not, ConstValue::Bool(b)) => Ok(ConstValue::Bool(!b)),
        (UnaryOp::Not, ConstValue::Int(v)) if is_integer_name(name) => Ok(wrapped(!v, name)),
        _ => Err(format!("operator {} does not apply to {ty}", op.symbol())),
    }
}

/// `v` truncated to the integer type `name`, as a conversion truncates.
fn wrapped(v: i128, name: &str) -> ConstValue {
    let unused = 128 - integer_bits(name);
    let v = if SIGNED_INTS.contains(&name) { (v << unused) >> unused } else { ((v as u128) << unused >> unused) as i128 };
    ConstValue::Int(v)
}

fn arithmetic(op: BinaryOp, ty: &Type, a: ConstValue, b: ConstValue) -> Result<ConstValue, String> {
    let Type::Primitive(name) = ty else { unreachable!("constants are primitives") };
    let invalid = || Err(format!("operator {} does not apply to {ty}", op.symbol()));
    match (a, b) {
        (ConstValue::Int(a), ConstValue::Int(b)) if is_integer_name(name) => match op {
            BinaryOp::Add => checked(a.checked_add(b), name),
            BinaryOp::Sub => checked(a.checked_sub(b), name),
            BinaryOp::Mul => checked(a.checked_mul(b), name),
            BinaryOp::Div | BinaryOp::Rem if b == 0 => Err("division by zero".to_string()),
            BinaryOp::Div => checked(a.checked_div(b), name),
            BinaryOp::Rem => checked(a.checked_rem(b), name),
            BinaryOp::And => Ok(ConstValue::Int(a & b)),
            BinaryOp::Or => Ok(ConstValue::Int(a | b)),
            BinaryOp::Xor => Ok(ConstValue::Int(a ^ b)),
            _ => invalid(),
        },
        (ConstValue::Float(a), ConstValue::Float(b)) => match op {
            BinaryOp::Add => Ok(rounded(a + b, name)),
            BinaryOp::Sub => Ok(rounded(a - b, name)),
            BinaryOp::Mul => Ok(rounded(a * b, name)),
            BinaryOp::Div => Ok(rounded(a / b, name)),
            BinaryOp::Rem => Ok(rounded(a % b, name)),
            _ => invalid(),
        },
        (ConstValue::Bool(a), ConstValue::Bool(b)) => match op {
            BinaryOp::And => Ok(ConstValue::Bool(a && b)),
            BinaryOp::Or => Ok(ConstValue::Bool(a || b)),
            BinaryOp::Xor => Ok(ConstValue::Bool(a != b)),
            _ => invalid(),
        },
        (ConstValue::Str(a), ConstValue::Str(b)) if op == BinaryOp::Add => Ok(ConstValue::Str(a + &b)),
        _ => invalid(),
    }
}

fn shift(op: BinaryOp, ty: &Type, a: ConstValue, count_type: &Type, count: ConstValue) -> Result<ConstValue, String> {
    let (Type::Primitive(name), ConstValue::Int(a), ConstValue::Int(n)) = (ty, a, count) else {
        return Err(format!("operator {} does not apply to {ty}", op.symbol()));
    };
    if !is_integer_name(name) || !is_integer(count_type) {
        return Err(format!("operator {} does not apply to {ty} and {count_type}", op.symbol()));
    }
    let bits = integer_bits(name);
    let Some(n) = u32::try_from(n).ok().filter(|n| *n < bits) else {
        return Err(format!("shift count {n} is out of range for {name}"));
    };
    // Bits a left shift moves out are discarded; `>>` is arithmetic on signed values, which
    // the `i128` representation gives.
    Ok(if op == BinaryOp::Shl { wrapped(a << n, name) } else { ConstValue::Int(a >> n) })
}

fn compare(op: BinaryOp, ty: &Type, a: &ConstValue, b: &ConstValue) -> Result<bool, String> {
    let ordering = match (a, b) {
        (ConstValue::Int(a), ConstValue::Int(b)) => a.partial_cmp(b),
        (ConstValue::Float(a), ConstValue::Float(b)) => a.partial_cmp(b),
        (ConstValue::Bool(_), ConstValue::Bool(_)) | (ConstValue::Str(_), ConstValue::Str(_)) => {
            return match op {
                BinaryOp::Eq => Ok(a == b),
                BinaryOp::Ne => Ok(a != b),
                _ => Err(format!("operator {} does not apply to {ty}", op.symbol())),
            };
        }
        _ => unreachable!("operands of one type"),
    };
    use std::cmp::Ordering::*;
    Ok(match op {
        BinaryOp::Eq => ordering == Some(Equal),
        BinaryOp::Ne => ordering != Some(Equal),
        BinaryOp::Lt => ordering == Some(Less),
        BinaryOp::Le => matches!(ordering, Some(Less | Equal)),
        BinaryOp::Gt => ordering == Some(Greater),
        BinaryOp::Ge => matches!(ordering, Some(Greater | Equal)),
        _ => unreachable!("a comparison"),
    })
}

/// `value` of type `from` converted to the numeric type `to`, as a run-time conversion
/// converts it.
fn convert(from: &Type, value: ConstValue, to: &str) -> Result<ConstValue, String> {
    match value {
        ConstValue::Int(v) if is_integer(from) || from.equals(&primitive("rune")) => {
            Ok(if is_float_name(to) { rounded(v as f64, to) } else { wrapped(v, to) })
        }
        ConstValue::Float(v) if is_float_name(to) => Ok(rounded(v, to)),
        // Rust's float-to-integer `as` rounds toward zero and saturates, with NaN as 0, as
        // Cooper's conversion does.
        ConstValue::Float(v) => {
            let (min, max) = int_range(to);
            Ok(ConstValue::Int((v as i128).clamp(min, max)))
        }
        _ => Err(format!("cannot convert {from} to {to}")),
    }
}
