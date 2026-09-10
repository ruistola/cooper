package ast

import (
	"github.com/ruistola/cooper/lexer"
)

type TypeExpr interface {
	typeExpr()
}

type Expr interface {
	expr()
}

type Stmt interface {
	stmt()
}

type NamedTypeExpr struct {
	TypeName string
}

func (t *NamedTypeExpr) typeExpr() {}

type ArrayTypeExpr struct {
	UnderlyingType TypeExpr
}

func (t *ArrayTypeExpr) typeExpr() {}

// PointerTypeExpr is the postfix `^` pointer type constructor, e.g. `T^`.
type PointerTypeExpr struct {
	UnderlyingType TypeExpr
}

func (t *PointerTypeExpr) typeExpr() {}

type FuncTypeExpr struct {
	ReturnType TypeExpr
	ParamTypes []TypeExpr
}

func (t *FuncTypeExpr) typeExpr() {}

type UnitTypeExpr struct{}

func (t *UnitTypeExpr) typeExpr() {}

// TypeApplicationExpr applies a type constructor to one or more argument types by
// juxtaposition, e.g. `Map string i32` or `Box i32`. The spine is parsed
// left-associatively; the constructor is typically a NamedTypeExpr and arity is
// checked later by the resolver. Postfix `[]`/`^` bind tighter than application,
// so compound arguments must be parenthesised (`(Map string i32)[]`).
type TypeApplicationExpr struct {
	Constructor TypeExpr
	Args        []TypeExpr
}

func (t *TypeApplicationExpr) typeExpr() {}

// TupleTypeExpr is an anonymous product type `(A, B, ...)` with at least two
// elements. A single-element parenthesised type collapses to that element and a
// zero-element one is the unit type, so those never produce a TupleTypeExpr.
type TupleTypeExpr struct {
	ElementTypes []TypeExpr
}

func (t *TupleTypeExpr) typeExpr() {}

type UnitExpr struct{}

func (e *UnitExpr) expr() {}

// TupleLiteralExpr is a tuple value `(a, b, ...)` with at least two elements. A
// single parenthesised expression is a GroupExpr and empty parens are UnitExpr,
// so those never produce a TupleLiteralExpr.
type TupleLiteralExpr struct {
	Elements []Expr
}

func (e *TupleLiteralExpr) expr() {}

type BoolLiteralExpr struct {
	Value bool
}

func (e *BoolLiteralExpr) expr() {}

type StringLiteralExpr struct {
	Value string
}

func (e *StringLiteralExpr) expr() {}

type IdentExpr struct {
	Value string
}

func (e *IdentExpr) expr() {}

type NumberLiteralExpr struct {
	Value string
}

func (e *NumberLiteralExpr) expr() {}

// NilLiteralExpr is the `nil` literal, the absent value of a pointer type.
type NilLiteralExpr struct{}

func (e *NilLiteralExpr) expr() {}

// AddressOfExpr is the prefix `&` operator, yielding a pointer to its operand.
type AddressOfExpr struct {
	Operand Expr
}

func (e *AddressOfExpr) expr() {}

// DerefExpr is the postfix `^` operator, yielding the value a pointer points to.
type DerefExpr struct {
	Operand Expr
}

func (e *DerefExpr) expr() {}

type UnaryExpr struct {
	Operator lexer.Token
	Rhs      Expr
}

func (e *UnaryExpr) expr() {}

type BinaryExpr struct {
	Lhs      Expr
	Operator lexer.Token
	Rhs      Expr
}

func (e *BinaryExpr) expr() {}

type BlockExpr struct {
	Statements []Stmt
	ResultExpr Expr
}

func (e *BlockExpr) expr() {}

type BlockStmt struct {
	Statements []Stmt
}

func (s *BlockStmt) stmt() {}

type ExpressionStmt struct {
	Expr              Expr
	ExplicitSemicolon bool
}

func (s *ExpressionStmt) stmt() {}

type GroupExpr struct {
	Expr Expr
}

func (e *GroupExpr) expr() {}

type VarDeclStmt struct {
	Var     TypedIdent
	InitVal Expr
}

func (s *VarDeclStmt) stmt() {}

type TypedIdent struct {
	Name string
	Type TypeExpr
}

type FuncDeclStmt struct {
	Receiver   *TypedIdent // nil for free functions, non-nil for methods
	Name       string
	TypeParams []string // type-parameter binders declared after the name (free functions and methods)
	Parameters []*TypedIdent
	ReturnType TypeExpr
	Body       *BlockStmt
}

func (s *FuncDeclStmt) stmt() {}

type FuncCallExpr struct {
	Func Expr
	Args []Expr
}

func (e *FuncCallExpr) expr() {}

type StructDeclStmt struct {
	Name       string
	TypeParams []string // type-parameter binders declared after the name, e.g. `struct Map K V`
	Members    []*TypedIdent
}

func (s *StructDeclStmt) stmt() {}

// OneofDeclStmt declares a sum type (tagged union), e.g.
//
//	oneof Result T E {
//	  Ok(T),
//	  Err(E),
//	}
//
// Type-parameter binders follow the name by juxtaposition (as with structs). Each
// variant carries an ordered, comma-delimited list of positional payload slots;
// an empty list (`None`) is a payload-free variant.
type OneofDeclStmt struct {
	Name       string
	TypeParams []string
	Variants   []*VariantDef
}

func (s *OneofDeclStmt) stmt() {}

// VariantDef is a single variant of a sum type: a name plus an ordered list of
// positional payload slot types (empty for a payload-free variant).
type VariantDef struct {
	Name    string
	Payload []TypeExpr
}

type StructLiteralExpr struct {
	Struct  Expr
	Members []*MemberAssignExpr
}

func (e *StructLiteralExpr) expr() {}

type StructMemberExpr struct {
	Struct Expr
	Member *IdentExpr
}

func (e *StructMemberExpr) expr() {}

type ArrayIndexExpr struct {
	Array Expr
	Index Expr
}

func (e *ArrayIndexExpr) expr() {}

type IfExpr struct {
	Cond Expr
	Then Expr
	Else Expr
}

func (e *IfExpr) expr() {}

type IfStmt struct {
	Cond Expr
	Then Stmt
	Else Stmt
}

func (s *IfStmt) stmt() {}

type ForStmt struct {
	Init Stmt
	Cond Expr
	Iter *ExpressionStmt
	Body *BlockStmt
}

func (s *ForStmt) stmt() {}

type AssignExpr struct {
	Assigne       Expr
	Operator      lexer.Token
	AssignedValue Expr
}

func (e *AssignExpr) expr() {}

type MemberAssignExpr struct {
	Name  string
	Value Expr
}

func (e *MemberAssignExpr) expr() {}

type VarDeclAssignExpr struct {
	Name          string
	AssignedValue Expr
}

func (e *VarDeclAssignExpr) expr() {}

// TupleDeclAssignExpr destructures a tuple-typed initializer, declaring each name
// and binding it to the corresponding element positionally. It backs both the
// walrus form `(a, b) := rhs` (Type nil; element types inferred from rhs) and the
// `let (a, b): (A, B) = rhs` form (Type set; element types taken from and checked
// against the annotation). Patterns are untyped: any type annotation lives on the
// enclosing `let`, never inside the pattern.
type TupleDeclAssignExpr struct {
	Names         []string
	Type          TypeExpr
	AssignedValue Expr
}

func (e *TupleDeclAssignExpr) expr() {}

type ReturnStmt struct {
	Expr Expr
}

func (s *ReturnStmt) stmt() {}

type UseDeclStmt struct {
	UseSpecs []*UseSpecExpr
}

func (s *UseDeclStmt) stmt() {}

type UseSpecExpr struct {
	Alias string   // optional local alias; empty when absent
	Path  []string // period-separated module path segments (at least one)
}

func (e *UseSpecExpr) expr() {}
