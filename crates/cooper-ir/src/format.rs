//! Formatting: `string(x)` for every type.
//!
//! A number or `bool` converts to text in the backend. Every other conversion to
//! `string` is expanded here, after monomorphization, when every type is concrete: a
//! `CString` is copied, a `string` is itself, and any other value is formatted by a
//! generated function, one per type, which calls the functions of the types it contains.
//! The text mirrors the literal syntax: `Point{x: 1, y: "a"}`, `[1, 2]`, `(1, 2)`,
//! `Some(3)`, `None`, `()`. A string inside a value is quoted. A function value is
//! `<func>`. A pointer is `nil`, or `&` and its target's text. A target the value reaches
//! more than once, through a cycle or sharing, is expanded once, labelled, and referred
//! to by its label afterwards: `#1=&Node{value: 1, next: &Node{value: 2, next: #1}}`.
//! Finding those targets takes a first pass over the value, whose text is discarded. A
//! target nested too deeply is `...`.

use super::*;

use cooper_frontend::ast::Align;

/// Expand every conversion to `string` in `functions`, returning them with the
/// formatting functions they need appended.
pub(crate) fn expand_formatting(mut functions: Vec<Function>, defs: &TypeDefs) -> Vec<Function> {
    let mut formatters = Formatters {
        defs,
        types: Vec::new(),
    };
    for function in &mut functions {
        let file = function.file.clone();
        visit_exprs_mut(&mut function.body, &mut |expr| formatters.rewrite(expr, &file));
    }
    // Generating one formatter may name the formatters of the types inside its type.
    let mut generated = 0;
    while generated < formatters.types.len() {
        let (ty, file) = formatters.types[generated].clone();
        let function = formatters.formatter(generated as u32, &ty, &file);
        functions.push(function);
        generated += 1;
    }
    functions
}

struct Formatters<'d> {
    defs: &'d TypeDefs,
    /// Each formatted type, by formatter index, with a file for its formatter's spans.
    types: Vec<(Type, String)>,
}

/// The parameter every formatter takes: the value to format.
const VALUE: &str = "value";

impl Formatters<'_> {
    /// Replace `expr` if it is a conversion to `string` of a value no backend
    /// instruction formats, or a value formatted by a format spec.
    fn rewrite(&mut self, expr: &mut IrExpr, file: &str) {
        let span = expr.span;
        match &mut expr.kind {
            IrExprKind::Convert(operand) if is_string(&expr.ty) && !is_scalar(&operand.ty) => {
                let operand = std::mem::replace(operand.as_mut(), unit(span));
                *expr = self.format_value(operand, file, span);
            }
            IrExprKind::Intrinsic {
                op: Intrinsic::Format(spec),
                args,
            } => {
                let spec = *spec;
                let value = args.pop().expect("a formatted value");
                *expr = self.format_spec(value, &spec, file, span);
            }
            _ => {}
        }
    }

    /// `value` formatted as `spec` asks: its digits in a radix, a float's to a
    /// precision, or `string(x)`, then padded to a width.
    fn format_spec(&mut self, value: IrExpr, spec: &FormatSpec, file: &str, span: Span) -> IrExpr {
        let numeric = is_scalar(&value.ty) && !matches!(&value.ty, Type::Primitive(name) if name == "bool");
        let text = if let Some(radix) = spec.radix {
            intrinsic(Intrinsic::FormatRadix(radix), vec![value], string_type(), span)
        } else if let Some(precision) = spec.precision {
            intrinsic(Intrinsic::FormatFixed { precision }, vec![value], string_type(), span)
        } else if is_scalar(&value.ty) {
            IrExpr {
                kind: IrExprKind::Convert(Box::new(value)),
                ty: string_type(),
                span,
            }
        } else {
            self.format_value(value, file, span)
        };
        let Some(width) = spec.width else {
            return text;
        };
        let align = spec.align.unwrap_or(if numeric { Align::Right } else { Align::Left });
        let pad = Intrinsic::Pad {
            width,
            align,
            fill: spec.fill,
            zero: spec.zero,
        };
        intrinsic(pad, vec![text], string_type(), span)
    }

    /// `string(operand)` for an operand that is not a number or `bool`.
    fn format_value(&mut self, operand: IrExpr, file: &str, span: Span) -> IrExpr {
        if is_string(&operand.ty) {
            operand
        } else if stdlib::is_c_string(&operand.ty) {
            intrinsic(Intrinsic::FromCString, vec![operand], string_type(), span)
        } else {
            let ty = operand.ty.clone();
            if !reaches_pointer(self.defs, &ty, &mut Vec::new()) {
                return self.call(&ty, operand, file, span);
            }
            // The value is evaluated first: formatting it must not interleave with any
            // other formatting the evaluation does. The first pass finds shared targets.
            let name = "fmt.value".to_string();
            let call = self.call(&ty, var(&name, &ty, span), file, span);
            block(
                vec![
                    let_stmt(&name, operand, span),
                    expr_stmt(intrinsic(Intrinsic::FormatBegin, Vec::new(), Type::Unit, span)),
                    expr_stmt(call.clone()),
                    expr_stmt(intrinsic(Intrinsic::FormatPrint, Vec::new(), Type::Unit, span)),
                ],
                call,
            )
        }
    }

    /// A call formatting `value`, of type `ty`, through its type's formatter.
    fn call(&mut self, ty: &Type, value: IrExpr, file: &str, span: Span) -> IrExpr {
        if is_scalar(ty) {
            return IrExpr {
                kind: IrExprKind::Convert(Box::new(value)),
                ty: string_type(),
                span,
            };
        }
        let index = match self.types.iter().position(|(known, _)| known.equals(ty)) {
            Some(index) => index,
            None => {
                self.types.push((ty.clone(), file.to_string()));
                self.types.len() - 1
            }
        };
        let item = Callable::Formatter { index: index as u32 };
        let callee = IrExpr {
            kind: IrExprKind::FuncRef {
                item,
                type_args: Vec::new(),
            },
            ty: Type::Func {
                return_type: Box::new(string_type()),
                param_types: vec![ty.clone()],
            },
            span,
        };
        IrExpr {
            kind: IrExprKind::Call {
                callee: Box::new(callee),
                args: vec![value],
            },
            ty: string_type(),
            span,
        }
    }

    /// The formatter of `ty`: a function of one parameter returning its text.
    fn formatter(&mut self, index: u32, ty: &Type, file: &str) -> Function {
        let span = Span::new(0, 0);
        let value = var(VALUE, ty, span);
        let body = match ty {
            Type::Primitive(name) if name == "string" => {
                vec![ret(intrinsic(Intrinsic::Quote, vec![value], string_type(), span))]
            }
            Type::Unit => vec![ret(text("()", span))],
            Type::Nil => vec![ret(text("nil", span))],
            Type::Func { .. } => vec![ret(text("<func>", span))],
            Type::Tuple(elems) => {
                let names: Vec<String> = (0..elems.len()).map(|i| format!("e.{i}")).collect();
                let bindings = names
                    .iter()
                    .zip(elems)
                    .map(|(name, ty)| Binder {
                        name: name.clone(),
                        ty: ty.clone(),
                    })
                    .collect();
                let destructure = IrExpr {
                    kind: IrExprKind::LetTuple {
                        bindings,
                        value: Box::new(value),
                    },
                    ty: ty.clone(),
                    span,
                };
                let parts = names
                    .iter()
                    .zip(elems)
                    .map(|(name, elem)| self.call(elem, var(name, elem, span), file, span))
                    .collect();
                vec![expr_stmt(destructure), ret(enclose("(", parts, ")", span))]
            }
            Type::Struct { id, .. } => {
                let members = self.defs.struct_members(ty).expect("a formatted struct is defined");
                let parts = members
                    .into_iter()
                    .map(|(name, member)| {
                        let field = IrExpr {
                            kind: IrExprKind::Field {
                                target: Box::new(value.clone()),
                                name: name.clone(),
                            },
                            ty: member.clone(),
                            span,
                        };
                        concat(text(&format!("{name}: "), span), self.call(&member, field, file, span))
                    })
                    .collect();
                vec![ret(enclose(&format!("{}{{", id.name), parts, "}", span))]
            }
            Type::Oneof { .. } => self.variants(ty, value, file, span),
            Type::Array(elem) => self.array(elem, value, file, span),
            Type::Pointer(target) => self.pointer(target, value, file, span),
            _ => unreachable!("a value of type {ty} is never formatted"),
        };
        Function {
            item: Callable::Formatter { index },
            file: file.to_string(),
            receiver: None,
            env: None,
            type_params: Vec::new(),
            type_args: Vec::new(),
            params: vec![Param {
                name: VALUE.to_string(),
                ty: ty.clone(),
                span,
            }],
            return_type: string_type(),
            body,
            span,
        }
    }

    /// `match value { V(p0, p1) => return "V(" + … + ")", … }`.
    fn variants(&mut self, ty: &Type, value: IrExpr, file: &str, span: Span) -> Vec<IrStmt> {
        let mut patterns = Vec::new();
        let mut actions = Vec::new();
        for variant in self.defs.variant_order(ty).to_vec() {
            let slots = self.defs.payload(ty, &variant).expect("a listed variant has a payload");
            let binders: Vec<Binder> = slots
                .iter()
                .enumerate()
                .map(|(i, slot)| Binder {
                    name: format!("p.{i}"),
                    ty: slot.clone(),
                })
                .collect();
            let action = if binders.is_empty() {
                text(&variant, span)
            } else {
                let parts = binders
                    .iter()
                    .map(|b| self.call(&b.ty, var(&b.name, &b.ty, span), file, span))
                    .collect();
                enclose(&format!("{variant}("), parts, ")", span)
            };
            patterns.push(IrPattern {
                kind: IrPatternKind::Variant { variant, binders },
                ty: ty.clone(),
                span,
            });
            actions.push(ret(action));
        }
        let tree = compile_match(self.defs, ty, &patterns);
        vec![IrStmt {
            kind: IrStmtKind::Match {
                scrutinee: value,
                actions,
                tree,
            },
            span,
        }]
    }

    /// `out := "["; sep := ""; for e in value { out = out + sep + f(e); sep = ", " }`.
    fn array(&mut self, elem: &Type, value: IrExpr, file: &str, span: Span) -> Vec<IrStmt> {
        let out = var("out", &string_type(), span);
        let sep = var("sep", &string_type(), span);
        let item = self.call(elem, var("e", elem, span), file, span);
        let body = vec![
            assign(out.clone(), concat(concat(out.clone(), sep.clone()), item)),
            assign(sep.clone(), text(", ", span)),
        ];
        vec![
            let_stmt("out", text("[", span), span),
            let_stmt("sep", text("", span), span),
            IrStmt {
                kind: IrStmtKind::ForEach {
                    array: Box::new(value),
                    index: None,
                    elem: Binder {
                        name: "e".to_string(),
                        ty: elem.clone(),
                    },
                    body,
                },
                span,
            },
            ret(concat(out, text("]", span))),
        ]
    }

    /// `nil`; a label (if shared), `&`, and the target's text; or, for a target already
    /// expanded or nested too deeply, its label or `...`.
    fn pointer(&mut self, target: &Type, value: IrExpr, file: &str, span: Span) -> Vec<IrStmt> {
        let boolean = Type::Primitive("bool".to_string());
        let is_nil = IrExpr {
            kind: IrExprKind::Binary {
                op: BinaryOp::Eq,
                lhs: Box::new(value.clone()),
                rhs: Box::new(IrExpr {
                    kind: IrExprKind::Nil,
                    ty: Type::Nil,
                    span,
                }),
            },
            ty: boolean.clone(),
            span,
        };
        let deref = IrExpr {
            kind: IrExprKind::Deref(Box::new(value.clone())),
            ty: target.clone(),
            span,
        };
        let label = intrinsic(Intrinsic::FormatLabel, vec![value.clone()], string_type(), span);
        let expanded = concat(concat(label, text("&", span)), self.call(target, deref, file, span));
        let enter = intrinsic(Intrinsic::FormatEnter, vec![value.clone()], boolean, span);
        vec![
            if_stmt(self.defs, is_nil, vec![ret(text("nil", span))], span),
            if_stmt(
                self.defs,
                enter,
                vec![
                    let_stmt("out", expanded, span),
                    expr_stmt(intrinsic(Intrinsic::FormatLeave, Vec::new(), Type::Unit, span)),
                    ret(var("out", &string_type(), span)),
                ],
                span,
            ),
            ret(intrinsic(Intrinsic::FormatReference, vec![value], string_type(), span)),
        ]
    }
}

/// Whether a value of type `ty` can hold a pointer, which is all that can make a
/// formatting reach one target twice: without one, formatting takes a single pass.
/// `visiting` holds the types being examined, which a type may contain through an array.
fn reaches_pointer(defs: &TypeDefs, ty: &Type, visiting: &mut Vec<Type>) -> bool {
    if visiting.iter().any(|seen| seen.equals(ty)) {
        return false;
    }
    match ty {
        Type::Pointer(_) => true,
        Type::Array(elem) => reaches_pointer(defs, elem, visiting),
        Type::Tuple(elems) => elems.iter().any(|e| reaches_pointer(defs, e, visiting)),
        Type::Struct { .. } | Type::Oneof { .. } => {
            visiting.push(ty.clone());
            let parts: Vec<Type> = match ty {
                Type::Struct { .. } => defs.struct_members(ty).unwrap_or_default().into_iter().map(|(_, t)| t).collect(),
                _ => defs
                    .variant_order(ty)
                    .iter()
                    .flat_map(|v| defs.payload(ty, v).unwrap_or_default())
                    .collect(),
            };
            let found = parts.iter().any(|p| reaches_pointer(defs, p, visiting));
            visiting.pop();
            found
        }
        _ => false,
    }
}

fn string_type() -> Type {
    Type::Primitive("string".to_string())
}

fn is_string(ty: &Type) -> bool {
    matches!(ty, Type::Primitive(name) if name == "string")
}

/// Whether the backend formats a value of type `ty` itself: a number or `bool`.
fn is_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Primitive(name) if name == "bool" || is_numeric_name(name))
}

fn unit(span: Span) -> IrExpr {
    IrExpr {
        kind: IrExprKind::Unit,
        ty: Type::Unit,
        span,
    }
}

fn text(s: &str, span: Span) -> IrExpr {
    IrExpr {
        kind: IrExprKind::Str(s.to_string()),
        ty: string_type(),
        span,
    }
}

fn var(name: &str, ty: &Type, span: Span) -> IrExpr {
    IrExpr {
        kind: IrExprKind::Var(name.to_string()),
        ty: ty.clone(),
        span,
    }
}

fn intrinsic(op: Intrinsic, args: Vec<IrExpr>, ty: Type, span: Span) -> IrExpr {
    IrExpr {
        kind: IrExprKind::Intrinsic { op, args },
        ty,
        span,
    }
}

fn concat(lhs: IrExpr, rhs: IrExpr) -> IrExpr {
    let span = lhs.span;
    IrExpr {
        kind: IrExprKind::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        ty: string_type(),
        span,
    }
}

/// `open` followed by `parts` separated by `, `, then `close`.
fn enclose(open: &str, parts: Vec<IrExpr>, close: &str, span: Span) -> IrExpr {
    let mut out = text(open, span);
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            out = concat(out, text(", ", span));
        }
        out = concat(out, part);
    }
    concat(out, text(close, span))
}

fn block(stmts: Vec<IrStmt>, result: IrExpr) -> IrExpr {
    let (ty, span) = (result.ty.clone(), result.span);
    IrExpr {
        kind: IrExprKind::Block {
            stmts,
            result: Box::new(result),
        },
        ty,
        span,
    }
}

fn let_stmt(name: &str, value: IrExpr, span: Span) -> IrStmt {
    IrStmt {
        kind: IrStmtKind::Var {
            name: name.to_string(),
            ty: value.ty.clone(),
            init: Some(value),
        },
        span,
    }
}

fn expr_stmt(expr: IrExpr) -> IrStmt {
    let span = expr.span;
    IrStmt {
        kind: IrStmtKind::Expr(expr),
        span,
    }
}

fn ret(value: IrExpr) -> IrStmt {
    let span = value.span;
    IrStmt {
        kind: IrStmtKind::Return(Some(value)),
        span,
    }
}

fn assign(target: IrExpr, value: IrExpr) -> IrStmt {
    let (ty, span) = (target.ty.clone(), target.span);
    expr_stmt(IrExpr {
        kind: IrExprKind::Assign {
            op: AssignOp::Assign,
            target: Box::new(target),
            value: Box::new(value),
        },
        ty,
        span,
    })
}

/// `if cond { then }`, with no `else`.
fn if_stmt(defs: &TypeDefs, cond: IrExpr, then: Vec<IrStmt>, span: Span) -> IrStmt {
    let patterns = bool_patterns(&cond.ty, span);
    let tree = compile_match(defs, &cond.ty, &patterns);
    IrStmt {
        kind: IrStmtKind::Match {
            scrutinee: cond,
            actions: vec![
                IrStmt {
                    kind: IrStmtKind::Block(then),
                    span,
                },
                IrStmt {
                    kind: IrStmtKind::Block(Vec::new()),
                    span,
                },
            ],
            tree,
        },
        span,
    }
}
