use crate::lexer::TokenKind;

/// Type expressions. Mirrors the Go `ast.TypeExpr` interface, but as a closed
/// `enum` so every `match` over it is checked for exhaustiveness at compile time.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Named(String),
    Array(Box<TypeExpr>),
    /// Postfix `^` pointer constructor, e.g. `T^`.
    Pointer(Box<TypeExpr>),
    Func {
        return_type: Box<TypeExpr>,
        param_types: Vec<TypeExpr>,
    },
    Unit,
    /// Type application by juxtaposition, e.g. `Map string i32`.
    Application {
        constructor: Box<TypeExpr>,
        args: Vec<TypeExpr>,
    },
    /// Anonymous product type `(A, B, ...)` with at least two elements.
    Tuple(Vec<TypeExpr>),
}

/// A name paired with its declared type (struct member, parameter, receiver).
#[derive(Debug, Clone, PartialEq)]
pub struct TypedIdent {
    pub name: String,
    pub ty: TypeExpr,
}

/// A single variant of a sum type: a name plus positional payload slot types.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantDef {
    pub name: String,
    pub payload: Vec<TypeExpr>,
}

/// A `name: value` initializer inside a struct literal.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberAssign {
    pub name: String,
    pub value: Expr,
}

/// A value block: statements followed by a trailing result expression (unit when
/// the block produces no value).
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Stmt>,
    pub result: Box<Expr>,
}

/// A match-arm pattern. Flat in this iteration: a type-qualified variant pattern
/// (optionally binding positional payload slots) or the wildcard `_`.
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Variant {
        type_name: String,
        variant: String,
        binders: Vec<String>,
    },
    Wildcard,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchExprArm {
    pub pattern: Pattern,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchStmtArm {
    pub pattern: Pattern,
    pub body: Stmt,
}

/// Expressions.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Unit,
    Tuple(Vec<Expr>),
    Bool(bool),
    Str(String),
    Ident(String),
    Number(String),
    Nil,
    /// Prefix `&`.
    AddressOf(Box<Expr>),
    /// Postfix `^`.
    Deref(Box<Expr>),
    Unary {
        op: TokenKind,
        rhs: Box<Expr>,
    },
    Binary {
        lhs: Box<Expr>,
        op: TokenKind,
        rhs: Box<Expr>,
    },
    Block(Block),
    Group(Box<Expr>),
    FuncCall {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    StructLiteral {
        target: Box<Expr>,
        members: Vec<MemberAssign>,
    },
    StructMember {
        target: Box<Expr>,
        member: String,
    },
    ArrayIndex {
        array: Box<Expr>,
        index: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchExprArm>,
    },
    Assign {
        target: Box<Expr>,
        op: TokenKind,
        value: Box<Expr>,
    },
    VarDeclAssign {
        name: String,
        value: Box<Expr>,
    },
    TupleDeclAssign {
        names: Vec<String>,
        ty: Option<TypeExpr>,
        value: Box<Expr>,
    },
}

/// A free function or method declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FuncDecl {
    /// `None` for free functions, `Some` for methods.
    pub receiver: Option<TypedIdent>,
    pub name: String,
    pub type_params: Vec<String>,
    pub params: Vec<TypedIdent>,
    pub return_type: Option<TypeExpr>,
    pub body: Vec<Stmt>,
}

/// A `use` specifier: an optional local alias plus a dotted module path.
#[derive(Debug, Clone, PartialEq)]
pub struct UseSpec {
    pub alias: Option<String>,
    pub path: Vec<String>,
}

/// Statements.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Block(Vec<Stmt>),
    Expression {
        expr: Expr,
        explicit_semicolon: bool,
    },
    VarDecl {
        name: String,
        ty: Option<TypeExpr>,
        init: Option<Expr>,
    },
    FuncDecl(FuncDecl),
    StructDecl {
        name: String,
        type_params: Vec<String>,
        members: Vec<TypedIdent>,
    },
    OneofDecl {
        name: String,
        type_params: Vec<String>,
        variants: Vec<VariantDef>,
    },
    If {
        cond: Expr,
        then: Box<Stmt>,
        els: Option<Box<Stmt>>,
    },
    For {
        init: Box<Stmt>,
        cond: Expr,
        iter: Expr,
        body: Vec<Stmt>,
    },
    Match {
        scrutinee: Expr,
        arms: Vec<MatchStmtArm>,
    },
    Return(Option<Expr>),
    Use(Vec<UseSpec>),
}
