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

func (t NamedTypeExpr) typeExpr() {}

type ArrayTypeExpr struct {
	UnderlyingType TypeExpr
}

func (t ArrayTypeExpr) typeExpr() {}

type FuncTypeExpr struct {
	ReturnType TypeExpr
	ParamTypes []TypeExpr
}

func (t FuncTypeExpr) typeExpr() {}

type UnitExpr struct{}

func (e UnitExpr) expr() {}

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
	Statements []Stmt
	ResultExpr Expr
}

func (e BlockExpr) expr() {}

type BlockStmt struct {
	Statements []Stmt
}

func (s BlockStmt) stmt() {}

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
	Type TypeExpr
}

type FuncDeclStmt struct {
	Name       string
	Parameters []TypedIdent
	ReturnType TypeExpr
	Body       BlockStmt
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

type IfStmt struct {
	Cond Expr
	Then Stmt
	Else Stmt
}

func (s IfStmt) stmt() {}

type ForStmt struct {
	Init Stmt
	Cond Expr
	Iter ExpressionStmt
	Body BlockStmt
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

type UseDeclStmt struct {
	UseSpecs []UseSpecExpr
}

func (s UseDeclStmt) stmt() {}

type UseSpecExpr struct {
	Name   string
	Module string //TODO: should this be more structural, like a ModulePath or something?
}

func (e UseSpecExpr) expr() {}
