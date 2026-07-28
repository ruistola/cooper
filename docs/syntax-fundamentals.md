# Syntax Fundamentals

## Source encoding

* Program sources are UTF-8.
  * Current lexer only tokenizes a narrow ASCII subset (to be expanded).
* Strings are by default encoded in UTF-8.
  * TODO: `rune` type for representing an individual code point (or grapheme — TBD).

## Semicolon inference

Semicolons act as statement separators and/or terminators. However, an endline can be
automatically converted into a semicolon, so explicit semicolons are rarely needed.

One reason to type an explicit semicolon: when a function or block expression is intended to
return nothing (the unit type).

* `foo` at the end of a block → the block's type is the type of `foo` (e.g. `i32`).
* `foo;` at the end of a block → the block's type is the unit type.

### Inference algorithm

Based on the Ahnfelt variant of the Scala implementation:

Given two consecutive tokens `a` and `b` separated by an `EOL` token (endline):

1. If `a` is in the `beforeSemicolon` category (a semicolon immediately after `a` is syntactically valid), **and**
2. If `b` is in the `afterSemicolon` category (a semicolon immediately before `b` is syntactically valid),

then the `EOL` token is converted into a semicolon.

### Additional details

* Redundant consecutive endlines are eliminated in the tokenization phase, as are all other
  whitespace tokens.
* Endline-to-semicolon conversions are disabled within parentheses (allowing multi-line
  expressions without escaping).

## Naming conventions

* **Types**: PascalCase (e.g., `MyStruct`, `Color`). Capitalization disambiguates types from instances.
* **Functions, methods, variables, fields**: camelCase (e.g., `myFunc`, `isValid`).
* **Visibility**: Not determined by capitalization (unlike Go). Separate mechanism TBD.

## Function type expressions

Function type expressions carry only the parameter types and return type — no parameter names:

```
type Handler = func(Request, Duration): Response
```

Parameter names are **not allowed** in function type expressions. Rationale: if names are
inconsequential (not checked or matched), their presence is misleading. Names belong in
function _declarations_ (where they're used in the body), not in type expressions.

```
// Function declaration — names required (used in body)
func process(req: Request, timeout: Duration): Response { ... }

// Function type expression — types only
type ProcessFn = func(Request, Duration): Response
```
