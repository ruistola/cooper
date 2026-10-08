//! Patterns and `match`.

use super::*;

impl Parser {
    fn parse_pattern(&mut self) -> PResult<Pattern> {
        let start = self.peek().span;
        match self.peek().kind {
            OpenParen => return self.parse_tuple_pattern(),
            True => {
                self.advance();
                return Ok(Pattern {
                    kind: PatternKind::Bool(true),
                    span: start,
                });
            }
            False => {
                self.advance();
                return Ok(Pattern {
                    kind: PatternKind::Bool(false),
                    span: start,
                });
            }
            Number => {
                let magnitude = self.expect(Number)?.text;
                return Ok(Pattern {
                    kind: PatternKind::Int {
                        negative: false,
                        magnitude,
                    },
                    span: start.to(self.prev_token().span),
                });
            }
            Dash => {
                self.expect(Dash)?;
                let magnitude = self.expect(Number)?.text;
                return Ok(Pattern {
                    kind: PatternKind::Int {
                        negative: true,
                        magnitude,
                    },
                    span: start.to(self.prev_token().span),
                });
            }
            _ => {}
        }
        let first = self.expect(Identifier)?.text;
        if first == "_" {
            return Ok(Pattern {
                kind: PatternKind::Wildcard,
                span: start,
            });
        }
        // A struct pattern names an uppercase type followed by a brace block of
        // field patterns: `Point { x: a, y: b }`.
        if self.peek().kind == OpenCurly {
            return self.parse_struct_pattern(first, start);
        }
        let (type_name, variant) = if self.peek().kind == Dot {
            self.expect(Dot)?;
            (Some(first), self.expect(Identifier)?.text)
        } else {
            (None, first)
        };
        // A bare camelCase identifier with no payload binds the whole value; an
        // uppercase one (or any name carrying payload binders) is a sum-type variant.
        if type_name.is_none()
            && self.peek().kind != OpenParen
            && !starts_uppercase(&variant)
        {
            return Ok(Pattern {
                kind: PatternKind::Binding(variant),
                span: start,
            });
        }
        let mut binders = Vec::new();
        if self.peek().kind == OpenParen {
            self.expect(OpenParen)?;
            while self.peek().kind != CloseParen {
                binders.push(self.expect(Identifier)?.text);
                if self.peek().kind == Comma {
                    self.expect(Comma)?;
                } else {
                    break;
                }
            }
            self.expect(CloseParen)?;
        }
        Ok(Pattern {
            kind: PatternKind::Variant {
                type_name,
                variant,
                binders,
            },
            span: start.to(self.prev_token().span),
        })
    }

    /// `( p1, p2, … )`. A single parenthesised pattern with no comma is a grouping,
    /// not a one-element tuple.
    fn parse_tuple_pattern(&mut self) -> PResult<Pattern> {
        let start = self.peek().span;
        self.expect(OpenParen)?;
        let mut elements = Vec::new();
        let mut had_comma = false;
        while self.peek().kind != CloseParen {
            elements.push(self.parse_pattern()?);
            if self.peek().kind == Comma {
                self.expect(Comma)?;
                had_comma = true;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        if elements.len() == 1 && !had_comma {
            return Ok(elements.pop().unwrap());
        }
        Ok(Pattern {
            kind: PatternKind::Tuple(elements),
            span: start.to(self.prev_token().span),
        })
    }

    /// `Name { field: pattern, … }`. `name` and the opening brace are already seen.
    fn parse_struct_pattern(&mut self, name: String, start: Span) -> PResult<Pattern> {
        self.expect(OpenCurly)?;
        let mut fields = Vec::new();
        while self.peek().kind != CloseCurly {
            let field_start = self.peek().span;
            let field = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let pattern = self.parse_pattern()?;
            fields.push(FieldPattern {
                name: field,
                pattern,
                span: field_start.to(self.prev_token().span),
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Pattern {
            kind: PatternKind::Struct { name, fields },
            span: start.to(self.prev_token().span),
        })
    }

    pub(super) fn parse_match_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Match)?;
        let scrutinee = self.parse_header_expr()?;
        self.expect(With)?;
        self.expect(OpenCurly)?;
        let mut arms = Vec::new();
        while self.peek().kind != CloseCurly && self.peek().kind != Eof {
            if self.peek().kind == Semicolon {
                self.advance();
                continue;
            }
            let pattern = self.parse_pattern()?;
            self.expect(FatArrow)?;
            let body = self.parse_branch_stmt()?;
            arms.push(StmtArm { pattern, body });
        }
        self.expect(CloseCurly)?;
        Ok(Stmt {
            kind: StmtKind::Match { scrutinee, arms },
            span: start.to(self.prev_token().span),
        })
    }

    pub(super) fn parse_match_expr(&mut self) -> PResult<ExprKind> {
        let scrutinee = self.parse_header_expr()?;
        self.expect(With)?;
        self.expect(OpenCurly)?;
        let mut arms = Vec::new();
        while self.peek().kind != CloseCurly && self.peek().kind != Eof {
            if self.peek().kind == Semicolon {
                self.advance();
                continue;
            }
            let pattern = self.parse_pattern()?;
            self.expect(FatArrow)?;
            let body = self.parse_branch_expr()?;
            arms.push(ExprArm { pattern, body });
            self.consume_statement_terminator()?;
        }
        self.expect(CloseCurly)?;
        Ok(ExprKind::Match {
            scrutinee: Box::new(scrutinee),
            arms,
        })
    }
}

/// Whether an identifier begins with an uppercase letter — the convention that
/// distinguishes a type or sum-type variant (`Point`, `None`) from a value binding.
fn starts_uppercase(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}
