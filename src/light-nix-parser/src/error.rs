use std::ops::Range;

use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::lexer::{Lexer, Token, TokenKind};

#[derive(Debug)]
pub struct ParseError<'input, 'allocator> {
    pub kind: ParseErrorKind,
    pub scope: Scope,
    pub expected: Expected,
    pub error_tokens: &'allocator [Token<'input>],
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Source,
    Statements,
    ImportStatement,
    EnumDefine,
    EnumVariant,
    TypeDefine,
    Typedef,
    UseDeclare,
    LetStatement,
    MutationPolicy,
    AssignStatement,
    FunctionDefine,
    FunctionArguments,
    Block,
    Expression,
    IfExpression,
    MatchExpression,
    MatchArm,
    Pattern,
    ReturnExpression,
    ElvisExpression,
    Primary,
    SomeValue,
    Array,
    FunctionCall,
    TypeInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Expected {
    Statement,
    StatementSeparator,
    EnumVariant,
    Typedef,
    UseElement,
    FunctionArgument,
    Block,
    Expression,
    MatchArm,
    Pattern,
    Value,
    Literal,
    StringLiteral,
    TypeInfo,
    IntegerLiteral,
    AccessMember,
    Token(TokenKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    InvalidStatement,
    InvalidStatementSeparator,
    InvalidImportStatement,
    InvalidEnumDefine,
    InvalidEnumVariant,
    InvalidTypedef,
    InvalidUseElement,
    InvalidLetStatement,
    InvalidMutationPolicy,
    InvalidAssignStatement,
    InvalidFunctionDefine,
    InvalidFunctionArgument,
    InvalidExpression,
    InvalidMatchExpression,
    InvalidMatchArm,
    InvalidPattern,
    InvalidSomeValue,
    InvalidElvisExpression,
    InvalidPrimaryAccess,
    InvalidArrayElement,
    InvalidFunctionCallArgument,
    InvalidTypeInfo,
    NonClosedBrace,
    NonClosedBracket,
    NonClosedParenthesis,
    NonClosedTypeParameter,
    UnexpectedToken,
}

pub(crate) fn recover_until<'input, 'allocator>(
    kind: ParseErrorKind,
    lexer: &mut Lexer<'input>,
    until: &[TokenKind],
    expected: Expected,
    scope: Scope,
    allocator: &'allocator Bump,
) -> ParseError<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    let mut error_tokens = Vec::new_in(allocator);

    loop {
        let Some(token) = lexer.current() else {
            break;
        };

        if until.contains(&token.kind) {
            break;
        }

        error_tokens.push(token);
        lexer.next();
    }

    let error_tokens = allocator.alloc_slice_fill_iter(error_tokens);

    ParseError {
        kind,
        scope,
        expected,
        error_tokens,
        span: anchor.elapsed(lexer),
    }
}

pub(crate) fn error_here<'input, 'allocator>(
    kind: ParseErrorKind,
    lexer: &mut Lexer<'input>,
    expected: Expected,
    scope: Scope,
) -> ParseError<'input, 'allocator> {
    let span = lexer
        .current()
        .map(|token| token.span.start..token.span.start)
        .unwrap_or_else(|| {
            let anchor = lexer.cast_anchor();
            anchor.elapsed(lexer)
        });

    ParseError {
        kind,
        scope,
        expected,
        error_tokens: &[],
        span,
    }
}
