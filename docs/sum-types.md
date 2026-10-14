# Sum Types

A `oneof` declares a nominal tagged union: a set of named **variants**, each with an
optional positional payload. Variants are not types; they exist only within their `oneof`.

```
oneof Color { Red, Green, Blue }

oneof Shape {
  Circle(i32),
  Rect(i32, i32),
}

oneof Result T E {
  Ok(T),
  Err(E),
}
```

A payload is a parenthesised, comma-separated list of type expressions, which may be generic
applications (`Full(Box i32)`). A generic `oneof` binds its type parameters after its name,
like a struct. Each instantiation is a distinct type: `Maybe i32` is not `Maybe string`.
The words `enum` and `union` are kept free for possible C-style integer enumerations and
untagged unions.

## Construction

A variant is qualified by its sum type, never by a module path:

```
Shape.Rect(3, 4)
Color.Green
Maybe.Some(5)
Maybe.None
```

A payload-free variant is a complete value, and a variant with a payload is completed by a
call that fills its slots. The qualifier may be dropped when any use determines the sum
type: an annotation, return type, call parameter, assignment, or comparison. That use may
appear later in the body.

```
let x: Result i32 string = Ok(5)
func f(): Maybe i32 { return None }
m := None
m = Maybe.Some(3)   # both values are Maybe i32
let y = Ok(5)       # no other use fixes the sum type: write Result.Ok(5)
```

A bare variant with no determining use is an error at body completion. Even a unique
variant name does not select its owning sum type; inference does not search declarations.

The zero value of a sum type is its first variant with a zero payload. By convention, a sum
type with a single payloadless variant lists it first (`oneof Maybe T { None, Some(T) }`), so
that variant is the zero value and a zero-initialized value never carries a payload.

A generic sum type's arguments are inferred from payloads and all uses of the constructed
value. A comparison relates both operand types (`m == None`, `None == m`), independently of
operand order. A qualified payload-free variant can also be constrained later:

```
m := Maybe.None
m = Maybe.Some(3)   # determines T as i32
```

A type argument still undetermined after solving requires an annotation. `m := Maybe.None`
with no determining use cannot infer `T`.

## `match`

`match` tests a scrutinee against arms in order and runs the first that matches. `with`
separates the scrutinee from the arms, as `then` does for `if`, and `=>` separates each
pattern from its body:

```
match shape with {
  Circle(r) => area := 3 * r * r
  Rect(w, h) => area := w * h
}
```

An arm body is a single expression or statement, or a braced block. Arms are separated like
statements, by semicolons that a newline supplies under
[semicolon inference](./syntax-fundamentals.md#semicolon-inference). An arm may therefore
continue over several lines, end with an explicit `;`, or share a line with another arm.
Commas do not separate arms. In statement position the arm values are discarded. In expression
position all arms must share one type, which is the type of the `match`:

```
kind := match shape with {
  Circle(r) => 1
  Rect(w, h) => 2
}
```

The scrutinee may be a sum type, struct, tuple, `bool`, or integer. If its type is unknown,
arm binders get fresh variables immediately; pattern validation and exhaustiveness wait
until another use determines the scrutinee type. Patterns:

| Pattern | Matches |
|---|---|
| `_` | anything, binding nothing |
| `name` | anything, binding it to `name` |
| `Variant`, `Variant(a, _)` | a variant, binding payload slots by position (`_` ignores one) |
| `Type.Variant(…)` | the same, qualified |
| `true`, `false` | a `bool` constant |
| `42`, `-1` | an integer constant |
| `(p, q)` | a tuple, component-wise |
| `Point { x: p, y: _ }` | a struct's listed fields; unlisted fields are ignored |

Tuple and struct patterns nest. Variant payloads bind names only. A `match` must be
exhaustive. A sum type is covered by naming every variant, and a `bool` by both constants.
Integers, tuples, and structs need an irrefutable arm: `_`, a binding, or a tuple or struct
pattern built only from those. An exhaustive match
whose every arm returns counts as returning on all paths.

### Testing one pattern

`value is pattern` is a `bool` saying whether the value matches, and the names the pattern
binds are available where it held. It covers the common case of wanting one variant's
payload, without a `match` whose other arm does nothing:

```
x := if foo is Some(x) then x else 0
if foo is Some(x) and x > 5 then use(x)
while next is Some(item) do next = process(item)
ok := foo is Some(_)
```

Bindings flow only into the `then` branch, a `while` body, and the right operands of `and`;
`or` and `!` bind nothing. The pattern is one pattern, so alternatives and anything larger
belong in a `match`.

### Leaving early

A `match` arm or `if` branch in expression position may be a `return`, `break`, or
`continue`, which fit any type. A failed step leaves while a successful one yields its
value, and an error can be given context on the way out:

```
n := match parse(text) with {
  Ok(v) => v
  Err(err) => return Result.Err("parsing {text}: {err}")
}
```

## Planned

* Named-field payloads.
* A layout that exploits that convention: a payloadless zero-value variant stored as the
  all-zero pattern, with any other pattern identifying a payload variant, as Rust does for
  `Option` of a pointer.
* Nested patterns inside variant payloads, or-patterns (`A | B`), and guards.
