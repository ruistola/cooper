package typechecker

import (
	"fmt"
	"github.com/ruistola/cooper/ast"
)

// Scope represents a lexical scope, with parent being nil if this is the module top scope
type Scope struct {
	parent      *Scope
	vars        map[string]Type
	structTypes map[string]StructType
	oneofTypes  map[string]OneofType
	funcs       map[string]FuncType
	// methods maps a receiver struct type name to that type's method set
	// (method name -> bound signature, i.e. parameters excluding the receiver).
	methods map[string]map[string]FuncType
}

// NewScope creates a new scope with optional parent
func NewScope(parent *Scope) *Scope {
	return &Scope{
		parent:      parent,
		vars:        make(map[string]Type),
		structTypes: make(map[string]StructType),
		oneofTypes:  make(map[string]OneofType),
		funcs:       make(map[string]FuncType),
		methods:     make(map[string]map[string]FuncType),
	}
}

// DefineVar adds a variable to the current scope
func (s *Scope) DefineVar(name string, varType Type) {
	s.vars[name] = varType
}

// LookupVarType looks up a variable type, checking parent scopes if not found
func (s *Scope) LookupVarType(name string) (Type, bool) {
	if varType, ok := s.vars[name]; ok {
		return varType, true
	}
	if s.parent != nil {
		return s.parent.LookupVarType(name)
	}
	return nil, false
}

// DefineStructType adds a struct type to the current scope
func (s *Scope) DefineStructType(name string, structType StructType) {
	s.structTypes[name] = structType
}

// LookupStructType looks up a struct type, checking parent scopes if not found
func (s *Scope) LookupStructType(name string) (StructType, bool) {
	if structType, ok := s.structTypes[name]; ok {
		return structType, true
	}
	if s.parent != nil {
		return s.parent.LookupStructType(name)
	}
	return StructType{}, false
}

// DefineOneofType adds a sum type to the current scope
func (s *Scope) DefineOneofType(name string, oneofType OneofType) {
	s.oneofTypes[name] = oneofType
}

// LookupOneofType looks up a sum type, checking parent scopes if not found
func (s *Scope) LookupOneofType(name string) (OneofType, bool) {
	if oneofType, ok := s.oneofTypes[name]; ok {
		return oneofType, true
	}
	if s.parent != nil {
		return s.parent.LookupOneofType(name)
	}
	return OneofType{}, false
}

// DefineFunc adds a function to the current scope
func (s *Scope) DefineFunc(name string, funcType FuncType) {
	s.funcs[name] = funcType
}

// LookupFunc looks up a function, checking parent scopes if not found
func (s *Scope) LookupFunc(name string) (FuncType, bool) {
	if funcType, ok := s.funcs[name]; ok {
		return funcType, true
	}
	if s.parent != nil {
		return s.parent.LookupFunc(name)
	}
	return FuncType{}, false
}

// DefineMethod registers a method (bound signature) on a receiver struct type
func (s *Scope) DefineMethod(recvType string, name string, funcType FuncType) {
	if s.methods[recvType] == nil {
		s.methods[recvType] = make(map[string]FuncType)
	}
	s.methods[recvType][name] = funcType
}

// LookupMethod looks up a method by receiver type and method name, checking parent scopes
func (s *Scope) LookupMethod(recvType string, name string) (FuncType, bool) {
	if byName, ok := s.methods[recvType]; ok {
		if funcType, ok := byName[name]; ok {
			return funcType, true
		}
	}
	if s.parent != nil {
		return s.parent.LookupMethod(recvType, name)
	}
	return FuncType{}, false
}

// namedTypeName returns the struct type name a method receiver keys on. A
// receiver is either a struct value (`(p: Product)`) or a pointer to a struct
// (`(p: Product^)`); both are keyed under the underlying struct name, so a
// pointer receiver expression is unwrapped first. Returns "" for anything else.
func namedTypeName(typeExpr ast.TypeExpr) string {
	if ptr, ok := typeExpr.(*ast.PointerTypeExpr); ok {
		typeExpr = ptr.UnderlyingType
	}
	// A generic receiver such as `(m: Map K V)` is a type application; it keys on
	// its constructor's name (Map), the same name the struct type registers under.
	if app, ok := typeExpr.(*ast.TypeApplicationExpr); ok {
		typeExpr = app.Constructor
	}
	if named, ok := typeExpr.(*ast.NamedTypeExpr); ok {
		return named.TypeName
	}
	return ""
}

// receiverPatternParams returns the type-parameter names a method receiver binds
// by the receiver-pattern rule: the bare-identifier arguments of a generic
// receiver such as `(m: Map K V)`, which introduces fresh parameters K and V. A
// non-generic receiver binds none. A pointer receiver is unwrapped first.
func receiverPatternParams(typeExpr ast.TypeExpr) []string {
	if ptr, ok := typeExpr.(*ast.PointerTypeExpr); ok {
		typeExpr = ptr.UnderlyingType
	}
	app, ok := typeExpr.(*ast.TypeApplicationExpr)
	if !ok {
		return nil
	}
	names := make([]string, 0, len(app.Args))
	for _, arg := range app.Args {
		if named, ok := arg.(*ast.NamedTypeExpr); ok {
			names = append(names, named.TypeName)
		}
	}
	return names
}

// underlyingStruct unwraps a single level of pointer indirection and reports the
// struct type a receiver keys on, so both `Product` and `Product^` receivers
// resolve to the Product method set. Returns false for non-struct receivers.
func underlyingStruct(t Type) (StructType, bool) {
	if ptr, ok := t.(PointerType); ok {
		t = ptr.ElemType
	}
	s, ok := t.(StructType)
	return s, ok
}

// ResolvedModule represents the result of symbol resolution
type ResolvedModule struct {
	RootScope *Scope         // Module-level scope
	Scopes    map[any]*Scope // Maps AST nodes to their scopes
	Errors    []string
}

// Resolver handles symbol resolution and builds symbol tables
type Resolver struct {
	errors     []string
	currScope  *Scope         // Current scope during traversal
	scopes     map[any]*Scope // Maps AST nodes to their scopes
	primitives map[string]Type
	// typeParams holds the type-parameter names in scope for the declaration
	// currently being resolved (a struct's or function's binders). A NamedTypeExpr
	// matching one of these resolves to a TypeParamType rather than a concrete type.
	typeParams map[string]bool
}

// NewResolver creates a new resolver with built-in primitive types
func NewResolver() *Resolver {
	return &Resolver{
		errors:    []string{},
		currScope: NewScope(nil),
		scopes:    make(map[any]*Scope),
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
	case *ast.NamedTypeExpr:
		if r.typeParams[e.TypeName] {
			return TypeParamType{Name: e.TypeName}
		}
		if prim, ok := r.primitives[e.TypeName]; ok {
			return prim
		}
		if structType, ok := r.currScope.LookupStructType(e.TypeName); ok {
			if len(structType.TypeParams) > 0 {
				r.Err(fmt.Sprintf("generic type %s requires %d type argument(s)", e.TypeName, len(structType.TypeParams)))
				return nil
			}
			return structType
		}
		if oneofType, ok := r.currScope.LookupOneofType(e.TypeName); ok {
			if len(oneofType.TypeParams) > 0 {
				r.Err(fmt.Sprintf("generic type %s requires %d type argument(s)", e.TypeName, len(oneofType.TypeParams)))
				return nil
			}
			return oneofType
		}
		r.Err(fmt.Sprintf("undefined type: %s", e.TypeName))
		return nil
	case *ast.TypeApplicationExpr:
		return r.resolveTypeApplication(e)
	case *ast.ArrayTypeExpr:
		elemType := r.ResolveType(e.UnderlyingType)
		if elemType == nil {
			return nil
		}
		return ArrayType{ElemType: elemType}
	case *ast.PointerTypeExpr:
		elemType := r.ResolveType(e.UnderlyingType)
		if elemType == nil {
			return nil
		}
		return PointerType{ElemType: elemType}
	case *ast.TupleTypeExpr:
		elemTypes := make([]Type, 0, len(e.ElementTypes))
		for _, astElem := range e.ElementTypes {
			elemType := r.ResolveType(astElem)
			if elemType == nil {
				return nil
			}
			elemTypes = append(elemTypes, elemType)
		}
		return TupleType{ElementTypes: elemTypes}
	case *ast.UnitTypeExpr:
		return UnitType{}
	case *ast.FuncTypeExpr:
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

// resolveTypeApplication resolves a juxtaposition type application such as
// `Box i32` or `Map string i32`. The constructor must name a generic struct whose
// declared arity matches the number of arguments; the result is an instantiation
// whose members have every type parameter substituted for the concrete arguments.
func (r *Resolver) resolveTypeApplication(e *ast.TypeApplicationExpr) Type {
	named, ok := e.Constructor.(*ast.NamedTypeExpr)
	if !ok {
		r.Err("type application requires a named type constructor")
		return nil
	}
	// The constructor names either a generic struct or a generic sum type; both
	// are instantiated by substituting the arguments for the declared parameters.
	var typeParams []string
	if structType, ok := r.currScope.LookupStructType(named.TypeName); ok {
		typeParams = structType.TypeParams
	} else if oneofType, ok := r.currScope.LookupOneofType(named.TypeName); ok {
		typeParams = oneofType.TypeParams
	} else {
		r.Err(fmt.Sprintf("undefined generic type: %s", named.TypeName))
		return nil
	}
	if len(typeParams) == 0 {
		r.Err(fmt.Sprintf("type %s is not generic and takes no type arguments", named.TypeName))
		return nil
	}
	if len(e.Args) != len(typeParams) {
		r.Err(fmt.Sprintf("generic type %s expects %d type argument(s), got %d", named.TypeName, len(typeParams), len(e.Args)))
		return nil
	}
	argTypes := make([]Type, 0, len(e.Args))
	for _, arg := range e.Args {
		argType := r.ResolveType(arg)
		if argType == nil {
			return nil
		}
		argTypes = append(argTypes, argType)
	}
	subst := make(map[string]Type, len(typeParams))
	for i, name := range typeParams {
		subst[name] = argTypes[i]
	}
	if structType, ok := r.currScope.LookupStructType(named.TypeName); ok {
		members := make(map[string]Type, len(structType.Members))
		for name, m := range structType.Members {
			members[name] = substitute(m, subst)
		}
		return StructType{
			Name:       structType.Name,
			Members:    members,
			TypeParams: structType.TypeParams,
			TypeArgs:   argTypes,
		}
	}
	oneofType, _ := r.currScope.LookupOneofType(named.TypeName)
	variants := make(map[string][]Type, len(oneofType.Variants))
	for name, payload := range oneofType.Variants {
		slots := make([]Type, len(payload))
		for i, slot := range payload {
			slots[i] = substitute(slot, subst)
		}
		variants[name] = slots
	}
	return OneofType{
		Name:         oneofType.Name,
		Variants:     variants,
		VariantOrder: oneofType.VariantOrder,
		TypeParams:   oneofType.TypeParams,
		TypeArgs:     argTypes,
	}
}

// Resolve performs symbol resolution on the module
func Resolve(module *ast.BlockStmt) *ResolvedModule {
	resolver := NewResolver()
	// Process module statements directly in root scope - don't create a child scope
	for _, stmt := range module.Statements {
		resolver.resolveStmt(stmt)
	}
	return &ResolvedModule{
		RootScope: resolver.currScope,
		Scopes:    resolver.scopes,
		Errors:    resolver.errors,
	}
}

// resolveStmt resolves symbols in a statement
func (r *Resolver) resolveStmt(stmt ast.Stmt) {
	switch s := stmt.(type) {
	case *ast.BlockStmt:
		r.resolveBlockStmt(s)
	case *ast.VarDeclStmt:
		r.resolveVarDeclStmt(s)
	case *ast.StructDeclStmt:
		r.resolveStructDeclStmt(s)
	case *ast.OneofDeclStmt:
		r.resolveOneofDeclStmt(s)
	case *ast.FuncDeclStmt:
		r.resolveFuncDeclStmt(s)
	case *ast.IfStmt:
		r.resolveIfStmt(s)
	case *ast.MatchStmt:
		r.resolveMatchStmt(s)
	case *ast.ForStmt:
		r.resolveForStmt(s)
	case *ast.ReturnStmt:
		r.resolveReturnStmt(s)
	case *ast.ExpressionStmt:
		r.resolveExpr(s.Expr)
	default:
		r.Err(fmt.Sprintf("unknown statement type: %T", stmt))
	}
}

// resolveBlockStmt resolves symbols in a block statement
func (r *Resolver) resolveBlockStmt(block *ast.BlockStmt) {
	oldTable := r.currScope
	r.currScope = NewScope(oldTable)
	for _, stmt := range block.Statements {
		r.resolveStmt(stmt)
	}
	r.currScope = oldTable
}

// resolveVarDeclStmt resolves a variable declaration
func (r *Resolver) resolveVarDeclStmt(stmt *ast.VarDeclStmt) {
	declaredType := r.ResolveType(stmt.Var.Type)
	if declaredType == nil {
		return
	}
	if stmt.InitVal != nil {
		r.resolveExpr(stmt.InitVal)
	}
	r.currScope.DefineVar(stmt.Var.Name, declaredType)
}

// resolveStructDeclStmt resolves a struct declaration
func (r *Resolver) resolveStructDeclStmt(stmt *ast.StructDeclStmt) {
	if _, ok := r.currScope.LookupStructType(stmt.Name); ok {
		r.Err(fmt.Sprintf("redeclared struct %s in the same scope", stmt.Name))
		return
	}
	members := make(map[string]Type)
	memberNames := make(map[string]bool)
	// The struct's binders are in scope while resolving its members, so a member
	// typed `T` resolves to a TypeParamType rather than an undefined type.
	r.typeParams = make(map[string]bool, len(stmt.TypeParams))
	for _, tp := range stmt.TypeParams {
		r.typeParams[tp] = true
	}
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
	r.typeParams = nil
	r.currScope.DefineStructType(stmt.Name, StructType{
		Name:       stmt.Name,
		Members:    members,
		TypeParams: stmt.TypeParams,
	})
}

// resolveOneofDeclStmt resolves a sum type declaration
func (r *Resolver) resolveOneofDeclStmt(stmt *ast.OneofDeclStmt) {
	if _, ok := r.currScope.LookupOneofType(stmt.Name); ok {
		r.Err(fmt.Sprintf("redeclared sum type %s in the same scope", stmt.Name))
		return
	}
	if _, ok := r.currScope.LookupStructType(stmt.Name); ok {
		r.Err(fmt.Sprintf("sum type %s collides with a struct of the same name", stmt.Name))
		return
	}
	// The type parameters are in scope while resolving variant payload slots.
	r.typeParams = make(map[string]bool, len(stmt.TypeParams))
	for _, tp := range stmt.TypeParams {
		r.typeParams[tp] = true
	}
	variants := make(map[string][]Type, len(stmt.Variants))
	order := make([]string, 0, len(stmt.Variants))
	for _, variant := range stmt.Variants {
		if _, ok := variants[variant.Name]; ok {
			r.Err(fmt.Sprintf("duplicate variant %s in sum type %s", variant.Name, stmt.Name))
			continue
		}
		slots := make([]Type, 0, len(variant.Payload))
		for _, slotExpr := range variant.Payload {
			slotType := r.ResolveType(slotExpr)
			if slotType == nil {
				continue
			}
			slots = append(slots, slotType)
		}
		variants[variant.Name] = slots
		order = append(order, variant.Name)
	}
	r.typeParams = nil
	r.currScope.DefineOneofType(stmt.Name, OneofType{
		Name:         stmt.Name,
		Variants:     variants,
		VariantOrder: order,
		TypeParams:   stmt.TypeParams,
	})
}

// resolveFuncDeclStmt resolves a function or method declaration
func (r *Resolver) resolveFuncDeclStmt(stmt *ast.FuncDeclStmt) {
	// The declaration's type parameters are in scope for its receiver, parameter,
	// and return types. A free function or method binds them after the name; a
	// method additionally binds receiver-pattern parameters (the bare-identifier
	// arguments of a generic receiver like `(m: Map K V)`).
	r.typeParams = make(map[string]bool)
	for _, tp := range stmt.TypeParams {
		r.typeParams[tp] = true
	}
	if stmt.Receiver != nil {
		for _, name := range receiverPatternParams(stmt.Receiver.Type) {
			r.typeParams[name] = true
		}
	}
	defer func() { r.typeParams = nil }()

	var returnType Type = UnitType{}
	if stmt.ReturnType != nil {
		returnType = r.ResolveType(stmt.ReturnType)
		if returnType == nil {
			return
		}
	}

	paramTypes := make([]Type, 0, len(stmt.Parameters))
	funcScope := NewScope(r.currScope)

	// A method binds its receiver as a local variable in the function scope. The
	// receiver is either a struct value or a pointer to a struct; both key their
	// method set under the underlying struct type. A value receiver binds a copy,
	// a pointer receiver binds the pointer (so mutations through it persist).
	var receiverType *StructType
	if stmt.Receiver != nil {
		recvType := r.ResolveType(stmt.Receiver.Type)
		if recvType == nil {
			return
		}
		structType, ok := underlyingStruct(recvType)
		if !ok {
			r.Err(fmt.Sprintf("method receiver %s must be a struct or pointer-to-struct type, found %s", stmt.Receiver.Name, recvType))
			return
		}
		receiverType = &structType
		funcScope.DefineVar(stmt.Receiver.Name, recvType)
	}

	for _, param := range stmt.Parameters {
		paramType := r.ResolveType(param.Type)
		if paramType == nil {
			return
		}
		paramTypes = append(paramTypes, paramType)
		funcScope.DefineVar(param.Name, paramType)
	}

	// The bound signature excludes the receiver: `product.isAffordable` has type
	// func(i32): bool, with the receiver captured as the closure environment.
	funcType := FuncType{
		ReturnType: returnType,
		ParamTypes: paramTypes,
	}

	if receiverType != nil {
		if _, ok := receiverType.Members[stmt.Name]; ok {
			r.Err(fmt.Sprintf("method %s collides with a field of struct %s", stmt.Name, receiverType.Name))
			return
		}
		if _, ok := r.currScope.LookupMethod(receiverType.Name, stmt.Name); ok {
			r.Err(fmt.Sprintf("redeclared method %s on struct %s", stmt.Name, receiverType.Name))
			return
		}
		r.currScope.DefineMethod(receiverType.Name, stmt.Name, funcType)
	} else {
		if _, ok := r.currScope.LookupFunc(stmt.Name); ok {
			r.Err(fmt.Sprintf("redeclared function %s in the same scope", stmt.Name))
			return
		}
		r.currScope.DefineFunc(stmt.Name, funcType)
	}

	// Record function scope in map using statement pointer as key
	r.scopes[stmt] = funcScope
	oldTable := r.currScope
	r.currScope = funcScope

	// Process function body statements directly in function scope
	for _, bodyStmt := range stmt.Body.Statements {
		r.resolveStmt(bodyStmt)
	}

	r.currScope = oldTable
}

// resolveIfStmt resolves an if statement
func (r *Resolver) resolveIfStmt(stmt *ast.IfStmt) {
	r.resolveExpr(stmt.Cond)
	r.resolveStmt(stmt.Then)
	if stmt.Else != nil {
		r.resolveStmt(stmt.Else)
	}
}

// resolveForStmt resolves a for statement
func (r *Resolver) resolveForStmt(stmt *ast.ForStmt) {
	r.resolveStmt(stmt.Init)
	r.resolveExpr(stmt.Cond)
	r.resolveExpr(stmt.Iter.Expr)
	r.resolveBlockStmt(stmt.Body)
}

// resolvePatternBinders opens a child scope and defines every payload binder a
// variant pattern introduces. Binder types are unknown until type checking, so
// each name is bound to a placeholder; the wildcard binder `_` introduces nothing.
// The caller is responsible for restoring the previous scope.
func (r *Resolver) resolvePatternBinders(pattern ast.Pattern) {
	if variant, ok := pattern.(*ast.VariantPattern); ok {
		for _, binder := range variant.Binders {
			if binder != "_" {
				r.currScope.DefineVar(binder, UnknownType{})
			}
		}
	}
}

// resolveMatchStmt resolves a statement-position match: the scrutinee, then each
// arm body in a fresh scope holding that arm's pattern binders.
func (r *Resolver) resolveMatchStmt(stmt *ast.MatchStmt) {
	r.resolveExpr(stmt.Scrutinee)
	for _, arm := range stmt.Arms {
		oldScope := r.currScope
		r.currScope = NewScope(oldScope)
		r.resolvePatternBinders(arm.Pattern)
		r.resolveStmt(arm.Body)
		r.currScope = oldScope
	}
}

// resolveMatchExpr resolves an expression-position match, mirroring
// resolveMatchStmt but with expression arm bodies.
func (r *Resolver) resolveMatchExpr(expr *ast.MatchExpr) {
	r.resolveExpr(expr.Scrutinee)
	for _, arm := range expr.Arms {
		oldScope := r.currScope
		r.currScope = NewScope(oldScope)
		r.resolvePatternBinders(arm.Pattern)
		r.resolveExpr(arm.Body)
		r.currScope = oldScope
	}
}

// resolveReturnStmt resolves a return statement
func (r *Resolver) resolveReturnStmt(stmt *ast.ReturnStmt) {
	if stmt.Expr != nil {
		r.resolveExpr(stmt.Expr)
	}
}

// resolveExpr resolves symbols in an expression
func (r *Resolver) resolveExpr(expr ast.Expr) {
	switch e := expr.(type) {
	case *ast.NumberLiteralExpr, *ast.StringLiteralExpr, *ast.BoolLiteralExpr:
		// Literals don't need resolution
	case *ast.NilLiteralExpr:
		// The nil literal needs no resolution
	case *ast.AddressOfExpr:
		r.resolveExpr(e.Operand)
	case *ast.DerefExpr:
		r.resolveExpr(e.Operand)
	case *ast.IdentExpr:
		// Check if identifier exists in symbol table
		if _, ok := r.currScope.LookupVarType(e.Value); !ok {
			if _, ok := r.currScope.LookupStructType(e.Value); !ok {
				if _, ok := r.currScope.LookupOneofType(e.Value); !ok {
					if _, ok := r.currScope.LookupFunc(e.Value); !ok {
						r.Err(fmt.Sprintf("undefined identifier: %s", e.Value))
					}
				}
			}
		}
	case *ast.BinaryExpr:
		r.resolveExpr(e.Lhs)
		r.resolveExpr(e.Rhs)
	case *ast.UnaryExpr:
		r.resolveExpr(e.Rhs)
	case *ast.GroupExpr:
		r.resolveExpr(e.Expr)
	case *ast.TupleLiteralExpr:
		for _, elem := range e.Elements {
			r.resolveExpr(elem)
		}
	case *ast.UnitExpr:
		// The unit literal needs no resolution
	case *ast.FuncCallExpr:
		r.resolveExpr(e.Func)
		for _, arg := range e.Args {
			r.resolveExpr(arg)
		}
	case *ast.StructLiteralExpr:
		r.resolveExpr(e.Struct)
		for _, member := range e.Members {
			r.resolveExpr(member.Value)
		}
	case *ast.StructMemberExpr:
		r.resolveExpr(e.Struct)
	case *ast.ArrayIndexExpr:
		r.resolveExpr(e.Array)
		r.resolveExpr(e.Index)
	case *ast.AssignExpr:
		r.resolveExpr(e.Assigne)
		r.resolveExpr(e.AssignedValue)
	case *ast.VarDeclAssignExpr:
		r.resolveExpr(e.AssignedValue)
		// The binding's type is inferred later by the type checker; define it now
		// with a placeholder so subsequent references resolve during this pass.
		r.currScope.DefineVar(e.Name, UnknownType{})
	case *ast.TupleDeclAssignExpr:
		r.resolveExpr(e.AssignedValue)
		// A type annotation (from a `let`) fixes each element type up front;
		// otherwise the type checker infers them from the initializer.
		var declaredElems []Type
		if e.Type != nil {
			if tupleType, ok := r.ResolveType(e.Type).(TupleType); ok {
				declaredElems = tupleType.ElementTypes
			}
		}
		for i, name := range e.Names {
			if i < len(declaredElems) {
				r.currScope.DefineVar(name, declaredElems[i])
			} else {
				r.currScope.DefineVar(name, UnknownType{})
			}
		}
	case *ast.MatchExpr:
		r.resolveMatchExpr(e)
	default:
		r.Err(fmt.Sprintf("unknown expression type: %T", expr))
	}
}
