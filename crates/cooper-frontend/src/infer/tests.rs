use super::*;
use crate::types::TypeId;

fn prim(name: &str) -> Type {
    Type::Primitive(name.to_string())
}

fn id(name: &str) -> TypeId {
    TypeId { module: "main".to_string(), name: name.to_string() }
}

#[test]
fn bindings_follow_merged_variables_in_both_directions() {
    let mut table = InferTable::default();
    let a = table.fresh(InferKind::General);
    let b = table.fresh(InferKind::General);
    let c = table.fresh(InferKind::General);
    table.unify(&a, &b).unwrap();
    table.unify(&c, &b).unwrap();
    table.unify(&a, &a).unwrap();
    assert!(!table.is_bound(&c));
    table.unify(&prim("string"), &b).unwrap();
    assert!(table.resolve(&a).equals(&prim("string")));
    assert!(table.resolve(&c).equals(&prim("string")));
    assert!(table.is_bound(&c));
    let conflict = table.unify(&c, &prim("bool")).unwrap_err();
    assert!(conflict.left.equals(&prim("string")));
    assert!(conflict.right.equals(&prim("bool")));
}

#[test]
fn literal_classes_constrain_concrete_bindings() {
    let mut table = InferTable::default();
    let integer = table.fresh(InferKind::IntLiteral);
    table.unify(&integer, &prim("i64")).unwrap();
    let integer_as_float = table.fresh(InferKind::IntLiteral);
    table.unify(&integer_as_float, &prim("f64")).unwrap();
    let float = table.fresh(InferKind::FloatLiteral);
    assert!(table.unify(&float, &prim("i32")).is_err());
    table.unify(&float, &prim("f32")).unwrap();
    let non_numeric = table.fresh(InferKind::IntLiteral);
    assert!(table.unify(&non_numeric, &prim("string")).is_err());
    assert!(!table.is_bound(&non_numeric));
}

#[test]
fn merged_literal_classes_default_once_at_their_representative() {
    let mut table = InferTable::default();
    let general = table.fresh(InferKind::General);
    let integer = table.fresh(InferKind::IntLiteral);
    let float = table.fresh(InferKind::FloatLiteral);
    assert_eq!(general.to_string(), "_");
    assert_eq!(integer.to_string(), "{integer}");
    assert_eq!(float.to_string(), "{float}");
    table.unify(&general, &integer).unwrap();
    assert_eq!(table.resolve(&general).to_string(), "{integer}");
    table.unify(&float, &integer).unwrap();
    assert_eq!(table.resolve(&integer).to_string(), "{float}");
    assert!(table.unify(&general, &prim("u32")).is_err());
    let unbound = table.fresh(InferKind::General);
    let default_int = table.fresh(InferKind::IntLiteral);
    let bound_int = table.fresh(InferKind::IntLiteral);
    table.unify(&bound_int, &prim("u8")).unwrap();
    table.default_literals();
    table.default_literals();
    assert!(table.resolve(&general).equals(&prim(DEFAULT_FLOAT)));
    assert!(table.resolve(&integer).equals(&prim(DEFAULT_FLOAT)));
    assert!(table.resolve(&float).equals(&prim(DEFAULT_FLOAT)));
    assert!(table.resolve(&default_int).equals(&prim(DEFAULT_INT)));
    assert!(table.resolve(&bound_int).equals(&prim("u8")));
    assert!(!table.is_bound(&unbound));
}

#[test]
fn occurs_check_rejects_direct_and_indirect_recursive_bindings() {
    let mut table = InferTable::default();
    let a = table.fresh(InferKind::General);
    let b = table.fresh(InferKind::General);
    assert!(table.unify(&a, &Type::Array(Box::new(a.clone()))).is_err());
    assert!(!table.is_bound(&a));
    table.unify(&b, &Type::Pointer(Box::new(a.clone()))).unwrap();
    assert!(table.is_bound(&b));
    assert!(table.unify(&a, &b).is_err());
    table.unify(&a, &prim("i32")).unwrap();
    assert!(table.resolve(&b).equals(&Type::Pointer(Box::new(prim("i32")))));
}

#[test]
fn nil_fits_pointers_without_determining_their_elements() {
    let mut table = InferTable::default();
    let elem = table.fresh(InferKind::General);
    let pointer = Type::Pointer(Box::new(elem.clone()));
    table.unify(&Type::Nil, &pointer).unwrap();
    table.unify(&pointer, &Type::Nil).unwrap();
    table.unify(&Type::Nil, &Type::Nil).unwrap();
    assert!(!table.is_bound(&elem));
    assert!(table.unify(&Type::Nil, &prim("i32")).is_err());
}

#[test]
fn unification_and_resolution_descend_through_composite_types() {
    let mut table = InferTable::default();
    let elem = table.fresh(InferKind::General);
    let shape = |elem: Type| Type::Func {
        param_types: vec![Type::Tuple(vec![
            Type::Array(Box::new(Type::Pointer(Box::new(elem.clone())))),
            Type::TypeParam("T".to_string()),
        ])],
        return_type: Box::new(Type::Oneof {
            id: id("Maybe"),
            type_args: vec![Type::Struct { id: id("Box"), type_args: vec![elem] }],
        }),
    };
    let template = shape(elem.clone());
    let concrete = shape(prim("f64"));
    assert!(table.is_bound(&template));
    table.unify(&template, &concrete).unwrap();
    assert!(table.resolve(&template).equals(&concrete));
    assert!(table.resolve(&elem).equals(&prim("f64")));
    let recursive = table.fresh(InferKind::General);
    assert!(table.unify(&recursive, &shape(recursive.clone())).is_err());
}

#[test]
fn structural_matching_preserves_arity_nominal_identity_and_rigid_parameters() {
    let mut table = InferTable::default();
    let tuple = Type::Tuple(vec![prim("i32")]);
    assert!(table.unify(&tuple, &Type::Tuple(vec![])).is_err());
    let function = |params| Type::Func {
        return_type: Box::new(Type::Unit),
        param_types: params,
    };
    assert!(table.unify(&function(vec![]), &function(vec![prim("i32")])).is_err());
    let boxed = Type::Struct { id: id("Box"), type_args: vec![prim("i32")] };
    let other_module = Type::Struct {
        id: TypeId { module: "other".to_string(), name: "Box".to_string() },
        type_args: vec![prim("i32")],
    };
    let sum = Type::Oneof { id: id("Box"), type_args: vec![prim("i32")] };
    let template = Type::Struct { id: id("Box"), type_args: vec![] };
    assert!(table.unify(&boxed, &other_module).is_err());
    assert!(table.unify(&boxed, &sum).is_err());
    assert!(table.unify(&boxed, &template).is_err());
    let parameter = Type::TypeParam("T".to_string());
    table.unify(&parameter, &parameter).unwrap();
    assert!(table.unify(&parameter, &prim("i32")).is_err());
    let inferred = table.fresh(InferKind::General);
    table.unify(&inferred, &parameter).unwrap();
    assert!(table.resolve(&inferred).equals(&parameter));
}
