package parser

import (
	"cooper/ast"
	"cooper/lexer"
	"fmt"
	"slices"
)

type parser struct {
	tokens []lexer.Token
	pos    int
}

func (p *parser) peek() lexer.Token {
	result := lexer.Token{}
	if p.pos < len(p.tokens) {
		result = p.tokens[p.pos]
	}
	return result
}

func (p *parser) consume(expected ...lexer.TokenType) lexer.Token {
	token := p.peek()
	if len(expected) > 0 && !slices.Contains(expected, token.Type) {
		panic(fmt.Sprintf("Expected %s, found %s\n", expected, token.Type))
	}
	p.pos++
	return token
}

func (p *parser) consumeStatementTerminator() {
	next := p.peek()
	switch next.Type {
	case lexer.SEMICOLON, lexer.EOL:
		p.consume()
	case lexer.EOF, lexer.CLOSE_CURLY:
		// OK but do nothing
	default:
		panic("Expected statement terminator")
	}
}

func headPrecedence(tokenType lexer.TokenType) int {
	switch tokenType {
	case lexer.EOF, lexer.EOL, lexer.SEMICOLON, lexer.OPEN_PAREN, lexer.OPEN_CURLY:
		return 0
	case lexer.NUMBER, lexer.STRING, lexer.WORD, lexer.TRUE, lexer.FALSE:
		return 1
	case lexer.PLUS, lexer.DASH:
		return 10
	default:
		panic(fmt.Sprintf("Cannot determine binding power for '%s' as a head token", tokenType))
	}
}

func tailPrecedence(tokenType lexer.TokenType) (int, int) {
	switch tokenType {
	case lexer.EOF, lexer.EOL, lexer.SEMICOLON, lexer.CLOSE_PAREN, lexer.COMMA, lexer.CLOSE_CURLY, lexer.CLOSE_BRACKET, lexer.THEN, lexer.ELSE:
		return 0, 0
	case lexer.EQUALS, lexer.PLUS_EQUALS, lexer.DASH_EQUALS:
		return 1, 2
	case lexer.OR, lexer.AND:
		return 4, 3
	case lexer.DOUBLE_EQUALS, lexer.NOT_EQUALS:
		return 5, 6
	case lexer.LESS, lexer.LESS_EQUALS, lexer.GREATER, lexer.GREATER_EQUALS:
		return 8, 7
	case lexer.PLUS, lexer.DASH:
		return 10, 9
	case lexer.STAR, lexer.SLASH, lexer.PERCENT:
		return 12, 11
	case lexer.OPEN_CURLY:
		return 13, 0
	case lexer.OPEN_PAREN, lexer.OPEN_BRACKET:
		return 14, 0
	case lexer.DOT:
		return 16, 15
	default:
		panic(fmt.Sprintf("Cannot determine binding power for '%s' as a tail token", tokenType))
	}
}

func Parse(tokens []lexer.Token) ast.BlockExpr {
	p := parser{tokens, 0}
	program := ast.BlockExpr{}
	for p.peek().Type != lexer.EOF {
		program.Statements = append(program.Statements, p.parseStmt())
	}
	return program
}

func (p *parser) parseStmt() ast.Stmt {
	switch p.peek().Type {
	case lexer.LET:
		return p.parseVarDeclStmt()
	case lexer.STRUCT:
		return p.parseStructDeclStmt()
	case lexer.FUNC:
		return p.parseFuncDeclStmt()
	case lexer.FOR:
		return p.parseForStmt()
	case lexer.RETURN:
		return p.parseReturnStmt()
	default:
		return p.parseExpressionStmt()
	}
}

func (p *parser) parseExpr(min_bp int) ast.Expr {
	token := p.consume()
	leftExpr := p.parseHeadExpr(token)
	for {
		nextToken := p.peek()
		if lbp, rbp := tailPrecedence(nextToken.Type); lbp <= min_bp {
			break
		} else {
			leftExpr = p.parseTailExpr(leftExpr, rbp)
		}
	}
	return leftExpr
}

func (p *parser) parseHeadExpr(token lexer.Token) ast.Expr {
	switch token.Type {
	case lexer.NUMBER:
		return ast.NumberLiteralExpr{
			Value: token.Value,
		}
	case lexer.STRING:
		return ast.StringLiteralExpr{
			Value: token.Value,
		}
	case lexer.IDENTIFIER:
		return ast.IdentExpr{
			Value: token.Value,
		}
	case lexer.TRUE, lexer.FALSE:
		return ast.BoolLiteralExpr{
			Value: (token.Type == lexer.TRUE),
		}
	case lexer.PLUS, lexer.DASH:
		rbp := headPrecedence(token.Type)
		rhs := p.parseExpr(rbp)
		return ast.UnaryExpr{
			Operator: token,
			Rhs:      rhs,
		}
	case lexer.OPEN_PAREN:
		rbp := headPrecedence(token.Type)
		rhs := p.parseExpr(rbp)
		p.consume(lexer.CLOSE_PAREN)
		return ast.GroupExpr{
			Expr: rhs,
		}
	case lexer.IF:
		return p.parseIfExpr()
	case lexer.OPEN_CURLY:
		rhs := p.parseBlockExpr()
		p.consume(lexer.CLOSE_CURLY)
		return rhs
	default:
		panic(fmt.Sprintf("Failed to parse head expression from token %v\n", token))
	}
}

func (p *parser) parseTailExpr(head ast.Expr, rbp int) ast.Expr {
	token := p.consume()
	switch token.Type {
	case lexer.EQUALS, lexer.PLUS_EQUALS, lexer.DASH_EQUALS:
		rhs := p.parseExpr(rbp)
		return ast.AssignExpr{
			Assigne:       head,
			Operator:      token,
			AssignedValue: rhs,
		}
	case lexer.PLUS,
		lexer.DASH,
		lexer.STAR,
		lexer.SLASH,
		lexer.PERCENT,
		lexer.LESS,
		lexer.LESS_EQUALS,
		lexer.GREATER,
		lexer.GREATER_EQUALS:
		rhs := p.parseExpr(rbp)
		return ast.BinaryExpr{
			Lhs:      head,
			Operator: token,
			Rhs:      rhs,
		}
	case lexer.OPEN_PAREN:
		return p.parseFuncCallExpr(head)
	case lexer.OPEN_CURLY:
		return p.parseStructLiteralExpr(head)
	case lexer.OPEN_BRACKET:
		return p.parseArrayIndexExpr(head)
	case lexer.DOT:
		return p.parseStructMemberExpr(head)
	default:
		panic(fmt.Sprintf("Failed to parse tail expression from token %v\n", token))
	}
}

func (p *parser) parseArrayType(innerType ast.Type) ast.Type {
	p.consume(lexer.OPEN_BRACKET)
	p.consume(lexer.CLOSE_BRACKET)
	arrayType := ast.ArrayType{
		UnderlyingType: innerType,
	}
	if p.peek().Type == lexer.OPEN_BRACKET {
		return p.parseArrayType(arrayType)
	}
	return arrayType
}

func (p *parser) parseType() ast.Type {
	if p.peek().Type == lexer.FUNC {
		return p.parseFuncType()
	}
	name := p.consume(lexer.IDENTIFIER).Value
	namedType := ast.NamedType{
		TypeName: name,
	}
	if p.peek().Type == lexer.OPEN_BRACKET {
		return p.parseArrayType(namedType)
	}
	return namedType
}

func (p *parser) parseFuncType() ast.FuncType {
	p.consume(lexer.FUNC)
	p.consume(lexer.OPEN_PAREN)
	paramTypes := []ast.Type{}
	for p.peek().Type != lexer.CLOSE_PAREN {
		if p.peek().Type == lexer.IDENTIFIER {
			name := p.consume(lexer.IDENTIFIER).Value
			if p.peek().Type == lexer.COLON {
				p.consume(lexer.COLON)
				paramType := p.parseType()
				paramTypes = append(paramTypes, paramType)
			} else {
				paramTypes = append(paramTypes, ast.NamedType{
					TypeName: name,
				})
			}
		} else {
			paramType := p.parseType()
			paramTypes = append(paramTypes, paramType)
		}
		if p.peek().Type == lexer.COMMA {
			p.consume(lexer.COMMA)
		} else {
			break
		}
	}
	p.consume(lexer.CLOSE_PAREN)
	var returnType ast.Type
	if p.peek().Type == lexer.COLON {
		p.consume(lexer.COLON)
		returnType = p.parseType()
	} else {
		returnType = ast.NamedType{TypeName: "void"}
	}
	return ast.FuncType{
		ReturnType: returnType,
		ParamTypes: paramTypes,
	}
}

func (p *parser) parseVarDeclStmt() ast.VarDeclStmt {
	p.consume(lexer.LET)
	varName := p.consume(lexer.IDENTIFIER).Value
	p.consume(lexer.COLON)
	varType := p.parseType()
	var initVal ast.Expr
	if p.peek().Type != lexer.SEMICOLON {
		p.consume(lexer.EQUALS)
		initVal = p.parseExpr(0)
	}
	p.consumeStatementTerminator()
	return ast.VarDeclStmt{
		Var: ast.TypedIdent{
			Name: varName,
			Type: varType,
		},
		InitVal: initVal,
	}
}

func (p *parser) parseFuncDeclStmt() ast.FuncDeclStmt {
	p.consume(lexer.FUNC)
	name := p.consume(lexer.IDENTIFIER).Value
	p.consume(lexer.OPEN_PAREN)
	params := make([]ast.TypedIdent, 0)
	for p.peek().Type != lexer.CLOSE_PAREN {
		paramName := p.consume(lexer.IDENTIFIER).Value
		p.consume(lexer.COLON)
		paramType := p.parseType()
		params = append(params, ast.TypedIdent{
			Name: paramName,
			Type: paramType,
		})
		if p.peek().Type == lexer.COMMA {
			p.consume(lexer.COMMA)
		}
	}
	p.consume(lexer.CLOSE_PAREN)
	var returnType ast.Type
	if p.peek().Type == lexer.COLON {
		p.consume(lexer.COLON)
		returnType = p.parseType()
	}
	p.consume(lexer.OPEN_CURLY)
	funcBody := p.parseBlockExpr()
	p.consume(lexer.CLOSE_CURLY)
	return ast.FuncDeclStmt{
		Name:       name,
		Parameters: params,
		ReturnType: returnType,
		Body:       funcBody,
	}
}

func (p *parser) parseStructDeclStmt() ast.StructDeclStmt {
	p.consume(lexer.STRUCT)
	name := p.consume(lexer.IDENTIFIER).Value
	p.consume(lexer.OPEN_CURLY)
	members := make([]ast.TypedIdent, 0)
	for p.peek().Type != lexer.CLOSE_CURLY {
		memberName := p.consume(lexer.IDENTIFIER).Value
		p.consume(lexer.COLON)
		memberType := p.parseType()
		newMember := ast.TypedIdent{
			Name: memberName,
			Type: memberType,
		}
		members = append(members, newMember)
		if p.peek().Type == lexer.COMMA {
			p.consume(lexer.COMMA)
		}
	}
	p.consume(lexer.CLOSE_CURLY)
	return ast.StructDeclStmt{
		Name:    name,
		Members: members,
	}
}

func (p *parser) parseIfExpr() ast.IfExpr {
	cond := p.parseExpr(0)
	var thenExpr ast.Expr
	if p.peek().Type == lexer.THEN {
		p.consume(lexer.THEN)
		thenExpr = p.parseExpr(0)
	} else if p.peek().Type == lexer.OPEN_CURLY {
		p.consume(lexer.OPEN_CURLY)
		thenExpr = p.parseBlockExpr()
		p.consume(lexer.CLOSE_CURLY)
	} else {
		panic("Expected 'then' or '{' after an if condition")
	}
	var elseExpr ast.Expr
	if p.peek().Type == lexer.ELSE {
		p.consume(lexer.ELSE)
		if p.peek().Type == lexer.OPEN_CURLY {
			p.consume(lexer.OPEN_CURLY)
			elseExpr = p.parseBlockExpr()
			p.consume(lexer.CLOSE_CURLY)
		} else {
			elseExpr = p.parseExpr(0)
		}
	}
	return ast.IfExpr{
		Cond: cond,
		Then: thenExpr,
		Else: elseExpr,
	}
}

func (p *parser) parseForStmt() ast.Stmt {
	p.consume(lexer.FOR)
	p.consume(lexer.OPEN_PAREN)
	initStmt := p.parseStmt()
	condExpr := p.parseExpressionStmt().(ast.ExpressionStmt).Expr
	iterStmt := ast.ExpressionStmt{Expr: p.parseExpr(0)}
	p.consume(lexer.CLOSE_PAREN)
	p.consume(lexer.OPEN_CURLY)
	body := p.parseBlockExpr()
	p.consume(lexer.CLOSE_CURLY)
	return ast.ForStmt{
		Init: initStmt,
		Cond: condExpr,
		Iter: iterStmt,
		Body: body,
	}
}

func (p *parser) parseFuncCallExpr(left ast.Expr) ast.FuncCallExpr {
	args := []ast.Expr{}
	for p.peek().Type != lexer.CLOSE_PAREN {
		args = append(args, p.parseExpr(0))
		if p.peek().Type == lexer.COMMA {
			p.consume(lexer.COMMA)
		}
	}
	p.consume(lexer.CLOSE_PAREN)
	return ast.FuncCallExpr{
		Func: left,
		Args: args,
	}
}

func (p *parser) parseStructLiteralExpr(left ast.Expr) ast.StructLiteralExpr {
	members := []ast.MemberAssignExpr{}
	for p.peek().Type != lexer.CLOSE_CURLY {
		memberName := p.consume(lexer.IDENTIFIER).Value
		p.consume(lexer.COLON)
		members = append(members, ast.MemberAssignExpr{
			Name:  memberName,
			Value: p.parseExpr(0),
		})
		p.consume(lexer.COMMA)
	}
	p.consume(lexer.CLOSE_CURLY)
	return ast.StructLiteralExpr{
		Struct:  left,
		Members: members,
	}
}

func (p *parser) parseStructMemberExpr(left ast.Expr) ast.StructMemberExpr {
	return ast.StructMemberExpr{
		Struct: left,
		Member: ast.IdentExpr{
			Value: p.consume(lexer.IDENTIFIER).Value,
		},
	}
}

func (p *parser) parseArrayIndexExpr(left ast.Expr) ast.ArrayIndexExpr {
	indexExpr := p.parseExpr(0)
	p.consume(lexer.CLOSE_BRACKET)
	return ast.ArrayIndexExpr{
		Array: left,
		Index: indexExpr,
	}
}

func (p *parser) parseReturnStmt() ast.ReturnStmt {
	p.consume(lexer.RETURN)
	if p.peek().Type == lexer.SEMICOLON {
		p.consumeStatementTerminator()
		return ast.ReturnStmt{Expr: nil}
	}
	return ast.ReturnStmt{
		Expr: p.parseExpressionStmt().(ast.ExpressionStmt).Expr,
	}
}

func (p *parser) parseExpressionStmt() ast.Stmt {
	expr := p.parseExpr(0)
	p.consumeStatementTerminator()
	return ast.ExpressionStmt{
		Expr: expr,
	}
}

func (p *parser) parseBlockExpr() ast.BlockExpr {
	statements := []ast.Stmt{}
	for nextToken := p.peek(); nextToken.Type != lexer.EOF && nextToken.Type != lexer.CLOSE_CURLY; nextToken = p.peek() {
		statements = append(statements, p.parseStmt())
	}
	return ast.BlockExpr{
		Statements: statements,
	}
}
