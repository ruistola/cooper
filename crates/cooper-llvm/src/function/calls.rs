//! Calls: function values, bound methods, thunks, and receivers.

use cooper_frontend::resolve::Callable;

use super::*;

impl Emitter<'_, '_> {
    /// A call: direct to a named function or method, or indirect through a function
    /// value, which passes the value's environment as a hidden first argument.
    pub(super) fn call(&mut self, callee: &IrExpr, args: &[IrExpr], result: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        if let IrExprKind::FuncRef {
            item: Callable::Extern { name },
            ..
        } = &callee.kind
        {
            return self.extern_call(name, args, result, span);
        }
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

    /// A call to the C function `name`: each argument crosses as its C counterpart
    /// (a one-member struct as its member), and the result comes back the same way.
    fn extern_call(&mut self, name: &str, args: &[IrExpr], result: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        let mut operands = Vec::with_capacity(args.len());
        for arg in args {
            let value = self.expr(arg)?.expect("a C argument has a value");
            let c_type = self.module.c_param(&arg.ty).or_else(|what| self.unsupported(&what, span))?;
            let operand = self.c_argument(&arg.ty, value);
            operands.push(format!("{c_type} {operand}"));
        }
        if matches!(result, Type::Unit) {
            self.inst(&format!("call void @{name}({})", operands.join(", ")));
            return Ok(None);
        }
        let c_type = self.module.c_return(result).or_else(|what| self.unsupported(&what, span))?;
        let operand = self.assign(&format!("call {c_type} @{name}({})", operands.join(", ")));
        self.c_result(result, operand, span).map(Some)
    }

    /// The operand `value`, of type `ty`, passes to C as: a one-member struct's member.
    fn c_argument(&mut self, ty: &Type, value: Value) -> String {
        match self.module.defs.struct_members(ty).as_deref() {
            Some([(_, member)]) => {
                let inner = self.assign(&format!("extractvalue {} {}, 0", value.ty, value.operand));
                let llvm = self.module.llvm_type(member).ok().flatten().expect("a C member has a value");
                self.c_argument(member, Value { ty: llvm, operand: inner })
            }
            _ => value.operand,
        }
    }

    /// The value of type `ty` that the C result `operand` stands for.
    fn c_result(&mut self, ty: &Type, operand: String, span: Span) -> Result<Value, CodegenError> {
        let llvm = self.value_type(ty, span)?.expect("a C result has a value");
        match self.module.defs.struct_members(ty).as_deref() {
            Some([(_, member)]) => {
                let inner = self.c_result(member, operand, span)?;
                Ok(self.aggregate(&llvm, vec![Some(inner)]))
            }
            _ => Ok(Value { ty: llvm, operand }),
        }
    }

    /// A function value for the function instance `symbol`, of function type `ty`: its
    /// thunk, with no environment.
    pub(super) fn function_value(&mut self, symbol: &str, ty: &Type, span: Span) -> Result<Value, CodegenError> {
        let thunk = self.thunk(symbol, ty, None, span)?;
        Ok(Value {
            ty: FUNC_VALUE.to_string(),
            operand: format!("{{ ptr {thunk}, ptr null }}"),
        })
    }

    /// A closure of the lifted literal `symbol`: its code, with an environment of
    /// pointers to the storage of each captured variable (heap storage, since the
    /// closure may outlive the frame). A literal capturing nothing has no environment.
    pub(super) fn closure(&mut self, symbol: &str, captures: &[String]) -> Result<Value, CodegenError> {
        let env = if captures.is_empty() {
            "null".to_string()
        } else {
            let env = self.heap_alloc(&format!("[{} x ptr]", captures.len()));
            for (index, name) in captures.iter().enumerate() {
                let place = self.lookup(name);
                let ptr = if place.llvm.is_some() { place.ptr } else { "null".to_string() };
                let slot = self.assign(&format!("getelementptr ptr, ptr {env}, i64 {index}"));
                self.inst(&format!("store ptr {ptr}, ptr {slot}"));
            }
            env
        };
        let pointer = |operand: String| Some(Value {
            ty: "ptr".to_string(),
            operand,
        });
        Ok(self.aggregate(FUNC_VALUE, vec![pointer(format!("@{symbol}")), pointer(env)]))
    }

    /// A method bound to `receiver`: the method's thunk, with the receiver as its
    /// environment. A pointer receiver is the environment itself; a value receiver is
    /// copied to the heap when bound, so later changes to the original do not reach it.
    pub(super) fn bound_method(&mut self, receiver: &IrExpr, symbol: &str, ty: &Type, span: Span) -> Result<Value, CodegenError> {
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
