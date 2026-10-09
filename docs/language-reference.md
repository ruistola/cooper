# Language Reference

A compact reference for reading and writing Cooper source: lexical rules, a grammar sketch,
and the constructs most often written wrongly. It is a quick aid, not a specification. The
other documents in this directory give the design and its reasons, and the programs in
`crates/cooper-cli/tests/programs/` are known-good examples.

**Cooper is its own language.** It resembles Go, Rust, and Kotlin in places, and those
resemblances mislead. Keywords delimit headers and bodies (`if … then`, `while … do`,
`match … with`), and curly braces mean a block of several statements or a data declaration,
never a single branch. `cooper run file.coop` builds and runs a file, and `cooper check`
only reports diagnostics.

## Not Rust, not Go

| Instead of | Write |
|---|---|
| `fn add(x: i32, y: i32) -> i32 { … }` | `func add(x: i32, y: i32): i32 { … }` |
| `let mut x = 5;`, `var x = 5` | `x := 5` or `let x: i32 = 5` (bindings are mutable; there is no `mut`) |
| `if x > 0 { … } else { … }` | `if x > 0 then … else …` (a body is one statement or a `{ … }` block) |
| `while x < 9 { … }`, `for i in 0..9 { … }` | `while x < 9 do …`, `for i in 0..9 do …` |
| `match x { A => 1, B => 2 }` | `match x with {`, then arms `A => 1`, separated like statements (a newline or `;`), never by commas |
| `impl Point { fn len(&self) … }` | `(p: Point) func len(): i32 { … }`, declared outside the struct |
| `*p`, `&mut x`, `Box<T>` | `p^`, `&x`, `T^` (`^` is the pointer sigil in types and values) |
| `Vec<i32>`, `[]int`, `Option<T>`, `Map<K, V>` | `i32[]`, `Maybe T`, `Map K V` (type application by juxtaposition) |
| `fn id<T>(x: T) -> T` | `func id T (x: T): T` (binders follow the name; no angle brackets) |
| `enum Maybe<T> { Some(T), None }` | `oneof Maybe T { None, Some(T) }` (list a lone payloadless variant first) |
| `Maybe::Some(5)` | `Maybe.Some(5)`, or bare `Some(5)` where inference fixes the sum type |
| `use std::io;`, `import "fmt"` | `use { std.io }` (a braced block of dotted paths) |
| `println!("{}", n)`, `fmt.Println(n)` | `std.io.println("n = " + string(n))` (the argument is a `string`) |
| `x as i64`, `int64(x)` | `i64(x)` (a conversion is a call on the target type) |
| `a && b`, `a \|\| b` | `a and b`, `a or b` (`!a` is prefix not) |
| `i++`, `i--` | `i += 1`, `i -= 1` |
| `// comment` | `# comment` |
| `null`, `None` for a missing pointer | `nil` |
| `cond ? a : b` | `if cond then a else b` (an expression; `else` is required) |
| `t.0` on a tuple | destructure: `(a, b) := t` |
| `try`, `?`, exceptions | none; model failure with a sum type such as `Result T E` |

## A tour

```
use {
    std.io,
}

struct Point {
    x: i32,
    y: i32,
}

oneof Shape {
    Empty,
    Circle(i32),
    Rect(i32, i32),
}

oneof Maybe T {
    None,
    Some(T),
}

(p: Point) func sum(): i32 {         # value receiver: operates on a copy
    return p.x + p.y
}

(p: Point^) func shift(dx: i32) {    # pointer receiver: mutates the caller's value
    p.x += dx
}

func area(s: Shape): i32 {
    return match s with {
        Circle(r) => 3 * r * r
        Rect(w, h) => w * h
        Empty => 0
    }
}

func first T (xs: T[]): Maybe T {    # generic: binder `T` after the name
    if xs.length() == 0 then return None
    return Some(xs[0])
}

func main(): i32 {
    p := Point{x: 1, y: 2}
    p.shift(10)
    total := p.sum()
    xs := [3, 4, 5]
    xs = xs.push(6)                  # growth returns the array
    for (i, v) in xs do total += i32(i) + v
    n := 0
    while n < 3 do n += 1
    if area(Rect(n, 2)) != 6 then return 1
    if first(xs) == Some(3) then std.io.println("ok: " + string(total))
    return 0
}
```

## Common forms

```
x := 3                               # declare, infer the type
let y: i64 = 3                       # annotate; `let z: i32` alone holds the zero value
(lo, hi) := (1, 2)                   # destructure a tuple; `(lo, hi) = (hi, lo)` reassigns
biggest := if a > b then a else b    # `if` as an expression
if a < b then a = 10 else a = 20     # `if` as a statement
while n < 3 do n += 1                # also `until … repeat`, `do … while`, `repeat … until`
for i in 0..10 do total += i         # `..` is half-open, `..=` inclusive
for (i, v) in xs do total += i32(i) + v   # index (an `i64`) and element
tail := xs[1..]                      # a slice shares elements: `xs[1..=2]`, `xs[..n]`
p := &a                              # `p^ += 1` writes through it; `p.field` dereferences
let box: Box i32 = Box{v: 7}         # struct literal; generic types apply by juxtaposition
let f: func(i32): i32 = inc          # function type; a bound method is a value too
add := func(x: i32): i32 { return x + k }   # a function literal, capturing `k` by reference
kind := match n with {               # integer, bool, tuple, struct, or sum-type scrutinee
    5 => 1
    _ => 0
}
```

## Lexical structure

* Comments run from `#` to the end of the line. Source is UTF-8; the lexer accepts an ASCII
  subset.
* Identifiers are `[A-Za-z_][A-Za-z0-9_]*`. Types are PascalCase and everything else is
  camelCase. Casing matters to the compiler only in patterns (below).
* Reserved words: `and as break continue do else false for func if in let match nil oneof or
  repeat return struct then true until use while with xor`.
* Numbers: `42`, `1_000`, `0xFF`, `0b1010`, `3.14`, `1e9`, `2.5e-3`. A float needs digits on
  both sides of the point. Strings are `"…"` with the escapes `\n \t \r \0 \\ \" \'`.
* A newline ends a statement when the token before it can end one and the token after it can
  begin one; `;` also separates. Newlines are ignored inside `()` and `[]`, inside `if`, loop,
  and `match` headers, and before a closing `}`. A line starting with `-`, `+`, or `!` begins
  a new statement, so continue an expression by ending the line with its operator or by
  parenthesising it.

## Grammar sketch

Notation: `"x"` is a literal token, `a*` zero or more, `a+` one or more, `a?` optional, `|` a
choice. `ident`, `number`, and `string` are tokens. Lists are comma-separated with an
optional trailing comma.

```
file       ::= useBlock* decl*
useBlock   ::= "use" "{" useItem* "}"                 # items: comma-separated
useItem    ::= path ("as" ident)? | path "." "{" (ident ("as" ident)?)* "}"
path       ::= ident ("." ident)*

decl       ::= struct | oneof | func | "let" …        # a module-level `let` parses; lowering rejects it
struct     ::= "struct" ident ident* "{" member* "}"  # ident* are type-parameter binders
member     ::= ident ":" type
oneof      ::= "oneof" ident ident* "{" variant* "}"
variant    ::= ident ("(" type* ")")?
func       ::= receiver? "func" ident ident* "(" param* ")" (":" type)? block
receiver   ::= "(" ident ":" type ")"
param      ::= ident ":" type

type       ::= typeAtom typeAtom*                     # juxtaposition: `Map string i32`
typeAtom   ::= (ident | "(" type* ")" | funcType) ("[" "]" | "^")*
funcType   ::= "func" "(" type* ")" (":" type)?       # `()` is unit; `(A, B)` a tuple

block      ::= "{" stmt* "}"
body       ::= block | stmt
stmt       ::= "let" ident (":" type)? ("=" expr)?
             | "let" "(" ident+ ")" (":" type)? "=" expr
             | ident ":=" expr | "(" ident+ ")" ":=" expr
             | "if" expr "then" body ("else" body)?
             | "while" expr "do" body | "until" expr "repeat" body
             | "do" body "while" expr | "repeat" body "until" expr
             | "for" bindings "in" expr "do" body
             | "match" expr "with" "{" arm* "}"
             | "return" expr? | "break" | "continue"
             | "func" ident "(" param* ")" (":" type)? block   # nested function, not generic
             | expr                                   # a call, an assignment, …
bindings   ::= ident | "(" ident "," ident ")"
arm        ::= pattern "=>" body                      # arms are separated like statements: a newline or ";", never ","

expr       ::= number | string | "true" | "false" | "nil" | ident | "(" ")"
             | "(" expr ")" | "(" expr ("," expr)+ ")" | "[" expr* "]"
             | expr "(" expr* ")"                     # call
             | expr "." ident | expr "^"              # field, dereference
             | expr "[" expr "]"                      # index
             | expr "[" expr? (".." | "..=") expr? "]"   # slice
             | expr "{" (ident ":" expr)* "}"         # struct literal: Point{x: 1, y: 2}
             | ("-" | "+" | "!" | "&") expr | expr binop expr
             | "if" expr "then" expr "else" expr
             | "match" expr "with" "{" (pattern "=>" expr)* "}"
             | "{" stmt* expr "}"                     # a value block
             | "func" "(" param* ")" (":" type)? block   # function literal

pattern    ::= "_" | ident | "true" | "false" | "-"? number
             | (ident ".")? ident ("(" (ident | "_")* ")")?   # variant: `Some(x)`, `None`
             | "(" pattern ("," pattern)+ ")"                 # tuple
             | ident "{" (ident ":" pattern)* "}"             # struct: `Point { x: a }`
```

In a pattern, a lowercase identifier alone binds the value and an uppercase one is a variant.

## Operators

From loosest to tightest binding:

| Level | Operators |
|---|---|
| 1 | `=` `+=` `-=` `*=` `/=` `%=` and `:=` (right-associative) |
| 2 | `..` `..=` (valid only as a `for` iterable or a slice bound) |
| 3 | `or` |
| 4 | `xor` |
| 5 | `and` |
| 6 | `==` `!=` |
| 7 | `<` `<=` `>` `>=` (chainable in one direction: `a <= x < b`) |
| 8 | `<<` `>>` |
| 9 | `+` `-` |
| 10 | `*` `/` `%` |
| 11 | prefix `-` `+` `!` `&` |
| 12 | postfix call `f(…)`, index `a[i]`, struct literal `T{…}`, field `.x`, dereference `^` |

Binary operators group left to right, so `10 - 3 - 2` is `5`, and assignment groups right to
left. `and`, `or`, and `xor` are logical on `bool` and bitwise on integers; only the logical
`and` and `or` short-circuit, and `xor` and the bitwise operators evaluate both operands.
Comparisons bind tighter than all three, so `x != nil and x.count > 10` needs no parentheses
([why](./syntax-fundamentals.md#logical-and-bitwise-operators)).

## Types

* Primitives: `i8 i16 i32 i64`, `u8 u16 u32 u64`, `f32 f64`, `bool`, `string`. Unit is `()`.
* Composites: tuple `(A, B)`, array `T[]`, pointer `T^`, function `func(A, B): R`.
* Declared types are nominal: `struct` and `oneof`, optionally generic (`Box T`, `Result T E`).
* There is no implicit numeric conversion. Arithmetic needs both operands to share one type,
  and a literal takes the type its uses give it ([Numeric Types](./numeric-types.md)).

## Behaviors worth remembering

* Values are stored inline and copied on assignment. Arrays share their elements, and
  pointers share their target. Grow an array with `xs = xs.push(v)`.
* Every type has a zero value, and `let x: T` alone holds it. Dereferencing `nil` yields zero
  values and discards writes; it does not fault. A sum type's zero value is its first variant,
  so list a lone payloadless variant first.
* Integer overflow, division by zero, an out-of-range shift count, and an out-of-range index
  stop the program with a runtime error (exit status 101).
* A function body leaves with a value only through `return`. A block evaluates to its trailing
  expression, and ending it with `;` makes it unit. `main` is `func main()` or
  `func main(): i32`, whose result is the exit status.
* `match` must be exhaustive: name every variant, or end with `_` or a binding.
* A method name may not collide with a field name, functions are never overloaded, and a
  module's top-level names share one unordered namespace across its files.
* `==` works on comparable types only: not function types, arrays, or types containing them.
* A function literal or nested function captures the variables it names by reference: they
  are shared with the enclosing body, not copied. Each execution of a binding, including
  each pass's loop variable, is a fresh variable, so a closure made in a loop keeps its own.
* Output goes through `std.io` (`print` and `println`, each taking a `string`).

## Not implemented yet

These are designed or reserved but rejected today. Do not write them:

* Generic nested functions, and parameter types inferred for function literals.
* Module-level variables, static arrays `T[N]`, and `extern func`.
* Methods across sum-type variants, and a standard library function used as a value.
* Named-field variant payloads, nested patterns inside variant payloads, or-patterns, guards.

## Where to read more

[Syntax Fundamentals](./syntax-fundamentals.md), [Control Flow](./control-flow.md),
[Methods and Functions](./methods-and-functions.md), [Sum Types](./sum-types.md),
[Generics](./generics.md), [Numeric Types](./numeric-types.md),
[Modules and Projects](./modules-and-projects.md), [Type System](./type-system.md),
[Memory and the Object Model](./memory-and-object-model.md), and
[Polymorphism](./polymorphism.md).
