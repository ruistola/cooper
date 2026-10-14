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

## `defer`

`defer STMT` runs `STMT` when the block it is in is left, after the statements deferred
later in that block, so deferred code unwinds last to first. It runs on every way out of the
block: falling off its end, `return`, `break`, and `continue`. Its scope is the block, not the
function, so a `defer` in a loop body runs at the end of each pass:

```
f := open(path)
defer close(f)
for line in lines do {
  mark := lock()
  defer unlock(mark)      # at the end of each pass
  if line == "" then continue
  process(line)
}
```

A deferred statement is the body of a function literal made at the `defer` and called at
each exit, so it captures the variables it names by reference, exactly as a closure does,
and sees their value at exit, not when it was deferred. An inner block that shadows a
variable does not change which one it refers to. A `return` or a value block evaluates its
value first and then runs the deferred code, which therefore cannot change that value, though
it can change what a returned pointer or array points to.

`defer` is a statement of a `{ }` block, not the single statement of an `if` or a loop. A
deferred statement cannot itself be a `defer`. Deferred code does not run when the program
panics.

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
