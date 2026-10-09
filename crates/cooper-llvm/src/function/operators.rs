//! Operators: unary and binary expressions, equality, arithmetic, and conversions.

use super::*;

impl Emitter<'_, '_> {
    pub(super) fn unary(
        &mut self,
        op: UnaryOp,
        operand: &IrExpr,
        ty: &Type,
        span: Span,
    ) -> Result<Option<Value>, CodegenError> {
        match op {
            UnaryOp::Pos => self.expr(operand),
            UnaryOp::Not => {
                // Flipping every bit: logical negation of an `i1`, complement of an integer.
                let value = self.expr(operand)?.expect("`!` applies to a bool or an integer");
                let operand = self.assign(&format!("xor {} {}, -1", value.ty, value.operand));
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

    pub(super) fn binary(
        &mut self,
        op: BinaryOp,
        lhs: &IrExpr,
        rhs: &IrExpr,
        span: Span,
    ) -> Result<Option<Value>, CodegenError> {
        let logical = matches!(Scalar::of(&lhs.ty), Some(Scalar::Bool));
        if logical && matches!(op, BinaryOp::And | BinaryOp::Or) {
            return self.short_circuit(op, lhs, rhs).map(Some);
        }
        if logical && op == BinaryOp::Xor {
            // Exclusive-or depends on both operands, so neither is skipped.
            let left = self.expr(lhs)?.expect("a logical operand is a bool");
            let right = self.expr(rhs)?.expect("a logical operand is a bool");
            let operand = self.assign(&format!("xor i1 {}, {}", left.operand, right.operand));
            return Ok(Some(Value {
                ty: "i1".to_string(),
                operand,
            }));
        }
        if op == BinaryOp::Add && is_string(&lhs.ty) {
            let left = self.expr(lhs)?.expect("a string has a value");
            let right = self.expr(rhs)?.expect("a string has a value");
            return Ok(Some(self.concat(&left, &right, span)?));
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
        if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            let count = Scalar::of(&rhs.ty).expect("a shift count is an integer");
            let operand = self.shift(op, scalar, &left, count, &right, span);
            return Ok(Some(Value { ty: left.ty, operand }));
        }
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
            _ if is_string(ty) => {
                self.module.declare(STRING_EQ_DECL);
                let (a, a_len) = self.string_parts(left);
                let (b, b_len) = self.string_parts(right);
                let same = self.assign(&format!(
                    "call i32 @cooper_string_eq(ptr {a}, i64 {a_len}, ptr {b}, i64 {b_len})"
                ));
                return Ok(self.assign(&format!("icmp ne i32 {same}, 0")));
            }
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
    pub(super) fn tag(&mut self, value: &Value) -> String {
        self.assign(&format!("extractvalue {} {}, 0", value.ty, value.operand))
    }

    /// Payload slot `index` of `variant` in the sum-type value `value`, with its type.
    pub(super) fn payload_slot(
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

    /// `value << count` or `value >> count`, where `count` is any integer type. A count
    /// at least the value's width, or negative, stops the program. Bits a left shift
    /// moves out are discarded, and `>>` is arithmetic on signed integers.
    fn shift(&mut self, op: BinaryOp, scalar: Scalar, value: &Value, count_scalar: Scalar, count: &Value, span: Span) -> String {
        let (Scalar::Int { bits, signed }, Scalar::Int { bits: count_bits, .. }) = (scalar, count_scalar) else {
            unreachable!("the checker allows shifts on integers only");
        };
        // An unsigned comparison also catches a negative signed count; every integer
        // type holds the largest width, 64.
        let (cty, c) = (&count.ty, &count.operand);
        let wide = self.assign(&format!("icmp uge {cty} {c}, {bits}"));
        self.panic_if(&wide, "shift count out of range", span);
        let ty = &value.ty;
        let count = match count_bits.cmp(&bits) {
            std::cmp::Ordering::Equal => c.clone(),
            std::cmp::Ordering::Greater => self.assign(&format!("trunc {cty} {c} to {ty}")),
            std::cmp::Ordering::Less => self.assign(&format!("zext {cty} {c} to {ty}")),
        };
        let instruction = match (op, signed) {
            (BinaryOp::Shl, _) => "shl",
            (_, true) => "ashr",
            (_, false) => "lshr",
        };
        self.assign(&format!("{instruction} {ty} {}, {count}", value.operand))
    }

    /// Arithmetic `op` on two values of one scalar type, including the bitwise `and`,
    /// `or`, and `xor` on integers. Integer overflow, division by zero, and the
    /// overflowing signed division `MIN / -1` stop the program.
    pub(super) fn arithmetic(&mut self, op: BinaryOp, scalar: Scalar, left: &Value, right: &Value, span: Span) -> String {
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
                BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => {
                    let instruction = match op {
                        BinaryOp::And => "and",
                        BinaryOp::Or => "or",
                        _ => "xor",
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
    pub(super) fn convert(&mut self, operand: &IrExpr, target: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        if is_string(target) {
            return self.format(operand, span).map(Some);
        }
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

    /// `string(x)`: the text of a number or bool. Integers print in decimal, floats as
    /// the shortest text that reads back as the same value, bools as `true`/`false`.
    fn format(&mut self, operand: &IrExpr, span: Span) -> Result<Value, CodegenError> {
        let value = self.expr(operand)?.expect("a formatted value is a scalar");
        let string = Type::Primitive("string".to_string());
        match Scalar::of(&operand.ty) {
            Some(Scalar::Bool) => {
                let yes = self.module.string("true");
                let no = self.module.string("false");
                let data = self.assign(&format!("select i1 {}, ptr {yes}, ptr {no}", value.operand));
                let length = self.assign(&format!("select i1 {}, i64 4, i64 5", value.operand));
                let fields = [("ptr", data), ("i64", length)]
                    .into_iter()
                    .map(|(ty, operand)| Some(Value { ty: ty.to_string(), operand }))
                    .collect();
                Ok(self.aggregate(STRING, fields))
            }
            Some(Scalar::Int { signed, .. }) => {
                let wide = self.widen_index(&value, &operand.ty);
                self.module.declare(FORMAT_INT_DECL);
                let out = self.slot(None, &string, span)?;
                self.inst(&format!(
                    "call void @cooper_format_int(i64 {wide}, i32 {}, ptr {})",
                    i32::from(signed),
                    out.ptr
                ));
                Ok(self.load(&out).expect("a string has a value"))
            }
            Some(Scalar::Float { bits }) => {
                let wide = if bits == 32 {
                    self.assign(&format!("fpext float {} to double", value.operand))
                } else {
                    value.operand
                };
                self.module.declare(FORMAT_FLOAT_DECL);
                let out = self.slot(None, &string, span)?;
                self.inst(&format!(
                    "call void @cooper_format_float(double {wide}, i32 {}, ptr {})",
                    i32::from(bits == 32),
                    out.ptr
                ));
                Ok(self.load(&out).expect("a string has a value"))
            }
            None => self.unsupported(&format!("converting {} to string", operand.ty), span),
        }
    }
}
