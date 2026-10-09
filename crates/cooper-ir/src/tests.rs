use super::*;
use cooper_frontend::resolve::resolve_into;
use cooper_frontend::{lexer, parser, typecheck};

/// Run the frontend over a snippet and return its declarations, the resolved
/// globals, and the tables the checker hands off.
fn check(source: &str) -> (Vec<Stmt>, Globals, Typed) {
    let tokens = lexer::tokenize(source).expect("snippet lexes");
    let parsed = parser::parse(tokens);
    assert!(parsed.errors.is_empty(), "snippet parses: {:?}", parsed.errors);
    let (globals, diags) = resolve_into(Globals::default(), "main", &parsed.decls);
    assert!(diags.is_empty(), "snippet resolves: {diags:?}");
    let checked = typecheck::check(&parsed.decls, &globals, &Default::default());
    assert!(checked.diags.is_empty(), "snippet checks: {:?}", checked.diags);
    (parsed.decls, globals, checked)
}

/// The expression returned by the sole function's trailing `return`.
fn return_expr(decls: &[Stmt]) -> &Expr {
    let StmtKind::FuncDecl(func) = &decls[0].kind else {
        panic!("expected a function declaration");
    };
    for stmt in &func.body {
        if let StmtKind::Return(Some(expr)) = &stmt.kind {
            return expr;
        }
    }
    panic!("expected a return statement");
}

fn is_primitive(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Primitive(n) if n == name)
}

/// The switch case testing a given sum-type variant.
fn case_for<'a>(cases: &'a [Case], variant: &str) -> &'a Case {
    cases
        .iter()
        .find(|c| matches!(&c.test, Test::Variant(v) if v == variant))
        .expect("a case for the variant")
}

#[test]
fn lowers_a_typed_arithmetic_expression() {
    let (decls, globals, checked) = check("func add(a: i32, b: i32): i32 { return a + b }");
    let ir = lower_expr(return_expr(&decls), &globals, &checked).expect("lowers");

    assert!(is_primitive(&ir.ty, "i32"), "result type carried: {:?}", ir.ty);
    let IrExprKind::Binary { op, lhs, rhs } = &ir.kind else {
        panic!("expected a binary node, got {:?}", ir.kind);
    };
    assert_eq!(*op, BinaryOp::Add);
    assert!(matches!(&lhs.kind, IrExprKind::Var(n) if n == "a"));
    assert!(is_primitive(&lhs.ty, "i32"));
    assert!(matches!(&rhs.kind, IrExprKind::Var(n) if n == "b"));
    assert!(is_primitive(&rhs.ty, "i32"));
}

#[test]
fn grouping_is_collapsed_and_literals_keep_their_inferred_width() {
    let (decls, globals, checked) = check("func f(x: i64): i64 { return (x + 1) }");
    let ir = lower_expr(return_expr(&decls), &globals, &checked).expect("lowers");

    // The parenthesised group is gone; the binary is the root.
    let IrExprKind::Binary { rhs, .. } = &ir.kind else {
        panic!("expected a binary node, got {:?}", ir.kind);
    };
    // The bare literal `1` adopted the i64 width from `x`.
    assert!(matches!(&rhs.kind, IrExprKind::Int(1)));
    assert!(is_primitive(&rhs.ty, "i64"), "literal width: {:?}", rhs.ty);
}

#[test]
fn lowers_a_function_signature_and_a_conditional_body() {
    let (decls, globals, checked) = check(
        "func max(a: i32, b: i32): i32 {\n\
         \tif a > b then { return a } else { return b }\n\
         \treturn a\n\
         }",
    );
    let functions = lower_module("main", &decls, &globals, &checked).expect("module lowers");

    assert_eq!(functions.len(), 1);
    let max = &functions[0];
    assert!(matches!(&max.item, Callable::Func { name, .. } if name == "max"));
    assert!(max.receiver.is_none());
    assert_eq!(max.params.len(), 2);
    assert_eq!(max.params[0].name, "a");
    assert!(is_primitive(&max.params[0].ty, "i32"));
    assert!(is_primitive(&max.return_type, "i32"));

    // The body is an `if` statement (desugared to a boolean `match`) followed
    // by a trailing return. The match tests the condition and selects between a
    // `then` action and an `else` action.
    let IrStmtKind::Match {
        scrutinee,
        actions,
        tree,
    } = &max.body[0].kind
    else {
        panic!("expected a match, got {:?}", max.body[0].kind);
    };
    assert!(is_primitive(&scrutinee.ty, "bool"));
    assert_eq!(actions.len(), 2);
    assert!(matches!(actions[0].kind, IrStmtKind::Block(_)));
    assert!(matches!(actions[1].kind, IrStmtKind::Block(_)));
    // The tree switches on the boolean and is exhaustive (no default).
    let Decision::Switch { cases, default, .. } = tree else {
        panic!("expected a switch, got {tree:?}");
    };
    assert_eq!(cases.len(), 2);
    assert!(default.is_none());
    assert!(matches!(max.body[1].kind, IrStmtKind::Return(Some(_))));
}

#[test]
fn lowers_a_local_binding_taking_its_type_from_the_initializer() {
    let (decls, globals, checked) =
        check("func f(n: i32): i32 { x := n + 1\n\treturn x }");
    let functions = lower_module("main", &decls, &globals, &checked).expect("module lowers");

    // `x := n + 1` lowers to an expression statement holding the walrus binding,
    // carrying the i32 type flowing from the initializer.
    let IrStmtKind::Expr(expr) = &functions[0].body[0].kind else {
        panic!("expected an expression statement, got {:?}", functions[0].body[0].kind);
    };
    let IrExprKind::Let { name, value } = &expr.kind else {
        panic!("expected a walrus binding, got {:?}", expr.kind);
    };
    assert_eq!(name, "x");
    assert!(is_primitive(&value.ty, "i32"));
}

#[test]
fn numeric_literals_are_decoded_to_their_values() {
    // A hexadecimal integer with separators and a float both decode exactly.
    let (decls, globals, checked) =
        check("func f(): u16 { return 0xFF_FF }");
    let ir = lower_expr(return_expr(&decls), &globals, &checked).expect("lowers");
    assert!(matches!(ir.kind, IrExprKind::Int(0xFFFF)));
    assert!(is_primitive(&ir.ty, "u16"));

    let (decls, globals, checked) = check("func g(): f64 { return 1.5 }");
    let ir = lower_expr(return_expr(&decls), &globals, &checked).expect("lowers");
    let IrExprKind::Float(v) = ir.kind else {
        panic!("expected a float literal, got {:?}", ir.kind);
    };
    assert_eq!(v, 1.5);
    assert!(is_primitive(&ir.ty, "f64"));
}

#[test]
fn lowers_a_struct_literal_with_its_members_in_source_order() {
    let (decls, globals, checked) = check(
        "struct Point { x: i32, y: i32 }\n\
         func origin(): Point { return Point { x: 1, y: 2 } }",
    );
    // The function whose body returns the literal is the second declaration.
    let StmtKind::FuncDecl(func) = &decls[1].kind else {
        panic!("expected the origin function");
    };
    let StmtKind::Return(Some(expr)) = &func.body[0].kind else {
        panic!("expected a return");
    };
    let ir = lower_expr(expr, &globals, &checked).expect("lowers");

    let IrExprKind::StructLiteral { name, members } = &ir.kind else {
        panic!("expected a struct literal, got {:?}", ir.kind);
    };
    assert_eq!(name, "Point");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].0, "x");
    assert!(is_primitive(&members[0].1.ty, "i32"));
    assert_eq!(members[1].0, "y");
}

#[test]
fn lowers_a_match_expression_binding_variant_payloads() {
    let (decls, globals, checked) = check(
        "oneof Shape { Circle(i32), Rect(i32, i32) }\n\
         func area(s: Shape): i32 {\n\
         \ta := match s with {\n\
         \t\tShape.Circle(r) => { r }\n\
         \t\tShape.Rect(w, h) => { w }\n\
         \t}\n\
         \treturn a\n\
         }",
    );
    // The match is the value of the `a := …` binding in the second declaration.
    let StmtKind::FuncDecl(func) = &decls[1].kind else {
        panic!("expected the area function");
    };
    let StmtKind::Expression(Expr { kind: ExprKind::Let { value, .. }, .. }) =
        &func.body[0].kind
    else {
        panic!("expected a walrus binding");
    };
    let ir = lower_expr(value, &globals, &checked).expect("lowers");

    let IrExprKind::Match {
        scrutinee,
        actions,
        tree,
    } = &ir.kind
    else {
        panic!("expected a match, got {:?}", ir.kind);
    };
    assert!(matches!(&scrutinee.kind, IrExprKind::Var(n) if n == "s"));
    assert!(is_primitive(&ir.ty, "i32"), "match result type: {:?}", ir.ty);
    assert_eq!(actions.len(), 2);

    // The tree switches on the sum type, exhaustively (no default), with a case
    // per variant whose leaf binds its payload slots by access.
    let Decision::Switch { cases, default, .. } = tree.as_ref() else {
        panic!("expected a switch, got {tree:?}");
    };
    assert!(default.is_none(), "all variants covered");
    assert_eq!(cases.len(), 2);

    let circle = case_for(cases, "Circle");
    let Decision::Leaf { bindings, action } = &circle.tree else {
        panic!("expected a leaf");
    };
    assert_eq!(*action, 0);
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].name, "r");
    assert!(is_primitive(&bindings[0].ty, "i32"));
    assert!(matches!(
        &bindings[0].access,
        Access::Payload { variant, index: 0, .. } if variant == "Circle"
    ));

    let rect = case_for(cases, "Rect");
    let Decision::Leaf { bindings, action } = &rect.tree else {
        panic!("expected a leaf");
    };
    assert_eq!(*action, 1);
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[1].name, "h");
    assert!(matches!(
        &bindings[1].access,
        Access::Payload { variant, index: 1, .. } if variant == "Rect"
    ));
}

#[test]
fn generic_variant_binders_lower_with_instantiated_slot_types() {
    // A scrutinee of an instantiated generic sum type binds payload slots at
    // their substituted types, not the sum type's parameters.
    let (decls, globals, checked) = check(
        "oneof Maybe T { None, Some(T) }\n\
         func unwrap(m: Maybe i32): i32 {\n\
         \ta := match m with {\n\
         \t\tMaybe.Some(x) => { x }\n\
         \t\tMaybe.None => { 0 }\n\
         \t}\n\
         \treturn a\n\
         }",
    );
    let StmtKind::FuncDecl(func) = &decls[1].kind else {
        panic!("expected the unwrap function");
    };
    let StmtKind::Expression(Expr { kind: ExprKind::Let { value, .. }, .. }) =
        &func.body[0].kind
    else {
        panic!("expected a walrus binding");
    };
    let ir = lower_expr(value, &globals, &checked).expect("lowers");
    let IrExprKind::Match { tree, .. } = &ir.kind else {
        panic!("expected a match");
    };
    let Decision::Switch { cases, .. } = tree.as_ref() else {
        panic!("expected a switch");
    };
    let some = case_for(cases, "Some");
    let Decision::Leaf { bindings, .. } = &some.tree else {
        panic!("expected a leaf");
    };
    assert_eq!(bindings[0].name, "x");
    assert!(
        is_primitive(&bindings[0].ty, "i32"),
        "payload slot instantiated to i32, got {:?}",
        bindings[0].ty
    );
}

#[test]
fn lowers_a_tuple_match_into_a_nested_decision_tree() {
    // A tuple scrutinee is deconstructed into its components; a literal in the
    // first position becomes a switch on that component, and the irrefutable
    // catch-all supplies the default (binding both components by access).
    let (decls, globals, checked) = check(
        "func classify(p: (i32, i32)): i32 {\n\
         \treturn match p with {\n\
         \t\t(0, y) => { y }\n\
         \t\t(x, y) => { x }\n\
         \t}\n\
         }",
    );
    let ir = lower_expr(return_expr(&decls), &globals, &checked).expect("lowers");
    let IrExprKind::Match { actions, tree, .. } = &ir.kind else {
        panic!("expected a match, got {:?}", ir.kind);
    };
    assert_eq!(actions.len(), 2);

    // The root tuple is deconstructed: the switch tests the first component.
    let Decision::Switch {
        access,
        ty,
        cases,
        default,
    } = tree.as_ref()
    else {
        panic!("expected a switch, got {tree:?}");
    };
    assert!(matches!(access, Access::Elem { index: 0, .. }));
    assert!(is_primitive(ty, "i32"));
    assert_eq!(cases.len(), 1);
    assert_eq!(
        cases[0].test,
        Test::Int {
            value: 0,
            negative: false
        }
    );

    // Matching `0` leaves the first arm, binding `y` to the second component.
    let Decision::Leaf { bindings, action } = &cases[0].tree else {
        panic!("expected a leaf under the matched literal");
    };
    assert_eq!(*action, 0);
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].name, "y");
    assert!(matches!(&bindings[0].access, Access::Elem { index: 1, .. }));

    // Any other first component falls through to the catch-all, binding both.
    let Some(default) = default else {
        panic!("an integer switch needs a default");
    };
    let Decision::Leaf { bindings, action } = default.as_ref() else {
        panic!("expected a leaf default");
    };
    assert_eq!(*action, 1);
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0].name, "x");
    assert!(matches!(&bindings[0].access, Access::Elem { index: 0, .. }));
    assert_eq!(bindings[1].name, "y");
}

#[test]
fn lowers_variant_construction_and_payload_free_access() {
    let (decls, globals, checked) = check(
        "oneof Maybe T { None, Some(T) }\n\
         func some(): Maybe i32 { return Maybe.Some(5) }\n\
         func none(): Maybe i32 { return Maybe.None }",
    );
    let variant_return = |idx: usize| {
        let StmtKind::FuncDecl(func) = &decls[idx].kind else {
            panic!("expected a function at {idx}");
        };
        let StmtKind::Return(Some(expr)) = &func.body[0].kind else {
            panic!("expected a return");
        };
        lower_expr(expr, &globals, &checked).expect("lowers")
    };

    // `Maybe.Some(5)` is a payloaded construction carrying the instantiated type.
    let some = variant_return(1);
    let IrExprKind::Variant { variant, args } = &some.kind else {
        panic!("expected a variant, got {:?}", some.kind);
    };
    assert_eq!(variant, "Some");
    assert_eq!(args.len(), 1);
    assert!(is_primitive(&args[0].ty, "i32"));
    assert!(matches!(&some.ty, Type::Oneof { id, .. } if id.name == "Maybe"));

    // `Maybe.None` is a payload-free variant value, not a field access.
    let none = variant_return(2);
    let IrExprKind::Variant { variant, args } = &none.kind else {
        panic!("expected a variant, got {:?}", none.kind);
    };
    assert_eq!(variant, "None");
    assert!(args.is_empty());
}

#[test]
fn numeric_conversions_lower_to_a_convert_node() {
    let (decls, globals, checked) = check("func widen(x: i32): i64 { return i64(x) }");
    let ir = lower_expr(return_expr(&decls), &globals, &checked).expect("lowers");
    let IrExprKind::Convert(value) = &ir.kind else {
        panic!("expected a conversion, got {:?}", ir.kind);
    };
    assert!(is_primitive(&ir.ty, "i64"), "destination type: {:?}", ir.ty);
    assert!(is_primitive(&value.ty, "i32"), "source type: {:?}", value.ty);
    assert!(matches!(&value.kind, IrExprKind::Var(n) if n == "x"));
}

#[test]
fn lowers_array_iteration_to_a_foreach() {
    let (decls, globals, checked) = check(
        "func sum(xs: i32[]): i32 {\n\
         \ttotal := 0\n\
         \tfor x in xs do { total += x }\n\
         \treturn total\n\
         }",
    );
    let functions = lower_module("main", &decls, &globals, &checked).expect("module lowers");
    let IrStmtKind::ForEach {
        array, index, elem, body,
    } = &functions[0].body[1].kind
    else {
        panic!("expected an array loop, got {:?}", functions[0].body[1].kind);
    };
    assert!(matches!(&array.kind, IrExprKind::Var(n) if n == "xs"));
    assert!(index.is_none(), "single binding has no index");
    assert_eq!(elem.name, "x");
    assert!(is_primitive(&elem.ty, "i32"), "element type: {:?}", elem.ty);
    assert_eq!(body.len(), 1);
}

#[test]
fn lowers_indexed_array_iteration_with_an_index_binder() {
    let (decls, globals, checked) = check(
        "func sum(xs: i32[]): i32 {\n\
         \ttotal := 0\n\
         \tfor (i, x) in xs do { total += x }\n\
         \treturn total\n\
         }",
    );
    let functions = lower_module("main", &decls, &globals, &checked).expect("module lowers");
    let IrStmtKind::ForEach { index, elem, .. } = &functions[0].body[1].kind else {
        panic!("expected an array loop, got {:?}", functions[0].body[1].kind);
    };
    let index = index.as_ref().expect("an index binder");
    assert_eq!(index.name, "i");
    assert!(is_primitive(&index.ty, INDEX_INT), "index type: {:?}", index.ty);
    assert_eq!(elem.name, "x");
}

#[test]
fn lowers_array_index_read_to_an_intrinsic() {
    let (decls, globals, checked) = check("func first(xs: i32[]): i32 { return xs[0] }");
    let ir = lower_expr(return_expr(&decls), &globals, &checked).expect("lowers");
    let IrExprKind::Intrinsic { op, args } = &ir.kind else {
        panic!("expected an intrinsic, got {:?}", ir.kind);
    };
    assert_eq!(*op, Intrinsic::ArrayGet);
    assert_eq!(args.len(), 2);
    assert!(matches!(&args[0].kind, IrExprKind::Var(n) if n == "xs"));
}

#[test]
fn lowers_array_length_method_to_an_intrinsic() {
    let (decls, globals, checked) = check("func size(xs: i32[]): i64 { return xs.length() }");
    let ir = lower_expr(return_expr(&decls), &globals, &checked).expect("lowers");
    let IrExprKind::Intrinsic { op, args } = &ir.kind else {
        panic!("expected an intrinsic, got {:?}", ir.kind);
    };
    assert_eq!(*op, Intrinsic::ArrayLength);
    assert_eq!(args.len(), 1);
    assert!(matches!(&args[0].kind, IrExprKind::Var(n) if n == "xs"));
}

#[test]
fn lowers_a_range_for_loop_with_a_typed_counter() {
    let (decls, globals, checked) = check(
        "func count(): i32 {\n\
         \ttotal := 0\n\
         \tfor i in 0..10 do { total += i }\n\
         \treturn total\n\
         }",
    );
    let functions = lower_module("main", &decls, &globals, &checked).expect("module lowers");
    let IrStmtKind::ForRange {
        var, ty, inclusive, body, ..
    } = &functions[0].body[1].kind
    else {
        panic!("expected a range loop, got {:?}", functions[0].body[1].kind);
    };
    assert_eq!(var, "i");
    assert!(is_primitive(ty, "i32"), "counter type: {ty:?}");
    assert!(!inclusive);
    assert_eq!(body.len(), 1);
}

#[test]
fn module_qualified_reference_names_the_declaration() {
    // A dependency exports a function; the main module reaches it qualified as
    // `dep.helper(..)`. The module prefix carries no runtime value: the callee lowers
    // to a reference naming the function by its defining module and name.
    let dep_tokens = lexer::tokenize("func helper(): i32 { return 1 }").expect("dep lexes");
    let dep = parser::parse(dep_tokens);
    assert!(dep.errors.is_empty(), "dep parses: {:?}", dep.errors);
    let (dep_globals, dep_diags) = resolve_into(Globals::default(), "dep", &dep.decls);
    assert!(dep_diags.is_empty(), "dep resolves: {dep_diags:?}");

    let main_tokens =
        lexer::tokenize("func main(): i32 { return dep.helper() }").expect("main lexes");
    let main = parser::parse(main_tokens);
    assert!(main.errors.is_empty(), "main parses: {:?}", main.errors);
    let (globals, diags) = resolve_into(Globals::default(), "main", &main.decls);
    assert!(diags.is_empty(), "main resolves: {diags:?}");

    let modules = HashMap::from([(vec!["dep".to_string()], dep_globals)]);
    let checked = typecheck::check(&main.decls, &globals, &modules);
    assert!(checked.diags.is_empty(), "main checks: {:?}", checked.diags);

    let ir = lower_expr(return_expr(&main.decls), &globals, &checked).expect("lowers");
    let IrExprKind::Call { callee, args } = &ir.kind else {
        panic!("expected a call, got {:?}", ir.kind);
    };
    assert!(args.is_empty());
    assert!(
        matches!(
            &callee.kind,
            IrExprKind::FuncRef { item: Callable::Func { module, name }, type_args }
                if module == "dep" && name == "helper" && type_args.is_empty()
        ),
        "callee names dep.helper, got {:?}",
        callee.kind
    );
}

#[test]
fn lowers_tuple_destructuring_into_typed_bindings() {
    let (decls, globals, checked) = check(
        "func f(): i32 {\n\
         \tp := (1, 2)\n\
         \t(a, b) := p\n\
         \treturn a + b\n\
         }",
    );
    let StmtKind::FuncDecl(func) = &decls[0].kind else {
        panic!("expected a function");
    };
    let StmtKind::Expression(expr) = &func.body[1].kind else {
        panic!("expected the destructuring statement");
    };
    let ir = lower_expr(expr, &globals, &checked).expect("lowers");
    let IrExprKind::LetTuple { bindings, value } = &ir.kind else {
        panic!("expected a tuple destructuring, got {:?}", ir.kind);
    };
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0].name, "a");
    assert!(is_primitive(&bindings[0].ty, "i32"));
    assert_eq!(bindings[1].name, "b");
    assert!(matches!(&value.kind, IrExprKind::Var(n) if n == "p"));
}

/// Check, lower, and monomorphize a one-file program.
fn monomorphized(source: &str) -> Vec<Function> {
    let (decls, globals, checked) = check(source);
    let functions = lower_module("main", &decls, &globals, &checked).expect("program lowers");
    monomorphize(&functions).expect("program monomorphizes")
}

/// The instances of the function or method named `name`, by type arguments.
fn instances_of<'f>(functions: &'f [Function], name: &str) -> Vec<&'f Function> {
    functions
        .iter()
        .filter(|f| match &f.item {
            Callable::Func { name: n, .. } | Callable::Method { name: n, .. } => n == name,
            Callable::Closure { .. } | Callable::Extern { .. } | Callable::Formatter { .. } => false,
        })
        .collect()
}

/// Whether `ty` mentions a type parameter anywhere.
fn mentions_type_param(ty: &Type) -> bool {
    match ty {
        Type::TypeParam(_) => true,
        Type::Array(e) | Type::Pointer(e) => mentions_type_param(e),
        Type::Tuple(es) => es.iter().any(mentions_type_param),
        Type::Func {
            return_type,
            param_types,
        } => mentions_type_param(return_type) || param_types.iter().any(mentions_type_param),
        Type::Struct { type_args, .. } | Type::Oneof { type_args, .. } => {
            type_args.iter().any(mentions_type_param)
        }
        _ => false,
    }
}

#[test]
fn a_generic_call_lowers_to_a_reference_carrying_its_type_arguments() {
    let (decls, globals, checked) = check(
        "func id T (x: T): T { return x }\nfunc f(): i64 { return id(3) }",
    );
    let functions = lower_module("main", &decls, &globals, &checked).expect("module lowers");
    let f = &functions[1];
    let IrStmtKind::Return(Some(ret)) = &f.body[0].kind else {
        panic!("expected a return");
    };
    let IrExprKind::Call { callee, .. } = &ret.kind else {
        panic!("expected a call, got {:?}", ret.kind);
    };
    let IrExprKind::FuncRef { type_args, .. } = &callee.kind else {
        panic!("expected a function reference, got {:?}", callee.kind);
    };
    assert!(is_primitive(&type_args[0], "i64"), "inferred from the return type");
    assert!(!mentions_type_param(&callee.ty), "callee type is solved: {}", callee.ty);
}

#[test]
fn monomorphization_instantiates_each_distinct_use_once() {
    let functions = monomorphized(concat!(
        "func id T (x: T): T { return x }\n",
        "func f(): i32 { return id(id(1)) }\n",
        "func g(): string { return id(\"s\") }\n",
        "func unused T (x: T): T { return x }\n",
    ));
    let ids = instances_of(&functions, "id");
    assert_eq!(ids.len(), 2, "one instance per type argument, shared between calls");
    let args: Vec<String> = ids.iter().map(|f| f.type_args[0].to_string()).collect();
    assert_eq!(args, ["i32", "string"]);
    assert!(instances_of(&functions, "unused").is_empty(), "unreached templates are dropped");
    for id in ids {
        assert!(!mentions_type_param(&id.params[0].ty));
        assert!(!mentions_type_param(&id.return_type));
    }
}

#[test]
fn monomorphization_follows_generic_calls_through_generic_bodies() {
    // `twice` reaches `id` only through its own type parameter; instantiating `twice`
    // at `bool` makes that reference concrete.
    let functions = monomorphized(concat!(
        "func id T (x: T): T { return x }\n",
        "func twice U (x: U): U { return id(id(x)) }\n",
        "func f(): bool { return twice(true) }\n",
    ));
    let twice = instances_of(&functions, "twice");
    assert_eq!(twice.len(), 1);
    assert!(is_primitive(&twice[0].type_args[0], "bool"));
    let ids = instances_of(&functions, "id");
    assert_eq!(ids.len(), 1);
    assert!(is_primitive(&ids[0].type_args[0], "bool"));
}

#[test]
fn monomorphization_instantiates_methods_of_generic_structs() {
    let functions = monomorphized(concat!(
        "struct Box T { v: T }\n",
        "(b: Box T) func get(): T { return b.v }\n",
        "(b: Box T) func map U (f: func(T): U): Box U { return Box{v: f(b.v)} }\n",
        "func len(s: string): i32 { return 1 }\n",
        "func f(b: Box string): i32 { return b.map(len).get() }\n",
    ));
    let map = instances_of(&functions, "map");
    assert_eq!(map.len(), 1);
    // `map`'s own binder comes first, then the receiver's: U = i32, T = string.
    let args: Vec<String> = map[0].type_args.iter().map(Type::to_string).collect();
    assert_eq!(args, ["i32", "string"]);
    let get = instances_of(&functions, "get");
    assert_eq!(get.len(), 1);
    assert!(is_primitive(&get[0].type_args[0], "i32"));
    let receiver = get[0].receiver.as_ref().expect("a method has a receiver");
    assert_eq!(receiver.ty.to_string(), "Box i32");
}

#[test]
fn polymorphic_recursion_is_reported_as_unbounded() {
    let (decls, globals, checked) = check(concat!(
        "func grow T (x: T): i32 { return grow((x, x)) }\n",
        "func f(): i32 { return grow(1) }\n",
    ));
    let functions = lower_module("main", &decls, &globals, &checked).expect("module lowers");
    let result = monomorphize(&functions);
    assert!(
        matches!(&result, Err(MonoError::UnboundedInstantiation { item: Callable::Func { name, .. }, .. }) if name == "grow"),
        "got {result:?}"
    );
}

#[test]
fn a_negative_literal_lowers_to_a_typed_negation() {
    // The checker folds the sign into the literal for range checking; the literal
    // beneath still carries the type, so lowering finds it.
    let (decls, globals, checked) = check("func f(): i32 { let d = -1\n return d }");
    let functions = lower_module("main", &decls, &globals, &checked).expect("module lowers");
    let IrStmtKind::Var { init: Some(init), .. } = &functions[0].body[0].kind else {
        panic!("expected a variable");
    };
    let IrExprKind::Unary { operand, .. } = &init.kind else {
        panic!("expected a negation, got {:?}", init.kind);
    };
    assert!(matches!(operand.kind, IrExprKind::Int(1)));
    assert!(is_primitive(&operand.ty, "i32"));
}
