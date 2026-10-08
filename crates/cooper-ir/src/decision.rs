//! Match compilation: patterns to decision trees, and the pattern helpers it uses.

use super::*;

/// The two boolean patterns an `if` desugars to: `true` (the `then` action) then
/// `false` (the `else` action), in that order.
pub(crate) fn bool_patterns(ty: &Type, span: Span) -> Vec<IrPattern> {
    vec![
        IrPattern {
            kind: IrPatternKind::Bool(true),
            ty: ty.clone(),
            span,
        },
        IrPattern {
            kind: IrPatternKind::Bool(false),
            ty: ty.clone(),
            span,
        },
    ]
}

/// A scrutinee position the matrix compiler discriminates on: where it is reached
/// from the scrutinee and the type of value found there.
#[derive(Clone)]
struct Occurrence {
    access: Access,
    ty: Type,
}

/// One pattern-matrix row: the patterns left to test (aligned with the current
/// occurrences), the bindings accumulated as columns were consumed, and the action
/// to run when the row matches.
#[derive(Clone)]
struct Row {
    columns: Vec<IrPattern>,
    bindings: Vec<MatchBinding>,
    action: usize,
}

/// Compile a match's arm patterns (in source order, the row index being the action
/// index) over a scrutinee of `ty` into a decision tree.
pub(crate) fn compile_match(defs: &TypeDefs, ty: &Type, patterns: &[IrPattern]) -> Decision {
    let occurrences = vec![Occurrence {
        access: Access::Root,
        ty: ty.clone(),
    }];
    let rows = patterns
        .iter()
        .enumerate()
        .map(|(action, pattern)| Row {
            columns: vec![pattern.clone()],
            bindings: Vec::new(),
            action,
        })
        .collect();
    compile(defs, occurrences, rows)
}

/// The matrix algorithm: the first row whose columns are all irrefutable wins;
/// otherwise test the leftmost column the top row cares about, deconstructing a
/// single-constructor type in place or branching on a `Switch`.
fn compile(defs: &TypeDefs, occurrences: Vec<Occurrence>, mut rows: Vec<Row>) -> Decision {
    let Some(first) = rows.first() else {
        return Decision::Fail;
    };
    if first.columns.iter().all(ir_is_irrefutable) {
        let mut row = rows.swap_remove(0);
        for (occ, pat) in occurrences.iter().zip(&row.columns) {
            collect_bindings(defs, pat, &occ.access, &occ.ty, &mut row.bindings);
        }
        return Decision::Leaf {
            bindings: row.bindings,
            action: row.action,
        };
    }
    let col = first
        .columns
        .iter()
        .position(|p| !ir_is_irrefutable(p))
        .expect("an all-irrefutable row was handled above");
    match &occurrences[col].ty {
        Type::Tuple(_) | Type::Struct { .. } => deconstruct(defs, col, occurrences, rows),
        _ => switch(defs, col, occurrences, rows),
    }
}

/// Expand a single-constructor (tuple or struct) column into its fields: the column
/// is replaced by one occurrence per field, and every row's pattern there by its
/// sub-patterns (missing struct fields and irrefutable patterns become wildcards).
fn deconstruct(defs: &TypeDefs, col: usize, occurrences: Vec<Occurrence>, rows: Vec<Row>) -> Decision {
    let subs = sub_fields(defs, &occurrences[col].access, &occurrences[col].ty);
    let mut new_occurrences = occurrences.clone();
    new_occurrences.splice(col..=col, subs.clone());
    let new_rows = rows
        .into_iter()
        .map(|mut row| {
            let pat = row.columns.remove(col);
            let subpats = deconstruct_pattern(defs, pat, &occurrences[col], &subs, &mut row.bindings);
            row.columns.splice(col..col, subpats);
            row
        })
        .collect();
    compile(defs, new_occurrences, new_rows)
}

/// The sub-patterns a tuple/struct column pattern contributes, aligned with `subs`.
/// A binding over the whole value is recorded before it is dropped to wildcards.
fn deconstruct_pattern(
    defs: &TypeDefs,
    pat: IrPattern,
    occ: &Occurrence,
    subs: &[Occurrence],
    bindings: &mut Vec<MatchBinding>,
) -> Vec<IrPattern> {
    let span = pat.span;
    match pat.kind {
        IrPatternKind::Wildcard => subs.iter().map(|s| ir_wildcard(s.ty.clone(), span)).collect(),
        IrPatternKind::Binding(name) => {
            bindings.push(MatchBinding {
                name,
                access: occ.access.clone(),
                ty: occ.ty.clone(),
            });
            subs.iter().map(|s| ir_wildcard(s.ty.clone(), span)).collect()
        }
        IrPatternKind::Tuple(ps) => ps,
        IrPatternKind::Struct { fields, .. } => {
            declared_members(defs, &occ.ty)
                .into_iter()
                .map(|(name, ty)| {
                    fields
                        .iter()
                        .find(|(f, _)| *f == name)
                        .map(|(_, p)| p.clone())
                        .unwrap_or_else(|| ir_wildcard(ty, span))
                })
                .collect()
        }
        _ => unreachable!("deconstruct handles only tuple/struct columns"),
    }
}

/// Branch on a column of a switchable type (`bool`, integer, or sum type): a case
/// per head constructor, plus a default from the irrefutable rows when the cases do
/// not exhaust the type.
fn switch(defs: &TypeDefs, col: usize, occurrences: Vec<Occurrence>, rows: Vec<Row>) -> Decision {
    let access = occurrences[col].access.clone();
    let ty = occurrences[col].ty.clone();
    let tests = head_tests(&rows, col);
    let mut cases = Vec::new();
    for test in &tests {
        let subs = test_sub_occurrences(defs, &access, &ty, test);
        let mut case_occurrences = occurrences.clone();
        case_occurrences.splice(col..=col, subs.clone());
        let case_rows = rows
            .iter()
            .filter_map(|row| specialize_row(row, col, test, &access, &ty, &subs))
            .collect();
        cases.push(Case {
            test: test.clone(),
            tree: compile(defs, case_occurrences, case_rows),
        });
    }
    let default = if tests_exhaustive(defs, &ty, &tests) {
        None
    } else {
        let mut default_occurrences = occurrences.clone();
        default_occurrences.remove(col);
        let default_rows = rows
            .iter()
            .filter(|row| ir_is_irrefutable(&row.columns[col]))
            .map(|row| default_row(row, col, &access, &ty))
            .collect();
        Some(Box::new(compile(defs, default_occurrences, default_rows)))
    };
    Decision::Switch {
        access,
        ty,
        cases,
        default,
    }
}

/// Specialize a row against the case constructor `test`: a matching constructor
/// row contributes its sub-patterns; an irrefutable row falls through as wildcards
/// (recording any binding); any other constructor drops out.
fn specialize_row(
    row: &Row,
    col: usize,
    test: &Test,
    access: &Access,
    ty: &Type,
    subs: &[Occurrence],
) -> Option<Row> {
    let span = row.columns[col].span;
    if ir_is_irrefutable(&row.columns[col]) {
        let mut r = row.clone();
        let removed = r.columns.remove(col);
        if let IrPatternKind::Binding(name) = removed.kind {
            r.bindings.push(MatchBinding {
                name,
                access: access.clone(),
                ty: ty.clone(),
            });
        }
        let wilds: Vec<_> = subs.iter().map(|s| ir_wildcard(s.ty.clone(), span)).collect();
        r.columns.splice(col..col, wilds);
        return Some(r);
    }
    if pattern_test(&row.columns[col]).as_ref() != Some(test) {
        return None;
    }
    let mut r = row.clone();
    let removed = r.columns.remove(col);
    let subpats = match removed.kind {
        IrPatternKind::Variant { binders, .. } => binders
            .into_iter()
            .map(|b| {
                let kind = if b.name == "_" {
                    IrPatternKind::Wildcard
                } else {
                    IrPatternKind::Binding(b.name)
                };
                IrPattern {
                    kind,
                    ty: b.ty,
                    span,
                }
            })
            .collect(),
        IrPatternKind::Bool(_) | IrPatternKind::Int { .. } => Vec::new(),
        _ => unreachable!("a refutable switch column is a variant or literal"),
    };
    r.columns.splice(col..col, subpats);
    Some(r)
}

/// A default-branch row: the tested (irrefutable) column is dropped, recording any
/// whole-value binding it introduced.
fn default_row(row: &Row, col: usize, access: &Access, ty: &Type) -> Row {
    let mut r = row.clone();
    let removed = r.columns.remove(col);
    if let IrPatternKind::Binding(name) = removed.kind {
        r.bindings.push(MatchBinding {
            name,
            access: access.clone(),
            ty: ty.clone(),
        });
    }
    r
}

/// The head constructors appearing in column `col`, in order of first appearance.
fn head_tests(rows: &[Row], col: usize) -> Vec<Test> {
    let mut tests = Vec::new();
    for row in rows {
        if let Some(test) = pattern_test(&row.columns[col]) {
            if !tests.contains(&test) {
                tests.push(test);
            }
        }
    }
    tests
}

/// The constructor a pattern tests, or `None` for an irrefutable pattern.
fn pattern_test(pat: &IrPattern) -> Option<Test> {
    match &pat.kind {
        IrPatternKind::Variant { variant, .. } => Some(Test::Variant(variant.clone())),
        IrPatternKind::Bool(b) => Some(Test::Bool(*b)),
        IrPatternKind::Int { value, negative } => Some(Test::Int {
            value: *value,
            negative: *negative,
        }),
        _ => None,
    }
}

/// The occurrences a matched constructor introduces: a variant exposes its payload
/// slots; `bool` and integer constructors expose nothing.
fn test_sub_occurrences(defs: &TypeDefs, access: &Access, ty: &Type, test: &Test) -> Vec<Occurrence> {
    match test {
        Test::Variant(variant) => {
            let payload = defs
                .payload(ty, variant)
                .expect("a variant test names a variant of its sum type");
            payload
                .iter()
                .enumerate()
                .map(|(index, slot)| Occurrence {
                    access: Access::Payload {
                        parent: Box::new(access.clone()),
                        variant: variant.clone(),
                        index,
                    },
                    ty: slot.clone(),
                })
                .collect()
        }
        Test::Bool(_) | Test::Int { .. } => Vec::new(),
    }
}

/// Whether the tested constructors cover the type: both booleans, or every variant
/// of a sum type. Integers are never exhausted by literals, so they always need a
/// default (the checker requires an irrefutable catch-all there).
fn tests_exhaustive(defs: &TypeDefs, ty: &Type, tests: &[Test]) -> bool {
    match ty {
        Type::Primitive(name) if name == "bool" => {
            tests.contains(&Test::Bool(true)) && tests.contains(&Test::Bool(false))
        }
        Type::Oneof { .. } => defs
            .variant_order(ty)
            .iter()
            .all(|v| tests.contains(&Test::Variant(v.clone()))),
        _ => false,
    }
}

/// The occurrences a tuple/struct value exposes, in a deterministic order (tuple
/// position, struct fields in declaration order).
fn sub_fields(defs: &TypeDefs, access: &Access, ty: &Type) -> Vec<Occurrence> {
    match ty {
        Type::Tuple(elems) => elems
            .iter()
            .enumerate()
            .map(|(index, elem)| Occurrence {
                access: Access::Elem {
                    parent: Box::new(access.clone()),
                    index,
                },
                ty: elem.clone(),
            })
            .collect(),
        Type::Struct { .. } => declared_members(defs, ty)
            .into_iter()
            .map(|(name, ty)| Occurrence {
                access: Access::Field {
                    parent: Box::new(access.clone()),
                    name,
                },
                ty,
            })
            .collect(),
        _ => unreachable!("sub_fields runs on tuple/struct occurrences only"),
    }
}

/// A struct occurrence's members as `(name, type)` pairs in declaration order, which
/// is also the struct's layout order.
fn declared_members(defs: &TypeDefs, ty: &Type) -> Vec<(String, Type)> {
    defs.struct_members(ty)
        .expect("a struct occurrence names a defined struct")
}

/// Whether a pattern matches every value of its type (so it never forces a test).
fn ir_is_irrefutable(pat: &IrPattern) -> bool {
    match &pat.kind {
        IrPatternKind::Wildcard | IrPatternKind::Binding(_) => true,
        IrPatternKind::Tuple(ps) => ps.iter().all(ir_is_irrefutable),
        IrPatternKind::Struct { fields, .. } => fields.iter().all(|(_, p)| ir_is_irrefutable(p)),
        _ => false,
    }
}

/// Collect the names an irrefutable pattern binds, each with the access and type of
/// the subvalue it reaches, descending through tuple and struct patterns.
fn collect_bindings(defs: &TypeDefs, pat: &IrPattern, access: &Access, ty: &Type, out: &mut Vec<MatchBinding>) {
    match &pat.kind {
        IrPatternKind::Wildcard => {}
        IrPatternKind::Binding(name) => out.push(MatchBinding {
            name: name.clone(),
            access: access.clone(),
            ty: ty.clone(),
        }),
        IrPatternKind::Tuple(ps) => {
            let Type::Tuple(elems) = ty else {
                unreachable!("a tuple pattern matches a tuple type");
            };
            for (index, (sub, elem_ty)) in ps.iter().zip(elems).enumerate() {
                collect_bindings(
                    defs,
                    sub,
                    &Access::Elem {
                        parent: Box::new(access.clone()),
                        index,
                    },
                    elem_ty,
                    out,
                );
            }
        }
        IrPatternKind::Struct { fields, .. } => {
            for (name, sub) in fields {
                let member_ty = &defs
                    .member(ty, name)
                    .expect("a checked struct pattern names existing fields");
                collect_bindings(
                    defs,
                    sub,
                    &Access::Field {
                        parent: Box::new(access.clone()),
                        name: name.clone(),
                    },
                    member_ty,
                    out,
                );
            }
        }
        _ => unreachable!("collect_bindings runs on irrefutable patterns only"),
    }
}

/// A wildcard pattern of a given type and span.
fn ir_wildcard(ty: Type, span: Span) -> IrPattern {
    IrPattern {
        kind: IrPatternKind::Wildcard,
        ty,
        span,
    }
}
