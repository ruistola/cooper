//! The typed IR: functions, statements, expressions, patterns, and decision trees.

use super::*;

/// A lowered function or method: the declaration it lowers, its signature, its body
/// as typed statements, and the span it was lowered from. A method carries its
/// receiver as the first bound place. `type_params` lists every binder of the
/// declaration's signature — its own, then those a generic receiver introduces — in
/// the order a reference supplies type arguments. A generic function as lowered is a
/// template whose types name those binders; [`monomorphize`] stamps out instances,
/// each with `type_args` holding one concrete argument per binder (empty for a
/// non-generic function, and for a template).
///
/// A lifted function literal or nested function has an `env`: the captured variables
/// its environment points to, in order. It inherits every binder of the declaration
/// it is lifted from.
#[derive(Debug, Clone)]
pub struct Function {
    pub item: Callable,
    /// The source file the declaration is in, which its spans index into.
    pub file: String,
    pub receiver: Option<Param>,
    pub env: Option<Vec<Param>>,
    pub type_params: Vec<String>,
    pub type_args: Vec<Type>,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub body: Vec<IrStmt>,
    pub span: Span,
}

/// A C function the program declares `extern`: its symbol and C-compatible signature.
#[derive(Debug, Clone)]
pub struct Extern {
    pub name: String,
    pub params: Vec<Type>,
    pub return_type: Type,
}

impl Extern {
    /// Whether two declarations give the symbol the same signature.
    pub fn same_signature(&self, other: &Extern) -> bool {
        self.return_type.equals(&other.return_type)
            && self.params.len() == other.params.len()
            && self.params.iter().zip(&other.params).all(|(a, b)| a.equals(b))
    }
}

/// A bound place in a signature: a name, its resolved type, and its source span.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

/// A typed statement.
#[derive(Debug, Clone)]
pub struct IrStmt {
    pub kind: IrStmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrStmtKind {
    /// A local binding; `ty` is the resolved type the name takes.
    Var {
        name: String,
        ty: Type,
        init: Option<IrExpr>,
    },
    Expr(IrExpr),
    Return(Option<IrExpr>),
    While {
        cond: IrExpr,
        body: Vec<IrStmt>,
        post_test: bool,
        until: bool,
    },
    /// Iteration over an integer range. `var` takes successive values of type `ty`
    /// from `start` up to `end` (inclusive when `inclusive`).
    ForRange {
        var: String,
        ty: Type,
        start: Box<IrExpr>,
        end: Box<IrExpr>,
        inclusive: bool,
        body: Vec<IrStmt>,
    },
    /// Iteration over the elements of an array. `elem` takes each element in turn;
    /// when `index` is present it takes the element's position. The loop drives from
    /// the array's length (the `ArrayLength` intrinsic) and reads each element
    /// (`ArrayGet`), but carries the array and binders abstractly so the backend
    /// chooses the lowering.
    ForEach {
        array: Box<IrExpr>,
        index: Option<Binder>,
        elem: Binder,
        body: Vec<IrStmt>,
    },
    Block(Vec<IrStmt>),
    Break,
    Continue,
    /// A `match` run for effect. The scrutinee is evaluated once, then `tree` tests
    /// it to select an `action` (a statement body); `actions` is indexed by the
    /// tree's leaves. A plain-boolean `if` desugars into this form. The checker
    /// verified the arms exhaust the scrutinee, so a reached `Decision::Fail` is
    /// impossible.
    Match {
        scrutinee: IrExpr,
        actions: Vec<IrStmt>,
        tree: Decision,
    },
}

/// A lowered decision tree: the nested tests a `match` compiles to, selecting a
/// leaf action. Built by the usefulness/matrix algorithm so each scrutinee position
/// is tested at most once along any path.
#[derive(Debug, Clone)]
pub enum Decision {
    /// Run the action at `action`, having bound each name in `bindings` to the
    /// subvalue its access reaches.
    Leaf {
        bindings: Vec<MatchBinding>,
        action: usize,
    },
    /// Test the subvalue at `access` (of type `ty`) against each case in turn;
    /// `default` is taken when no case matches (absent when the cases are
    /// exhaustive over the type).
    Switch {
        access: Access,
        ty: Type,
        cases: Vec<Case>,
        default: Option<Box<Decision>>,
    },
    /// No arm matched — unreachable after the checker's exhaustiveness check.
    Fail,
}

/// One arm of a [`Decision::Switch`]: the constructor tested and the subtree taken
/// when it matches.
#[derive(Debug, Clone)]
pub struct Case {
    pub test: Test,
    pub tree: Decision,
}

/// A constructor a [`Switch`](Decision::Switch) discriminates on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Test {
    /// A sum-type variant, selected by name.
    Variant(String),
    /// A boolean constant.
    Bool(bool),
    /// An integer constant: its decoded magnitude and whether it is negated.
    Int { value: u128, negative: bool },
}

/// A name bound by a matched pattern: the subvalue it names (via `access`) and that
/// subvalue's resolved type.
#[derive(Debug, Clone)]
pub struct MatchBinding {
    pub name: String,
    pub access: Access,
    pub ty: Type,
}

/// A path from the `match` scrutinee to a subvalue a decision tree tests or binds.
#[derive(Debug, Clone)]
pub enum Access {
    /// The scrutinee itself.
    Root,
    /// A named field of a struct subvalue.
    Field { parent: Box<Access>, name: String },
    /// A positional component of a tuple subvalue.
    Elem { parent: Box<Access>, index: usize },
    /// A positional payload slot of a sum-type variant subvalue.
    Payload {
        parent: Box<Access>,
        variant: String,
        index: usize,
    },
}

/// A lowered pattern: its shape, the type of the value it matches at this position
/// (so bindings and sub-patterns carry a resolved type for the backend), and the
/// span it was lowered from.
#[derive(Debug, Clone)]
pub struct IrPattern {
    pub kind: IrPatternKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrPatternKind {
    /// Matches anything, binds nothing.
    Wildcard,
    /// Matches anything and binds the whole value to a name (its type is on the node).
    Binding(String),
    /// A sum-type variant, binding its payload slots positionally. A binder named
    /// `_` discards its slot.
    Variant {
        variant: String,
        binders: Vec<Binder>,
    },
    /// A boolean literal pattern matching one constant of a `bool` scrutinee.
    Bool(bool),
    /// An integer literal pattern: the decoded magnitude and whether it is negated.
    /// The node's type fixes its width and signedness.
    Int {
        value: u128,
        negative: bool,
    },
    /// A tuple pattern, matching a tuple value component-wise.
    Tuple(Vec<IrPattern>),
    /// A struct pattern, matching the listed fields; unlisted fields are wildcards.
    Struct {
        name: String,
        fields: Vec<(String, IrPattern)>,
    },
}

/// A payload slot bound by a variant pattern: the name introduced and the slot's
/// resolved type.
#[derive(Debug, Clone)]
pub struct Binder {
    pub name: String,
    pub ty: Type,
}

/// A typed expression: a shape, the resolved type it evaluates to, and the source
/// span it was lowered from.
#[derive(Debug, Clone)]
pub struct IrExpr {
    pub kind: IrExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum IrExprKind {
    /// An integer literal's decoded magnitude; its resolved type (width and
    /// signedness) is on the node, and any sign is a surrounding `Unary` operator.
    Int(u128),
    /// A floating-point literal's decoded value; its width is on the node.
    Float(f64),
    Bool(bool),
    /// A string literal's decoded contents (escapes resolved).
    Str(String),
    Nil,
    Unit,
    /// A reference to a bound variable or parameter.
    Var(String),
    /// A function value for the lifted function literal `item`, instantiated at
    /// `type_args` (the enclosing declaration's binders), whose environment refers to
    /// the variables named by `captures` — the same variables, not copies.
    Closure {
        item: Callable,
        type_args: Vec<Type>,
        captures: Vec<String>,
    },
    /// A reference to a free function, instantiated at `type_args` (one per binder of
    /// its signature; none when it is not generic).
    FuncRef { item: Callable, type_args: Vec<Type> },
    /// A method bound to its receiver value, instantiated at `type_args`. Calling it
    /// passes `receiver` as the method's receiver.
    Method {
        receiver: Box<IrExpr>,
        item: Callable,
        type_args: Vec<Type>,
    },
    Tuple(Vec<IrExpr>),
    Unary {
        op: UnaryOp,
        operand: Box<IrExpr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<IrExpr>,
        rhs: Box<IrExpr>,
    },
    Call {
        callee: Box<IrExpr>,
        args: Vec<IrExpr>,
    },
    Field {
        target: Box<IrExpr>,
        name: String,
    },
    /// A call to a runtime-touching built-in operation, modelled abstractly rather
    /// than as a concrete runtime call. Arguments are fixed by `op`: the receiver
    /// array first, then an index, then a value (see [`Intrinsic`]).
    Intrinsic {
        op: Intrinsic,
        args: Vec<IrExpr>,
    },
    /// Postfix `^` load through a pointer.
    Deref(Box<IrExpr>),
    /// Prefix `&`, taking the address of an addressable place.
    AddressOf(Box<IrExpr>),
    Assign {
        op: AssignOp,
        target: Box<IrExpr>,
        value: Box<IrExpr>,
    },
    /// Walrus binding `name := value`, evaluating to the bound value.
    Let {
        name: String,
        value: Box<IrExpr>,
    },
    /// A value block: statements run for effect, then a trailing result expression.
    Block {
        stmts: Vec<IrStmt>,
        result: Box<IrExpr>,
    },
    /// Construction of a nominal struct value. The struct's name and type arguments
    /// are on the node's resolved type; members are carried in source order, each a
    /// field name paired with its lowered value.
    StructLiteral {
        name: String,
        members: Vec<(String, IrExpr)>,
    },
    /// A `match` producing a value: the scrutinee is evaluated once, then `tree`
    /// selects one of `actions` (each an expression of the node's resolved type). A
    /// plain-boolean `if` expression desugars into this form.
    Match {
        scrutinee: Box<IrExpr>,
        actions: Vec<IrExpr>,
        tree: Box<Decision>,
    },
    /// Construction of a sum-type value: a variant name and its payload arguments
    /// (empty for a payload-free variant). The owning sum type and its type arguments
    /// are on the node's resolved type.
    Variant {
        variant: String,
        args: Vec<IrExpr>,
    },
    /// A constructor-style numeric conversion `T(x)`. The source value is the operand;
    /// the destination type is the node's resolved type.
    Convert(Box<IrExpr>),
    /// Tuple-destructuring binding `(a, b) := value`: each name is bound to the
    /// matching component of the tuple `value`, and the whole expression evaluates to
    /// that tuple (the node's resolved type).
    LetTuple {
        bindings: Vec<Binder>,
        value: Box<IrExpr>,
    },
}

/// A runtime-touching built-in operation, on a blessed collection or of the standard
/// library, modelled as an abstract typed intrinsic a backend lowers behind the runtime
/// boundary. Each variant fixes its argument order; a receiver array, where there is
/// one, is first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intrinsic {
    /// A fresh array holding the given elements in order (an `[a, b, c]` literal).
    /// Args: the elements; the array type is on the node.
    ArrayLiteral,
    /// `length(): u64` — the element count of an array. Args: `[array]`.
    ArrayLength,
    /// `get(i): T` — the element at an index (the `a[i]` read). Args: `[array, index]`.
    ArrayGet,
    /// `set(i, v): unit` — store an element at an index (the `a[i] = v` write).
    /// Args: `[array, index, value]`.
    ArraySet,
    /// `push(v): T[]` — append, yielding the (possibly reallocated) array.
    /// Args: `[array, value]`.
    ArrayPush,
    /// Interior pointer to an element (the `&a[i]` address-of). Args: `[array, index]`.
    ArrayElementPtr,
    /// The elements from `start` up to `end` (exclusive, or inclusive when `inclusive`)
    /// as an array sharing them, with capacity equal to its length so that growing it
    /// never writes into the original (`a[lo..hi]`). Args: `[array, start]` to the
    /// array's end, or `[array, start, end]`.
    ArraySlice { inclusive: bool },
    /// `copy(): T[]` — a fresh array holding a copy of the elements. Args: `[array]`.
    ArrayCopy,
    /// `reserve(n): T[]` — the array with room for at least `n` more elements,
    /// reallocated if it lacks it. Args: `[array, n]`.
    ArrayReserve,
    /// `std.io.print(s)`, or `std.io.println(s)` when `newline`: write the string to
    /// standard output. Args: `[string]`.
    Print { newline: bool },
    /// The conversion `CString(s)`: a NUL-terminated heap copy of the string, as a
    /// `std.ffi.CString`. Args: `[string]`.
    ToCString,
    /// The conversion `string(c)` of a `CString`: a string copied from the C string up
    /// to its NUL, empty when it is null. Args: `[c_string]`.
    FromCString,
    /// `std.ffi.copyBytes(p, n)`: an array holding a copy of the `n` bytes at `p`.
    /// Args: `[pointer, count]`.
    CopyBytes,
}
