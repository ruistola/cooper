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
}

func (s StructType) String() string {
	return s.Name
}

func (s StructType) Equals(other Type) bool {
	if o, ok := other.(StructType); ok {
		return s.Name == o.Name
	}
	return false
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