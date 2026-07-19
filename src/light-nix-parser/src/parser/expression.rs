use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        AST, AccessOperator, Array, BinaryExpression, BinaryOperator, ElseBranch, ElseBranchValue,
        Expression, FunctionCall, IfBranch, IfExpression, Literal, LiteralValue, Primary,
        PrimaryAccess, ReturnExpression, Spanned, StringLiteral, UnaryExpression, UnaryOperator,
        Value,
    },
    error::{Expected, ParseErrorKind, Scope, error_here, recover_until},
    lexer::{Lexer, TokenKind},
};

use super::{ParseErrors, current_kind, is_statement_start, skip_line_feed, skip_list_separator};

pub(super) fn parse_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator Expression<'input, 'allocator>> {
    match current_kind(lexer) {
        TokenKind::If => parse_if_expression(lexer, errors, allocator)
            .map(|expression| allocator.alloc(Expression::If(expression)) as &_),
        TokenKind::Return => parse_return_expression(lexer, errors, allocator)
            .map(|expression| allocator.alloc(Expression::Return(expression)) as &_),
        _ => parse_binary_expression(lexer, errors, allocator, 1),
    }
}

fn parse_binary_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
    minimum_precedence: u8,
) -> Option<&'allocator Expression<'input, 'allocator>> {
    let mut left = parse_factor(lexer, errors, allocator)?;

    loop {
        let Some((operator, precedence)) = binary_operator(current_kind(lexer)) else {
            break;
        };
        if precedence < minimum_precedence {
            break;
        }

        let operator_token = lexer.next().unwrap();
        let operator = Spanned::new(operator, operator_token.span);

        let Some(right) = parse_binary_expression(lexer, errors, allocator, precedence + 1) else {
            errors.push(error_here(
                ParseErrorKind::InvalidExpression,
                lexer,
                Expected::Expression,
                Scope::Expression,
            ));
            break;
        };

        let span = left.span().start..right.span().end;
        let binary = allocator.alloc(BinaryExpression {
            left,
            operator,
            right,
            span,
        });
        left = allocator.alloc(Expression::Binary(binary));
    }

    Some(left)
}

fn parse_factor<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator Expression<'input, 'allocator>> {
    let operator = match current_kind(lexer) {
        TokenKind::Plus => UnaryOperator::Positive,
        TokenKind::Minus => UnaryOperator::Negate,
        _ => return parse_primary_expression(lexer, errors, allocator),
    };

    let anchor = lexer.cast_anchor();
    let operator_token = lexer.next().unwrap();
    let operator = Spanned::new(operator, operator_token.span);

    let Some(operand) = parse_primary_expression(lexer, errors, allocator) else {
        errors.push(error_here(
            ParseErrorKind::InvalidExpression,
            lexer,
            Expected::Value,
            Scope::Expression,
        ));
        return None;
    };

    let unary = allocator.alloc(UnaryExpression {
        operator,
        operand,
        span: anchor.elapsed(lexer),
    });

    Some(allocator.alloc(Expression::Unary(unary)))
}

fn parse_primary_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator Expression<'input, 'allocator>> {
    let primary = parse_primary(lexer, errors, allocator)?;
    let primary = allocator.alloc(primary);
    Some(allocator.alloc(Expression::Primary(primary)))
}

fn parse_primary<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<Primary<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let value = parse_value(lexer, errors, allocator)?;
    let mut accesses = Vec::new_in(allocator);

    loop {
        let operator = match current_kind(lexer) {
            TokenKind::Dot => AccessOperator::Dot,
            TokenKind::ThinArrow => AccessOperator::ThinArrow,
            TokenKind::DoubleColon => AccessOperator::DoubleColon,
            _ => break,
        };

        let access_anchor = lexer.cast_anchor();
        let operator_token = lexer.next().unwrap();
        let operator = Spanned::new(operator, operator_token.span);

        skip_line_feed(lexer);

        if current_kind(lexer) != TokenKind::Literal {
            let error = recover_until(
                ParseErrorKind::InvalidPrimaryAccess,
                lexer,
                &expression_end_tokens(),
                Expected::AccessMember,
                Scope::Primary,
                allocator,
            );
            errors.push(error);
            break;
        }

        let member_token = lexer.next().unwrap();
        let member = Literal::new(member_token.text, member_token.span);
        let call = parse_function_call(lexer, errors, allocator);

        accesses.push(PrimaryAccess {
            operator,
            member,
            call,
            span: access_anchor.elapsed(lexer),
        });
    }

    let accesses = allocator.alloc_slice_fill_iter(accesses);

    Some(Primary {
        value,
        accesses,
        span: anchor.elapsed(lexer),
    })
}

fn parse_value<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<Value<'input, 'allocator>> {
    let token = lexer.current()?;

    match token.kind {
        TokenKind::BracketLeft => parse_array(lexer, errors, allocator).map(Value::Array),
        TokenKind::Literal => {
            let anchor = lexer.cast_anchor();
            let token = lexer.next().unwrap();
            let literal = Literal::new(token.text, token.span);
            let call = parse_function_call(lexer, errors, allocator);
            Some(Value::Literal(LiteralValue {
                literal,
                call,
                span: anchor.elapsed(lexer),
            }))
        }
        TokenKind::FloatNumeric | TokenKind::Inf | TokenKind::Nan => {
            let token = lexer.next().unwrap();
            Some(Value::Numeric(Spanned::new(token.text, token.span)))
        }
        TokenKind::StringLiteral => {
            let token = lexer.next().unwrap();
            Some(Value::String(StringLiteral::new(token.text, token.span)))
        }
        TokenKind::True | TokenKind::False => {
            let value = token.kind == TokenKind::True;
            let token = lexer.next().unwrap();
            Some(Value::Boolean(Spanned::new(value, token.span)))
        }
        TokenKind::Null => {
            let token = lexer.next().unwrap();
            Some(Value::Null(Spanned::new((), token.span)))
        }
        _ => None,
    }
}

fn parse_array<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator Array<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::BracketLeft {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    skip_line_feed(lexer);

    let mut values = Vec::new_in(allocator);

    loop {
        if is_statement_start(current_kind(lexer)) {
            break;
        }
        match current_kind(lexer) {
            TokenKind::BracketRight | TokenKind::None => break,
            _ => {}
        }

        let Some(value) = parse_value(lexer, errors, allocator) else {
            let error = recover_until(
                ParseErrorKind::InvalidArrayElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::BracketRight,
                ],
                Expected::Value,
                Scope::Array,
                allocator,
            );
            errors.push(error);

            if current_kind(lexer) == TokenKind::BracketRight {
                break;
            }
            skip_list_separator(lexer);
            continue;
        };
        values.push(value);

        if current_kind(lexer) == TokenKind::BracketRight {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidArrayElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::BracketRight,
                ],
                Expected::Token(TokenKind::Comma),
                Scope::Array,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    if current_kind(lexer) != TokenKind::BracketRight {
        let error = if is_statement_start(current_kind(lexer)) {
            error_here(
                ParseErrorKind::NonClosedBracket,
                lexer,
                Expected::Token(TokenKind::BracketRight),
                Scope::Array,
            )
        } else {
            recover_until(
                ParseErrorKind::NonClosedBracket,
                lexer,
                &[TokenKind::BracketRight],
                Expected::Token(TokenKind::BracketRight),
                Scope::Array,
                allocator,
            )
        };
        errors.push(error);
    }
    if current_kind(lexer) == TokenKind::BracketRight {
        lexer.next();
    }

    let values = allocator.alloc_slice_fill_iter(values);
    Some(allocator.alloc(Array {
        values,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_function_call<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator FunctionCall<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::ParenthesisLeft {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    skip_line_feed(lexer);

    let mut arguments = Vec::new_in(allocator);

    loop {
        if is_statement_start(current_kind(lexer))
            && !matches!(current_kind(lexer), TokenKind::If | TokenKind::Return)
        {
            break;
        }
        match current_kind(lexer) {
            TokenKind::ParenthesisRight | TokenKind::None => break,
            _ => {}
        }

        let Some(argument) = parse_expression(lexer, errors, allocator) else {
            let error = recover_until(
                ParseErrorKind::InvalidFunctionCallArgument,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::ParenthesisRight,
                ],
                Expected::Expression,
                Scope::FunctionCall,
                allocator,
            );
            errors.push(error);

            if current_kind(lexer) == TokenKind::ParenthesisRight {
                break;
            }
            skip_list_separator(lexer);
            continue;
        };
        arguments.push(argument);

        if current_kind(lexer) == TokenKind::ParenthesisRight {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidFunctionCallArgument,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::ParenthesisRight,
                ],
                Expected::Token(TokenKind::Comma),
                Scope::FunctionCall,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    if current_kind(lexer) != TokenKind::ParenthesisRight {
        let error = if is_statement_start(current_kind(lexer)) {
            error_here(
                ParseErrorKind::NonClosedParenthesis,
                lexer,
                Expected::Token(TokenKind::ParenthesisRight),
                Scope::FunctionCall,
            )
        } else {
            recover_until(
                ParseErrorKind::NonClosedParenthesis,
                lexer,
                &[TokenKind::ParenthesisRight],
                Expected::Token(TokenKind::ParenthesisRight),
                Scope::FunctionCall,
                allocator,
            )
        };
        errors.push(error);
    }
    if current_kind(lexer) == TokenKind::ParenthesisRight {
        lexer.next();
    }

    let arguments = arguments.into_iter().copied();
    let arguments = allocator.alloc_slice_fill_iter(arguments);

    Some(allocator.alloc(FunctionCall {
        arguments,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_if_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator IfExpression<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let branch = parse_if_branch(lexer, errors, allocator)?;
    let mut else_branches = Vec::new_in(allocator);

    loop {
        let else_anchor = lexer.cast_anchor();
        skip_line_feed(lexer);

        if current_kind(lexer) != TokenKind::Else {
            lexer.back_to_anchor(else_anchor);
            break;
        }
        lexer.next();

        let value = if current_kind(lexer) == TokenKind::If {
            parse_if_branch(lexer, errors, allocator).map(ElseBranchValue::If)
        } else {
            super::statement::parse_block(lexer, errors, allocator).map(ElseBranchValue::Block)
        };

        let Some(value) = value else {
            let error = recover_until(
                ParseErrorKind::InvalidExpression,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::Block,
                Scope::IfExpression,
                allocator,
            );
            errors.push(error);
            break;
        };

        else_branches.push(ElseBranch {
            value,
            span: else_anchor.elapsed(lexer),
        });
    }

    let else_branches = allocator.alloc_slice_fill_iter(else_branches);
    Some(allocator.alloc(IfExpression {
        branch,
        else_branches,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_if_branch<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator IfBranch<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::If {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    let Some(condition) = parse_expression(lexer, errors, allocator) else {
        let error = recover_until(
            ParseErrorKind::InvalidExpression,
            lexer,
            &[TokenKind::BraceLeft, TokenKind::LineFeed],
            Expected::Expression,
            Scope::IfExpression,
            allocator,
        );
        errors.push(error);
        return None;
    };

    let Some(body) = super::statement::parse_block(lexer, errors, allocator) else {
        errors.push(error_here(
            ParseErrorKind::InvalidExpression,
            lexer,
            Expected::Token(TokenKind::BraceLeft),
            Scope::IfExpression,
        ));
        return None;
    };

    Some(allocator.alloc(IfBranch {
        condition,
        body,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_return_expression<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator ReturnExpression<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Return {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    let value = match current_kind(lexer) {
        TokenKind::LineFeed
        | TokenKind::Semicolon
        | TokenKind::Comma
        | TokenKind::BraceRight
        | TokenKind::BracketRight
        | TokenKind::ParenthesisRight
        | TokenKind::None => None,
        _ => match parse_expression(lexer, errors, allocator) {
            Some(value) => Some(value),
            None => {
                errors.push(error_here(
                    ParseErrorKind::InvalidExpression,
                    lexer,
                    Expected::Expression,
                    Scope::ReturnExpression,
                ));
                None
            }
        },
    };

    Some(allocator.alloc(ReturnExpression {
        value,
        span: anchor.elapsed(lexer),
    }))
}

fn binary_operator(kind: TokenKind) -> Option<(BinaryOperator, u8)> {
    match kind {
        TokenKind::Or => Some((BinaryOperator::Or, 1)),
        TokenKind::And => Some((BinaryOperator::And, 2)),
        TokenKind::DoubleEquals => Some((BinaryOperator::Equal, 3)),
        TokenKind::NotEquals => Some((BinaryOperator::NotEqual, 3)),
        TokenKind::LessThan => Some((BinaryOperator::LessThan, 4)),
        TokenKind::GreaterThan => Some((BinaryOperator::GreaterThan, 4)),
        TokenKind::LessThanOrEqual => Some((BinaryOperator::LessThanOrEqual, 4)),
        TokenKind::GreaterThanOrEqual => Some((BinaryOperator::GreaterThanOrEqual, 4)),
        TokenKind::Plus => Some((BinaryOperator::Add, 5)),
        TokenKind::Minus => Some((BinaryOperator::Subtract, 5)),
        TokenKind::Asterisk => Some((BinaryOperator::Multiply, 6)),
        TokenKind::Slash => Some((BinaryOperator::Divide, 6)),
        _ => None,
    }
}

fn expression_end_tokens() -> [TokenKind; 8] {
    [
        TokenKind::LineFeed,
        TokenKind::Semicolon,
        TokenKind::Comma,
        TokenKind::Equal,
        TokenKind::BraceLeft,
        TokenKind::BraceRight,
        TokenKind::BracketRight,
        TokenKind::ParenthesisRight,
    ]
}
