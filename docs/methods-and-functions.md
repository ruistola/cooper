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
is the receiver. The receiver is not part of the function type:

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
