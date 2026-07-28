# Methods and Functions

## Overview

Cooper has both free functions and methods. Methods are syntactic sugar for functions with a
distinguished first argument (the receiver). They exist primarily for ergonomic dot-notation
chaining and subject-verb-object readability.

## Method declaration syntax

Methods are declared separately from their associated type, with an explicit receiver:

```
func (r: RedPayload).isValid(): bool {
    r.price > 0
}
```

This is the **only** form of method declaration. There is no embedded/inline method syntax
within struct declarations.

## Struct declarations are data only

Struct declarations contain only data fields (including function-typed fields, which are
stored per-instance as pointers):

```
struct MyStruct {
    myField: i32,
    callback: func(i32): bool,   // function-typed field (per-instance data)
}
```

Methods are always declared outside the struct body:

```
func (m: MyStruct).compute(x: i32): i32 {
    m.myField + x
}
```

## Why no embedded methods

We explored and rejected declaring methods inside struct bodies (making the struct declaration
a lexical scope where fields are accessible without a receiver prefix). While aesthetically
appealing in small examples, it introduces problems at scale:

* **Shadowing ambiguity.** Without an explicit receiver, a method parameter with the same name
  as a field creates silent confusion. Requiring a compile error on collision helps, but then
  method-to-method calls within the struct also need resolution rules (does a bare `helper()`
  call the sibling method on the same receiver, or a module-level function?).

* **Receiver semantics become implicit.** With embedded declarations, there's no natural place
  to express value vs pointer receiver. The separate declaration syntax makes this explicit.

* **Multiple declaration sites.** If methods can be declared both inline and separately, readers
  must check two locations. A single declaration form means one place to look.

* **OOP drift.** Embedding methods in struct bodies nudges toward class-like patterns. Keeping
  structs as pure data encourages the preferred design: plain data structs + free functions,
  with methods reserved for cases of genuine tight coupling (e.g., `str.contains()`,
  `arr.length()`).

## Receivers

All receivers are by reference. The compiler may optimize small types (copy instead of
dereference) where profitable, but the semantic model is always reference semantics.

Rationale:
* Eliminates a concept (value vs pointer receiver) that is really about performance, not
  semantics.
* Mutability/constness is handled orthogonally (affecting variable bindings and parameters
  in general, not just method receivers specifically).
* Consistent with closure environment capture, which is also by reference.

## Method binding

Accessing a method via dot notation on an instance creates a closure:

```
let reader: func([]u8): (i32, Error) = myfile.read
```

This closure captures the instance as its environment. It can be passed anywhere a matching
function type is expected — this is the primary mechanism for polymorphism in Cooper (see
[Polymorphism and Interfaces](./polymorphism-and-interfaces.md)).

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

## Shadowing rules

* **Same-scope rebinding** (re-declaring a name in the same block): Allowed. Useful for
  iterative transformation of values (especially with errors-as-values patterns).
* **Cross-scope shadowing** (inner scope hides outer scope variable): Allowed. Banning this
  generally would be impractical — adding a variable to a parent scope shouldn't break all
  downstream code.
* **Method parameter shadowing a receiver field**: Compile error. Since the receiver prefix
  (`r.field`) makes field access explicit, this case is unambiguous in practice, but the
  error prevents accidental misreads.
