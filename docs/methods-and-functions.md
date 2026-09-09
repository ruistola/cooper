# Methods and Functions

## Overview

Cooper has both free functions and methods. Methods are syntactic sugar for functions with a
distinguished first argument (the receiver). They exist primarily for ergonomic dot-notation
chaining and subject-verb-object readability.

## Function declarations

```
func add(x: i32, y: i32): i32 {
  return x + y
}

func greet(name: string) {
  print("hello, " + name)
}
```

Parameters use `name: Type` syntax. Return type follows the parameter list as `: Type`.
Omitting the return type indicates the unit type. In general, the last expression
within a block expression is the value of that block, and function bodies could be
considered a kind of block expression; however, for clarity, use of `return` is
mandatory to exit the function body with a non-unit value.

## Method declarations

Methods are declared separately from their associated type, with an explicit receiver:

```
struct Product {
  name: string,
  price: i32,
  weight: f32,
}

(p: Product) func isAffordable(budget: i32): bool {
  return p.price <= budget
}

(p: Product) func shippingCost(ratePerKg: f32): f32 {
  return p.weight * ratePerKg
}
```

This is the **only** form of method declaration. There is no embedded/inline method syntax
within struct declarations.

The receiver is **not** part of the method's signature from a type-compatibility perspective.
See [Runtime representation](#runtime-representation) below.

## Struct declarations are data only

Struct declarations contain only data fields (including function-typed fields, which are
stored per-instance as pointers):

```
struct Monster {
  name: string,
  health: i32,
  onDeath: func(string),
}
```

Methods are always declared outside the struct body:

```
(m: Monster) func isAlive(): bool {
  return m.health > 0
}

(m: Monster^) func takeDamage(amount: i32) {
  m.health -= amount
  if m.health <= 0 then m.onDeath(m.name)
}
```

## Why no embedded methods

We explored and rejected declaring methods inside struct bodies (making the struct declaration
a lexical scope where fields are accessible without a receiver prefix). While aesthetically
appealing in small examples, it introduces problems at scale:

* **Method-to-method resolution.** Without an explicit receiver, a bare `helper()` call inside
  a method body is ambiguous: does it call a sibling method on the same receiver, or a
  module-level function? Introducing resolution rules adds implicit behavior — the opposite
  of Cooper's design goal.

* **Multiple declaration sites.** If methods can be declared both inline and separately, readers
  must check two locations. A single declaration form means one place to look.

* **OOP drift.** Embedding methods in struct bodies nudges toward class-like patterns. Keeping
  structs as pure data encourages the preferred design: plain data structs + free functions,
  with methods reserved for cases of genuine tight coupling (e.g., `str.contains()`,
  `arr.length()`).

## Receivers

A receiver may be declared by value (`(p: Product)`) or by pointer (`(p: Product^)`), with
the same semantics those forms carry anywhere else in the language. A value receiver
operates on a copy, whereas a pointer receiver operates on the caller's object through
indirection.

The choice is the ordinary value-vs-pointer decision, made on two grounds:

* **Mutation.** A method that must modify the receiver takes a pointer receiver. A method
  that only reads can take either.
* **Cost.** Even for a read-only method, a large struct may be cheaper to pass by pointer
  and access field-by-field within the body than to copy wholesale.

## Method binding

Accessing a method via dot notation on an instance creates a closure:

```
type ReadFn = func(u8[]): i32

let read: ReadFn = myFile.read
```

This closure captures the instance as its environment. It can be passed anywhere a matching
function type is expected — this is the primary mechanism for polymorphism in Cooper (see
[Polymorphism and Interfaces](./polymorphism-and-interfaces.md)).

```
func process(read: func(u8[]): i32) {
  let buf: u8[] = makeBuffer(1024)
  let n: i32 = read(buf)
  // ...
}

process(myFile.read)      // method binding: captures myFile
process(mySocket.read)    // same signature, different implementation
process(func(buf: u8[]): i32 { 0 })  // ad hoc lambda also works
```

## Runtime representation

All function-typed values share a uniform runtime representation: a pair of pointers.

```
// Conceptual internal layout (not user-visible syntax)
//   code: pointer to the executable code
//   env:  pointer to captured environment (or null)
```

| Source expression | `code` points to | `env` points to |
|-------------------|-----------------|-----------------|
| Free function `sqrt` | sqrt implementation | null |
| Lambda `func(x: i32): i32 { x + y }` | lambda body | captured variables (e.g., y) |
| Method binding `product.isAffordable` | Product.isAffordable code | the product instance |

The calling convention always passes `env` as a hidden first argument. Free functions
receive it and ignore it. This means:

* **The method receiver is not part of the function type signature.** A method
  `(p: Product) func isAffordable(budget: i32): bool` produces, when bound via
  `myProduct.isAffordable`, a value of type `func(i32): bool`. The receiver becomes
  the captured environment — invisible to the caller.

* **No special dispatch machinery is needed.** Calling a function value is always the same
  operation: load the code pointer, load the env pointer, call with env as hidden first arg
  followed by the explicit arguments.

* **One fewer indirection than Go interfaces.** Go interface calls chase a pointer to an
  itable, then load the method address from the table (sequential dependent loads). Cooper
  function values load code and env from adjacent memory (same cache line, parallel loads).

## Methods on types from other modules

Methods can only be declared on types defined in the same module. To add behavior to a
foreign type, write a free function or wrap the type. This avoids orphan-rule complexity
and keeps method sets locally determinable.

## Function fields vs methods

The distinction is clear and semantically significant:

| Aspect | Method | Function field |
|--------|--------|----------------|
| Declaration | Outside struct body, explicit receiver | Inside struct body, as a data field |
| Storage | Shared (one implementation for all instances) | Per-instance (stored as pointer) |
| Call cost | Direct call (or tag dispatch through union) | Indirect call through pointer |
| Use case | Behavior inherent to the type | Per-instance strategy/callback |

Example combining both:

```
struct Button {
  label: string,
  onClick: func(),
}

(b: Button) func render() {
  drawText(b.label)
}

// onClick is per-instance (each button does something different)
// render is shared (all buttons draw the same way)
```

## Shadowing rules

* **Same-scope rebinding** (re-declaring a name in the same block): Allowed. Useful for
  iterative transformation of values (especially with errors-as-values patterns).
* **Cross-scope shadowing** (inner scope hides outer scope variable): Allowed. Banning this
  generally would be impractical — adding a variable to a parent scope shouldn't break all
  downstream code.
