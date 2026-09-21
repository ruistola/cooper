# Control Flow

## Decision

Cooper has one iteration construct, `for`, over a fixed set of built-in iterables,
and four conditional loops arranged as a pre-test/post-test × while/until matrix.
There is **no C-style three-clause `for`**: the ranged form and `while` together
cover its uses without a special comma-and-semicolon header.

Every loop follows the shipped `if … then` shape: `<header> <keyword> <body>`,
where the body is a braced block or a single statement. The delimiter keyword —
never a brace — marks where the header ends and the body begins, so a struct
literal in a header is unambiguous and needs no parenthesisation rule.

Loops are **statements**, not expressions, and evaluate to unit. There is no
`loop`, no for-expression, and no `break value`; these are deferred.

## The loop matrix

`while` and `do` are partners, as are `until` and `repeat`. A pre-test loop uses
its partner keyword as the body delimiter; a post-test loop uses it as the leading
opener, so the body always runs at least once.

```
while COND do BODY        // pre-test,  repeat while COND is true
until COND repeat BODY    // pre-test,  repeat while COND is false
do BODY while COND        // post-test, runs ≥1×, repeat while COND is true
repeat BODY until COND    // post-test, runs ≥1×, repeat while COND is false
```

```
for BINDINGS in ITER do BODY   // ranged iteration
```

```
BODY     ::= "{" stmt* "}" | stmt
BINDINGS ::= ident | "(" ident ("," ident)+ ")"
```

`break` and `continue` are bare statements. They target the **innermost** loop;
there are no loop labels yet. Using either outside a loop is a compile error.

Cooper uses `continue`, not `next`: because a block is an expression, a trailing
bare `next` would already mean "this block's value is the variable `next`", so a
contextual `next` keyword would be ambiguous.

## Iteration

Only built-in iterables are supported; user-defined iteration awaits the
constraint/protocol work.

* **Range** `a..b` is half-open. Both endpoints must share one integer type, and a
  bare literal range such as `0..10` defaults to `i32` like any integer literal. A
  range binds a single loop variable of that element type. A range is meaningful
  *only* as a `for` iterable; writing `a..b` in any other position is an error.
* **Array** `T[]` binds either the element (`for v in xs`) or an
  `(index: i32, value: T)` pair (`for (i, v) in xs`).
* Any other iterand type is rejected with "cannot iterate over type …".

```
total := 0
for i in 0..10 do total += i          // 0,1,…,9

for (i, v) in xs do total += i + v    // index and element

n := 0
while n < 5 do {
  n += 1
  if n == 3 then continue
  if n == 4 then break
}

repeat n -= 1 until n == 0            // runs at least once
```

## Semicolon inference

Semicolon inference (see [Syntax Fundamentals](syntax-fundamentals.md)) treats the
loop-*starting* keywords `while`, `do`, `until`, `repeat`, `break`, and `continue`
as tokens a preceding statement may terminate before, so a statement on the line
above a loop closes cleanly.

A consequence is that a **delimiter keyword must stay on the same line as its
header expression**: a newline immediately before `do`, `while`, `until`, or
`repeat` would be read as a statement terminator. A post-test `do { … } while C`
still spans multiple lines, because a closing `}` never precedes an inferred
semicolon.
