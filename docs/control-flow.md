# Control Flow

Cooper has one iteration construct, `for`, over built-in iterables, and four conditional
loops forming a pre-test/post-test × while/until matrix. Every loop has the shape of
`if … then`: a header, a delimiter keyword, then a body that is a braced block or a single
statement. Loops are statements of unit type.

## The loop matrix

`while` and `do` are partners, as are `until` and `repeat`. A pre-test loop uses
its partner keyword as the body delimiter; a post-test loop uses it as the leading
opener, so the body always runs at least once.

```
while COND do BODY        # pre-test,  repeat while COND is true
until COND repeat BODY    # pre-test,  repeat while COND is false
do BODY while COND        # post-test, runs ≥1×, repeat while COND is true
repeat BODY until COND    # post-test, runs ≥1×, repeat while COND is false
```

```
for BINDINGS in ITER do BODY   # ranged iteration
```

```
BODY     ::= "{" stmt* "}" | stmt
BINDINGS ::= ident | "(" ident ("," ident)+ ")"
```

`break` and `continue` target the innermost loop. Using either outside a loop is a compile
error.

## Iteration

* **Range:** `a..b` is half-open and `a..=b` includes `b`. Both endpoints share one integer
  type (a bare `0..10` is `i32`), which the single loop variable takes. A range is valid only
  as a `for` iterable.
* **Array:** `T[]` binds the element (`for v in xs`) or an index and the element
  (`for (i, v) in xs`, with `i: i64`).

The loop variables are bound afresh on each pass from a hidden position, so assigning one
lasts only until the end of that pass and never changes the iteration, and a closure created
in a pass keeps that pass's variable.

```
total := 0
for i in 0..10 do total += i          # 0,1,…,9

for (i, v) in xs do total += i + v    # index and element

n := 0
while n < 5 do {
  n += 1
  if n == 3 then continue
  if n == 4 then break
}

repeat n -= 1 until n == 0            # runs at least once
```

## Header expressions and newlines

Newlines are insignificant inside a header expression (an `if`/`while`/`until` condition, a
`for` iterable, or a `match` scrutinee), so a header may wrap and its delimiter keyword may
sit on a later line:

```
while a
  and b
do total += 1

for i in
  0..10
do total += i
```

A braced block inside a header is still a statement context. A post-test condition ends at
its newline, so a multi-line one is parenthesised.
