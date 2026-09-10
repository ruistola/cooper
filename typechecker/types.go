package typechecker

import "fmt"

// Type represents a type in the Cooper language
type Type interface {
	String() string
	Equals(other Type) bool
}

// UnknownType is a placeholder used by the resolver for variables whose type is
// inferred later by the type checker (walrus and tuple-destructuring bindings). It
// exists only to satisfy identifier-existence checks during the resolve pass and is
// overwritten with the concrete type during type checking, so it never participates
// in a real equality check.
type UnknownType struct{}

func (t UnknownType) String() string { return "<unknown>" }

func (t UnknownType) Equals(other Type) bool {
	_, ok := other.(UnknownType)
	return ok
}

// UnitType represents the unit type ()
type UnitType struct{}

func (t UnitType) String() string {
	return "()"
}

func (t UnitType) Equals(other Type) bool {
	if _, ok := other.(UnitType); ok {
		return true
	}
	return false
}

// PrimitiveType represents primitive types like i32, string, bool
type PrimitiveType struct {
	Name string
}

func (p PrimitiveType) String() string {
	return p.Name
}

func (p PrimitiveType) Equals(other Type) bool {
	if o, ok := other.(PrimitiveType); ok {
		return p.Name == o.Name
	}
	return false
}

// ArrayType represents array types like i32[]
type ArrayType struct {
	ElemType Type
}

func (a ArrayType) String() string {
	return fmt.Sprintf("%s[]", a.ElemType)
}

func (a ArrayType) Equals(other Type) bool {
	if o, ok := other.(ArrayType); ok {
		return a.ElemType.Equals(o.ElemType)
	}
	return false
}

// PointerType represents pointer types like T^
type PointerType struct {
	ElemType Type
}

func (p PointerType) String() string {
	return fmt.Sprintf("%s^", p.ElemType)
}

// Equals treats a pointer as equal to another pointer with the same element type,
// and (provisionally) as compatible with the untyped nil literal so that nil may be
// assigned to or compared against any pointer. The fuller nil strategy is TBD.
func (p PointerType) Equals(other Type) bool {
	if o, ok := other.(PointerType); ok {
		return p.ElemType.Equals(o.ElemType)
	}
	if _, ok := other.(NilType); ok {
		return true
	}
	return false
}

// NilType is the type of the `nil` literal, compatible with any pointer type.
type NilType struct{}

func (n NilType) String() string {
	return "nil"
}

func (n NilType) Equals(other Type) bool {
	switch other.(type) {
	case NilType, PointerType:
		return true
	}
	return false
}

// TupleType represents an anonymous product type (A, B, ...) with at least two
// elements. One-element and zero-element parenthesised types never reach here:
// they collapse to the element type and the unit type respectively.
type TupleType struct {
	ElementTypes []Type
}

func (t TupleType) String() string {
	elems := ""
	for i, e := range t.ElementTypes {
		if i > 0 {
			elems += ", "
		}
		elems += e.String()
	}
	return fmt.Sprintf("(%s)", elems)
}

func (t TupleType) Equals(other Type) bool {
	o, ok := other.(TupleType)
	if !ok || len(t.ElementTypes) != len(o.ElementTypes) {
		return false
	}
	for i, e := range t.ElementTypes {
		if !e.Equals(o.ElementTypes[i]) {
			return false
		}
	}
	return true
}

// FuncType represents function types
type FuncType struct {
	ReturnType Type
	ParamTypes []Type
}

func (f FuncType) String() string {
	params := ""
	for i, param := range f.ParamTypes {
		if i > 0 {
			params += ","
		}
		params += param.String()
	}
	return fmt.Sprintf("func(%s):%s", params, f.ReturnType)
}

func (f FuncType) Equals(other Type) bool {
	o, ok := other.(FuncType)
	if !ok || len(f.ParamTypes) != len(o.ParamTypes) {
		return false
	}
	if !f.ReturnType.Equals(o.ReturnType) {
		return false
	}
	for i, param := range f.ParamTypes {
		if !param.Equals(o.ParamTypes[i]) {
			return false
		}
	}
	return true
}

// StructType represents user-defined struct types
type StructType struct {
	Name    string
	Members map[string]Type
	// TypeParams are the declared type-parameter names for a generic struct
	// template, e.g. ["T"] for `struct Box T`. Empty for non-generic structs.
	TypeParams []string
	// TypeArgs are the concrete arguments of an instantiation, e.g. [i32] for
	// `Box i32`. Empty for a template or a non-generic struct. A struct is
	// nominal by Name and TypeArgs, so `Box i32` and `Box string` differ.
	TypeArgs []Type
}

func (s StructType) String() string {
	if len(s.TypeArgs) == 0 {
		return s.Name
	}
	args := ""
	for i, a := range s.TypeArgs {
		if i > 0 {
			args += " "
		}
		args += a.String()
	}
	return fmt.Sprintf("%s %s", s.Name, args)
}

func (s StructType) Equals(other Type) bool {
	o, ok := other.(StructType)
	if !ok || s.Name != o.Name || len(s.TypeArgs) != len(o.TypeArgs) {
		return false
	}
	for i, a := range s.TypeArgs {
		if !a.Equals(o.TypeArgs[i]) {
			return false
		}
	}
	return true
}

// TypeParamType is a reference to a bound type parameter (e.g. `T` inside
// `struct Box T { value: T }`). It is a placeholder that substitution replaces
// with a concrete argument type at instantiation. Two type parameters are equal
// only when they share a name, but a template is never compared directly against
// an instantiation: substitution eliminates every TypeParamType first.
type TypeParamType struct {
	Name string
}

func (t TypeParamType) String() string { return t.Name }

func (t TypeParamType) Equals(other Type) bool {
	o, ok := other.(TypeParamType)
	return ok && t.Name == o.Name
}

// substitute replaces every TypeParamType in t according to the given mapping of
// parameter name to concrete argument type, returning the resulting type. Types
// with no type parameters are returned structurally unchanged.
func substitute(t Type, subst map[string]Type) Type {
	switch ty := t.(type) {
	case TypeParamType:
		if replacement, ok := subst[ty.Name]; ok {
			return replacement
		}
		return ty
	case ArrayType:
		return ArrayType{ElemType: substitute(ty.ElemType, subst)}
	case PointerType:
		return PointerType{ElemType: substitute(ty.ElemType, subst)}
	case TupleType:
		elems := make([]Type, len(ty.ElementTypes))
		for i, e := range ty.ElementTypes {
			elems[i] = substitute(e, subst)
		}
		return TupleType{ElementTypes: elems}
	case FuncType:
		params := make([]Type, len(ty.ParamTypes))
		for i, p := range ty.ParamTypes {
			params[i] = substitute(p, subst)
		}
		return FuncType{ReturnType: substitute(ty.ReturnType, subst), ParamTypes: params}
	case StructType:
		if len(ty.TypeArgs) == 0 {
			return ty
		}
		args := make([]Type, len(ty.TypeArgs))
		for i, a := range ty.TypeArgs {
			args[i] = substitute(a, subst)
		}
		members := make(map[string]Type, len(ty.Members))
		for name, m := range ty.Members {
			members[name] = substitute(m, subst)
		}
		return StructType{Name: ty.Name, Members: members, TypeParams: ty.TypeParams, TypeArgs: args}
	default:
		return t
	}
}

// unify matches a template type (which may contain TypeParamType placeholders)
// against a concrete type, recording each parameter's inferred binding in subst.
// It reports false on a structural mismatch or a parameter bound inconsistently to
// two different types. Concrete-vs-concrete positions are compared with Equals.
func unify(template Type, concrete Type, subst map[string]Type) bool {
	switch tt := template.(type) {
	case TypeParamType:
		if bound, ok := subst[tt.Name]; ok {
			return bound.Equals(concrete)
		}
		subst[tt.Name] = concrete
		return true
	case ArrayType:
		ct, ok := concrete.(ArrayType)
		return ok && unify(tt.ElemType, ct.ElemType, subst)
	case PointerType:
		ct, ok := concrete.(PointerType)
		return ok && unify(tt.ElemType, ct.ElemType, subst)
	case TupleType:
		ct, ok := concrete.(TupleType)
		if !ok || len(tt.ElementTypes) != len(ct.ElementTypes) {
			return false
		}
		for i := range tt.ElementTypes {
			if !unify(tt.ElementTypes[i], ct.ElementTypes[i], subst) {
				return false
			}
		}
		return true
	case FuncType:
		ct, ok := concrete.(FuncType)
		if !ok || len(tt.ParamTypes) != len(ct.ParamTypes) {
			return false
		}
		for i := range tt.ParamTypes {
			if !unify(tt.ParamTypes[i], ct.ParamTypes[i], subst) {
				return false
			}
		}
		return unify(tt.ReturnType, ct.ReturnType, subst)
	default:
		return template.Equals(concrete)
	}
}

// Type utility functions
func IsUnit(t Type) bool {
	if _, ok := t.(UnitType); ok {
		return true
	}
	return false
}

func IsPrimitive(t Type, name string) bool {
	if p, ok := t.(PrimitiveType); ok {
		return p.Name == name
	}
	return false
}

func IsNumeric(t Type) bool {
	if p, ok := t.(PrimitiveType); ok {
		return p.Name == "i8" || p.Name == "i32" || p.Name == "i64" || p.Name == "f32" || p.Name == "f64"
	}
	return false
}