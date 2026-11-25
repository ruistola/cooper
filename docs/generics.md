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
union Maybe T { … }
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

Each generic construction or callable reference introduces fresh variables for its type
arguments. Equalities from members, payloads, receivers, call arguments, return types, and
other uses constrain those variables throughout the function body. Information also flows
from later statements; arguments and operands need no special checking order.

```
x := Maybe.Some(true)                   # Maybe bool
let r: Result i32 string = Ok(5)         # the payload fixes T; the annotation fixes E
wide := id(3)
let a: i64 = wide                       # fixes id's T and the literal to i64
g := id
let copy: func(i64): i64 = g             # fixes this reference's T to i64
```

Local bindings are monomorphic: `g` has one instantiation, not a separately inferred type
at each call. A parameter that remains undetermined at body completion requires an
annotation. See [Type System](./type-system.md#inference) for the inference model.

Generics are compiled by monomorphization: each distinct instantiation becomes its own
concrete function.

## Requirements

A generic function or method may use the built-in operators on its type parameters, and
literals as their values. What its body does with a parameter becomes a **requirement** on it:
numeric for arithmetic or a literal, ordered for `<`, comparable for `==`, integer for an
index or a shift, float for a float literal, and so on (the predicates of
[Type System](./type-system.md#inference)). The signature does not spell them out; the
compiler infers them, and checks each instantiation against them:

```
func max T (a: T, b: T): T {
    if a < b then return b
    return a
}

max(2, 3)              # fine: i32 is ordered
max(true, false)       # error: values of type bool cannot be ordered
                       #   note: function max uses < on T at main.coop:2:8
```

A generic declaration that passes a parameter on to another takes on that one's
requirements, so `func largest T (xs: T[])` calling `max` requires an ordered `T` too, and the
error names the chain. A literal requires a type that holds its value: a body that compares
`x > 300` cannot be instantiated with `u8`. A struct or sum type built from a parameter is
comparable when the parameter is.

The predicates are the built-in ones only. Nothing can be declared to meet them, and they
never select an implementation: each instantiation compiles the same body for its own
types. Other behavior is passed in as a function argument (`xs.sortBy(less)`,
`xs.reduce(0, add)`). Only module-level declarations have requirements; a nested generic
function cannot use operators on its own type parameters.
