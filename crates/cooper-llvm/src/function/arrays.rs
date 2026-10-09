//! Strings, arrays, and the runtime intrinsics.

use super::*;

impl Emitter<'_, '_> {
    /// The data pointer and length of a string value.
    pub(super) fn string_parts(&mut self, string: &Value) -> (String, String) {
        let data = self.assign(&format!("extractvalue {STRING} {}, 0", string.operand));
        let len = self.assign(&format!("extractvalue {STRING} {}, 1", string.operand));
        (data, len)
    }

    /// A fresh string holding `left` followed by `right`.
    pub(super) fn concat(&mut self, left: &Value, right: &Value, span: Span) -> Result<Value, CodegenError> {
        self.module.declare(CONCAT_DECL);
        let (a, a_len) = self.string_parts(left);
        let (b, b_len) = self.string_parts(right);
        let out = self.slot(None, &Type::Primitive("string".to_string()), span)?;
        self.inst(&format!(
            "call void @cooper_string_concat(ptr {a}, i64 {a_len}, ptr {b}, i64 {b_len}, ptr {})",
            out.ptr
        ));
        Ok(self.load(&out).expect("a string has a value"))
    }

    /// The size in bytes of one value of LLVM type `llvm`, as an `i64` operand.
    fn size_of(&mut self, llvm: &str) -> String {
        let end = self.assign(&format!("getelementptr {llvm}, ptr null, i32 1"));
        self.assign(&format!("ptrtoint ptr {end} to i64"))
    }

    /// A built-in array operation.
    pub(super) fn intrinsic(&mut self, op: Intrinsic, args: &[IrExpr], result: &Type, span: Span) -> Result<Option<Value>, CodegenError> {
        if let Intrinsic::Print { newline } = op {
            let text = self.expr(&args[0])?.expect("a string has a value");
            let (data, length) = self.string_parts(&text);
            self.module.declare(PRINT_DECL);
            self.inst(&format!(
                "call void @cooper_print(ptr {data}, i64 {length}, i32 {})",
                i32::from(newline)
            ));
            return Ok(None);
        }
        if matches!(op, Intrinsic::ToCString | Intrinsic::FromCString | Intrinsic::CopyBytes) {
            return self.ffi(op, args, result, span).map(Some);
        }
        if matches!(
            op,
            Intrinsic::FormatBegin
                | Intrinsic::FormatPrint
                | Intrinsic::FormatEnter
                | Intrinsic::FormatLeave
                | Intrinsic::FormatLabel
                | Intrinsic::FormatReference
                | Intrinsic::FormatRadix(_)
                | Intrinsic::FormatFixed { .. }
                | Intrinsic::Pad { .. }
                | Intrinsic::Quote
        ) {
            return self.formatting(op, args, span);
        }
        if op == Intrinsic::ArrayLiteral {
            let Type::Array(elem) = result else {
                unreachable!("an array literal is an array");
            };
            let values = args.iter().map(|a| self.expr(a)).collect::<Result<Vec<_>, _>>()?;
            return self.array_literal(elem, values, span).map(Some);
        }
        let Type::Array(elem) = &args[0].ty else {
            unreachable!("an array intrinsic's first argument is the array");
        };
        let elem = (**elem).clone();
        let array = self.expr(&args[0])?.expect("an array has a value");
        match op {
            Intrinsic::ArrayLiteral
            | Intrinsic::Print { .. }
            | Intrinsic::ToCString
            | Intrinsic::FromCString
            | Intrinsic::CopyBytes
            | Intrinsic::FormatBegin
            | Intrinsic::FormatPrint
            | Intrinsic::FormatEnter
            | Intrinsic::FormatLeave
            | Intrinsic::FormatLabel
            | Intrinsic::FormatReference
            | Intrinsic::FormatRadix(_)
            | Intrinsic::FormatFixed { .. }
            | Intrinsic::Pad { .. }
            | Intrinsic::Quote => unreachable!("handled above"),
            Intrinsic::Format(_) => unreachable!("expanded before code generation"),
            Intrinsic::ArrayLength => {
                let operand = self.assign(&format!("extractvalue {ARRAY} {}, 1", array.operand));
                Ok(Some(Value {
                    ty: "i64".to_string(),
                    operand,
                }))
            }
            Intrinsic::ArrayGet | Intrinsic::ArraySet | Intrinsic::ArrayElementPtr => {
                let index = self.expr(&args[1])?.expect("an index has a value");
                let ptr = self.element(&array, &index, &args[1].ty, &elem, span)?;
                let place = Place {
                    ptr: ptr.clone(),
                    llvm: self.value_type(&elem, span)?,
                    ty: elem,
                    nullable: false,
                };
                match op {
                    Intrinsic::ArrayGet => Ok(self.load(&place)),
                    Intrinsic::ArraySet => {
                        let value = self.expr(&args[2])?;
                        self.store(&place, value.as_ref());
                        Ok(None)
                    }
                    _ => Ok(Some(Value {
                        ty: "ptr".to_string(),
                        operand: ptr,
                    })),
                }
            }
            Intrinsic::ArrayPush => {
                let value = self.expr(&args[1])?;
                self.push(&array, &elem, value, span).map(Some)
            }
            Intrinsic::ArraySlice { inclusive } => {
                let start = self.expr(&args[1])?.expect("a bound has a value");
                let start = self.widen_index(&start, &args[1].ty);
                let end = match args.get(2) {
                    Some(end_expr) => {
                        let end = self.expr(end_expr)?.expect("a bound has a value");
                        let end = self.widen_index(&end, &end_expr.ty);
                        Some(if inclusive { self.assign(&format!("add i64 {end}, 1")) } else { end })
                    }
                    None => None,
                };
                self.slice(&array, &elem, &start, end, span).map(Some)
            }
            Intrinsic::ArrayCopy => self.copy(&array, &elem, span).map(Some),
            Intrinsic::ArrayReserve => {
                let count = self.expr(&args[1])?.expect("a count has a value");
                self.reserve(&array, &elem, &count.operand, span).map(Some)
            }
        }
    }

    /// A `std.ffi` copy between Cooper and C data, of result type `result`.
    fn ffi(&mut self, op: Intrinsic, args: &[IrExpr], result: &Type, span: Span) -> Result<Value, CodegenError> {
        let arg = self.expr(&args[0])?.expect("an ffi argument has a value");
        match op {
            Intrinsic::ToCString => {
                let (data, length) = self.string_parts(&arg);
                self.module.declare(TO_CSTRING_DECL);
                let c = self.assign(&format!("call ptr @cooper_to_cstring(ptr {data}, i64 {length})"));
                let llvm = self.value_type(result, span)?.expect("a CString has a value");
                let pointer = Value {
                    ty: "ptr".to_string(),
                    operand: c,
                };
                Ok(self.aggregate(&llvm, vec![Some(pointer)]))
            }
            Intrinsic::FromCString => {
                let c = self.assign(&format!("extractvalue {} {}, 0", arg.ty, arg.operand));
                self.module.declare(FROM_CSTRING_DECL);
                let out = self.slot(None, result, span)?;
                self.inst(&format!("call void @cooper_from_cstring(ptr {c}, ptr {})", out.ptr));
                Ok(self.load(&out).expect("a string has a value"))
            }
            Intrinsic::CopyBytes => {
                let count = self.expr(&args[1])?.expect("a count has a value");
                let negative = self.assign(&format!("icmp slt i64 {}, 0", count.operand));
                self.panic_if(&negative, "negative byte count", span);
                self.module.declare(COPY_BYTES_DECL);
                let data = self.assign(&format!(
                    "call ptr @cooper_copy_bytes(ptr {}, i64 {})",
                    arg.operand, count.operand
                ));
                let word = |operand: &str| Some(Value {
                    ty: "i64".to_string(),
                    operand: operand.to_string(),
                });
                let fields = vec![
                    Some(Value {
                        ty: "ptr".to_string(),
                        operand: data,
                    }),
                    word(&count.operand),
                    word(&count.operand),
                ];
                Ok(self.aggregate(ARRAY, fields))
            }
            _ => unreachable!("not a std.ffi intrinsic"),
        }
    }

    /// A fresh array of the given elements, its capacity exactly its length.
    fn array_literal(&mut self, elem: &Type, values: Vec<Option<Value>>, span: Span) -> Result<Value, CodegenError> {
        let count = values.len();
        if count == 0 {
            return Ok(Value {
                ty: ARRAY.to_string(),
                operand: "zeroinitializer".to_string(),
            });
        }
        let data = match self.value_type(elem, span)? {
            Some(llvm) => {
                let storage = self.heap_alloc(&format!("[{count} x {llvm}]"));
                for (index, value) in values.iter().enumerate() {
                    let value = value.as_ref().expect("an element has a value");
                    let slot = self.assign(&format!("getelementptr {llvm}, ptr {storage}, i64 {index}"));
                    self.inst(&format!("store {llvm} {}, ptr {slot}", value.operand));
                }
                storage
            }
            None => "null".to_string(),
        };
        let pointer = Value {
            ty: "ptr".to_string(),
            operand: data,
        };
        let length = Value {
            ty: "i64".to_string(),
            operand: count.to_string(),
        };
        Ok(self.aggregate(ARRAY, vec![Some(pointer), Some(length.clone()), Some(length)]))
    }

    /// The address of element `index` (of integer type `index_ty`) of `array`, stopping
    /// the program when the index is out of bounds.
    pub(super) fn element(
        &mut self,
        array: &Value,
        index: &Value,
        index_ty: &Type,
        elem: &Type,
        span: Span,
    ) -> Result<String, CodegenError> {
        let index = self.widen_index(index, index_ty);
        let length = self.assign(&format!("extractvalue {ARRAY} {}, 1", array.operand));
        // Compared unsigned, a negative index is a huge one: one test rejects both.
        let outside = self.assign(&format!("icmp uge i64 {index}, {length}"));
        self.panic_if(&outside, "index out of bounds", span);
        let data = self.assign(&format!("extractvalue {ARRAY} {}, 0", array.operand));
        Ok(match self.value_type(elem, span)? {
            Some(llvm) => self.assign(&format!("getelementptr {llvm}, ptr {data}, i64 {index}")),
            None => data,
        })
    }

    /// An integer of type `ty` (an index, or a number to format) as an `i64` operand,
    /// extended by its signedness.
    pub(super) fn widen_index(&mut self, index: &Value, ty: &Type) -> String {
        match Scalar::of(ty) {
            Some(Scalar::Int { bits: 64, .. }) => index.operand.clone(),
            Some(Scalar::Int { signed: true, .. }) => {
                self.assign(&format!("sext {} {} to i64", index.ty, index.operand))
            }
            Some(Scalar::Int { signed: false, .. }) => {
                self.assign(&format!("zext {} {} to i64", index.ty, index.operand))
            }
            _ => unreachable!("a checked index is an integer"),
        }
    }

    /// Elements `start` up to `end` (the array's end when `None`) of `array`, sharing
    /// its elements, with capacity equal to its length so growing the slice copies
    /// first. Bounds outside `0 <= start <= end <= length` stop the program.
    fn slice(
        &mut self,
        array: &Value,
        elem: &Type,
        start: &str,
        end: Option<String>,
        span: Span,
    ) -> Result<Value, CodegenError> {
        let a = &array.operand;
        let data = self.assign(&format!("extractvalue {ARRAY} {a}, 0"));
        let length = self.assign(&format!("extractvalue {ARRAY} {a}, 1"));
        let end = end.unwrap_or_else(|| length.clone());
        // Unsigned, a negative bound is huge: checking `end <= length` and then
        // `start <= end` rejects every bound outside the array.
        let past_end = self.assign(&format!("icmp ugt i64 {end}, {length}"));
        self.panic_if(&past_end, "slice bounds out of range", span);
        let reversed = self.assign(&format!("icmp ugt i64 {start}, {end}"));
        self.panic_if(&reversed, "slice bounds out of range", span);
        let count = self.assign(&format!("sub i64 {end}, {start}"));
        let first = match self.value_type(elem, span)? {
            Some(llvm) => self.assign(&format!("getelementptr {llvm}, ptr {data}, i64 {start}")),
            // Unit elements occupy no storage.
            None => data,
        };
        Ok(self.array_value(first, count.clone(), count))
    }

    /// `array.copy()`: a fresh array holding a copy of the elements, with capacity
    /// equal to its length.
    fn copy(&mut self, array: &Value, elem: &Type, span: Span) -> Result<Value, CodegenError> {
        let a = &array.operand;
        let data = self.assign(&format!("extractvalue {ARRAY} {a}, 0"));
        let length = self.assign(&format!("extractvalue {ARRAY} {a}, 1"));
        let storage = match self.value_type(elem, span)? {
            Some(llvm) => self.grown(&data, &length, &length, &llvm),
            None => data,
        };
        Ok(self.array_value(storage, length.clone(), length))
    }

    /// `array.reserve(n)`: the array itself when its spare capacity holds `n` more
    /// elements, otherwise a copy with capacity for the larger of `length + n` and
    /// twice the current capacity. A negative `n` reserves nothing.
    fn reserve(&mut self, array: &Value, elem: &Type, count: &str, span: Span) -> Result<Value, CodegenError> {
        let Some(llvm) = self.value_type(elem, span)? else {
            return Ok(array.clone());
        };
        let a = &array.operand;
        let data = self.assign(&format!("extractvalue {ARRAY} {a}, 0"));
        let length = self.assign(&format!("extractvalue {ARRAY} {a}, 1"));
        let capacity = self.assign(&format!("extractvalue {ARRAY} {a}, 2"));
        let negative = self.assign(&format!("icmp slt i64 {count}, 0"));
        let wanted = self.assign(&format!("select i1 {negative}, i64 0, i64 {count}"));
        let spare = self.assign(&format!("sub i64 {capacity}, {length}"));
        let fits = self.assign(&format!("icmp ule i64 {wanted}, {spare}"));
        let from = self.current.clone();
        let grow = self.fresh("reserve");
        let ready = self.fresh("reserved");
        self.terminate(&format!("br i1 {fits}, label %{ready}, label %{grow}"));
        self.start_block(&grow);
        let needed = self.assign(&format!("add i64 {length}, {wanted}"));
        let doubled = self.assign(&format!("shl i64 {capacity}, 1"));
        let more = self.assign(&format!("icmp ugt i64 {needed}, {doubled}"));
        let grown_capacity = self.assign(&format!("select i1 {more}, i64 {needed}, i64 {doubled}"));
        let grown = self.grown(&data, &length, &grown_capacity, &llvm);
        let grown_from = self.current.clone();
        self.start_block(&ready);
        let storage = self.assign(&format!("phi ptr [ {data}, %{from} ], [ {grown}, %{grown_from} ]"));
        let room = self.assign(&format!(
            "phi i64 [ {capacity}, %{from} ], [ {grown_capacity}, %{grown_from} ]"
        ));
        Ok(self.array_value(storage, length, room))
    }

    /// Fresh storage for `capacity` elements of LLVM type `llvm`, holding a copy of the
    /// `length` elements at `data`.
    fn grown(&mut self, data: &str, length: &str, capacity: &str, llvm: &str) -> String {
        self.module.declare(GROW_DECL);
        let size = self.size_of(llvm);
        self.assign(&format!(
            "call ptr @cooper_array_grow(ptr {data}, i64 {length}, i64 {capacity}, i64 {size})"
        ))
    }

    /// An array value from its storage, length, and capacity operands.
    fn array_value(&mut self, storage: String, length: String, capacity: String) -> Value {
        let fields = [("ptr", storage), ("i64", length), ("i64", capacity)]
            .into_iter()
            .map(|(ty, operand)| Some(Value { ty: ty.to_string(), operand }))
            .collect();
        self.aggregate(ARRAY, fields)
    }

    /// `array.push(value)`: the array with `value` appended. Spare capacity is written
    /// in place, so the result shares the original's storage; a full array is copied
    /// into fresh storage of twice the capacity (at least four) first.
    fn push(&mut self, array: &Value, elem: &Type, value: Option<Value>, span: Span) -> Result<Value, CodegenError> {
        let a = &array.operand;
        let data = self.assign(&format!("extractvalue {ARRAY} {a}, 0"));
        let length = self.assign(&format!("extractvalue {ARRAY} {a}, 1"));
        let capacity = self.assign(&format!("extractvalue {ARRAY} {a}, 2"));
        let Some(llvm) = self.value_type(elem, span)? else {
            let longer = self.assign(&format!("add i64 {length}, 1"));
            return Ok(self.array_value(data, longer.clone(), longer));
        };
        let full = self.assign(&format!("icmp eq i64 {length}, {capacity}"));
        let from = self.current.clone();
        let grow = self.fresh("grow");
        let ready = self.fresh("ready");
        self.terminate(&format!("br i1 {full}, label %{grow}, label %{ready}"));
        self.start_block(&grow);
        let empty = self.assign(&format!("icmp eq i64 {capacity}, 0"));
        let doubled = self.assign(&format!("shl i64 {capacity}, 1"));
        let grown_capacity = self.assign(&format!("select i1 {empty}, i64 4, i64 {doubled}"));
        let grown = self.grown(&data, &length, &grown_capacity, &llvm);
        let grown_from = self.current.clone();
        self.start_block(&ready);
        let storage = self.assign(&format!("phi ptr [ {data}, %{from} ], [ {grown}, %{grown_from} ]"));
        let room = self.assign(&format!(
            "phi i64 [ {capacity}, %{from} ], [ {grown_capacity}, %{grown_from} ]"
        ));
        let slot = self.assign(&format!("getelementptr {llvm}, ptr {storage}, i64 {length}"));
        if let Some(value) = value {
            self.inst(&format!("store {llvm} {}, ptr {slot}", value.operand));
        }
        let longer = self.assign(&format!("add i64 {length}, 1"));
        Ok(self.array_value(storage, longer, room))
    }

    /// A zeroed heap allocation for one value of LLVM type `llvm`.
    pub(super) fn heap_alloc(&mut self, llvm: &str) -> String {
        self.module.declare(ALLOC_DECL);
        let size = self.size_of(llvm);
        self.assign(&format!("call ptr @cooper_alloc(i64 {size})"))
    }
}
