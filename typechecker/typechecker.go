package typechecker

import (
	"fmt"
	"github.com/ruistola/cooper/ast"
	"github.com/ruistola/cooper/lexer"
)

type TypeChecker struct {
	Errors                []string
	currScope             *Scope         // Current scope during traversal
	scopes                map[any]*Scope // AST nodes to their scopes (from resolver)
	primitives            map[string]Type
	currentFuncReturnType Type
	// expectedType is the type an expression is being checked against, when known
	// from immediate context (an annotated `let` initializer or a `return` value).
	// It supplies the type arguments a generic variant construction cannot infer
	// from its payload alone (e.g. `Result.Ok(5)` needs the E from the context, and
	// `Maybe.None` needs both). It applies only to the checked expression's head and
	// is consumed (cleared) before checking sub-expressions.
	expectedType Type
}

func NewTypeChecker(rootScope *Scope, scopes map[any]*Scope) *TypeChecker {
	return &TypeChecker{
		Errors:    []string{},
		currScope: rootScope,
		scopes:    scopes,
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
	case *ast.OneofDeclStmt:
		tc.CheckOneofDeclStmt(s)
	case *ast.FuncDeclStmt:
		tc.CheckFuncDeclStmt(s)
	case *ast.IfStmt:
		tc.CheckIfStmt(s)
	case *ast.MatchStmt:
		tc.CheckMatchStmt(s)
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
	oldTable := tc.currScope
	tc.currScope = NewScope(oldTable)
	for _, stmt := range block.Statements {
		tc.CheckStmt(stmt)
	}
	tc.currScope = oldTable
}

func (tc *TypeChecker) CheckVarDeclStmt(stmt *ast.VarDeclStmt) {
	declaredType, ok := tc.currScope.LookupVarType(stmt.Var.Name)
	if !ok {
		tc.Err(fmt.Sprintf("unknown variable: %s", stmt.Var.Name))
		return
	}
	if stmt.InitVal != nil {
		tc.expectedType = declaredType
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
	if _, ok := tc.currScope.LookupStructType(stmt.Name); !ok {
		tc.Err(fmt.Sprintf("unknown struct: %s", stmt.Name))
		return
	}
	// Additional type checking for struct members can be added here if needed
}

// CheckOneofDeclStmt verifies the sum type was registered during resolution.
func (tc *TypeChecker) CheckOneofDeclStmt(stmt *ast.OneofDeclStmt) {
	if _, ok := tc.currScope.LookupOneofType(stmt.Name); !ok {
		tc.Err(fmt.Sprintf("unknown sum type: %s", stmt.Name))
	}
}

func (tc *TypeChecker) CheckFuncDeclStmt(stmt *ast.FuncDeclStmt) {
	var funcType FuncType
	var ok bool
	if stmt.Receiver != nil {
		funcType, ok = tc.currScope.LookupMethod(namedTypeName(stmt.Receiver.Type), stmt.Name)
	} else {
		funcType, ok = tc.currScope.LookupFunc(stmt.Name)
	}
	if !ok {
		tc.Err(fmt.Sprintf("unknown function: %s", stmt.Name))
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
	oldTable := tc.currScope
	tc.currScope = funcScope

	// Type check function body statements directly in function scope
	for _, bodyStmt := range stmt.Body.Statements {
		tc.CheckStmt(bodyStmt)
	}

	// Restore previous context
	tc.currentFuncReturnType = oldReturnType
	tc.currScope = oldTable
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

// checkMatchArmPattern validates a single arm pattern against the scrutinee's sum
// type and binds its payload slots in the current scope. It reports the covered
// variant name (empty for a wildcard) and whether the pattern is the catch-all `_`.
func (tc *TypeChecker) checkMatchArmPattern(oneof OneofType, pattern ast.Pattern) (string, bool) {
	switch p := pattern.(type) {
	case *ast.WildcardPattern:
		return "", true
	case *ast.VariantPattern:
		if p.TypeName != oneof.Name {
			tc.Err(fmt.Sprintf("pattern names sum type %s but the scrutinee has type %s", p.TypeName, oneof.Name))
			return "", false
		}
		payload, ok := oneof.Variants[p.Variant]
		if !ok {
			tc.Err(fmt.Sprintf("%s is not a variant of sum type %s", p.Variant, oneof.Name))
			return "", false
		}
		if len(p.Binders) != len(payload) {
			tc.Err(fmt.Sprintf("variant pattern %s.%s binds %d slot(s) but the variant has %d", oneof.Name, p.Variant, len(p.Binders), len(payload)))
			return p.Variant, false
		}
		for i, binder := range p.Binders {
			if binder != "_" {
				tc.currScope.DefineVar(binder, payload[i])
			}
		}
		return p.Variant, false
	default:
		tc.Err(fmt.Sprintf("unsupported pattern type: %T", pattern))
		return "", false
	}
}

// checkMatchExhaustiveness reports an error unless the arms cover every variant of
// the sum type, either explicitly or through a trailing wildcard `_`.
func (tc *TypeChecker) checkMatchExhaustiveness(oneof OneofType, covered map[string]bool, hasWildcard bool) {
	if hasWildcard {
		return
	}
	missing := []string{}
	for _, variant := range oneof.VariantOrder {
		if !covered[variant] {
			missing = append(missing, variant)
		}
	}
	if len(missing) > 0 {
		tc.Err(fmt.Sprintf("non-exhaustive match on sum type %s: missing variant(s) %v", oneof.Name, missing))
	}
}

// CheckMatchStmt type checks a statement-position match. The scrutinee must be a
// sum type; each arm pattern is validated and its binders scoped to that arm's
// body, and the arms must exhaustively cover the sum type. Arm bodies are
// statements whose values are discarded.
func (tc *TypeChecker) CheckMatchStmt(stmt *ast.MatchStmt) {
	scrutineeType := tc.CheckExpr(stmt.Scrutinee)
	if scrutineeType == nil {
		return
	}
	oneof, ok := scrutineeType.(OneofType)
	if !ok {
		tc.Err(fmt.Sprintf("cannot match on non-sum-type value of type %s", scrutineeType))
		return
	}
	covered := make(map[string]bool)
	hasWildcard := false
	for _, arm := range stmt.Arms {
		oldScope := tc.currScope
		tc.currScope = NewScope(oldScope)
		variant, wildcard := tc.checkMatchArmPattern(oneof, arm.Pattern)
		if wildcard {
			hasWildcard = true
		} else if variant != "" {
			covered[variant] = true
		}
		tc.CheckStmt(arm.Body)
		tc.currScope = oldScope
	}
	tc.checkMatchExhaustiveness(oneof, covered, hasWildcard)
}

// CheckMatchExpr type checks an expression-position match. Beyond the rules of the
// statement form, every arm body is an expression and their types must unify: the
// match's type is that common arm type.
func (tc *TypeChecker) CheckMatchExpr(expr *ast.MatchExpr) Type {
	scrutineeType := tc.CheckExpr(expr.Scrutinee)
	if scrutineeType == nil {
		return nil
	}
	oneof, ok := scrutineeType.(OneofType)
	if !ok {
		tc.Err(fmt.Sprintf("cannot match on non-sum-type value of type %s", scrutineeType))
		return nil
	}
	covered := make(map[string]bool)
	hasWildcard := false
	var matchType Type
	for _, arm := range expr.Arms {
		oldScope := tc.currScope
		tc.currScope = NewScope(oldScope)
		variant, wildcard := tc.checkMatchArmPattern(oneof, arm.Pattern)
		if wildcard {
			hasWildcard = true
		} else if variant != "" {
			covered[variant] = true
		}
		armType := tc.CheckExpr(arm.Body)
		tc.currScope = oldScope
		if armType == nil {
			continue
		}
		if matchType == nil {
			matchType = armType
		} else if !matchType.Equals(armType) {
			tc.Err(fmt.Sprintf("match arms have mismatched types: %s and %s", matchType, armType))
		}
	}
	tc.checkMatchExhaustiveness(oneof, covered, hasWildcard)
	if matchType == nil {
		return UnitType{}
	}
	return matchType
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
	tc.expectedType = tc.currentFuncReturnType
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
	// expectedType is a head-only hint: capture it for this expression and clear
	// it so it never leaks into sub-expressions. Only the dispatches that can act
	// on it (variant access and construction) restore it before recursing.
	expected := tc.expectedType
	tc.expectedType = nil
	switch e := expr.(type) {
	case *ast.NumberLiteralExpr:
		return tc.primitives["i32"] // todo; evaluate the number literal to determine exact type
	case *ast.StringLiteralExpr:
		return tc.primitives["string"]
	case *ast.BoolLiteralExpr:
		return tc.primitives["bool"]
	case *ast.NilLiteralExpr:
		return NilType{}
	case *ast.UnitExpr:
		return UnitType{}
	case *ast.TupleLiteralExpr:
		return tc.CheckTupleLiteralExpr(e)
	case *ast.IdentExpr:
		if varType, ok := tc.currScope.LookupVarType(e.Value); ok {
			return varType
		}
		if structType, ok := tc.currScope.LookupStructType(e.Value); ok {
			return structType
		}
		if oneofType, ok := tc.currScope.LookupOneofType(e.Value); ok {
			return oneofType
		}
		if funcType, ok := tc.currScope.LookupFunc(e.Value); ok {
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
		tc.expectedType = expected
		return tc.CheckFuncCallExpr(e)
	case *ast.StructLiteralExpr:
		return tc.CheckStructLiteralExpr(e)
	case *ast.StructMemberExpr:
		tc.expectedType = expected
		return tc.CheckStructMemberExpr(e)
	case *ast.ArrayIndexExpr:
		return tc.CheckArrayIndexExpr(e)
	case *ast.AddressOfExpr:
		return tc.CheckAddressOfExpr(e)
	case *ast.DerefExpr:
		return tc.CheckDerefExpr(e)
	case *ast.AssignExpr:
		return tc.CheckAssignExpr(e)
	case *ast.VarDeclAssignExpr:
		return tc.CheckVarDeclAssignExpr(e)
	case *ast.TupleDeclAssignExpr:
		return tc.CheckTupleDeclAssignExpr(e)
	case *ast.MatchExpr:
		return tc.CheckMatchExpr(e)
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
	// A call whose callee is a variant access (`Maybe.Some(5)`) is sum-type
	// construction, not an ordinary function call: its type arguments are inferred
	// by unifying the payload slots against the arguments.
	if member, ok := expr.Func.(*ast.StructMemberExpr); ok {
		expected := tc.expectedType
		if oneofType, ok := tc.CheckExpr(member.Struct).(OneofType); ok {
			tc.expectedType = expected
			return tc.checkVariantConstruction(oneofType, member.Member.Value, expr.Args)
		}
	}
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

// checkVariantAccess type checks a bare variant access `Oneof.Variant`. A
// payload-free variant is a complete value of the sum type; a variant with a
// payload yields its constructor as a function type (its payload slots as
// parameters, the sum type as the result). For a generic sum type, a payload-free
// variant cannot have its type arguments inferred here, so its arguments must be
// determinable — until bidirectional checking lands, that means only non-generic
// payload-free variants type check standalone.
func (tc *TypeChecker) checkVariantAccess(oneof OneofType, variantName string) Type {
	expected := tc.expectedType
	tc.expectedType = nil
	payload, ok := oneof.Variants[variantName]
	if !ok {
		tc.Err(fmt.Sprintf("%s is not a variant of sum type %s", variantName, oneof.Name))
		return nil
	}
	if len(payload) == 0 {
		if len(oneof.TypeParams) > 0 {
			subst := make(map[string]Type)
			if !tc.seedSubstFromExpected(oneof, expected, subst) {
				tc.Err(fmt.Sprintf("cannot infer type arguments for payload-free variant %s.%s; an explicit annotation is required", oneof.Name, variantName))
				return nil
			}
			return tc.instantiateOneof(oneof, subst)
		}
		return oneof
	}
	return FuncType{ReturnType: oneof, ParamTypes: payload}
}

// checkVariantConstruction type checks a variant construction `Oneof.Variant(args)`.
// The argument count must match the variant's payload arity; for a generic sum type
// the type arguments are inferred by unifying each payload slot against the
// corresponding argument, and every parameter must be determined.
func (tc *TypeChecker) checkVariantConstruction(oneof OneofType, variantName string, args []ast.Expr) Type {
	expected := tc.expectedType
	tc.expectedType = nil
	payload, ok := oneof.Variants[variantName]
	if !ok {
		tc.Err(fmt.Sprintf("%s is not a variant of sum type %s", variantName, oneof.Name))
		return nil
	}
	if len(args) != len(payload) {
		tc.Err(fmt.Sprintf("variant %s.%s expects %d argument(s), got %d", oneof.Name, variantName, len(payload), len(args)))
		return nil
	}
	subst := make(map[string]Type)
	tc.seedSubstFromExpected(oneof, expected, subst)
	for i, arg := range args {
		argType := tc.CheckExpr(arg)
		if argType == nil {
			return nil
		}
		if !unify(payload[i], argType, subst) {
			tc.Err(fmt.Sprintf("argument %d to variant %s.%s type mismatch: expected %s, found %s", i+1, oneof.Name, variantName, substitute(payload[i], subst), argType))
			return nil
		}
	}
	if len(oneof.TypeParams) == 0 {
		return oneof
	}
	return tc.instantiateOneof(oneof, subst)
}

// seedSubstFromExpected fills a substitution map with type arguments taken from an
// expected sum-type instantiation, letting a generic variant construction obtain
// the arguments its payload cannot determine (e.g. the E of `Result.Ok(5)` or both
// arguments of `Maybe.None`). It reports true when the expected type is a matching
// instantiation of the same generic sum type (same name and parameter arity) that
// carries concrete type arguments, populating one entry per type parameter.
func (tc *TypeChecker) seedSubstFromExpected(oneof OneofType, expected Type, subst map[string]Type) bool {
	exp, ok := expected.(OneofType)
	if !ok {
		return false
	}
	if exp.Name != oneof.Name || len(exp.TypeArgs) != len(oneof.TypeParams) {
		return false
	}
	for i, param := range oneof.TypeParams {
		subst[param] = exp.TypeArgs[i]
	}
	return true
}

// instantiateOneof builds a concrete instantiation of a generic sum type from an
// inferred substitution, requiring every type parameter to be determined and
// substituting each variant's payload slots. On an underdetermined parameter it
// reports an error and returns nil.
func (tc *TypeChecker) instantiateOneof(oneof OneofType, subst map[string]Type) Type {
	typeArgs := make([]Type, 0, len(oneof.TypeParams))
	for _, param := range oneof.TypeParams {
		arg, ok := subst[param]
		if !ok {
			tc.Err(fmt.Sprintf("cannot infer type argument %s for sum type %s", param, oneof.Name))
			return nil
		}
		typeArgs = append(typeArgs, arg)
	}
	variants := make(map[string][]Type, len(oneof.Variants))
	for name, slots := range oneof.Variants {
		substituted := make([]Type, len(slots))
		for i, slot := range slots {
			substituted[i] = substitute(slot, subst)
		}
		variants[name] = substituted
	}
	return OneofType{
		Name:         oneof.Name,
		Variants:     variants,
		VariantOrder: oneof.VariantOrder,
		TypeParams:   oneof.TypeParams,
		TypeArgs:     typeArgs,
	}
}

func (tc *TypeChecker) CheckStructLiteralExpr(expr *ast.StructLiteralExpr) Type {
	var structType StructType
	structTypeValue := tc.CheckExpr(expr.Struct)
	structType, ok := structTypeValue.(StructType)
	if !ok {
		tc.Err(fmt.Sprintf("expression of type %s cannot be used as a struct", structTypeValue))
		return nil
	}
	// A generic struct is constructed against its template (type parameters
	// unbound); each member assignment constrains the type arguments, which are
	// inferred by unifying the member's template type against the assigned value.
	isGeneric := len(structType.TypeParams) > 0 && len(structType.TypeArgs) == 0
	subst := make(map[string]Type)
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
		if isGeneric {
			if !unify(assigneType, assignedValueType, subst) {
				tc.Err(fmt.Sprintf("cannot assign %s to member %s of generic struct %s", assignedValueType, member.Name, structType.Name))
				continue
			}
		} else if !assigneType.Equals(assignedValueType) {
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
	if isGeneric {
		return tc.instantiateFromSubst(structType, subst)
	}
	return structType
}

// instantiateFromSubst builds a concrete instantiation of a generic struct
// template from an inferred parameter substitution, requiring every type
// parameter to have been determined by the member assignments. On an
// underdetermined parameter it reports an error and returns the template unchanged.
func (tc *TypeChecker) instantiateFromSubst(template StructType, subst map[string]Type) Type {
	typeArgs := make([]Type, 0, len(template.TypeParams))
	for _, param := range template.TypeParams {
		arg, ok := subst[param]
		if !ok {
			tc.Err(fmt.Sprintf("cannot infer type argument %s for generic struct %s", param, template.Name))
			return template
		}
		typeArgs = append(typeArgs, arg)
	}
	members := make(map[string]Type, len(template.Members))
	for name, m := range template.Members {
		members[name] = substitute(m, subst)
	}
	return StructType{
		Name:       template.Name,
		Members:    members,
		TypeParams: template.TypeParams,
		TypeArgs:   typeArgs,
	}
}

func (tc *TypeChecker) CheckStructMemberExpr(expr *ast.StructMemberExpr) Type {
	expected := tc.expectedType
	structTypeValue := tc.CheckExpr(expr.Struct)
	// A member access whose base is a sum type names a variant constructor, e.g.
	// `Maybe.Some` or `Maybe.None`. A payload-free variant is a complete value
	// here; a variant with payload is constructed via a call (handled in
	// CheckFuncCallExpr), so accessing it uncalled yields its constructor signature.
	if oneofType, ok := structTypeValue.(OneofType); ok {
		tc.expectedType = expected
		return tc.checkVariantAccess(oneofType, expr.Member.Value)
	}
	// Auto-deref: `p.field` and `p.method()` transparently work through a pointer
	// to a struct, so member access never requires an explicit `p^.field`.
	if ptr, ok := structTypeValue.(PointerType); ok {
		structTypeValue = ptr.ElemType
	}
	structType, ok := structTypeValue.(StructType)
	if !ok {
		tc.Err(fmt.Sprintf("expression of type %s cannot be used as a struct", structTypeValue))
		return nil
	}
	memberType, ok := structType.Members[expr.Member.Value]
	if !ok {
		// Not a data field: fall back to a method on this struct type. A bound
		// method has its receiver stripped from the signature (the receiver
		// becomes the captured environment), so it is usable anywhere a matching
		// function type is expected, and `x.method(args)` type checks through the
		// ordinary call path.
		if methodType, ok := tc.currScope.LookupMethod(structType.Name, expr.Member.Value); ok {
			return methodType
		}
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

// CheckAddressOfExpr type checks the prefix `&` operator. The operand must be
// addressable (a variable, struct field, array element, or dereference), and the
// result is a pointer to the operand's type.
func (tc *TypeChecker) CheckAddressOfExpr(expr *ast.AddressOfExpr) Type {
	if !tc.isAddressable(expr.Operand) {
		tc.Err("cannot take the address of a non-addressable expression")
		return nil
	}
	operandType := tc.CheckExpr(expr.Operand)
	if operandType == nil {
		return nil
	}
	return PointerType{ElemType: operandType}
}

// CheckDerefExpr type checks the postfix `^` operator. The operand must be a
// pointer, and the result is the pointed-to element type.
func (tc *TypeChecker) CheckDerefExpr(expr *ast.DerefExpr) Type {
	operandType := tc.CheckExpr(expr.Operand)
	if operandType == nil {
		return nil
	}
	ptrType, ok := operandType.(PointerType)
	if !ok {
		tc.Err(fmt.Sprintf("cannot dereference non-pointer type %s", operandType))
		return nil
	}
	return ptrType.ElemType
}

// CheckTupleLiteralExpr type checks a tuple value `(a, b, ...)`, producing a
// TupleType from the element types. A nil element type propagates as a failure.
func (tc *TypeChecker) CheckTupleLiteralExpr(expr *ast.TupleLiteralExpr) Type {
	elemTypes := make([]Type, 0, len(expr.Elements))
	for _, elem := range expr.Elements {
		elemType := tc.CheckExpr(elem)
		if elemType == nil {
			return nil
		}
		elemTypes = append(elemTypes, elemType)
	}
	return TupleType{ElementTypes: elemTypes}
}

// isAddressable reports whether an expression denotes a storage location whose
// address can be taken. Temporaries (literals, call results, arithmetic) are not
// addressable.
func (tc *TypeChecker) isAddressable(expr ast.Expr) bool {
	switch e := expr.(type) {
	case *ast.IdentExpr:
		_, ok := tc.currScope.LookupVarType(e.Value)
		return ok
	case *ast.StructMemberExpr, *ast.ArrayIndexExpr, *ast.DerefExpr:
		return true
	case *ast.GroupExpr:
		return tc.isAddressable(e.Expr)
	default:
		return false
	}
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
	tc.currScope.DefineVar(expr.Name, assignedValueType)
	return assignedValueType
}

// CheckTupleDeclAssignExpr type checks a tuple-destructuring binding. The
// initializer must be a tuple whose arity matches the pattern. When the `let` form
// carries a type annotation, each element is checked against the declared type the
// resolver bound; otherwise each name is inferred from the corresponding element.
func (tc *TypeChecker) CheckTupleDeclAssignExpr(expr *ast.TupleDeclAssignExpr) Type {
	rhsType := tc.CheckExpr(expr.AssignedValue)
	if rhsType == nil {
		return nil
	}
	tupleType, ok := rhsType.(TupleType)
	if !ok {
		tc.Err(fmt.Sprintf("cannot destructure non-tuple value of type %s", rhsType))
		return nil
	}
	if len(tupleType.ElementTypes) != len(expr.Names) {
		tc.Err(fmt.Sprintf("destructuring pattern binds %d names but value has %d elements", len(expr.Names), len(tupleType.ElementTypes)))
		return nil
	}
	for i, name := range expr.Names {
		elemType := tupleType.ElementTypes[i]
		if expr.Type != nil {
			declaredType, ok := tc.currScope.LookupVarType(name)
			if ok && !declaredType.Equals(elemType) {
				tc.Err(fmt.Sprintf("type mismatch: %s declared as %s but bound to %s", name, declaredType, elemType))
			}
		} else {
			tc.currScope.DefineVar(name, elemType)
		}
	}
	return rhsType
}
