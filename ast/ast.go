package ast

import (
	"github.com/ruistola/cooper/lexer"
)

type Type interface {
	_type()
}

type Expr interface {
	expr()
}

type Stmt interface {
	stmt()
}

type NamedType struct {
	TypeName string
}

func (t NamedType) _type() {}

type ArrayType struct {
	UnderlyingType Type
}

func (t ArrayType) _type() {}

type FuncType struct {
	ReturnType Type
	ParamTypes []Type
}

func (t FuncType) _type() {}

type BoolLiteralExpr struct {
	Value bool
}

func (e BoolLiteralExpr) expr() {}

type StringLiteralExpr struct {
	Value string
}

func (e StringLiteralExpr) expr() {}

type IdentExpr struct {
	Value string
}

func (e IdentExpr) expr() {}

type NumberLiteralExpr struct {
	Value string
}

func (e NumberLiteralExpr) expr() {}

type UnaryExpr struct {
	Operator lexer.Token
	Rhs      Expr
}

func (e UnaryExpr) expr() {}

type BinaryExpr struct {
	Lhs      Expr
	Operator lexer.Token
	Rhs      Expr
}

func (e BinaryExpr) expr() {}

type BlockExpr struct {
	Statements    []Stmt
	SuppressValue bool
}

func (e BlockExpr) expr() {}

type ExpressionStmt struct {
	Expr              Expr
	ExplicitSemicolon bool
}

func (s ExpressionStmt) stmt() {}

type GroupExpr struct {
	Expr Expr
}

func (e GroupExpr) expr() {}

type VarDeclStmt struct {
	Var     TypedIdent
	InitVal Expr
}

func (s VarDeclStmt) stmt() {}

type TypedIdent struct {
	Name string
	Type Type
}

type FuncDeclStmt struct {
	Name       string
	Parameters []TypedIdent
	ReturnType Type
	Body       BlockExpr
}

func (s FuncDeclStmt) stmt() {}

type FuncCallExpr struct {
	Func Expr
	Args []Expr
}

func (e FuncCallExpr) expr() {}

type StructDeclStmt struct {
	Name    string
	Members []TypedIdent
}

func (s StructDeclStmt) stmt() {}

type StructLiteralExpr struct {
	Struct  Expr
	Members []MemberAssignExpr
}

func (e StructLiteralExpr) expr() {}

type StructMemberExpr struct {
	Struct Expr
	Member IdentExpr
}

func (e StructMemberExpr) expr() {}

type ArrayIndexExpr struct {
	Array Expr
	Index Expr
}

func (e ArrayIndexExpr) expr() {}

type IfExpr struct {
	Cond Expr
	Then Expr
	Else Expr
}

func (e IfExpr) expr() {}

type ForStmt struct {
	Init Stmt
	Cond Expr
	Iter ExpressionStmt
	Body BlockExpr
}

func (s ForStmt) stmt() {}

type AssignExpr struct {
	Assigne       Expr
	Operator      lexer.Token
	AssignedValue Expr
}

func (e AssignExpr) expr() {}

type MemberAssignExpr struct {
	Name  string
	Value Expr
}

func (e MemberAssignExpr) expr() {}

type ReturnStmt struct {
	Expr Expr
}

func (s ReturnStmt) stmt() {}
