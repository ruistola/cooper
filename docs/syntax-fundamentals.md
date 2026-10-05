# Syntax Fundamentals

## Source encoding

Sources and strings are UTF-8. The lexer currently accepts an ASCII subset. A type for a
single code point or grapheme is still to be decided.

## Semicolon inference

Semicolons separate statements, and a newline becomes one when both of these hold, following
Ahnfelt's variant of Scala's rule:

1. the token before the newline can end a statement, and
2. the token after it can begin one.

Newlines are insignificant inside parentheses and brackets, inside `if`/loop/`match` headers
(see [Control Flow](./control-flow.md#header-expressions-and-newlines)), and directly before a
closing `}`. A block evaluates to its trailing expression; ending the block with an explicit
`;` makes it unit instead:

```
{ foo }      # the type of foo
{ foo; }     # unit
```

## Naming conventions

Types are PascalCase; functions, methods, variables, and fields are camelCase. Casing carries
no visibility meaning. The visibility mechanism is still to be designed.

## Arrays

`T[]` is the default sequence type: a growable run of elements with a length and capacity.
A **static array** `T[N]` has a fixed compile-time length.

```
let items: i32[]
let matrix: i32[][]
let callbacks: func(i32)[]
let rgb: u8[3]
let grid: i32[8][8]
```

An array literal is a bracketed, comma-separated list. An annotation fixes its element type.
Otherwise the element type comes from the first element that is not a bare numeric literal or
`nil`, and an all-literal list takes a float type if any literal is a float. An empty literal
needs an annotation.

```
let xs: i32[] = [0, 1, 2, 4, 8]
ys := [1, 2.5]               # f32[]
let empty: f64[] = []
```

Values come from zero values, literals, and (later) built-in constructor functions for
capacity hints. Whether a value lives on the stack or the heap is the compiler's decision.

### Subranges and views

Indexing with a range yields a fresh copy. `&` instead yields a **view**, an ordinary `T[]`
sharing the original's storage, or, for a single index, a pointer to the element:

```
let ys: i32[] = xs[2..]      # copy of 2, 4, 8
let ws: i32[] = &xs[2..]     # view: ws[0] = 99 writes xs[2]
let p: i32^ = &xs[2]         # pointer to one element
```

**`&` shares elements; growth always forks.** A view starts with capacity equal to its
length, so appending to it reallocates into fresh storage. Appending returns the possibly-new
array:

```
xs = xs.push(9)
```

A function that appends returns the array. Arrays with outstanding views may reallocate on
growth, after which the views still see the old storage.

## Pointers

`T^` is a pointer to `T`. In value position the postfix `^` dereferences, prefix `&` takes an
address, and member access dereferences automatically. The caret is used only for pointers.

```
let p: Point^
let pp: Point^^
let ps: Point^[]         # array of pointers
let sp: Point[]^         # pointer to an array

p^.value = 10            # explicit dereference
let x: i32 = p.value     # automatic dereference
```

`nil` is the value of a pointer with no target. Dereferencing it yields zero values (see
[Memory and the Object Model](./memory-and-object-model.md)).

## Function types

A function type lists parameter types and the return type, without parameter names:

```
func run(step: func(Request, Duration): Response) { … }
```

Types are written where they are used. There is no alias declaration. A distinct nominal type
backed by an existing one (e.g. `LengthMm` over `i32`) is a planned feature with its own
keyword.

## Tuples

A tuple is an anonymous, structural product type of two or more elements, compared by shape.
`(T)` is just `T` in parentheses, and `()` is unit.

```
let pair: (i32, string) = (1, "a")
let nested: (i32, (string, bool))
```

## Variable bindings

`let` binds a name with a type, an initializer, or both. `:=` declares and initializes,
inferring the type:

```
let count: i32 = 0
let name: string
total := 0
```

A parenthesised pattern destructures a tuple positionally. Any annotation goes on the `let`,
not inside the pattern. The same shape on the left of `=` reassigns existing variables:

```
(x, y) := origin()
let (a, b): (i32, string) = row
(a, b) = (b, a)
```

## Logical and bitwise operators

A symbol is a unary prefix and a keyword is a binary infix: negation is `!`, and the binary
connectives are `and`, `or`, and `xor`. Each is logical on `bool` operands and bitwise on
integer operands. Mixed operands are a type error. On `bool`, `and` and `or` short-circuit.
Shifts are `<<` and `>>`.

```
if !ready and pending or !blocked then start()
```

## Chained comparison

A comparison chain running in one direction desugars to the conjunction of its adjacent
pairs, evaluating each middle operand once. `min <= x <= max` means `min <= x and x <= max`.
Chains must be all `<`/`<=` or all `>`/`>=`.
