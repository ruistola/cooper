# Generics

Generic types and functions use **juxtaposition** for type application, **PascalCase type
parameters introduced by an explicit binder**, and **inference-only instantiation**: type
arguments are written in type position only, never at call sites.

```
struct Map K V { … }                     # K, V are type parameters
let scores: Map string i32 = …           # Map applied to string and i32
let maybe: Maybe bool = Some(true)       # type arguments inferred
```

## Type application

Juxtaposition keeps `<>` free for comparison and `[]` free for arrays and indexing. Type
application parses as a left-associative spine (`Map string i32` is `App(App(Map, string),
i32)`); arity is checked by the resolver, not the parser. A type ends at any token that
cannot begin a type atom — `,`, a closing delimiter, `=`, `{`, or a statement terminator:

```
func f(m: Map string i32, n: i32): Map string i32 { … }   # `,` and `{` end a type
let x: Map string i32 = …                                  # `=` ends a type
```

Application binds looser than the postfix `[]` and `^`, so a compound argument or operand
is parenthesised:

```
(Map string i32)[]        # array of maps; Map string i32[] is Map of string and i32[]
(Box T)^                  # pointer to a Box T
Map (i32, string) bool    # a tuple as one argument
List (func(i32): bool)    # a function type as one argument
Map string (List i32)     # nested application
```

## Binders

A binder is the run of identifiers between a declaration's name and its body or
value-parameter list:

```
struct Map K V { … }
oneof Maybe T { … }
func map T U (f: func(T): U, xs: List T): List U { … }
```

A name in a signature is a type parameter exactly when a binder introduces it; casing is a
convention, never load-bearing. Every binder must introduce a **fresh** name, distinct from
its siblings and from every type in scope, so a name is never both a parameter and a concrete
type. A concrete instantiation is an ordinary type anywhere a type is expected:

```
func f(other: Map string i32): i32 { … }   # only this instantiation
func g K (other: Map K string): i32 { … }  # K a parameter, string concrete
```

## Generic methods

A receiver pattern binds the receiver type's parameters; method-local parameters use the
binder after the method name. Binders are per declaration, so one signature may mention
several instantiations of the same type:

```
(m: Map K V) func get(key: K): V { … }
(m: Map K V) func transform T U (other: Map T U): Map K U { … }
(m: Map K _) func keys(): List K { … }      # `_` for an unused parameter
```

A receiver pattern's slots are always binders, so `(m: Map string i32)` is rejected: methods
apply to every instantiation of their type.

## Instantiation

Type arguments are inferred at every use: a generic struct or sum-type construction from its
member and payload values, a generic function or method call from its arguments and the
expected result type, and a method's receiver-pattern parameters from the receiver. A generic
function used as a value takes its type arguments from the expected function type. When
inference under-determines a parameter, annotate the binding:

```
x := Some(true)                          # Maybe bool
let r: Result i32 string = Ok(5)         # Ok fixes T; the annotation fixes E
let a: i64 = id(3)                       # the expected type fixes T before 3 is checked
```

Generics are compiled by monomorphization: each distinct instantiation becomes its own
concrete function.

## Constraints

Type parameters are unbounded. A generic that needs an operation on its parameter takes it
as a function argument (`fold(xs, 0, add)`). Bounds, if added, will go in a separate
`where` clause rather than inline in the signature.
