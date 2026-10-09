//! Statements and control flow: bindings, loops, `match`, and decision trees.

use super::*;

impl Emitter<'_, '_> {
    pub(super) fn stmts(&mut self, stmts: &[IrStmt]) -> Result<(), CodegenError> {
        stmts.iter().try_for_each(|s| self.stmt(s))
    }

    pub(super) fn stmt(&mut self, stmt: &IrStmt) -> Result<(), CodegenError> {
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
            IrStmtKind::ForEach {
                array,
                index,
                elem,
                body,
            } => self.for_each(array, index.as_ref(), elem, body, stmt.span)?,
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

    /// `for var in start..end` (or `..=`). Both bounds are evaluated once. A hidden
    /// counter drives the loop, and each pass binds `var` afresh to its value, so an
    /// assignment to `var` lasts only for that pass and a closure created in it keeps
    /// its own variable. An exclusive range tests before each pass; an inclusive one
    /// tests for the last value before stepping, so a range ending at the type's
    /// maximum never overflows its counter.
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
            let counter = e.slot(None, ty, span)?;
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
            let result = e.scoped(|e| {
                let i = e.load(&counter).expect("a counter has a value");
                let bound = e.bind(var, ty, span)?;
                e.store(&bound, Some(&i));
                e.stmts(body)
            });
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

    /// Evaluate `scrutinee` once and run the action its decision tree selects. An
    /// expression `match` of non-unit `result` type yields the selected action's value.
    pub(super) fn matching(
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

    /// `for elem in array` (or `for (index, elem) in array`): the array is evaluated
    /// once, and each pass binds the element (and its `i64` index).
    fn for_each(
        &mut self,
        array: &IrExpr,
        index: Option<&Binder>,
        elem: &Binder,
        body: &[IrStmt],
        span: Span,
    ) -> Result<(), CodegenError> {
        let array = self.expr(array)?.expect("an array has a value");
        let data = self.assign(&format!("extractvalue {ARRAY} {}, 0", array.operand));
        let length = self.assign(&format!("extractvalue {ARRAY} {}, 1", array.operand));
        let counter = self.slot(None, &Type::Primitive("u64".to_string()), span)?;
        self.store(&counter, Some(&Value { ty: "i64".to_string(), operand: "0".to_string() }));
        let head = self.fresh("head");
        let pass = self.fresh("body");
        let step = self.fresh("step");
        let exit = self.fresh("exit");
        self.start_block(&head);
        let position = self.load(&counter).expect("a counter has a value");
        let more = self.assign(&format!("icmp ult i64 {}, {length}", position.operand));
        self.terminate(&format!("br i1 {more}, label %{pass}, label %{exit}"));
        self.start_block(&pass);
        self.scoped(|e| -> Result<(), CodegenError> {
            let element_ty = e.value_type(&elem.ty, span)?;
            let element = match &element_ty {
                Some(llvm) => {
                    let slot = e.assign(&format!("getelementptr {llvm}, ptr {data}, i64 {}", position.operand));
                    let operand = e.assign(&format!("load {llvm}, ptr {slot}"));
                    Some(Value { ty: llvm.clone(), operand })
                }
                None => None,
            };
            let bound = e.bind(&elem.name, &elem.ty, span)?;
            e.store(&bound, element.as_ref());
            if let Some(index) = index {
                let bound = e.bind(&index.name, &index.ty, span)?;
                e.store(&bound, Some(&position));
            }
            e.loops.push(Loop {
                continue_to: step.clone(),
                break_to: exit.clone(),
            });
            let result = e.stmts(body);
            e.loops.pop();
            result
        })?;
        self.start_block(&step);
        let position = self.load(&counter).expect("a counter has a value");
        let next = self.assign(&format!("add nuw i64 {}, 1", position.operand));
        self.store(&counter, Some(&Value { ty: "i64".to_string(), operand: next }));
        self.terminate(&format!("br label %{head}"));
        self.start_block(&exit);
        Ok(())
    }
}
