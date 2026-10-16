# Syntax Fundamentals

## Comments and documentation

A comment runs from `#` to the end of the line. A block of comment lines directly above a
declaration (a `struct` and its members, a `oneof` and its variants, a function or method, an
`extern`, a `type`, a module-level variable) documents it, as in Go: the lines are kept, without
the `#` and the space after it, and a blank line between the comment and the declaration detaches
it. Tools show it as the declaration's documentation. Any other comment is dropped.

## Source encoding

A source file is UTF-8, and a file that is not valid UTF-8 is rejected. Comments, string
literals, and rune literals may hold any text; identifiers and the rest of the syntax are
ASCII. A string literal is double-quoted and accepts the escapes `\n`, `\t`, `\r`, `\0`,
`\\`, `\"`, `\'`, `\{`, and `\u{HEX}` for a Unicode scalar value.

## Runes

A `rune` is a Unicode scalar value: a code point other than the surrogates, 0 to 0x10FFFF. It
is a type of its own, not an integer. Its zero value is U+0000. A rune literal is single
quoted and holds exactly one character or one escape (`\n`, `\t`, `\r`, `\0`, `\\`, `\'`,
`\"`, `\u{HEX}`):

```
r := 'x'
accent := 'é'
smile := '\u{1F600}'
```

Runes compare with `==` and ordering (`'a' <= r <= 'z'`), and match against rune literal
patterns. There is no arithmetic on them. `u32(r)` is the scalar value, and `rune(n)` makes a
rune from any integer, stopping the program if `n` is not a scalar value. `string(r)` is the
character as text; inside a printed value a rune is quoted, as a string is (`['a', 'b']`).

A `for` loop over a string yields its runes, and with two variables the byte offset each one
starts at, which is the offset slicing and searching use:

```
for r in s do …
for (i, r) in s do …       # i is an i64 byte offset
```

## String interpolation

Every string literal interpolates: a `{…}` inside it holds an expression, whose value is
formatted as `string(x)` formats it ([Type System](./type-system.md#formatting)) and spliced
in. `\{` writes a brace, and a `}` alone is literal.

```
x := 5
println("X is {x}, doubled {x * 2}")       # X is 5, doubled 10
println("p = {Point{x: 1, y: 2}}")         # p = Point{x: 1, y: 2}
println("{greet("bob")} \{literal}")       # a hole may hold strings and braces
```

A hole is any single expression, written on one line. A string inside a value is quoted as
`string(x)` quotes it (`"{[name]}"` gives `["Q"]`), while a string in a hole on its own is
spliced as is.

An empty hole `{}` is positional: it takes its value from the call the literal is the
template of, for an expression better kept outside the text. `std.fmt.format` returns the
text, and `print` and `println` accept the same arguments:

```
println("{} + {} = {}", a, b, a + b)
label := format("total {} of {limit}", sum(xs))
```

Each argument fills the next `{}` in order. The template must be a literal, and its `{}`
holes and the arguments must match in number, or the call is a compile error. The
template's own holes are evaluated first, then the arguments, each once, in order.

### Format specs

After a `:` in a hole, a spec says how to format its value, in Rust's syntax
`[[fill]align][0][width][.precision][radix]`:

| Spec | Meaning | Example |
|---|---|---|
| width | at least this many characters | `"{n:5}"` → `  255` |
| `<` `>` `^` | align left, right, or center within the width; a number is right-aligned by default, anything else left | `"{s:>5}"` → `   ab` |
| fill | the character to pad with, before the alignment | `"{n:*^7}"` → `**255**` |
| `0` | pad a number with zeros after its sign | `"{-42:06}"` → `-00042` |
| `.precision` | digits after a float's point | `"{f:.2}"` → `3.14` |
| `x` `X` `b` `o` | an integer in hexadecimal, binary, or octal; a negative one as its two's complement bits | `"{n:08x}"` → `000000ff` |

A positional hole takes a spec the same way: `"{:>4}"`. A radix applies to integers, a
precision to floats, and zero padding to numbers; any other use is a compile error. An
integer literal given a precision is a float literal: `"{1:.2}"` gives `1.00`. The `:`
that begins a spec is the first one outside the expression's brackets, braces,
parentheses, and strings, and not part of `:=`.

## Semicolon inference

Semicolons separate statements, and a newline becomes one when both of these hold, following
Ahnfelt's variant of Scala's rule:

1. the token before the newline can end a statement, and
2. the token after it can begin one.

Newlines are insignificant inside parentheses and brackets, inside `if`/loop/`match` headers
(see [Control Flow](./control-flow.md#header-expressions-and-newlines)), and directly before a
closing `}`. Since a prefix operator (`-`, `+`, `!`) can begin a statement, a line starting
with one is a new statement; to continue an expression onto the next line, end the line with
its operator (`total := a +`) or wrap the expression in parentheses. A block evaluates to its trailing expression; ending the block with an explicit
`;` makes it unit instead:

```
{ foo }      # the type of foo
{ foo; }     # unit
```

## Naming conventions

Types are PascalCase; functions, methods, variables, and fields are camelCase. Casing carries
no visibility meaning. The visibility mechanism is still to be designed.

## Arrays

`T[]` is the default sequence type: a growable run of elements with a length and capacity. An
array value refers to its elements, so assigning or passing an array, or slicing one, shares
them; `copy()` makes an independent copy. A **static array** `T[N]` has a fixed length,
written as an integer literal, and is covered [below](#static-arrays).

```
let items: i32[]
let matrix: i32[][]
let callbacks: func(i32)[]
let rgb: u8[3]
let grid: i32[3][2]
```

An array literal is a bracketed, comma-separated list. Its elements share one type, which an
annotation or any use of the elements or the array fixes, and an all-literal list takes a
float type if any literal is a float. An empty literal takes its element type from later uses
(`xs := []`, then `xs = xs.push(1)`); if nothing constrains it, it needs an annotation.

```
let xs: i32[] = [0, 1, 2, 4, 8]
ys := [1, 2.5]               # f32[]
let empty: f64[] = []
```

An index may be of any integer type; one outside the array, negative included, stops the
program with a runtime error. Lengths, and the positions built-in iteration exposes, are
`i64`: signed, so `length - 1` on an empty array is -1, and still wider than any addressable
size. `&xs[i]` is a pointer to one element, and `&xs` a pointer to the array variable itself.

### Slices

`xs[lo..hi]` (or `lo..=hi`) is an array of the selected elements, sharing them with `xs`. An
omitted `lo` is 0 and an omitted `hi` the array's end (`xs[2..]`, `xs[..n]`, `xs[..]`). Bounds
outside `0 <= lo <= hi <= length` stop the program.

```
tail := xs[2..]              # shares 2, 4, 8 with xs: tail[0] = 99 writes xs[2]
copied := xs[2..].copy()     # an independent copy
```

### Growth

Appending follows a return-value idiom, since it may hand back the same array (when there is
spare capacity) or one in fresh storage, which no other array shares:

```
xs = xs.push(9)
xs = xs.reserve(1000)        # room for at least 1000 more elements
```

A function that appends returns the array, or takes a pointer to the caller's array
(`log^ = log^.push(line)`). **A slice's capacity is its length**, so growing a slice always
moves it to fresh storage first and never writes into the array it came from. Two copies of
one array with spare capacity both append into the same storage, so after `xs = xs.push(..)`
other copies of the old `xs` should not be appended to. Arrays shared with a grown array keep
seeing its old storage.

### Static arrays

A static array `T[N]` is a value of exactly `N` elements, stored inline: in a struct, it is
part of the struct. Assigning, passing, or returning one copies it, and its zero value has
every element zero. A literal takes its length from the type it is expected to have, which an
annotation, parameter, or return type gives, and the count must match; without such a type
a literal is a `T[]`.

```
let rgb: u8[3] = [10, 20, 30]
let zero: i32[4]            # [0, 0, 0, 0]
b := rgb                    # a copy
b[0] = 99                   # rgb is unchanged
rgb.length()                # 3
```

Types are read left to right, as `Point^[]` is: `i32[3][2]` is an array of 2 arrays of 3
`i32`, so `grid[row][col]` with `row < 2` and `col < 3`.

A static array has indexing, `for` iteration, and `length()`, but no `push`, `reserve`, or
`copy()`, since its length is fixed and assignment copies. To use one where a `T[]` is
expected, slice it: `a[..]` is a `T[]` sharing the array's elements, so a function taking
`i32[]` serves static and dynamic arrays alike. Like any slice its capacity is its length,
so `push` on it never writes into the array. Only an array stored in a variable (or a field
or element of one) can be sliced; a function's result must be assigned first.

```
func sum(xs: i32[]): i32 { … }
sum(nums[..])               # nums: i32[4]
```

Static arrays are not comparable with `==` yet.

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

A function type lists parameter types and the return type. Parameter names may be written as
documentation; they are not part of the type and never need to match the names of the function
passed in:

```
func run(step: func(Request, Duration): Response) { … }
func read(into: func(buf: u8[], limit: i32): i32) { … }
```

### Type aliases

`type Name = T` gives a type a name. An alias is transparent: it *is* the type, so any value of
the aliased type fits and nothing about the type changes. Its purpose is documentation, with a doc
comment on the alias, and shorter signatures. It is the way to document a tuple or a function type
that a signature would otherwise have to spell out, since the elements of a tuple have no names:

```
# A rectangle: left, top, width, height.
type Rect = (i32, i32, i32, i32)

# Called for each token; reports errors seen, bytes used, and whether more pages remain.
type TokenHandler = func(tok: u8, data: u8[], errIf: u8): (bool, i32, bool)

func draw(r: Rect, handle: TokenHandler) { … }
```

An alias may name another alias but not itself, takes no type arguments, and is declared at module
level, importable with `use { geometry.Rect }`. Messages show the type it stands for. When the
parts of a value should be reached by name, declare a `struct`. A distinct nominal type backed by
an existing one (e.g. `LengthMm` over `i32`) is a planned feature with its own keyword.

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
connectives are `and`, `or`, and `xor`. Each of the four is logical on `bool` operands and
bitwise on integer operands, so `!` on an integer is its complement (`!0` is `-1`). Mixed operands are a type error. Which of the two applies is a predicate
on the operands' type, decided once inference has solved the body, so a later use may
fix it. On `bool`, `and` and `or` short-circuit, while `xor` evaluates both operands, since
its result depends on both. On integers, all three evaluate both operands, like arithmetic.
Shifts are `<<` and `>>`, binding tighter than comparison and looser than `+` and `-` as in
C, so `1 << n - 1` is `1 << (n - 1)` (see [Numeric Types](./numeric-types.md) for their
semantics). Only the arithmetic operators have compound assignments (`+=` `-=` `*=` `/=`
`%=`); a bitwise update is written out, as in `x = x << 8`.

`and` binds tighter than `xor`, which binds tighter than `or`, and all three bind looser than
comparison. Binary operators on the same level group left to right. `and` over `or` is the order boolean
connectives take in mainstream languages, and `and` over `xor` over `or` is the order the
bitwise operators take in C, Java, Rust, and Python, so one keyword holds one position
whatever its operands. Sitting below comparison lets a condition read naturally
(`x != nil and x.count > 10`). It differs from Rust, Python, and Go, where bitwise operators
bind tighter than comparison, so `mask and 0xFF == 0` groups as `mask and (0xFF == 0)`: an
integer `and` a `bool`, which is a type error rather than the silent misparse C allows.
Write `(mask and 0xFF) == 0`.

```
if !ready and pending or !blocked then start()   # (!ready and pending) or !blocked
```

## Chained comparison

A comparison chain running in one direction desugars to the conjunction of its adjacent
pairs, evaluating each middle operand once. `min <= x <= max` means `min <= x and x <= max`.
Like `and`, a chain stops at its first false pair, leaving later operands unevaluated.
Chains must be all `<`/`<=` or all `>`/`>=`; mixing directions is a syntax error. A
parenthesized comparison is an ordinary `bool` operand, so `(a < b) < c` is not a chain.
