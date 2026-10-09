# Methods and Functions

## Functions

```
func add(x: i32, y: i32): i32 {
  return x + y
}

func greet(name: string) {
  print("hello, " + name)
}
```

Parameters are `name: Type`, and the return type follows the parameter list as `: Type`. An
omitted return type means unit. A function body leaves with a value only through `return`,
even though other blocks evaluate to their trailing expression.

## Function literals and nested functions

A function literal is an expression whose value is a function. Its parameters and return
type are written as in a declaration, but either may be left out for inference to supply.
An omitted parameter type is inferred from the literal's uses, and an omitted return type
from its `return` values, or is unit when it returns none:

```
double := func(x: i32): i32 { return x * 2 }
process(func(b: u8[]): (i32, Error) { … })
tens := map(xs, func(x) { return x * 10 })   # x and the result take xs's element type
```

A literal's types are inferred with the rest of the enclosing body, so the use that fixes
them may come before or after it: in `sq := func(v) { return v * v }`, a later `sq(w)` with
`w: u8` makes `v` a `u8`. A type nothing determines is an error asking for an annotation.

A function declared inside a body is a nested function: a variable holding a function
value, in scope from its declaration on and within its own body, so it may call itself. Its
signature is written out in full, like any declaration's.
Two nested functions cannot call each other, and a nested function cannot be generic yet.

```
func fib(n: i32): i32 {
  if n < 2 then return n
  return fib(n - 1) + fib(n - 2)
}
```

`return` leaves the innermost function, literal or not. `break` and `continue` leave the
innermost loop within the same function, so a literal's body cannot leave a loop around the
literal.

### Captures

A literal or nested function may name the variables of the bodies enclosing it. It
captures each by reference: the variable is shared, not copied, so a change made on either
side is visible on the other, and the variable lives as long as any closure capturing it.

```
count := 0
inc := func() { count += 1 }
inc()
inc()                        # count is 2
```

Every execution of a binding creates a new variable, and a `for` loop binds its variable
afresh on each pass, so closures made in a loop each keep their own:

```
for i in 0..3 do fs = fs.push(func(): i32 { return i })   # fs[k]() is k
```

A variable declared outside the loop is one variable, shared by every closure capturing it.

A literal's body is checked as part of the enclosing body's inference, so a captured
variable's type may be fixed by a use before or after the literal (see
[Type System](./type-system.md#inference)).

## Methods

A method is a function with a distinguished first argument, the receiver, declared outside
its type with an explicit receiver. This is the only method syntax, and struct declarations
hold only data fields.

```
struct Product {
  name: string,
  price: i32,
  weight: f32,
}

(p: Product) func isAffordable(budget: i32): bool {
  return p.price <= budget
}

(p: Product^) func discount(amount: i32) {
  p.price -= amount
}
```

A value receiver (`(p: Product)`) operates on a copy, and a pointer receiver (`(p: Product^)`)
operates on the caller's value. Use a pointer receiver to mutate, or to avoid copying a large
struct. Methods may only be declared on types defined in the same module; behavior for a
foreign type is a free function. A method name may not collide with a field name of its type.

## Method binding

Accessing a method on a value without calling it yields a function value whose environment
is the receiver: the receiver's pointer for a pointer receiver, or a copy of the receiver,
taken when the method is bound, for a value receiver. The receiver is not part of the
function type:

```
let check: func(i32): bool = product.isAffordable
```

This is how a method supplies a capability (see [Polymorphism](./polymorphism.md)).

## Function fields

A struct field of function type holds a per-instance function value, while a method is one
implementation shared by every instance:

```
struct Button {
  label: string,
  onClick: func(),
}

(b: Button) func render() {
  drawText(b.label)
}
```

## Runtime representation

Every function value is a pair of pointers: `code` and `env`. Calling one loads both and
passes `env` as a hidden first argument, and a free function ignores it.

| Source expression | `code` | `env` |
|---|---|---|
| Free function `sqrt` | `sqrt` | null |
| Lambda `func(x: i32): i32 { return x + y }` | the lambda body | captured variables (`y`) |
| Bound method `product.isAffordable` | `Product.isAffordable` | the receiver |

## Shadowing

Within a block, a later `let` may rebind a name from the same block or an enclosing one. The
later binding follows the earlier one and may use it.

A module's top-level declarations (structs, sum types, functions, and module-level variables)
form one unordered namespace across all of the module's files (see
[Modules and Projects](./modules-and-projects.md)). Declaring a name twice at module level, in
one file or in sibling files, is a redeclaration error.
