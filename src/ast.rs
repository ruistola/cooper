//! The abstract syntax tree.
//!
//! Every node records the [`Span`] of the source it was parsed from, so later
//! passes can attach precise diagnostics. Each syntactic category is split into a
//! `*Kind` enum (the shape) paired with a wrapper struct that adds the span; the
//! closed enums make every downstream `match` exhaustiveness-checked.

use crate::diag::Span;

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::And => "and",
            BinaryOp::Or => "or",
        }
    }
}

/// A prefix unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Pos,
    Not,
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Pos => "+",
            UnaryOp::Not => "!",
        }
    }
}

/// A (possibly compound) assignment operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
}

impl AssignOp {
    pub fn symbol(self) -> &'static str {
        match self {
            AssignOp::Assign => "=",
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
        }
    }
}

/// A type expression, e.g. `i32`, `T^`, `Map string i32`, or `(A, B)`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExprKind {
    Named(String),
    Array(Box<TypeExpr>),
    /// Postfix `^` pointer, e.g. `T^`.
    Pointer(Box<TypeExpr>),
    Func {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
    },
    Unit,
    /// Juxtaposition type application, e.g. `Map string i32`.
    Application {
        constructor: Box<TypeExpr>,
        args: Vec<TypeExpr>,
    },
    /// Anonymous product type `(A, B, ...)` with at least two elements.
    Tuple(Vec<TypeExpr>),
}

/// A name paired with its declared type (struct member, parameter, or receiver).
#[derive(Debug, Clone, PartialEq)]
pub struct TypedIdent {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

/// One variant of a sum type: a name and its positional payload slot types.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantDef {
    pub name: String,
    pub payload: Vec<TypeExpr>,
    pub span: Span,
}

/// A `name: value` initializer inside a struct literal.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberInit {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

/// A value block: statements followed by a trailing result expression (unit when
/// the block produces no value).
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Stmt>,
    pub result: Box<Expr>,
}

/// A match-arm pattern: a type-qualified variant (optionally binding positional
/// payload slots) or the wildcard `_`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    Variant {
        type_name: String,
        variant: String,
        binders: Vec<String>,
    },
    Wildcard,
}

/// A match arm whose body is an expression.
#[derive(Debug, Clone, PartialEq)]
pub struct ExprArm {
    pub pattern: Pattern,
    pub body: Expr,
}

/// A match arm whose body is a statement.
#[derive(Debug, Clone, PartialEq)]
pub struct StmtArm {
    pub pattern: Pattern,
    pub body: Stmt,
}

/// An expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Unit,
    Tuple(Vec<Expr>),
    Bool(bool),
    Str(String),
    Ident(String),
    /// A numeric literal, kept as its source text pending literal typing.
    Number(String),
    Nil,
    /// Prefix `&`.
    AddressOf(Box<Expr>),
    /// Postfix `^`.
    Deref(Box<Expr>),
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Block(Block),
    Group(Box<Expr>),
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    StructLiteral {
        target: Box<Expr>,
        members: Vec<MemberInit>,
    },
    Field {
        target: Box<Expr>,
        name: String,
    },
    Index {
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
        arms: Vec<ExprArm>,
    },
    Assign {
        op: AssignOp,
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// Walrus binding `name := value`.
    Let {
        name: String,
        value: Box<Expr>,
    },
    /// Tuple-destructuring binding `(a, b) := value` or `let (a, b): T = value`.
    LetTuple {
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
    pub span: Span,
}

/// A `use` specifier: an optional local alias and a dotted module path.
#[derive(Debug, Clone, PartialEq)]
pub struct UseSpec {
    pub alias: Option<String>,
    pub path: Vec<String>,
}

/// A statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Block(Vec<Stmt>),
    Expression(Expr),
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
        arms: Vec<StmtArm>,
    },
    Return(Option<Expr>),
    Use(Vec<UseSpec>),
}
