# Polymorphism

Cooper has no interface construct. A function takes the capability it needs as a
function-typed parameter, with the signature written inline where it is required, and any
function value of that type satisfies it.

```
func process(read: func(u8[]): (i32, Error)) {
  (n, err) := read(buf)
  # ...
}

process(myFile.read)                     # a bound method
process(func(b: u8[]): (i32, Error) { … })   # a lambda
process(myMockRead)                      # any matching function
```

A function value is a code pointer and an environment pointer, so a polymorphic call costs
one indirect call (see [Methods and Functions](./methods-and-functions.md#runtime-representation)).
Satisfaction is structural and checked at the call site: a function of the wrong signature
is a type error.

When several operations must travel together, pass them as separate parameters or as a
struct of function-typed fields. Capabilities are required statically: there is no runtime
query for whether a value supports an operation. Sum types with `match` model values whose
capabilities differ by case.

## Methods across sum-type variants

When every variant of a sum type carries a payload type that defines a method with the same
signature, that method can be called on the sum-type value. It dispatches by branching on
the variant tag. A variant whose payload lacks the method makes the call a compile error
(`not all variants of Attack define isLethal(): bool`).

```
struct Melee {
  damage: i32,
}

(m: Melee) func isLethal(): bool {
  return m.damage > 100
}

struct Ranged {
  damage: i32,
  accuracy: f32,
}

(r: Ranged) func isLethal(): bool {
  return r.damage > 50 and r.accuracy > 0.9
}

oneof Attack {
  Melee(Melee),
  Ranged(Ranged),
}

func anyLethal(attacks: Attack[]): bool {
  for a in attacks do {
    if a.isLethal() then return true
  }
  return false
}
```
