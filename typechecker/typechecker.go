package typechecker

import (
	"fmt"
	"github.com/ruistola/cooper/ast"
	"github.com/ruistola/cooper/lexer"
)

type TypeChecker struct {
	Errors                []string
	symbolTable           *SymbolTable         // Current scope during traversal
	scopes                map[any]*SymbolTable // AST nodes to their scopes (from resolver)
	primitives            map[string]Type
	currentFuncReturnType Type
}

func NewTypeChecker(symbolTable *SymbolTable, scopes map[any]*SymbolTable) *TypeChecker {
	return &TypeChecker{
		Errors:      []string{},
		symbolTable: symbolTable,
		scopes:      scopes,
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

func (tc *TypeChecker) Err(msg string) {
	coloredMsg := fmt.Sprintf("\033[31mType Error: %s\033[0m", msg)
	tc.Errors = append(tc.Errors, coloredMsg)
}

func Check(module *ast.BlockStmt) []string {
	// First pass: Resolve symbols
	resolved := Resolve(module)
	allErrors := resolved.Errors

	// Second pass: Type checking
	if len(resolved.Errors) == 0 {
		tc := NewTypeChecker(resolved.RootScope, resolved.Scopes)
		// Process module statements directly in root scope
		for _, stmt := range module.Statements {
			tc.CheckStmt(stmt)
		}
		allErrors = append(allErrors, tc.Errors...)

		// Third pass: Semantic analysis (only if type checking passed)
		if len(tc.Errors) == 0 {
			semanticErrors := AnalyzeSemantics(module, resolved.RootScope)
			allErrors = append(allErrors, semanticErrors...)
		}
	}

	return allErrors
}

func (tc *TypeChecker) CheckStmt(stmt ast.Stmt) {
	switch s := stmt.(type) {
	case *ast.BlockStmt:
		tc.CheckBlockStmt(s)
	case *ast.VarDeclStmt:
		tc.CheckVarDeclStmt(s)
	case *ast.StructDeclStmt:
		tc.CheckStructDeclStmt(s)
	case *ast.FuncDeclStmt:
		tc.CheckFuncDeclStmt(s)
	case *ast.IfStmt:
		tc.CheckIfStmt(s)
	case *ast.ForStmt:
		tc.CheckForStmt(s)
	case *ast.ReturnStmt:
		tc.CheckReturnStmt(s)
	case *ast.ExpressionStmt:
		tc.CheckExpr(s.Expr)
	default:
		tc.Err(fmt.Sprintf("unknown statement type: %T", stmt))
	}
}

func (tc *TypeChecker) CheckBlockStmt(block *ast.BlockStmt) {
	oldTable := tc.symbolTable
	tc.symbolTable = NewSymbolTable(oldTable)
	for _, stmt := range block.Statements {
		tc.CheckStmt(stmt)
	}
	tc.symbolTable = oldTable
}

func (tc *TypeChecker) CheckVarDeclStmt(stmt *ast.VarDeclStmt) {
	// Type already resolved by resolver, just get it from symbol table
	declaredType, ok := tc.symbolTable.LookupVarType(stmt.Var.Name)
	if !ok {
		tc.Err(fmt.Sprintf("variable %s not found in symbol table", stmt.Var.Name))
		return
	}
	if stmt.InitVal != nil {
		initType := tc.CheckExpr(stmt.InitVal)
		if initType == nil {
			return
		}
		if !declaredType.Equals(initType) {
			tc.Err(fmt.Sprintf("type mismatch: variable %s declared as %s but initialized with %s", stmt.Var.Name, declaredType, initType))
		}
	}
}

func (tc *TypeChecker) CheckStructDeclStmt(stmt *ast.StructDeclStmt) {
	// Struct already declared by resolver, just verify it exists
	if _, ok := tc.symbolTable.LookupStructType(stmt.Name); !ok {
		tc.Err(fmt.Sprintf("struct %s not found in symbol table", stmt.Name))
		return
	}
	// Additional type checking for struct members can be added here if needed
}

func (tc *TypeChecker) CheckFuncDeclStmt(stmt *ast.FuncDeclStmt) {
	// Function already declared by resolver, get its type
	funcType, ok := tc.symbolTable.LookupFunc(stmt.Name)
	if !ok {
		tc.Err(fmt.Sprintf("function %s not found in symbol table", stmt.Name))
		return
	}

	// Get the function scope from the resolver's scope map using statement pointer
	funcScope, ok := tc.scopes[stmt]
	if !ok {
		tc.Err(fmt.Sprintf("function %s scope not found in scope map", stmt.Name))
		return
	}

	// Set up function context
	oldReturnType := tc.currentFuncReturnType
	tc.currentFuncReturnType = funcType.ReturnType
	oldTable := tc.symbolTable
	tc.symbolTable = funcScope

	// Type check function body statements directly in function scope
	for _, bodyStmt := range stmt.Body.Statements {
		tc.CheckStmt(bodyStmt)
	}

	// Restore previous context
	tc.currentFuncReturnType = oldReturnType
	tc.symbolTable = oldTable
}

func (tc *TypeChecker) CheckIfStmt(stmt *ast.IfStmt) {
	condType := tc.CheckExpr(stmt.Cond)
	if !IsPrimitive(condType, "bool") {
		tc.Err("if- statement condition does not evaluate to a boolean type")
	}
	tc.CheckStmt(stmt.Then)
	if stmt.Else != nil {
		tc.CheckStmt(stmt.Else)
	}
}

func (tc *TypeChecker) CheckForStmt(stmt *ast.ForStmt) {
	tc.CheckStmt(stmt.Init)
	condType := tc.CheckExpr(stmt.Cond)
	if !IsPrimitive(condType, "bool") {
		tc.Err("for- statement condition does not evaluate to a boolean type")
	}
	tc.CheckStmt(stmt.Iter)
	tc.CheckStmt(stmt.Body)
}

func (tc *TypeChecker) CheckReturnStmt(stmt *ast.ReturnStmt) {
	if tc.currentFuncReturnType == nil {
		tc.Err("return statement outside of function")
		return
	}
	isUnitReturn := IsUnit(tc.currentFuncReturnType)
	if stmt.Expr == nil {
		if !isUnitReturn {
			tc.Err(fmt.Sprintf("expected function to return %s", tc.currentFuncReturnType))
		}
		return
	}
	exprType := tc.CheckExpr(stmt.Expr)
	switch {
	case exprType == nil:
		return
	case isUnitReturn:
		tc.Err("cannot return a value from a function with no declared return type")
	case !exprType.Equals(tc.currentFuncReturnType):
		tc.Err(fmt.Sprintf("return type mismatch: expected %s, found %s", tc.currentFuncReturnType, exprType))
	}
}

func (tc *TypeChecker) CheckExpr(expr ast.Expr) Type {
	switch e := expr.(type) {
	case *ast.NumberLiteralExpr:
		return tc.primitives["i32"] // todo; evaluate the number literal to determine exact type
	case *ast.StringLiteralExpr:
		return tc.primitives["string"]
	case *ast.BoolLiteralExpr:
		return tc.primitives["bool"]
	case *ast.IdentExpr:
		if varType, ok := tc.symbolTable.LookupVarType(e.Value); ok {
			return varType
		}
		if structType, ok := tc.symbolTable.LookupStructType(e.Value); ok {
			return structType
		}
		if funcType, ok := tc.symbolTable.LookupFunc(e.Value); ok {
			return funcType
		}
		tc.Err(fmt.Sprintf("undefined variable: %s", e.Value))
		return nil
	case *ast.BinaryExpr:
		return tc.CheckBinaryExpr(e)
	case *ast.UnaryExpr:
		return tc.CheckUnaryExpr(e)
	case *ast.GroupExpr:
		return tc.CheckExpr(e.Expr)
	case *ast.FuncCallExpr:
		return tc.CheckFuncCallExpr(e)
	case *ast.StructLiteralExpr:
		return tc.CheckStructLiteralExpr(e)
	case *ast.StructMemberExpr:
		return tc.CheckStructMemberExpr(e)
	case *ast.ArrayIndexExpr:
		return tc.CheckArrayIndexExpr(e)
	case *ast.AssignExpr:
		return tc.CheckAssignExpr(e)
	case *ast.VarDeclAssignExpr:
		return tc.CheckVarDeclAssignExpr(e)
	default:
		tc.Err(fmt.Sprintf("unknown expression type: %T", expr))
		return nil
	}
}

func (tc *TypeChecker) CheckBinaryExpr(expr *ast.BinaryExpr) Type {
	leftType := tc.CheckExpr(expr.Lhs)
	rightType := tc.CheckExpr(expr.Rhs)
	if leftType == nil || rightType == nil {
		return nil
	}
	switch expr.Operator.Type {
	case lexer.PLUS, lexer.DASH, lexer.STAR, lexer.SLASH, lexer.PERCENT:
		if IsNumeric(leftType) && IsNumeric(rightType) {
			return leftType // no specific reason, just pick one arbitrarily until we have e.g. type promotion (i32 -> f32 etc.)
		}
		if expr.Operator.Type == lexer.PLUS && IsPrimitive(leftType, "string") && IsPrimitive(rightType, "string") {
			return tc.primitives["string"]
		}
		tc.Err(fmt.Sprintf("invalid operands for %s: %s and %s", expr.Operator.Value, leftType, rightType))
		return nil
	case lexer.DOUBLE_EQUALS, lexer.NOT_EQUALS:
		if !leftType.Equals(rightType) {
			tc.Err(fmt.Sprintf("cannot compare %s and %s", leftType, rightType))
			return nil
		}
		return tc.primitives["bool"]
	case lexer.LESS, lexer.LESS_EQUALS, lexer.GREATER, lexer.GREATER_EQUALS:
		if IsNumeric(leftType) && IsNumeric(rightType) {
			return tc.primitives["bool"]
		}
		tc.Err(fmt.Sprintf("invalid operands for %s: %s and %s", expr.Operator.Value, leftType, rightType))
		return nil
	case lexer.OR, lexer.AND:
		if IsPrimitive(leftType, "bool") && IsPrimitive(rightType, "bool") {
			return tc.primitives["bool"]
		}
		tc.Err(fmt.Sprintf("invalid operands for %s: %s and %s", expr.Operator.Value, leftType, rightType))
		return nil
	default:
		tc.Err(fmt.Sprintf("unsupported binary operator: %s", expr.Operator.Value))
		return nil
	}
}

func (tc *TypeChecker) CheckUnaryExpr(expr *ast.UnaryExpr) Type {
	operandType := tc.CheckExpr(expr.Rhs)
	if operandType == nil {
		return nil
	}
	switch expr.Operator.Type {
	case lexer.PLUS, lexer.DASH:
		if IsNumeric(operandType) {
			return operandType
		}
		tc.Err(fmt.Sprintf("invalid operand for %s: %s", expr.Operator.Value, operandType))
		return nil
	case lexer.NOT:
		if IsPrimitive(operandType, "bool") {
			return tc.primitives["bool"]
		}
		tc.Err(fmt.Sprintf("invalid operand for %s: %s", expr.Operator.Value, operandType))
		return nil
	default:
		tc.Err(fmt.Sprintf("unsupported unary operator: %s", expr.Operator.Value))
		return nil
	}
}

func (tc *TypeChecker) CheckFuncCallExpr(expr *ast.FuncCallExpr) Type {
	funcType := tc.CheckExpr(expr.Func)
	if funcType == nil {
		return nil
	}
	ft, ok := funcType.(FuncType)
	if !ok {
		tc.Err(fmt.Sprintf("cannot call non-function value of type %s", funcType))
		return nil
	}
	if len(expr.Args) != len(ft.ParamTypes) {
		tc.Err(fmt.Sprintf("wrong number of arguments, expected %d, found %d", len(ft.ParamTypes), len(expr.Args)))
		return nil
	}
	for i, arg := range expr.Args {
		argType := tc.CheckExpr(arg)
		if argType == nil {
			return nil
		}
		if !ft.ParamTypes[i].Equals(argType) {
			tc.Err(fmt.Sprintf("argument %d type mismatch: expected %s, found %s", i+1, ft.ParamTypes[i], argType))
			return nil
		}
	}
	return ft.ReturnType
}

func (tc *TypeChecker) CheckStructLiteralExpr(expr *ast.StructLiteralExpr) Type {
	var structType StructType
	structTypeValue := tc.CheckExpr(expr.Struct)
	structType, ok := structTypeValue.(StructType)
	if !ok {
		tc.Err(fmt.Sprintf("expression of type %s cannot be used as a struct", structTypeValue))
		return nil
	}
	assignedMembers := make(map[string]bool, len(structType.Members))
	for memberName := range structType.Members {
		assignedMembers[memberName] = false
	}
	for _, member := range expr.Members {
		assigneType, ok := structType.Members[member.Name]
		if !ok {
			tc.Err(fmt.Sprintf("%s is not a member of struct %s", member.Name, structType.Name))
			continue
		}
		if assignedMembers[member.Name] == true {
			tc.Err(fmt.Sprintf("struct member %s assigned multiple times", member.Name))
			continue
		}
		assignedValueType := tc.CheckExpr(member.Value)
		if assignedValueType == nil {
			continue
		}
		if !assigneType.Equals(assignedValueType) {
			tc.Err(fmt.Sprintf("cannot assign %s to %s of struct member %s", assignedValueType, assigneType, member.Name))
			continue
		}
		assignedMembers[member.Name] = true
	}
	for memberName, assigned := range assignedMembers {
		if !assigned {
			tc.Err(fmt.Sprintf("struct member %s is not assigned a value", memberName))
		}
	}
	return structType
}

func (tc *TypeChecker) CheckStructMemberExpr(expr *ast.StructMemberExpr) Type {
	structTypeValue := tc.CheckExpr(expr.Struct)
	structType, ok := structTypeValue.(StructType)
	if !ok {
		tc.Err(fmt.Sprintf("expression of type %s cannot be used as a struct", structTypeValue))
		return nil
	}
	memberType, ok := structType.Members[expr.Member.Value]
	if !ok {
		tc.Err(fmt.Sprintf("%s is not a member of struct %s", expr.Member.Value, structType.Name))
		return nil
	}
	return memberType
}

func (tc *TypeChecker) CheckArrayIndexExpr(expr *ast.ArrayIndexExpr) Type {
	if !IsNumeric(tc.CheckExpr(expr.Index)) {
		tc.Err(fmt.Sprintf("array index expression does not result in a numeric type: %s", expr.Index))
		return nil
	}
	arrayExprType := tc.CheckExpr(expr.Array)
	if arrayExprType == nil {
		return nil
	}
	arrayType, ok := arrayExprType.(ArrayType)
	if !ok {
		tc.Err(fmt.Sprintf("cannot index non-array type %s", arrayType))
		return nil
	}
	return arrayType.ElemType
}

func (tc *TypeChecker) CheckAssignExpr(expr *ast.AssignExpr) Type {
	assigneType := tc.CheckExpr(expr.Assigne)
	assignedValueType := tc.CheckExpr(expr.AssignedValue)
	switch expr.Operator.Type {
	case lexer.EQUALS:
		if !assigneType.Equals(assignedValueType) {
			tc.Err(fmt.Sprintf("cannot assign %s to %s", assignedValueType, assigneType))
		}
	case lexer.PLUS_EQUALS:
		numeric := IsNumeric(assigneType) && IsNumeric(assignedValueType)
		strings := IsPrimitive(assigneType, "string") && IsPrimitive(assignedValueType, "string")
		if !numeric && !strings {
			tc.Err(fmt.Sprintf("invalid operands for %s: %s and %s", expr.Operator.Value, assigneType, assignedValueType))
		}
	case lexer.DASH_EQUALS:
		numeric := IsNumeric(assigneType) && IsNumeric(assignedValueType)
		if !numeric {
			tc.Err(fmt.Sprintf("invalid operands for %s: %s and %s", expr.Operator.Value, assigneType, assignedValueType))
		}
	}
	return assigneType
}

func (tc *TypeChecker) CheckVarDeclAssignExpr(expr *ast.VarDeclAssignExpr) Type {
	assignedValueType := tc.CheckExpr(expr.AssignedValue)
	tc.symbolTable.DefineVar(expr.Name, assignedValueType)
	return assignedValueType
}
