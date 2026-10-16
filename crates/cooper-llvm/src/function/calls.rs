//! Calls: function values, bound methods, thunks, and receivers.

use cooper_frontend::resolve::Callable;

use crate::ffi::{shim_symbol, Passing};

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

    /// A call to the C function `name`. Each argument and the result cross as
    /// [`Passing`] says: a scalar directly (out of any one-member struct around it), or
    /// a struct by pointer to the C wrapper, which makes the by-value call.
    fn extern_call(&mut self, name: &str, args: &[IrExpr], result: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        let unsupported = |e: &Self, what: String| e.unsupported::<()>(&what, span).unwrap_err();
        let params: Vec<Type> = args.iter().map(|a| a.ty.clone()).collect();
        let shim = self.module.needs_shim(&params, result).map_err(|w| unsupported(self, w))?;
        let mut operands = Vec::with_capacity(args.len() + 1);
        let result_slot = if shim && !matches!(result, Type::Unit) {
            let slot = self.slot(None, result, span)?;
            operands.push(format!("ptr {}", slot.ptr));
            Some(slot)
        } else {
            None
        };
        let fixed = self.module.variadics.get(name).copied();
        let mut declared = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if matches!(arg.ty, Type::Func { .. }) {
                let symbol = self.callback(arg, span)?;
                operands.push(format!("ptr {symbol}"));
                declared.push("ptr".to_string());
                continue;
            }
            let value = self.expr(arg)?.expect("a C argument has a value");
            if fixed.is_some_and(|fixed| index >= fixed) {
                let (c_type, operand) = self.promoted(arg, value, span)?;
                operands.push(format!("{c_type} {operand}"));
                continue;
            }
            match self.module.passing(&arg.ty).map_err(|w| unsupported(self, w))? {
                Passing::Indirect => {
                    let temp = self.slot(None, &arg.ty, span)?;
                    self.store(&temp, Some(&value));
                    operands.push(format!("ptr {}", temp.ptr));
                }
                Passing::Direct { .. } => {
                    let c_type = self.module.c_param(&arg.ty).map_err(|w| unsupported(self, w))?;
                    let operand = self.unwrap_members(&arg.ty, value, span)?;
                    operands.push(format!("{c_type} {operand}"));
                    declared.push(c_type);
                }
            }
        }
        // A call of a variadic function names its function type, to say where the
        // variable arguments begin.
        let variadic = fixed.is_some().then(|| format!("({}, ...)", declared.join(", ")).replace("(, ", "("));
        let target = if shim { shim_symbol(name) } else { name.to_string() };
        if let Some(slot) = result_slot {
            self.inst(&format!("call void @{target}({})", operands.join(", ")));
            return Ok(self.load(&slot));
        }
        if matches!(result, Type::Unit) {
            self.inst(&format!("call void @{target}({})", operands.join(", ")));
            return Ok(None);
        }
        let c_type = self.module.c_return(result).map_err(|w| unsupported(self, w))?;
        let operand = self.assign(&format!(
            "call {c_type} {}@{target}({})",
            variadic.map(|v| format!("{v} ")).unwrap_or_default(),
            operands.join(", ")
        ));
        self.wrap_members(result, operand, span).map(Some)
    }

    /// A variable argument of a C call, widened as C's default promotions do: integers
    /// narrower than `int` and `bool` to `int`, `float` to `double`. Only a scalar or a
    /// pointer can be passed.
    fn promoted(&mut self, arg: &IrExpr, value: Value, span: Span) -> Result<(String, String), CodegenError> {
        match Scalar::of(&arg.ty) {
            Some(Scalar::Bool) => Ok(("i32".to_string(), self.assign(&format!("zext i1 {} to i32", value.operand)))),
            Some(Scalar::Int { bits, signed }) if bits < 32 => {
                let op = if signed { "sext" } else { "zext" };
                Ok(("i32".to_string(), self.assign(&format!("{op} {} {} to i32", value.ty, value.operand))))
            }
            Some(Scalar::Float { bits: 32 }) => {
                Ok(("double".to_string(), self.assign(&format!("fpext float {} to double", value.operand))))
            }
            Some(_) => Ok((value.ty, value.operand)),
            None if matches!(arg.ty, Type::Pointer(_) | Type::Nil) => Ok(("ptr".to_string(), value.operand)),
            None => match &arg.ty {
                Type::Struct { .. } if matches!(self.module.passing(&arg.ty), Ok(Passing::Direct { .. })) => {
                    let c_type = self.module.c_param(&arg.ty).map_err(|w| self.unsupported::<()>(&w, span).unwrap_err())?;
                    let operand = self.unwrap_members(&arg.ty, value, span)?;
                    Ok((c_type, operand))
                }
                _ => self.unsupported(&format!("passing {} as a variable argument to C", arg.ty), span),
            },
        }
    }

    /// The C function pointer for the function `arg` names: the function itself when it is
    /// a C function, otherwise a wrapper with the C calling convention (which extends
    /// small integers), defined on first use. Only a function named directly qualifies, since
    /// a C callback has nowhere to keep a closure's environment.
    fn callback(&mut self, arg: &IrExpr, span: Span) -> Result<String, CodegenError> {
        let IrExprKind::FuncRef { item, type_args } = &arg.kind else {
            return self.unsupported("a C callback that is not the name of a function", span);
        };
        if let Callable::Extern { name } = item {
            return Ok(format!("@{name}"));
        }
        let symbol = mangle::symbol(item, type_args);
        let name = format!("@{symbol}.c");
        if self.module.thunks.contains_key(&name) {
            return Ok(name);
        }
        let Type::Func { param_types, return_type } = &arg.ty else {
            unreachable!("a callback has a function type");
        };
        let unsupported = |e: &Self, what: String| e.unsupported::<()>(&what, span).unwrap_err();
        let ret = self.signature_type(return_type, span)?;
        let c_ret = if ret == "void" { ret.clone() } else { self.module.c_return(return_type).map_err(|w| unsupported(self, w))? };
        let mut params = Vec::new();
        let mut args = Vec::new();
        for (index, ty) in param_types.iter().enumerate() {
            let llvm = self.value_type(ty, span)?.expect("a callback parameter has a value");
            params.push(format!("{} %a{index}", self.module.c_param(ty).map_err(|w| unsupported(self, w))?));
            args.push(format!("{llvm} %a{index}"));
        }
        let call = format!("call {ret} @{symbol}({})", args.join(", "));
        let body = if ret == "void" {
            format!("  {call}\n  ret void\n")
        } else {
            format!("  %r = {call}\n  ret {ret} %r\n")
        };
        self.module.thunks.insert(
            name.clone(),
            format!("define private {c_ret} {name}({}) {{\n{body}}}\n", params.join(", ")),
        );
        Ok(name)
    }

    /// The scalar inside the one-member structs around `value`, of type `ty`.
    fn unwrap_members(&mut self, ty: &Type, value: Value, span: Span) -> Result<String, CodegenError> {
        match self.module.defs.struct_members(ty).as_deref() {
            Some([(_, member)]) => {
                let inner = self.extract(&value, 0, member, span)?.expect("a C member has a value");
                self.unwrap_members(member, inner, span)
            }
            _ => Ok(value.operand),
        }
    }

    /// The value of type `ty` that the direct C result `operand` stands for, wrapping
    /// it in the one-member structs `ty` has around its scalar.
    fn wrap_members(&mut self, ty: &Type, operand: String, span: Span) -> Result<Value, CodegenError> {
        let llvm = self.value_type(ty, span)?.expect("a C result has a value");
        match self.module.defs.struct_members(ty).as_deref() {
            Some([(_, member)]) => {
                let inner = self.wrap_members(member, operand, span)?;
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
    /// pointers to the storage of each captured variable (in the frame when analysis shows the
    /// closure does not outlive it, otherwise on the heap). A literal capturing nothing has no environment.
    /// Without a `symbol`, the closure has no code: it holds only the environment of a
    /// generic function, whose uses supply the code.
    pub(super) fn closure(&mut self, symbol: Option<&str>, captures: &[String], span: Span) -> Result<Value, CodegenError> {
        let env = if captures.is_empty() {
            "null".to_string()
        } else {
            let shape = format!("[{} x ptr]", captures.len());
            let desc = self.module.pointers_descriptor(captures.len());
            let env = if self.stack_envs.contains(&span) { self.stack_alloc(&shape) } else { self.heap_alloc(&shape, desc.as_deref()) };
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
        Ok(self.aggregate(FUNC_VALUE, vec![pointer(symbol.map_or("null".to_string(), |symbol| format!("@{symbol}"))), pointer(env)]))
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
            let desc = self.module.type_descriptor(&wanted);
            let copy = self.heap_alloc(&value.ty, desc.as_deref());
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
