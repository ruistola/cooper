package typechecker

import "fmt"

// Type represents a type in the Cooper language
type Type interface {
	String() string
	Equals(other Type) bool
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