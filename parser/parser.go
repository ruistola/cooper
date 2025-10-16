package parser

import (
	"fmt"
	"github.com/ruistola/cooper/ast"
	"github.com/ruistola/cooper/lexer"
	"slices"
)

// For semicolon inference, the parser keeps track of whether the token currently being inspected
// is inside parentheses, square brackets, or non-compound-statement(/expression) curly braces.
// These can be nested, so they must be stored in a stack.
//
// The parser also keeps track of whether the current token is inside a then- branch of an if- statement
// or if- expression, where (and only where) the "else" keyword is also a valid statement terminator.
type parser struct {
	tokens       []lexer.Token
	pos          int
	parenStack   []lexer.TokenType
	inThenBranch bool
}

func newParser(tokens []lexer.Token) parser {
	return parser{
		tokens:       tokens,
		pos:          0,
		parenStack:   make([]lexer.TokenType, 0),
		inThenBranch: false,
	}
}

var (
	// An EOL may be converted into a semicolon only if the previous token is one of the following:
	beforeSemicolon []lexer.TokenType = []lexer.TokenType{
		lexer.NUMBER,
		lexer.STRING,
		lexer.IDENTIFIER,
		lexer.UNDERSCORE,
		lexer.COMMA,
		lexer.CLOSE_BRACKET,
		lexer.CLOSE_CURLY,
		lexer.CLOSE_PAREN,
		lexer.ELSE,
		lexer.FALSE,
		lexer.RETURN,
		lexer.THEN,
		lexer.TRUE,
	}

	// An EOL may be converted into a semicolon only if the next token is one of the following:
	afterSemicolon []lexer.TokenType = []lexer.TokenType{
		lexer.EOF,
		lexer.COMMENT,
		lexer.NUMBER,
		lexer.STRING,
		lexer.IDENTIFIER,
		lexer.UNDERSCORE,
		lexer.SEMICOLON,
		lexer.OPEN_CURLY,
		lexer.CLOSE_CURLY,
		lexer.OPEN_PAREN,
		lexer.FALSE,
		lexer.FOR,
		lexer.FUNC,
		lexer.IF,
		lexer.ELSE,
		lexer.LET,
		lexer.RETURN,
		lexer.STRUCT,
		lexer.TRUE,
		lexer.USE,
	}
)

// A closing parenthesis, curly brace, or square bracket (provided as the argument) has been encountered,
// and must be matched by a corresponding opening one.
// TODO: Collect errors instead of panicking and exiting early?
func (p *parser) popParenStack(t lexer.TokenType) {
	if len(p.parenStack) == 0 {
		panic(fmt.Sprintf("Unmatched pairwise symbol '%s' found\n", t))
	}
	top := p.parenStack[len(p.parenStack)-1]
	if (top == lexer.OPEN_PAREN && t != lexer.CLOSE_PAREN) ||
		(top == lexer.OPEN_CURLY && t != lexer.CLOSE_CURLY) ||
		(top == lexer.OPEN_BRACKET && t != lexer.CLOSE_BRACKET) {
		panic(fmt.Sprintf("Unmatched pairwise symbol '%s' found\n", t))
	}
	p.parenStack = p.parenStack[:len(p.parenStack)-1]
}

// prevToken returns the previous token or EOF
func (p *parser) prevToken() lexer.Token {
	result := lexer.Token{}
	if p.pos > 0 {
		result = p.tokens[p.pos-1]
	}
	return result
}

// currentToken returns the current token or EOF
func (p *parser) currentToken() lexer.Token {
	result := lexer.Token{}
	if p.pos < len(p.tokens) {
		result = p.tokens[p.pos]
	}
	return result
}

// nextToken returns the next token or EOF
func (p *parser) nextToken() lexer.Token {
	result := lexer.Token{}
	if p.pos+1 < len(p.tokens) {
		result = p.tokens[p.pos+1]
	}
	return result
}

// Returns the current token without advancing the parser position.
// Converts EOL to semicolon when applicable, deletes as whitespace otherwise.
// Redundant consecutive EOLs have already been omitted by the lexer.
// This way, the rest of the parser can remain completely whitespace ignorant,
// and only expect semicolons when an explicit statement terminator is required.
func (p *parser) peek() lexer.Token {
	currToken := p.currentToken()
	if currToken.Type == lexer.EOL {
		isBeforeClosingBrace := p.nextToken().Type == lexer.CLOSE_CURLY
		// Don't convert EOL into a semicolon if this would be the last expression in a block.
		// If the user's intention is specifically to return nothing from a block expression,
		// they must insert an explicit semicolon.
		if isBeforeClosingBrace {
			p.tokens = append(p.tokens[:p.pos], p.tokens[p.pos+1:]...)
			// Recurse to get the new current token
			return p.peek()
		}
		// If the current token is an EOL, replace with a semicolon or delete as whitespace.
		isOutsideParens := len(p.parenStack) == 0
		statementCanTerminate := slices.Contains(beforeSemicolon, p.prevToken().Type) && slices.Contains(afterSemicolon, p.nextToken().Type)
		if isOutsideParens && statementCanTerminate && p.nextToken().Type != lexer.EOF {
			// EOL is applicable as a statement terminator, replace it with an explicit SEMICOLON token
			p.tokens[p.pos] = lexer.Token{
				Type:   lexer.SEMICOLON,
				Value:  ";",
				SrcPos: currToken.SrcPos,
			}
		} else {
			// EOL is not a statement terminator, remove from token stream as whitespace
			p.tokens = append(p.tokens[:p.pos], p.tokens[p.pos+1:]...)
			// Recurse to get the new current token
			return p.peek()
		}
	}
	return p.currentToken()
}

// Consumes a single token which must be one of the expected token types, if any have been
// provided as arguments. If the type of the next token is not any of the expected, panics.
// When called without arguments, accepts any token (including EOF). Returns the consumed token.
// Updates the parser's paren stack as appropriate.
func (p *parser) consume(expected ...lexer.TokenType) lexer.Token {
	currToken := p.peek()
	if len(expected) > 0 && !slices.Contains(expected, currToken.Type) {
		panic(fmt.Sprintf("Expected %s, found %s\n", expected, currToken.Type))
	}
	switch currToken.Type {
	case lexer.OPEN_PAREN, lexer.OPEN_BRACKET:
		p.parenStack = append(p.parenStack, currToken.Type)
	case lexer.CLOSE_PAREN, lexer.CLOSE_BRACKET:
		p.popParenStack(currToken.Type)
	case lexer.CLOSE_CURLY:
		// Open curly is contextual and can only be pushed to the stack by specific
		// parsing functions, but close curly braces can be safely popped, if a matching
		// open curly is found on the top of the stack (implying the parser was inside
		// a struct definition or struct literal body).
		if len(p.parenStack) > 0 && p.parenStack[len(p.parenStack)-1] == lexer.OPEN_CURLY {
			p.popParenStack(currToken.Type)
		}
	}
	p.pos++
	return currToken
}

func (p *parser) statementTerminates() bool {
	switch p.peek().Type {
	case lexer.SEMICOLON, lexer.EOF, lexer.CLOSE_CURLY:
		return true
	case lexer.ELSE:
		// when inside an if-statement, allow the "else" keyword to behave as a terminator for the then-branch
		return p.inThenBranch
	default:
		return false
	}
}

// Primarily intended for consuming a semicolon, but from the parser point of view,
// technically EOF and the closing curly brace are also valid (it is up to
// semantic analysis to determine whether ok in context).
func (p *parser) consumeStatementTerminator() {
	switch p.peek().Type {
	case lexer.SEMICOLON:
		p.consume()
	case lexer.EOF:
		// OK but do nothing
	case lexer.CLOSE_CURLY:
		// Don't consume, let the block parser handle it
	case lexer.ELSE:
		if p.inThenBranch {
			// Don't consume, let parseIfStmt handle it
			return
		}
		fallthrough
	default:
		panic("Expected statement terminator")
	}
}

// Right binding power of tokens that may appear in the head position of an expression (Pratt: NUD).
func headPrecedence(tokenType lexer.TokenType) int {
	switch tokenType {
	case lexer.EOF, lexer.SEMICOLON, lexer.OPEN_PAREN, lexer.OPEN_CURLY:
		return 0
	case lexer.NUMBER, lexer.STRING, lexer.WORD, lexer.TRUE, lexer.FALSE:
		return 1
	case lexer.PLUS, lexer.DASH:
		return 10
	default:
		panic(fmt.Sprintf("Cannot determine binding power for '%s' as a head token", tokenType))
	}
}

// Binding power of tokens that may appear in the tail position of an expression (Pratt: LED).
// Unequal left vs right binding power to enforce left or right associativity as appropriate.
func tailPrecedence(tokenType lexer.TokenType) (int, int) {
	switch tokenType {
	case lexer.EOF, lexer.SEMICOLON, lexer.CLOSE_PAREN, lexer.COMMA, lexer.CLOSE_CURLY, lexer.CLOSE_BRACKET, lexer.THEN, lexer.ELSE:
		return 0, 0
	case lexer.EQUALS, lexer.PLUS_EQUALS, lexer.DASH_EQUALS, lexer.COLON_EQUALS:
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

// Parse converts a slice of tokens into an AST that can then be used as input for type checking and semantic analysis.
func Parse(tokens []lexer.Token) ast.BlockStmt {
	p := newParser(tokens)
	program := ast.BlockStmt{}
	for p.peek().Type != lexer.EOF {
		program.Statements = append(program.Statements, p.parseStmt())
	}
	return program
}

// parseStmt looks at the current token and invokes the appropriate keyword specific
// parser function. If the token isn't any of the statement opening keywords,
// defaults to expression parsing where the expression is handled as a statement,
// ignoring the expression value.
func (p *parser) parseStmt() ast.Stmt {
	switch p.peek().Type {
	case lexer.FOR:
		return p.parseForStmt()
	case lexer.FUNC:
		return p.parseFuncDeclStmt()
	case lexer.IF:
		return p.parseIfStmt()
	case lexer.LET:
		return p.parseVarDeclStmt()
	case lexer.RETURN:
		return p.parseReturnStmt()
	case lexer.STRUCT:
		return p.parseStructDeclStmt()
	case lexer.USE:
		return p.parseUseDeclStmt()
	default:
		return p.parseExpressionStmt()
	}
}

// A Pratt parser for parsing expressions.
func (p *parser) parseExpr(min_bp int) ast.Expr {
	token := p.consume()
	leftExpr := p.parseHeadExpr(token)
	for {
		token = p.peek()
		if lbp, rbp := tailPrecedence(token.Type); lbp <= min_bp {
			break
		} else {
			leftExpr = p.parseTailExpr(leftExpr, rbp)
		}
	}
	return leftExpr
}

// Parses the token provided in the argument as a token in the head (NUD) position.
// May parse subexpressions recursively. Returns the (possibly compound) expression.
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

// Parses a tail expression, or the right-hand side expression of some head expression
// that is provided as an argument. May parse subexpressions recursively. Passes the minimum
// binding power forward to recursive calls (to determine expression boundary) provided
// as an argument. Returns the (possibly compound) expression.
func (p *parser) parseTailExpr(head ast.Expr, rbp int) ast.Expr {
	currToken := p.peek()
	switch currToken.Type {
	case lexer.COLON_EQUALS:
		return p.parseDeclAssignExpr(head)
	case lexer.EQUALS,
		lexer.PLUS_EQUALS,
		lexer.DASH_EQUALS,
		lexer.STAR_EQUALS,
		lexer.SLASH_EQUALS:
		operator := p.consume()
		rhs := p.parseExpr(rbp)
		return ast.AssignExpr{
			Assigne:       head,
			Operator:      operator,
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
		operator := p.consume()
		rhs := p.parseExpr(rbp)
		return ast.BinaryExpr{
			Lhs:      head,
			Operator: operator,
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
		panic(fmt.Sprintf("Failed to parse tail expression from token %v\n", currToken))
	}
}

// -----------------
// Parsing functions
// -----------------

func (p *parser) parseTypeExpr() ast.TypeExpr {
	var t ast.TypeExpr
	if p.peek().Type == lexer.OPEN_PAREN {
		// If a type expression is enclosed in parens, ignore them and parse the TypeExpr inside
		p.consume(lexer.OPEN_PAREN)
		t = p.parseTypeExpr()
		p.consume(lexer.CLOSE_PAREN)
	} else if p.peek().Type == lexer.FUNC {
		// The type expression starts with a `func` keyword, so a complete function type expression must follow
		t = p.parseFuncTypeExpr()
	} else {
		// The type expression must be a built-in like `i32` or a user defined type (e.g. `Foo` which is declared elsewhere)
		// TODO: Should there be a StructTypeExpr for anonymous structs that start with the `struct` keyword (like FuncTypeExpr) ?
		name := p.consume(lexer.IDENTIFIER).Value
		t = ast.NamedTypeExpr{
			TypeName: name,
		}
	}
	// If a type expression is followed by square brackets, then the complete type expression is T[]
	if p.peek().Type == lexer.OPEN_BRACKET {
		t = p.parseArrayTypeExpr(t)
	}
	return t
}

func (p *parser) parseArrayTypeExpr(innerType ast.TypeExpr) ast.TypeExpr {
	p.consume(lexer.OPEN_BRACKET)
	p.consume(lexer.CLOSE_BRACKET)
	arrayType := ast.ArrayTypeExpr{
		UnderlyingType: innerType,
	}
	if p.peek().Type == lexer.OPEN_BRACKET {
		return p.parseArrayTypeExpr(arrayType)
	}
	return arrayType
}

// For clarification:
//
//	func add(x: i32, y: i32): i64 {
//	  return x + y
//	}
//
// is a function declaration,
//
// func(i32,i32):i64
//
// is a function type expression.
func (p *parser) parseFuncTypeExpr() ast.FuncTypeExpr {
	p.consume(lexer.FUNC)
	p.consume(lexer.OPEN_PAREN)
	paramTypes := []ast.TypeExpr{}
	for p.peek().Type != lexer.CLOSE_PAREN {
		if p.peek().Type == lexer.IDENTIFIER {
			name := p.consume(lexer.IDENTIFIER).Value
			if p.peek().Type == lexer.COLON {
				p.consume(lexer.COLON)
				paramType := p.parseTypeExpr()
				paramTypes = append(paramTypes, paramType)
			} else {
				paramTypes = append(paramTypes, ast.NamedTypeExpr{
					TypeName: name,
				})
			}
		} else {
			paramType := p.parseTypeExpr()
			paramTypes = append(paramTypes, paramType)
		}
		if p.peek().Type == lexer.COMMA {
			p.consume(lexer.COMMA)
		} else {
			break
		}
	}
	p.consume(lexer.CLOSE_PAREN)
	var returnType ast.TypeExpr
	if p.peek().Type == lexer.COLON {
		p.consume(lexer.COLON)
		returnType = p.parseTypeExpr()
	} else {
		returnType = ast.NamedTypeExpr{TypeName: "void"}
	}
	return ast.FuncTypeExpr{
		ReturnType: returnType,
		ParamTypes: paramTypes,
	}
}

func (p *parser) parseDeclAssignExpr(expr ast.Expr) ast.VarDeclAssignExpr {
	p.consume(lexer.COLON_EQUALS)
	if identExpr, ok := expr.(ast.IdentExpr); ok {
		return ast.VarDeclAssignExpr{
			Name:          identExpr.Value,
			AssignedValue: p.parseExpr(0),
		}
	} else {
		panic("The left-hand side of a declaration-assignment must be an identifier")
	}
}

func (p *parser) parseVarDeclStmt() ast.VarDeclStmt {
	p.consume(lexer.LET)
	varName := p.consume(lexer.IDENTIFIER).Value
	p.consume(lexer.COLON)
	varType := p.parseTypeExpr()
	var initVal ast.Expr
	if !p.statementTerminates() {
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
		paramType := p.parseTypeExpr()
		params = append(params, ast.TypedIdent{
			Name: paramName,
			Type: paramType,
		})
		if p.peek().Type == lexer.COMMA {
			p.consume(lexer.COMMA)
		}
	}
	p.consume(lexer.CLOSE_PAREN)
	var returnType ast.TypeExpr
	if p.peek().Type == lexer.COLON {
		p.consume(lexer.COLON)
		returnType = p.parseTypeExpr()
	}
	p.consume(lexer.OPEN_CURLY)
	funcBody := p.parseBlockStmt()
	p.consume(lexer.CLOSE_CURLY)
	return ast.FuncDeclStmt{
		Name:       name,
		Parameters: params,
		ReturnType: returnType,
		Body:       funcBody,
	}
}

// A user defined record type.
// Example:
//
//	struct Foo {
//	  bar: i32,
//	  baz: string,
//	}
func (p *parser) parseStructDeclStmt() ast.StructDeclStmt {
	p.consume(lexer.STRUCT)
	name := p.consume(lexer.IDENTIFIER).Value
	p.consume(lexer.OPEN_CURLY)
	p.parenStack = append(p.parenStack, lexer.OPEN_CURLY)
	members := make([]ast.TypedIdent, 0)
	for p.peek().Type != lexer.CLOSE_CURLY {
		memberName := p.consume(lexer.IDENTIFIER).Value
		p.consume(lexer.COLON)
		memberType := p.parseTypeExpr()
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

// Example:
//
// let x = if x < 0 then 0 else x
func (p *parser) parseIfExpr() ast.IfExpr {
	cond := p.parseExpr(0)
	p.consume(lexer.THEN)
	var thenExpr ast.Expr
	if p.peek().Type == lexer.OPEN_CURLY {
		p.consume(lexer.OPEN_CURLY)
		thenExpr = p.parseBlockExpr()
		p.consume(lexer.CLOSE_CURLY)
	} else {
		thenExpr = p.parseExpr(0)
	}
	// There might be a semicolon resulting from an EOL conversion
	// (the `beforeSemicolon` and `afterSemicolon` categories can't differentiate
	// between lexer.ELSE in a statement vs expression context)
	// so just consume it silently if there is one
	if p.peek().Type == lexer.SEMICOLON {
		p.consume(lexer.SEMICOLON)
	}
	var elseExpr ast.Expr
	p.consume(lexer.ELSE)
	if p.peek().Type == lexer.OPEN_CURLY {
		p.consume(lexer.OPEN_CURLY)
		elseExpr = p.parseBlockExpr()
		p.consume(lexer.CLOSE_CURLY)
	} else {
		elseExpr = p.parseExpr(0)
	}
	return ast.IfExpr{
		Cond: cond,
		Then: thenExpr,
		Else: elseExpr,
	}
}

// Example:
//
//	if x < 0 then {
//	  doA()
//	  doB()
//	}
func (p *parser) parseIfStmt() ast.Stmt {
	p.consume(lexer.IF)
	cond := p.parseExpr(0)
	p.consume(lexer.THEN)
	var thenStmt ast.Stmt
	if p.peek().Type == lexer.OPEN_CURLY {
		p.consume(lexer.OPEN_CURLY)
		thenStmt = p.parseBlockStmt()
		p.consume(lexer.CLOSE_CURLY)
	} else {
		p.inThenBranch = true
		thenStmt = p.parseStmt()
		p.inThenBranch = false
	}
	var elseStmt ast.Stmt
	if p.peek().Type == lexer.ELSE {
		p.consume(lexer.ELSE)
		if p.peek().Type == lexer.OPEN_CURLY {
			p.consume(lexer.OPEN_CURLY)
			elseStmt = p.parseBlockStmt()
			p.consume(lexer.CLOSE_CURLY)
		} else {
			elseStmt = p.parseStmt()
		}
	}
	return ast.IfStmt{
		Cond: cond,
		Then: thenStmt,
		Else: elseStmt,
	}
}

// TODO: Ranges
func (p *parser) parseForStmt() ast.Stmt {
	p.consume(lexer.FOR)
	p.consume(lexer.OPEN_PAREN)
	initStmt := p.parseStmt()
	condExpr := p.parseExpressionStmt().(ast.ExpressionStmt).Expr
	iterStmt := ast.ExpressionStmt{Expr: p.parseExpr(0)}
	p.consume(lexer.CLOSE_PAREN)
	p.consume(lexer.OPEN_CURLY)
	body := p.parseBlockStmt()
	p.consume(lexer.CLOSE_CURLY)
	return ast.ForStmt{
		Init: initStmt,
		Cond: condExpr,
		Iter: iterStmt,
		Body: body,
	}
}

func (p *parser) parseFuncCallExpr(left ast.Expr) ast.FuncCallExpr {
	p.consume(lexer.OPEN_PAREN)
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
	p.consume(lexer.OPEN_CURLY)
	p.parenStack = append(p.parenStack, lexer.OPEN_CURLY)
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
	p.consume(lexer.DOT)
	return ast.StructMemberExpr{
		Struct: left,
		Member: ast.IdentExpr{
			Value: p.consume(lexer.IDENTIFIER).Value,
		},
	}
}

func (p *parser) parseArrayIndexExpr(left ast.Expr) ast.ArrayIndexExpr {
	p.consume(lexer.OPEN_BRACKET)
	indexExpr := p.parseExpr(0)
	p.consume(lexer.CLOSE_BRACKET)
	return ast.ArrayIndexExpr{
		Array: left,
		Index: indexExpr,
	}
}

func (p *parser) parseReturnStmt() ast.ReturnStmt {
	p.consume(lexer.RETURN)
	if p.statementTerminates() {
		p.consumeStatementTerminator()
		return ast.ReturnStmt{Expr: nil}
	}
	expr := p.parseExpr(0)
	p.consumeStatementTerminator()
	return ast.ReturnStmt{Expr: expr}
}

func (p *parser) parseExpressionStmt() ast.Stmt {
	expr := p.parseExpr(0)
	explicitSemicolon := p.peek().Type == lexer.SEMICOLON
	p.consumeStatementTerminator()
	return ast.ExpressionStmt{
		Expr:              expr,
		ExplicitSemicolon: explicitSemicolon,
	}
}

func (p *parser) parseBlockStmt() ast.BlockStmt {
	statements := []ast.Stmt{}
	for token := p.peek(); token.Type != lexer.EOF && token.Type != lexer.CLOSE_CURLY; token = p.peek() {
		statements = append(statements, p.parseStmt())
	}
	return ast.BlockStmt{
		Statements: statements,
	}
}

func (p *parser) parseBlockExpr() ast.BlockExpr {
	statements := p.parseBlockStmt().Statements
	var resultExpr ast.Expr = ast.UnitExpr{}
	if len(statements) > 0 {
		if exprStmt, ok := statements[len(statements)-1].(ast.ExpressionStmt); ok {
			resultExpr = exprStmt.Expr
			statements = statements[:len(statements)-1]
		}
	}
	return ast.BlockExpr{
		Statements: statements,
		ResultExpr: resultExpr,
	}
}

// Always required to be a use block, to minimize diff noise from when the number of declared uses
// goes from 1 -> 2. Like struct declarations and struct literals, trailing comma is mandatory,
// for the same reason: consistency and less noise in diffs.
//
//	use {
//	  moduleNameOrPath,
//	  alias: moduleNameOrPath,
//	}
func (p *parser) parseUseDeclStmt() ast.UseDeclStmt {
	specs := make([]ast.UseSpecExpr, 0)
	p.consume(lexer.USE)
	p.consume(lexer.OPEN_CURLY)
	for p.peek().Type != lexer.CLOSE_CURLY {
		name := p.consume(lexer.IDENTIFIER).Value
		p.consume(lexer.COLON)
		module := p.consume(lexer.IDENTIFIER).Value
		specs = append(specs, ast.UseSpecExpr{
			Name:   name,
			Module: module,
		})
		p.consume(lexer.COMMA)
	}
	p.consume(lexer.CLOSE_CURLY)
	return ast.UseDeclStmt{
		UseSpecs: specs,
	}
}
