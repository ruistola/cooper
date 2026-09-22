# Control Flow

## Decision

Cooper has one iteration construct, `for`, over a fixed set of built-in iterables,
and four conditional loops arranged as a pre-test/post-test × while/until matrix.

Every loop follows the same shape as `if … then`: `<header> <keyword> <body>`,
where the body is a braced block or a single statement. The delimiter keyword marks
where the header ends and the body begins, so a struct literal in a header reads
unambiguously.

Loops are **statements** and evaluate to unit.

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

`break` and `continue` are bare statements that target the innermost loop;
using either outside a loop is a compile error. Labels and break-with-value may
get introduced later but for now, they are out of scope.

## Iteration

* **Range** `a..b` is half-open and `a..=b` includes `b`. Both endpoints share one
  integer type, and a bare literal range such as `0..10` defaults to `i32` like any
  integer literal. A range binds a single loop variable of that element type and is
  meaningful only as a `for` iterable.
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

## Header expressions and newlines

Semicolon inference (see [Syntax Fundamentals](syntax-fundamentals.md)) rewrites a
newline to a statement terminator between statements. Inside a header expression —
an `if`/`while`/`until` condition, a `for` iterable, or a `match` scrutinee —
newlines are insignificant, exactly as they are inside brackets. A condition may
therefore wrap across lines, and its delimiter keyword may sit on a later line:

```
while a
  and b
do total += 1

for i in
  0..10
do total += i
```

A braced block is a statement context even when it appears inside a header, so its
own statements still terminate at newlines.

The post-test condition in `do BODY while COND` and `repeat BODY until COND` is the
final component of the statement, so its terminating newline ends the loop; a
multi-line post-test condition is parenthesised.
