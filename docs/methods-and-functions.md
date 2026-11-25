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

### Variadic parameters

The last parameter of a declared function or method may be written `name: ...T`. It takes any
number of `T` arguments, and the body sees it as a `T[]`, empty when there are none. A call
lists the arguments, or passes an array or slice as all of them with a trailing `...`:

```
func max(vals: ...i32): i32 { … }      # vals is an i32[]

max(3, 9, 4)
max()                                  # vals is empty
max(nums...)                           # nums: i32[], shared as the argument array
```

It is sugar for passing an array: the arguments are packed into a fresh array, while `xs...`
passes `xs` itself, so the function can see (and write through) the caller's elements. A
spread is the only argument after the fixed ones. The function's type is an ordinary one taking
an array (`func(i32[]): i32`), so a variadic function used as a value is called with an array.
Function literals and nested functions cannot be variadic.

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
Two nested functions cannot call each other. A nested function may be generic, declared with
binders like any function (`func tag T (x: T): string { … }`): each use instantiates it at
its own types, and its binders may not reuse a name the enclosing function binds.

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
for i in 0..3 do fs.push(func(): i32 { return i })   # fs[k]() is k
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

### Methods of built-in types

A method is declared in the module that defines its type. A built-in type has a home module in
the standard library, which declares its methods with the same syntax: `string`'s are in
`std.strings` (`(s: string) func trim(): string { … }`), the arrays' in `std.arrays`
(`(xs: T[]) func any(p: func(T): bool): bool { … }`), `rune`'s in `std.runes`, and those
of the numeric types, their checked conversions, in `std.math`. A home module is part of every
program, so the methods are available without a `use`. No other module can declare a method on
a built-in type, and the methods the compiler implements itself cannot be redeclared:
`s.length()`, `s.byteAt(i)`, the arrays' `length`, `push`, `copy`, `contains`, and `find`, a
float's `bits()`, and `floatFromBits()` on `u32` and `u64` (see
[Numeric Types](./numeric-types.md#conversions-are-explicit-and-constructor-style)).

The methods of `string` work in byte offsets, as slicing does:

```
s.contains("ab")        s.find("ab")        # Maybe i64
s.startsWith("a")       s.endsWith("z")
s.trim()                s.trimStart()          s.trimEnd()      # ASCII whitespace
s.split(",")            s.replace("a", "b")    s.times(3)
s.padStart(8)           s.padEnd(8)                             # to 8 runes, with spaces
s.toAsciiLower()        s.toAsciiUpper()
s.parseInt()            s.parseFloat()                          # Maybe i64, Maybe f64
```

`parseInt` reads an optional sign and decimal digits, and `parseFloat` also a fraction and an
exponent (`"-2.5e3"`, `".5"`), and `inf`, `-inf`, and `nan`. Anything else is `None`, spaces and
the `_` and `0x` of source literals included, and so is a number out of the type's range.

`std.math` has the checked conversions between the numeric types: `n.toU8()` is `Some` of the
same number as a `u8` if there is one, and `None` otherwise. Every integer and float type has
`toI8` through `toU64` for the other integer types, a float's fraction dropped as `u8(f)` drops
it. Every integer type has `toRune()`, and `std.runes` gives a rune `toU8()`:

```
b := if n.toU8() is Some(b) then b else return false
if s.parseInt() is Some(n) and n.toU16() is Some(port) then listen(port)
```

`std.strings` also has the `StringBuilder` (`write`, `writeRune`, `build`) and `join(parts, sep)`.
A `u8[]` has the built-in `utf8(): Maybe string`, which copies the bytes into a string when they
are valid UTF-8.

The methods of a `T[]`:

```
xs.contains(x)          xs.find(x)                              # bool, Maybe i64
xs.any(p)               xs.all(p)              xs.findWhere(p)  # p: func(T): bool
xs.map(f)               xs.filter(p)           xs.reduce(0, add)   # new arrays, and a value
xs.reverse()            xs.sort()              xs.sortBy(less)  # in place
```

`contains` and `find` compare elements with `==`, so they need a comparable element type, and
`sort` orders them with `<`, so it needs numbers or runes ([Generics](./generics.md#requirements)).
Both sorts are stable; `less` is a strict order such as `func(a, b) { return a.age < b.age }`. `reduce` folds from
the left, starting with its first argument, which may be of another type than the elements:
`xs.reduce("", func(s, x) { return s + string(x) })`.

The methods of `rune` classify and map ASCII only, and their names say so: `isAscii()`,
`isAsciiDigit()`, `isAsciiHexDigit()`, `isAsciiLetter()`, `isAsciiAlphanumeric()`,
`isAsciiUpper()`, `isAsciiLower()`, `isAsciiSpace()`, `toAsciiUpper()`, and `toAsciiLower()`.
The names without `Ascii` are left for Unicode-aware versions. `r.utf8Length()` is the number
of bytes the rune takes in a string.

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
