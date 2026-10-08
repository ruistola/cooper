//! Declarations: functions, methods, structs, sum types, `use` blocks, and the type
//! expressions their signatures contain.

use super::*;

impl Parser {
    pub(super) fn parse_type_expr(&mut self) -> PResult<TypeExpr> {
        let head = self.parse_type_atom()?;
        if !self.next_starts_type_atom() {
            return Ok(head);
        }
        let start = head.span;
        let mut args = Vec::new();
        while self.next_starts_type_atom() {
            args.push(self.parse_type_atom()?);
        }
        Ok(TypeExpr {
            kind: TypeExprKind::Application {
                constructor: Box::new(head),
                args,
            },
            span: start.to(self.prev_token().span),
        })
    }

    fn next_starts_type_atom(&mut self) -> bool {
        matches!(self.peek().kind, Identifier | OpenParen | Func)
    }

    fn parse_type_atom(&mut self) -> PResult<TypeExpr> {
        let start = self.peek().span;
        let mut kind = if self.peek().kind == OpenParen {
            self.expect(OpenParen)?;
            let inner = if self.peek().kind == CloseParen {
                TypeExprKind::Unit
            } else {
                let mut elems = vec![self.parse_type_expr()?];
                while self.peek().kind == Comma {
                    self.expect(Comma)?;
                    if self.peek().kind == CloseParen {
                        break;
                    }
                    elems.push(self.parse_type_expr()?);
                }
                if elems.len() == 1 {
                    elems.pop().unwrap().kind
                } else {
                    TypeExprKind::Tuple(elems)
                }
            };
            self.expect(CloseParen)?;
            inner
        } else if self.peek().kind == Func {
            self.parse_func_type_expr()?
        } else {
            TypeExprKind::Named(self.expect(Identifier)?.text)
        };
        let mut ty = TypeExpr {
            kind,
            span: start.to(self.prev_token().span),
        };
        loop {
            kind = match self.peek().kind {
                OpenBracket => {
                    self.expect(OpenBracket)?;
                    self.expect(CloseBracket)?;
                    TypeExprKind::Array(Box::new(ty))
                }
                Chevron => {
                    self.expect(Chevron)?;
                    TypeExprKind::Pointer(Box::new(ty))
                }
                _ => return Ok(ty),
            };
            ty = TypeExpr {
                kind,
                span: start.to(self.prev_token().span),
            };
        }
    }

    fn parse_func_type_expr(&mut self) -> PResult<TypeExprKind> {
        self.expect(Func)?;
        self.expect(OpenParen)?;
        let mut params = Vec::new();
        while self.peek().kind != CloseParen {
            if self.peek().kind == Identifier {
                let name_tok = self.expect(Identifier)?;
                if self.peek().kind == Colon {
                    self.expect(Colon)?;
                    params.push(self.parse_type_expr()?);
                } else {
                    params.push(TypeExpr {
                        kind: TypeExprKind::Named(name_tok.text),
                        span: name_tok.span,
                    });
                }
            } else {
                params.push(self.parse_type_expr()?);
            }
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        let ret = if self.peek().kind == Colon {
            self.expect(Colon)?;
            self.parse_type_expr()?
        } else {
            let span = self.prev_token().span;
            TypeExpr {
                kind: TypeExprKind::Unit,
                span,
            }
        };
        Ok(TypeExprKind::Func {
            params,
            ret: Box::new(ret),
        })
    }

    pub(super) fn parse_func_decl(&mut self) -> PResult<FuncDecl> {
        let start = self.peek().span;
        let receiver = if self.peek().kind == OpenParen {
            let rstart = self.peek().span;
            self.expect(OpenParen)?;
            let name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let ty = self.parse_type_expr()?;
            self.expect(CloseParen)?;
            Some(TypedIdent {
                name,
                ty,
                span: rstart.to(self.prev_token().span),
            })
        } else {
            None
        };
        self.expect(Func)?;
        let name = self.expect(Identifier)?.text;
        let mut type_params = Vec::new();
        while self.peek().kind == Identifier {
            type_params.push(self.expect(Identifier)?.text);
        }
        self.expect(OpenParen)?;
        let mut params = Vec::new();
        while self.peek().kind != CloseParen {
            let pstart = self.peek().span;
            let param_name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let param_type = self.parse_type_expr()?;
            params.push(TypedIdent {
                name: param_name,
                ty: param_type,
                span: pstart.to(self.prev_token().span),
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseParen)?;
        let return_type = if self.peek().kind == Colon {
            self.expect(Colon)?;
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        self.expect(OpenCurly)?;
        let body = self.parse_block_stmt();
        self.expect(CloseCurly)?;
        Ok(FuncDecl {
            receiver,
            name,
            type_params,
            params,
            return_type,
            body,
            span: start.to(self.prev_token().span),
        })
    }

    pub(super) fn parse_struct_decl_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Struct)?;
        let name = self.expect(Identifier)?.text;
        let mut type_params = Vec::new();
        while self.peek().kind == Identifier {
            type_params.push(self.expect(Identifier)?.text);
        }
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut members = Vec::new();
        while self.peek().kind != CloseCurly {
            let mstart = self.peek().span;
            let member_name = self.expect(Identifier)?.text;
            self.expect(Colon)?;
            let member_type = self.parse_type_expr()?;
            members.push(TypedIdent {
                name: member_name,
                ty: member_type,
                span: mstart.to(self.prev_token().span),
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Stmt {
            kind: StmtKind::StructDecl {
                name,
                type_params,
                members,
            },
            span: start.to(self.prev_token().span),
        })
    }

    pub(super) fn parse_oneof_decl_stmt(&mut self) -> PResult<Stmt> {
        let start = self.peek().span;
        self.expect(Oneof)?;
        let name = self.expect(Identifier)?.text;
        let mut type_params = Vec::new();
        while self.peek().kind == Identifier {
            type_params.push(self.expect(Identifier)?.text);
        }
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut variants = Vec::new();
        while self.peek().kind != CloseCurly {
            let vstart = self.peek().span;
            let variant_name = self.expect(Identifier)?.text;
            let mut payload = Vec::new();
            if self.peek().kind == OpenParen {
                self.expect(OpenParen)?;
                while self.peek().kind != CloseParen {
                    payload.push(self.parse_type_expr()?);
                    if self.peek().kind == Comma {
                        self.expect(Comma)?;
                    } else {
                        break;
                    }
                }
                self.expect(CloseParen)?;
            }
            variants.push(VariantDef {
                name: variant_name,
                payload,
                span: vstart.to(self.prev_token().span),
            });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(Stmt {
            kind: StmtKind::OneofDecl {
                name,
                type_params,
                variants,
            },
            span: start.to(self.prev_token().span),
        })
    }

    pub(super) fn parse_use_block(&mut self) -> PResult<Vec<UseSpec>> {
        self.expect(Use)?;
        self.expect(OpenCurly)?;
        self.paren_stack.push(OpenCurly);
        let mut specs = Vec::new();
        while self.peek().kind != CloseCurly {
            self.parse_use_entry(&mut specs)?;
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(specs)
    }

    /// Parse one comma-separated entry of a use block. An entry is a dotted path
    /// with an optional trailing `as` rename, or a path prefix followed by a braced
    /// group (`prefix.{ a, b as c }`) that expands into one binding per item. Every
    /// resulting binding is appended to `specs`, flattening groups away.
    fn parse_use_entry(&mut self, specs: &mut Vec<UseSpec>) -> PResult<()> {
        let start = self.peek().span;
        let mut prefix = vec![self.expect(Identifier)?.text];
        loop {
            if self.peek().kind != Dot {
                break;
            }
            self.expect(Dot)?;
            if self.peek().kind == OpenCurly {
                return self.parse_use_group(&prefix, specs);
            }
            prefix.push(self.expect(Identifier)?.text);
        }
        let alias = self.parse_use_alias()?;
        let span = Span::new(start.start, self.current_token().span.end);
        specs.push(UseSpec {
            path: prefix,
            alias,
            span,
        });
        Ok(())
    }

    /// Parse a braced group of item bindings sharing `prefix` as their module path,
    /// appending one flattened `UseSpec` per item.
    fn parse_use_group(&mut self, prefix: &[String], specs: &mut Vec<UseSpec>) -> PResult<()> {
        self.expect(OpenCurly)?;
        while self.peek().kind != CloseCurly {
            let item = self.expect(Identifier)?;
            let alias = self.parse_use_alias()?;
            let span = Span::new(item.span.start, self.current_token().span.end);
            let mut path = prefix.to_vec();
            path.push(item.text);
            specs.push(UseSpec { path, alias, span });
            if self.peek().kind == Comma {
                self.expect(Comma)?;
            } else {
                break;
            }
        }
        self.expect(CloseCurly)?;
        Ok(())
    }

    /// Parse an optional `as <identifier>` rename, returning the alias if present.
    fn parse_use_alias(&mut self) -> PResult<Option<String>> {
        if self.peek().kind == As {
            self.expect(As)?;
            Ok(Some(self.expect(Identifier)?.text))
        } else {
            Ok(None)
        }
    }
}
