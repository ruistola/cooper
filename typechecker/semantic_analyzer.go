package typechecker

import (
	"fmt"
	"github.com/ruistola/cooper/ast"
	"slices"
)

// SemanticAnalyzer handles semantic validation and control flow analysis
type SemanticAnalyzer struct {
	errors      []string
	symbolTable *SymbolTable
}

// NewSemanticAnalyzer creates a new semantic analyzer
func NewSemanticAnalyzer(symbolTable *SymbolTable) *SemanticAnalyzer {
	return &SemanticAnalyzer{
		errors:      []string{},
		symbolTable: symbolTable,
	}
}

// Err adds an error to the semantic analyzer's error list
func (sa *SemanticAnalyzer) Err(msg string) {
	coloredMsg := fmt.Sprintf("\033[31mSemantic Error: %s\033[0m", msg)
	sa.errors = append(sa.errors, coloredMsg)
}

// AnalyzeSemantics performs semantic analysis on the program
func AnalyzeSemantics(program ast.BlockStmt, symbolTable *SymbolTable) []string {
	analyzer := NewSemanticAnalyzer(symbolTable)
	analyzer.analyzeBlockStmt(program)
	return analyzer.errors
}

// analyzeStmt analyzes semantic rules for a statement
func (sa *SemanticAnalyzer) analyzeStmt(stmt ast.Stmt) {
	switch s := stmt.(type) {
	case ast.BlockStmt:
		sa.analyzeBlockStmt(s)
	case ast.VarDeclStmt:
		sa.analyzeVarDeclStmt(s)
	case ast.StructDeclStmt:
		sa.analyzeStructDeclStmt(s)
	case ast.FuncDeclStmt:
		sa.analyzeFuncDeclStmt(s)
	case ast.IfStmt:
		sa.analyzeIfStmt(s)
	case ast.ForStmt:
		sa.analyzeForStmt(s)
	case ast.ReturnStmt:
		sa.analyzeReturnStmt(s)
	case ast.ExpressionStmt:
		sa.analyzeExpr(s.Expr)
	default:
		sa.Err(fmt.Sprintf("unknown statement type for semantic analysis: %T", stmt))
	}
}

// analyzeBlockStmt analyzes a block statement for semantic rules
func (sa *SemanticAnalyzer) analyzeBlockStmt(block ast.BlockStmt) {
	for _, stmt := range block.Statements {
		sa.analyzeStmt(stmt)
	}
	// Check for unreachable code
	sa.checkUnreachableCode(block)
}

// analyzeVarDeclStmt analyzes variable declarations for semantic rules
func (sa *SemanticAnalyzer) analyzeVarDeclStmt(stmt ast.VarDeclStmt) {
	// Variable declaration semantic rules can be added here
	// For example: checking if variable shadows outer scope variables, etc.
	if stmt.InitVal != nil {
		sa.analyzeExpr(stmt.InitVal)
	}
}

// analyzeStructDeclStmt analyzes struct declarations for semantic rules
func (sa *SemanticAnalyzer) analyzeStructDeclStmt(stmt ast.StructDeclStmt) {
	// Struct declaration semantic rules can be added here
	// For example: checking for recursive struct definitions, etc.
}

// analyzeFuncDeclStmt analyzes function declarations for semantic rules
func (sa *SemanticAnalyzer) analyzeFuncDeclStmt(stmt ast.FuncDeclStmt) {
	// Get function type from symbol table
	funcType, ok := sa.symbolTable.LookupFunc(stmt.Name)
	if !ok {
		sa.Err(fmt.Sprintf("function %s not found in symbol table during semantic analysis", stmt.Name))
		return
	}

	// Analyze function body
	sa.analyzeBlockStmt(stmt.Body)

	// Check that all code paths return a value if needed
	if funcType.ReturnType != nil && !IsUnit(funcType.ReturnType) {
		if !sa.blockReturns(stmt.Body) {
			sa.Err(fmt.Sprintf("function '%s' with return type %s does not return a value in all code paths", stmt.Name, funcType.ReturnType))
		}
	}
}

// analyzeIfStmt analyzes if statements for semantic rules
func (sa *SemanticAnalyzer) analyzeIfStmt(stmt ast.IfStmt) {
	sa.analyzeExpr(stmt.Cond)
	sa.analyzeStmt(stmt.Then)
	if stmt.Else != nil {
		sa.analyzeStmt(stmt.Else)
	}
}

// analyzeForStmt analyzes for statements for semantic rules
func (sa *SemanticAnalyzer) analyzeForStmt(stmt ast.ForStmt) {
	sa.analyzeStmt(stmt.Init)
	sa.analyzeExpr(stmt.Cond)
	sa.analyzeStmt(stmt.Iter)
	sa.analyzeBlockStmt(stmt.Body)
}

// analyzeReturnStmt analyzes return statements for semantic rules
func (sa *SemanticAnalyzer) analyzeReturnStmt(stmt ast.ReturnStmt) {
	if stmt.Expr != nil {
		sa.analyzeExpr(stmt.Expr)
	}
}

// analyzeExpr analyzes expressions for semantic rules
func (sa *SemanticAnalyzer) analyzeExpr(expr ast.Expr) {
	switch e := expr.(type) {
	case ast.NumberLiteralExpr, ast.StringLiteralExpr, ast.BoolLiteralExpr:
		// Literals don't need semantic analysis
	case ast.IdentExpr:
		// Identifier semantic rules can be added here
	case ast.BinaryExpr:
		sa.analyzeExpr(e.Lhs)
		sa.analyzeExpr(e.Rhs)
	case ast.UnaryExpr:
		sa.analyzeExpr(e.Rhs)
	case ast.GroupExpr:
		sa.analyzeExpr(e.Expr)
	case ast.FuncCallExpr:
		sa.analyzeExpr(e.Func)
		for _, arg := range e.Args {
			sa.analyzeExpr(arg)
		}
	case ast.StructLiteralExpr:
		sa.analyzeExpr(e.Struct)
		for _, member := range e.Members {
			sa.analyzeExpr(member.Value)
		}
	case ast.StructMemberExpr:
		sa.analyzeExpr(e.Struct)
	case ast.ArrayIndexExpr:
		sa.analyzeExpr(e.Array)
		sa.analyzeExpr(e.Index)
	case ast.AssignExpr:
		sa.analyzeExpr(e.Assigne)
		sa.analyzeExpr(e.AssignedValue)
	case ast.VarDeclAssignExpr:
		sa.analyzeExpr(e.AssignedValue)
	default:
		sa.Err(fmt.Sprintf("unknown expression type for semantic analysis: %T", expr))
	}
}

// stmtReturns checks if a statement returns in all paths
func (sa *SemanticAnalyzer) stmtReturns(stmt ast.Stmt) bool {
	switch s := stmt.(type) {
	case ast.BlockStmt:
		return sa.blockReturns(s)
	case ast.ReturnStmt:
		return true
	case ast.IfStmt:
		if s.Else == nil {
			return false
		}
		return sa.stmtReturns(s.Then) && sa.stmtReturns(s.Else)
	}
	return false
}

// blockReturns checks if a block returns in all paths
func (sa *SemanticAnalyzer) blockReturns(block ast.BlockStmt) bool {
	if len(block.Statements) == 0 {
		return false
	}
	return slices.ContainsFunc(block.Statements, func(stmt ast.Stmt) bool {
		return sa.stmtReturns(stmt)
	})
}

// checkUnreachableCode detects unreachable code after return statements
func (sa *SemanticAnalyzer) checkUnreachableCode(block ast.BlockStmt) {
	for i := range len(block.Statements) - 1 {
		if sa.stmtReturns(block.Statements[i]) {
			sa.Err(fmt.Sprintf("unreachable code after statement %d", i+1))
			break
		}
	}

	// Recursively check nested blocks
	for _, stmt := range block.Statements {
		switch s := stmt.(type) {
		case ast.BlockStmt:
			sa.checkUnreachableCode(s)
		case ast.IfStmt:
			if thenBlock, ok := s.Then.(ast.BlockStmt); ok {
				sa.checkUnreachableCode(thenBlock)
			}
			if s.Else != nil {
				if elseBlock, ok := s.Else.(ast.BlockStmt); ok {
					sa.checkUnreachableCode(elseBlock)
				}
			}
		case ast.ForStmt:
			sa.checkUnreachableCode(s.Body)
		}
	}
}

