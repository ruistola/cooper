package typechecker

import (
	"fmt"
	"github.com/ruistola/cooper/ast"
)

// SymbolTable represents a symbol table with parent-child relationships for scoping
type SymbolTable struct {
	parent      *SymbolTable
	vars        map[string]Type
	structTypes map[string]StructType
	funcs       map[string]FuncType
}

// NewSymbolTable creates a new symbol table with optional parent
func NewSymbolTable(parent *SymbolTable) *SymbolTable {
	return &SymbolTable{
		parent:      parent,
		vars:        make(map[string]Type),
		structTypes: make(map[string]StructType),
		funcs:       make(map[string]FuncType),
	}
}

// DefineVar adds a variable to the current scope
func (st *SymbolTable) DefineVar(name string, varType Type) {
	st.vars[name] = varType
}

// LookupVarType looks up a variable type, checking parent scopes if not found
func (st *SymbolTable) LookupVarType(name string) (Type, bool) {
	if varType, ok := st.vars[name]; ok {
		return varType, true
	}
	if st.parent != nil {
		return st.parent.LookupVarType(name)
	}
	return nil, false
}

// DefineStructType adds a struct type to the current scope
func (st *SymbolTable) DefineStructType(name string, structType StructType) {
	st.structTypes[name] = structType
}

// LookupStructType looks up a struct type, checking parent scopes if not found
func (st *SymbolTable) LookupStructType(name string) (StructType, bool) {
	if structType, ok := st.structTypes[name]; ok {
		return structType, true
	}
	if st.parent != nil {
		return st.parent.LookupStructType(name)
	}
	return StructType{}, false
}

// DefineFunc adds a function to the current scope
func (st *SymbolTable) DefineFunc(name string, funcType FuncType) {
	st.funcs[name] = funcType
}

// LookupFunc looks up a function, checking parent scopes if not found
func (st *SymbolTable) LookupFunc(name string) (FuncType, bool) {
	if funcType, ok := st.funcs[name]; ok {
		return funcType, true
	}
	if st.parent != nil {
		return st.parent.LookupFunc(name)
	}
	return FuncType{}, false
}

// ResolvedModule represents the result of symbol resolution
type ResolvedModule struct {
	SymbolTable *SymbolTable
	Errors      []string
}

// Resolver handles symbol resolution and builds symbol tables
type Resolver struct {
	errors      []string
	symbolTable *SymbolTable
	primitives  map[string]Type
}

// NewResolver creates a new resolver with built-in primitive types
func NewResolver() *Resolver {
	return &Resolver{
		errors:      []string{},
		symbolTable: NewSymbolTable(nil),
		primitives: map[string]Type{
			"bool":   PrimitiveType{Name: "bool"},
			"string": PrimitiveType{Name: "string"},
			"i8":     PrimitiveType{Name: "i8"},
			"i32":    PrimitiveType{Name: "i32"},
			"i64":    PrimitiveType{Name: "i64"},
			"f32":    PrimitiveType{Name: "f32"},
			"f64":    PrimitiveType{Name: "f64"},
		},
	}
}

// Err adds an error to the resolver's error list
func (r *Resolver) Err(msg string) {
	coloredMsg := fmt.Sprintf("\033[31mResolve Error: %s\033[0m", msg)
	r.errors = append(r.errors, coloredMsg)
}

// ResolveType converts an AST type expression to a concrete Type
func (r *Resolver) ResolveType(typeExpr ast.TypeExpr) Type {
	switch e := typeExpr.(type) {
	case ast.NamedTypeExpr:
		if prim, ok := r.primitives[e.TypeName]; ok {
			return prim
		}
		if structType, ok := r.symbolTable.LookupStructType(e.TypeName); ok {
			return structType
		}
		r.Err(fmt.Sprintf("undefined type: %s", e.TypeName))
		return nil
	case ast.ArrayTypeExpr:
		elemType := r.ResolveType(e.UnderlyingType)
		if elemType == nil {
			return nil
		}
		return ArrayType{ElemType: elemType}
	case ast.FuncTypeExpr:
		paramTypes := []Type{}
		for _, astParamType := range e.ParamTypes {
			paramType := r.ResolveType(astParamType)
			if paramType == nil {
				continue
			}
			paramTypes = append(paramTypes, paramType)
		}
		returnType := r.ResolveType(e.ReturnType)
		if returnType == nil {
			return nil
		}
		return FuncType{
			ReturnType: returnType,
			ParamTypes: paramTypes,
		}
	default:
		r.Err(fmt.Sprintf("unknown type: %T", typeExpr))
		return nil
	}
}

// Resolve performs symbol resolution on the module
func Resolve(module ast.BlockStmt) *ResolvedModule {
	resolver := NewResolver()
	resolver.resolveBlockStmt(module)
	return &ResolvedModule{
		SymbolTable: resolver.symbolTable,
		Errors:      resolver.errors,
	}
}

// resolveStmt resolves symbols in a statement
func (r *Resolver) resolveStmt(stmt ast.Stmt) {
	switch s := stmt.(type) {
	case ast.BlockStmt:
		r.resolveBlockStmt(s)
	case ast.VarDeclStmt:
		r.resolveVarDeclStmt(s)
	case ast.StructDeclStmt:
		r.resolveStructDeclStmt(s)
	case ast.FuncDeclStmt:
		r.resolveFuncDeclStmt(s)
	case ast.IfStmt:
		r.resolveIfStmt(s)
	case ast.ForStmt:
		r.resolveForStmt(s)
	case ast.ReturnStmt:
		r.resolveReturnStmt(s)
	case ast.ExpressionStmt:
		r.resolveExpr(s.Expr)
	default:
		r.Err(fmt.Sprintf("unknown statement type: %T", stmt))
	}
}

// resolveBlockStmt resolves symbols in a block statement
func (r *Resolver) resolveBlockStmt(block ast.BlockStmt) {
	oldTable := r.symbolTable
	r.symbolTable = NewSymbolTable(oldTable)
	for _, stmt := range block.Statements {
		r.resolveStmt(stmt)
	}
	r.symbolTable = oldTable
}

// resolveVarDeclStmt resolves a variable declaration
func (r *Resolver) resolveVarDeclStmt(stmt ast.VarDeclStmt) {
	declaredType := r.ResolveType(stmt.Var.Type)
	if declaredType == nil {
		return
	}
	if stmt.InitVal != nil {
		r.resolveExpr(stmt.InitVal)
	}
	r.symbolTable.DefineVar(stmt.Var.Name, declaredType)
}

// resolveStructDeclStmt resolves a struct declaration
func (r *Resolver) resolveStructDeclStmt(stmt ast.StructDeclStmt) {
	if _, ok := r.symbolTable.LookupStructType(stmt.Name); ok {
		r.Err(fmt.Sprintf("redeclared struct %s in the same scope", stmt.Name))
		return
	}
	members := make(map[string]Type)
	memberNames := make(map[string]bool)
	for _, member := range stmt.Members {
		if memberNames[member.Name] {
			r.Err(fmt.Sprintf("duplicate member %s in struct %s", member.Name, stmt.Name))
			continue
		}
		memberType := r.ResolveType(member.Type)
		if memberType != nil {
			members[member.Name] = memberType
			memberNames[member.Name] = true
		}
	}
	r.symbolTable.DefineStructType(stmt.Name, StructType{
		Name:    stmt.Name,
		Members: members,
	})
}

// resolveFuncDeclStmt resolves a function declaration
func (r *Resolver) resolveFuncDeclStmt(stmt ast.FuncDeclStmt) {
	if _, ok := r.symbolTable.LookupFunc(stmt.Name); ok {
		r.Err(fmt.Sprintf("redeclared function %s in the same scope", stmt.Name))
		return
	}

	var returnType Type = UnitType{}
	if stmt.ReturnType != nil {
		returnType = r.ResolveType(stmt.ReturnType)
		if returnType == nil {
			return
		}
	}

	paramTypes := make([]Type, 0, len(stmt.Parameters))
	funcBodyTable := NewSymbolTable(r.symbolTable)

	for _, param := range stmt.Parameters {
		paramType := r.ResolveType(param.Type)
		if paramType == nil {
			return
		}
		paramTypes = append(paramTypes, paramType)
		funcBodyTable.DefineVar(param.Name, paramType)
	}

	funcType := FuncType{
		ReturnType: returnType,
		ParamTypes: paramTypes,
	}
	r.symbolTable.DefineFunc(stmt.Name, funcType)

	// Resolve function body in new scope
	oldTable := r.symbolTable
	r.symbolTable = funcBodyTable
	r.resolveBlockStmt(stmt.Body)
	r.symbolTable = oldTable
}

// resolveIfStmt resolves an if statement
func (r *Resolver) resolveIfStmt(stmt ast.IfStmt) {
	r.resolveExpr(stmt.Cond)
	r.resolveStmt(stmt.Then)
	if stmt.Else != nil {
		r.resolveStmt(stmt.Else)
	}
}

// resolveForStmt resolves a for statement
func (r *Resolver) resolveForStmt(stmt ast.ForStmt) {
	r.resolveStmt(stmt.Init)
	r.resolveExpr(stmt.Cond)
	r.resolveStmt(stmt.Iter)
	r.resolveBlockStmt(stmt.Body)
}

// resolveReturnStmt resolves a return statement
func (r *Resolver) resolveReturnStmt(stmt ast.ReturnStmt) {
	if stmt.Expr != nil {
		r.resolveExpr(stmt.Expr)
	}
}

// resolveExpr resolves symbols in an expression
func (r *Resolver) resolveExpr(expr ast.Expr) {
	switch e := expr.(type) {
	case ast.NumberLiteralExpr, ast.StringLiteralExpr, ast.BoolLiteralExpr:
		// Literals don't need resolution
	case ast.IdentExpr:
		// Check if identifier exists in symbol table
		if _, ok := r.symbolTable.LookupVarType(e.Value); !ok {
			if _, ok := r.symbolTable.LookupStructType(e.Value); !ok {
				if _, ok := r.symbolTable.LookupFunc(e.Value); !ok {
					r.Err(fmt.Sprintf("undefined identifier: %s", e.Value))
				}
			}
		}
	case ast.BinaryExpr:
		r.resolveExpr(e.Lhs)
		r.resolveExpr(e.Rhs)
	case ast.UnaryExpr:
		r.resolveExpr(e.Rhs)
	case ast.GroupExpr:
		r.resolveExpr(e.Expr)
	case ast.FuncCallExpr:
		r.resolveExpr(e.Func)
		for _, arg := range e.Args {
			r.resolveExpr(arg)
		}
	case ast.StructLiteralExpr:
		r.resolveExpr(e.Struct)
		for _, member := range e.Members {
			r.resolveExpr(member.Value)
		}
	case ast.StructMemberExpr:
		r.resolveExpr(e.Struct)
	case ast.ArrayIndexExpr:
		r.resolveExpr(e.Array)
		r.resolveExpr(e.Index)
	case ast.AssignExpr:
		r.resolveExpr(e.Assigne)
		r.resolveExpr(e.AssignedValue)
	case ast.VarDeclAssignExpr:
		r.resolveExpr(e.AssignedValue)
		// Define the variable in current scope (type inference will happen in type checker)
		// For now, we can't determine the type without the type checker
	default:
		r.Err(fmt.Sprintf("unknown expression type: %T", expr))
	}
}
