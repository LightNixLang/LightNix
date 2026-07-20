use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        AssignStatement, Block, EnumDefine, Expression, FunctionArgument, FunctionArguments,
        FunctionAttribute, FunctionDefine, HostDefine, Inputs, InputsElement, LetStatement,
        Literal, Source, Spanned, Statement, Statements, StringLiteral, TypeDefine, TypeInfo,
        Typedef, TypedefBlock, TypedefValue, UseDeclare,
    },
    error::{Expected, ParseErrorKind, Scope, error_here, recover_until},
    lexer::{Lexer, TokenKind},
};

use super::{
    ParseErrors, current_kind, expression::parse_expression, is_statement_start, skip_line_feed,
    skip_list_separator, skip_statement_separator,
};

pub fn parse_source<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> &'allocator Source<'input, 'allocator> {
    let statements = parse_statements(lexer, errors, allocator, false);
    allocator.alloc(statements)
}

fn parse_statements<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
    stop_at_brace: bool,
) -> Statements<'input, 'allocator> {
    let anchor = lexer.cast_anchor();
    let mut statements = Vec::new_in(allocator);

    skip_statement_separator(lexer);

    loop {
        let kind = current_kind(lexer);
        if kind == TokenKind::None || stop_at_brace && kind == TokenKind::BraceRight {
            break;
        }

        let statement = match parse_statement(lexer, errors, allocator) {
            Some(statement) => statement,
            None => {
                let error = recover_until(
                    ParseErrorKind::InvalidStatement,
                    lexer,
                    &[
                        TokenKind::LineFeed,
                        TokenKind::Semicolon,
                        TokenKind::BraceRight,
                    ],
                    Expected::Statement,
                    Scope::Statements,
                    allocator,
                );
                errors.push(error);

                if stop_at_brace && current_kind(lexer) == TokenKind::BraceRight {
                    break;
                }
                if !stop_at_brace && current_kind(lexer) == TokenKind::BraceRight {
                    lexer.next();
                }
                skip_statement_separator(lexer);
                continue;
            }
        };
        statements.push(statement);

        if skip_statement_separator(lexer) {
            continue;
        }

        let kind = current_kind(lexer);
        if kind == TokenKind::None || stop_at_brace && kind == TokenKind::BraceRight {
            continue;
        }

        if is_statement_start(kind) {
            errors.push(error_here(
                ParseErrorKind::InvalidStatementSeparator,
                lexer,
                Expected::StatementSeparator,
                Scope::Statements,
            ));
            continue;
        }

        let error = recover_until(
            ParseErrorKind::InvalidStatementSeparator,
            lexer,
            &[
                TokenKind::LineFeed,
                TokenKind::Semicolon,
                TokenKind::BraceRight,
            ],
            Expected::StatementSeparator,
            Scope::Statements,
            allocator,
        );
        errors.push(error);
        skip_statement_separator(lexer);
    }

    let statements = allocator.alloc_slice_fill_iter(statements);
    Statements {
        statements,
        span: anchor.elapsed(lexer),
    }
}

fn parse_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<Statement<'input, 'allocator>> {
    match current_kind(lexer) {
        TokenKind::Inputs => parse_inputs(lexer, errors, allocator).map(Statement::Inputs),
        TokenKind::Enum => parse_enum_define(lexer, errors, allocator).map(Statement::EnumDefine),
        TokenKind::Type => parse_type_define(lexer, errors, allocator).map(Statement::TypeDefine),
        TokenKind::Use => parse_use_declare(lexer, errors, allocator).map(Statement::UseDeclare),
        TokenKind::Host => parse_host_define(lexer, errors, allocator).map(Statement::HostDefine),
        TokenKind::Let => {
            parse_let_statement(lexer, errors, allocator).map(Statement::LetStatement)
        }
        TokenKind::Inline | TokenKind::Opaque => {
            parse_function_define(lexer, errors, allocator).map(Statement::FunctionDefine)
        }
        _ => parse_assign_or_expression_statement(lexer, errors, allocator),
    }
}

fn parse_inputs<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator Inputs<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Inputs {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    if !recover_opening_token(
        lexer,
        errors,
        allocator,
        TokenKind::BraceLeft,
        Scope::Inputs,
    ) {
        return None;
    }

    skip_statement_separator(lexer);
    let mut elements = Vec::new_in(allocator);

    loop {
        if is_statement_start(current_kind(lexer)) {
            break;
        }
        match current_kind(lexer) {
            TokenKind::BraceRight | TokenKind::None => break,
            _ => {}
        }

        let element_anchor = lexer.cast_anchor();
        let key = if current_kind(lexer) == TokenKind::Literal {
            parse_literal(lexer).unwrap()
        } else {
            let error = recover_until(
                ParseErrorKind::InvalidInputsElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::InputsElement,
                Scope::Inputs,
                allocator,
            );
            errors.push(error);
            if current_kind(lexer) == TokenKind::BraceRight {
                break;
            }
            skip_statement_separator(lexer);
            continue;
        };

        if current_kind(lexer) != TokenKind::Equal {
            let error = recover_until(
                ParseErrorKind::InvalidInputsElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::Token(TokenKind::Equal),
                Scope::Inputs,
                allocator,
            );
            errors.push(error);
            skip_statement_separator(lexer);
            continue;
        }
        lexer.next();

        if current_kind(lexer) != TokenKind::StringLiteral {
            let error = recover_until(
                ParseErrorKind::InvalidInputsElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::StringLiteral,
                Scope::Inputs,
                allocator,
            );
            errors.push(error);
            skip_statement_separator(lexer);
            continue;
        }
        let value_token = lexer.next().unwrap();
        let value = StringLiteral::new(value_token.text, value_token.span);

        elements.push(InputsElement {
            key,
            value,
            span: element_anchor.elapsed(lexer),
        });

        if current_kind(lexer) == TokenKind::BraceRight {
            break;
        }
        if !skip_statement_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidInputsElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::StatementSeparator,
                Scope::Inputs,
                allocator,
            );
            errors.push(error);
            skip_statement_separator(lexer);
        }
    }

    close_delimiter(
        lexer,
        errors,
        allocator,
        TokenKind::BraceRight,
        ParseErrorKind::NonClosedBrace,
        Scope::Inputs,
    );

    let elements = allocator.alloc_slice_fill_iter(elements);
    Some(allocator.alloc(Inputs {
        elements,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_enum_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator EnumDefine<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Enum {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    let Some(name) = parse_literal(lexer) else {
        errors.push(error_here(
            ParseErrorKind::InvalidEnumVariant,
            lexer,
            Expected::Literal,
            Scope::EnumDefine,
        ));
        return None;
    };

    if !recover_opening_token(
        lexer,
        errors,
        allocator,
        TokenKind::BraceLeft,
        Scope::EnumDefine,
    ) {
        return None;
    }
    skip_line_feed(lexer);

    let mut variants = Vec::new_in(allocator);

    loop {
        if is_statement_start(current_kind(lexer)) {
            break;
        }
        match current_kind(lexer) {
            TokenKind::BraceRight | TokenKind::None => break,
            TokenKind::Literal => variants.push(parse_literal(lexer).unwrap()),
            _ => {
                let error = recover_until(
                    ParseErrorKind::InvalidEnumVariant,
                    lexer,
                    &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                    Expected::EnumVariant,
                    Scope::EnumDefine,
                    allocator,
                );
                errors.push(error);
            }
        }

        if current_kind(lexer) == TokenKind::BraceRight {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidEnumVariant,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                Expected::Token(TokenKind::Comma),
                Scope::EnumDefine,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    close_delimiter(
        lexer,
        errors,
        allocator,
        TokenKind::BraceRight,
        ParseErrorKind::NonClosedBrace,
        Scope::EnumDefine,
    );

    let variants = allocator.alloc_slice_fill_iter(variants);
    Some(allocator.alloc(EnumDefine {
        name,
        variants,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_type_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator TypeDefine<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Type {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    let Some(name) = parse_literal(lexer) else {
        errors.push(error_here(
            ParseErrorKind::InvalidTypedef,
            lexer,
            Expected::Literal,
            Scope::TypeDefine,
        ));
        return None;
    };

    let body = parse_typedef_block(lexer, errors, allocator)?;
    Some(allocator.alloc(TypeDefine {
        name,
        body,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_typedef_block<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator TypedefBlock<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::BraceLeft {
        errors.push(error_here(
            ParseErrorKind::InvalidTypedef,
            lexer,
            Expected::Token(TokenKind::BraceLeft),
            Scope::Typedef,
        ));
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    skip_line_feed(lexer);
    let mut fields = Vec::new_in(allocator);

    loop {
        if is_statement_start(current_kind(lexer)) {
            break;
        }
        match current_kind(lexer) {
            TokenKind::BraceRight | TokenKind::None => break,
            _ => {}
        }

        let field_anchor = lexer.cast_anchor();
        let Some(name) = parse_literal(lexer) else {
            let error = recover_until(
                ParseErrorKind::InvalidTypedef,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                Expected::Typedef,
                Scope::Typedef,
                allocator,
            );
            errors.push(error);
            if current_kind(lexer) == TokenKind::BraceRight {
                break;
            }
            skip_list_separator(lexer);
            continue;
        };

        if current_kind(lexer) != TokenKind::Colon {
            let error = recover_until(
                ParseErrorKind::InvalidTypedef,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                Expected::Token(TokenKind::Colon),
                Scope::Typedef,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
            continue;
        }
        lexer.next();

        let value = if current_kind(lexer) == TokenKind::BraceLeft {
            parse_typedef_block(lexer, errors, allocator).map(TypedefValue::Block)
        } else {
            parse_type_info(lexer, errors, allocator).map(TypedefValue::TypeInfo)
        };

        let Some(value) = value else {
            let error = recover_until(
                ParseErrorKind::InvalidTypedef,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                Expected::TypeInfo,
                Scope::Typedef,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
            continue;
        };

        fields.push(Typedef {
            name,
            value,
            span: field_anchor.elapsed(lexer),
        });

        if current_kind(lexer) == TokenKind::BraceRight {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidTypedef,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                Expected::Token(TokenKind::Comma),
                Scope::Typedef,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    close_delimiter(
        lexer,
        errors,
        allocator,
        TokenKind::BraceRight,
        ParseErrorKind::NonClosedBrace,
        Scope::Typedef,
    );

    let fields = allocator.alloc_slice_fill_iter(fields);
    Some(allocator.alloc(TypedefBlock {
        fields,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_use_declare<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator UseDeclare<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Use {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    if !recover_opening_token(
        lexer,
        errors,
        allocator,
        TokenKind::BracketLeft,
        Scope::UseDeclare,
    ) {
        return None;
    }
    skip_line_feed(lexer);

    let mut names = Vec::new_in(allocator);
    loop {
        if is_statement_start(current_kind(lexer)) {
            break;
        }
        match current_kind(lexer) {
            TokenKind::BracketRight | TokenKind::None => break,
            TokenKind::Literal => names.push(parse_literal(lexer).unwrap()),
            _ => {
                let error = recover_until(
                    ParseErrorKind::InvalidUseElement,
                    lexer,
                    &[
                        TokenKind::LineFeed,
                        TokenKind::Comma,
                        TokenKind::BracketRight,
                    ],
                    Expected::UseElement,
                    Scope::UseDeclare,
                    allocator,
                );
                errors.push(error);
            }
        }

        if current_kind(lexer) == TokenKind::BracketRight {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidUseElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::BracketRight,
                ],
                Expected::Token(TokenKind::Comma),
                Scope::UseDeclare,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    close_delimiter(
        lexer,
        errors,
        allocator,
        TokenKind::BracketRight,
        ParseErrorKind::NonClosedBracket,
        Scope::UseDeclare,
    );

    let names = allocator.alloc_slice_fill_iter(names);
    Some(allocator.alloc(UseDeclare {
        names,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_host_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator HostDefine<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Host {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    if current_kind(lexer) != TokenKind::StringLiteral {
        errors.push(error_here(
            ParseErrorKind::UnexpectedToken,
            lexer,
            Expected::StringLiteral,
            Scope::HostDefine,
        ));
        return None;
    }
    let host_token = lexer.next().unwrap();
    let host = StringLiteral::new(host_token.text, host_token.span);
    let body = parse_block(lexer, errors, allocator)?;

    Some(allocator.alloc(HostDefine {
        host,
        body,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_let_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator LetStatement<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Let {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    let tunable = if current_kind(lexer) == TokenKind::Tunable {
        lexer.next();
        true
    } else {
        false
    };

    let Some(name) = parse_literal(lexer) else {
        errors.push(error_here(
            ParseErrorKind::InvalidLetStatement,
            lexer,
            Expected::Literal,
            Scope::LetStatement,
        ));
        return None;
    };

    if current_kind(lexer) != TokenKind::Equal {
        errors.push(error_here(
            ParseErrorKind::InvalidLetStatement,
            lexer,
            Expected::Token(TokenKind::Equal),
            Scope::LetStatement,
        ));
        return None;
    }
    lexer.next();
    skip_line_feed(lexer);

    let Some(value) = parse_expression(lexer, errors, allocator) else {
        errors.push(error_here(
            ParseErrorKind::InvalidLetStatement,
            lexer,
            Expected::Expression,
            Scope::LetStatement,
        ));
        return None;
    };

    Some(allocator.alloc(LetStatement {
        tunable,
        name,
        value,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_assign_or_expression_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<Statement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let target = parse_expression(lexer, errors, allocator)?;

    if current_kind(lexer) != TokenKind::Equal {
        if matches!(target, Expression::If(_) | Expression::Return(_)) {
            return Some(Statement::Expression(target));
        }
        lexer.back_to_anchor(anchor);
        return None;
    }
    lexer.next();

    let Some(value) = parse_expression(lexer, errors, allocator) else {
        errors.push(error_here(
            ParseErrorKind::InvalidAssignStatement,
            lexer,
            Expected::Expression,
            Scope::AssignStatement,
        ));
        return None;
    };

    let assignment = allocator.alloc(AssignStatement {
        target,
        value,
        span: anchor.elapsed(lexer),
    });
    Some(Statement::AssignStatement(assignment))
}

fn parse_function_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator FunctionDefine<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();

    let attribute = match current_kind(lexer) {
        TokenKind::Inline => FunctionAttribute::Inline,
        TokenKind::Opaque => FunctionAttribute::Opaque,
        _ => return None,
    };
    let attribute_token = lexer.next().unwrap();
    let attribute = Spanned::new(attribute, attribute_token.span);

    if current_kind(lexer) != TokenKind::Function {
        errors.push(error_here(
            ParseErrorKind::InvalidFunctionDefine,
            lexer,
            Expected::Token(TokenKind::Function),
            Scope::FunctionDefine,
        ));
        return None;
    }
    lexer.next();

    let Some(name) = parse_literal(lexer) else {
        errors.push(error_here(
            ParseErrorKind::InvalidFunctionDefine,
            lexer,
            Expected::Literal,
            Scope::FunctionDefine,
        ));
        return None;
    };

    let arguments = parse_function_arguments(lexer, errors, allocator)?;
    let return_type = parse_function_return_type(lexer, errors, allocator);
    let body = parse_block(lexer, errors, allocator)?;

    Some(allocator.alloc(FunctionDefine {
        attribute,
        name,
        arguments,
        return_type,
        body,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_function_return_type<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator TypeInfo<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::ThinArrow {
        return None;
    }
    lexer.next();

    let return_type = parse_type_info(lexer, errors, allocator);
    if return_type.is_none() {
        let error = recover_until(
            ParseErrorKind::InvalidFunctionDefine,
            lexer,
            &[TokenKind::BraceLeft, TokenKind::LineFeed],
            Expected::TypeInfo,
            Scope::FunctionDefine,
            allocator,
        );
        errors.push(error);
    }

    return_type
}

fn parse_function_arguments<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<FunctionArguments<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::ParenthesisLeft {
        errors.push(error_here(
            ParseErrorKind::InvalidFunctionDefine,
            lexer,
            Expected::Token(TokenKind::ParenthesisLeft),
            Scope::FunctionArguments,
        ));
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    skip_line_feed(lexer);
    let mut arguments = Vec::new_in(allocator);

    loop {
        if is_statement_start(current_kind(lexer)) {
            break;
        }
        match current_kind(lexer) {
            TokenKind::ParenthesisRight | TokenKind::None => break,
            _ => {}
        }

        let argument_anchor = lexer.cast_anchor();
        let Some(name) = parse_literal(lexer) else {
            let error = recover_until(
                ParseErrorKind::InvalidFunctionArgument,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::ParenthesisRight,
                ],
                Expected::FunctionArgument,
                Scope::FunctionArguments,
                allocator,
            );
            errors.push(error);
            if current_kind(lexer) == TokenKind::ParenthesisRight {
                break;
            }
            skip_list_separator(lexer);
            continue;
        };

        if current_kind(lexer) != TokenKind::Colon {
            let error = recover_until(
                ParseErrorKind::InvalidFunctionArgument,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::ParenthesisRight,
                ],
                Expected::Token(TokenKind::Colon),
                Scope::FunctionArguments,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
            continue;
        }
        lexer.next();

        let Some(type_info) = parse_type_info(lexer, errors, allocator) else {
            let error = recover_until(
                ParseErrorKind::InvalidFunctionArgument,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::ParenthesisRight,
                ],
                Expected::TypeInfo,
                Scope::FunctionArguments,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
            continue;
        };

        arguments.push(FunctionArgument {
            name,
            type_info,
            span: argument_anchor.elapsed(lexer),
        });

        if current_kind(lexer) == TokenKind::ParenthesisRight {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidFunctionArgument,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::ParenthesisRight,
                ],
                Expected::Token(TokenKind::Comma),
                Scope::FunctionArguments,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    close_delimiter(
        lexer,
        errors,
        allocator,
        TokenKind::ParenthesisRight,
        ParseErrorKind::NonClosedParenthesis,
        Scope::FunctionArguments,
    );

    let arguments = allocator.alloc_slice_fill_iter(arguments);
    Some(FunctionArguments {
        arguments,
        span: anchor.elapsed(lexer),
    })
}

pub(super) fn parse_block<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator Block<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::BraceLeft {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    let statements = parse_statements(lexer, errors, allocator, true);

    close_delimiter(
        lexer,
        errors,
        allocator,
        TokenKind::BraceRight,
        ParseErrorKind::NonClosedBrace,
        Scope::Block,
    );

    Some(allocator.alloc(Block {
        statements,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_type_info<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator TypeInfo<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let name = parse_literal(lexer)?;

    let parameter = if current_kind(lexer) == TokenKind::LessThan {
        lexer.next();

        let parameter = parse_type_info(lexer, errors, allocator);
        if parameter.is_none() {
            errors.push(error_here(
                ParseErrorKind::InvalidTypeInfo,
                lexer,
                Expected::TypeInfo,
                Scope::TypeInfo,
            ));
        }

        if current_kind(lexer) != TokenKind::GreaterThan {
            let error = recover_until(
                ParseErrorKind::NonClosedTypeParameter,
                lexer,
                &[
                    TokenKind::GreaterThan,
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::BraceRight,
                    TokenKind::ParenthesisRight,
                ],
                Expected::Token(TokenKind::GreaterThan),
                Scope::TypeInfo,
                allocator,
            );
            errors.push(error);
        }
        if current_kind(lexer) == TokenKind::GreaterThan {
            lexer.next();
        }

        parameter
    } else {
        None
    };

    Some(allocator.alloc(TypeInfo {
        name,
        parameter,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_literal<'input>(lexer: &mut Lexer<'input>) -> Option<Literal<'input>> {
    if current_kind(lexer) != TokenKind::Literal {
        return None;
    }

    let token = lexer.next().unwrap();
    Some(Literal::new(token.text, token.span))
}

fn recover_opening_token<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
    token: TokenKind,
    scope: Scope,
) -> bool {
    if current_kind(lexer) == token {
        lexer.next();
        return true;
    }

    let error = recover_until(
        ParseErrorKind::UnexpectedToken,
        lexer,
        &[
            token,
            TokenKind::LineFeed,
            TokenKind::Semicolon,
            TokenKind::None,
        ],
        Expected::Token(token),
        scope,
        allocator,
    );
    errors.push(error);

    if current_kind(lexer) == token {
        lexer.next();
        true
    } else {
        false
    }
}

fn close_delimiter<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
    token: TokenKind,
    kind: ParseErrorKind,
    scope: Scope,
) {
    if current_kind(lexer) != token {
        let error = if is_statement_start(current_kind(lexer)) {
            error_here(kind, lexer, Expected::Token(token), scope)
        } else {
            recover_until(
                kind,
                lexer,
                &[token],
                Expected::Token(token),
                scope,
                allocator,
            )
        };
        errors.push(error);
    }

    if current_kind(lexer) == token {
        lexer.next();
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::vec::Vec;
    use bumpalo::Bump;

    use crate::{
        ast::{BinaryOperator, Expression, Statement, TypedefValue},
        error::ParseErrorKind,
        lexer::Lexer,
        parser::parse_source,
    };

    #[test]
    fn parses_the_complete_grammar() {
        let source = r#"
inputs {
    nixpkgs = "github:NixOS/nixpkgs"
    home = "github:nix-community/home-manager";
}

enum Profile {
    Desktop,
    Laptop
}

type Config {
    profile: Profile,
    users: List<String>,
    nested: {
        enabled: Bool
    }
}

use [
    nixpkgs,
    home
]

host "desktop" {
    let tunable profile = Profile::Desktop
    programs.shojiwm.enable = true
    if profile == Profile::Desktop {
        programs.firefox.enable = true
    } else {
        programs.firefox.enable = false
    }
}

inline function calculate(left: Number, right: Number) -> Number {
    let result = left + right * 2
    let difference = left-right
    let called = math::sum([left, right], - right)
    return called
}
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 6);

        let Statement::TypeDefine(ty) = &ast.statements[2] else {
            panic!("expected type definition");
        };
        assert_eq!(ty.body.fields.len(), 3);
        assert!(matches!(ty.body.fields[2].value, TypedefValue::Block(_)));

        let Statement::FunctionDefine(function) = &ast.statements[5] else {
            panic!("expected function definition");
        };
        assert_eq!(function.return_type.unwrap().name.value, "Number");
        let Statement::LetStatement(result) = &function.body.statements.statements[0] else {
            panic!("expected let statement");
        };
        let Expression::Binary(add) = result.value else {
            panic!("expected addition");
        };
        assert_eq!(add.operator.value, BinaryOperator::Add);
        let Expression::Binary(multiply) = add.right else {
            panic!("expected multiplication on the right");
        };
        assert_eq!(multiply.operator.value, BinaryOperator::Multiply);

        let Statement::LetStatement(difference) = &function.body.statements.statements[1] else {
            panic!("expected difference");
        };
        let Expression::Binary(subtract) = difference.value else {
            panic!("expected subtraction");
        };
        assert_eq!(subtract.operator.value, BinaryOperator::Subtract);

        let Statement::LetStatement(called) = &function.body.statements.statements[2] else {
            panic!("expected function call");
        };
        let Expression::Primary(primary) = called.value else {
            panic!("expected primary expression");
        };
        let call = primary.accesses[0].call.unwrap();
        assert_eq!(call.arguments.len(), 2);
        assert!(matches!(call.arguments[1], Expression::Unary(_)));

        assert!(matches!(
            function.body.statements.statements[3],
            Statement::Expression(Expression::Return(_))
        ));
    }

    #[test]
    fn recovers_inside_a_statement_and_parses_following_statements() {
        let source = r#"
inputs {
    broken = 123;
    good = "value"
}
enum Profile { Desktop, =, Laptop }
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.len() >= 2, "expected recoverable errors");
        assert_eq!(ast.statements.len(), 3);

        let Statement::Inputs(inputs) = &ast.statements[0] else {
            panic!("expected inputs");
        };
        assert_eq!(inputs.elements.len(), 1);
        assert_eq!(inputs.elements[0].key.value, "good");
        assert!(matches!(ast.statements[2], Statement::LetStatement(_)));
    }

    #[test]
    fn missing_closing_delimiter_does_not_consume_the_next_statement() {
        let source = r#"
use [nixpkgs
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(!errors.is_empty());
        assert_eq!(ast.statements.len(), 2);
        assert!(matches!(ast.statements[0], Statement::UseDeclare(_)));
        assert!(matches!(ast.statements[1], Statement::LetStatement(_)));
    }

    #[test]
    fn function_return_type_is_optional_and_recovers_when_missing() {
        let source = r#"
opaque function inferred() {
    return true
}
inline function broken() -> {
    return false
}
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert_eq!(errors.len(), 1, "parse errors: {errors:#?}");
        assert_eq!(errors[0].kind, ParseErrorKind::InvalidFunctionDefine);
        assert_eq!(ast.statements.len(), 3);

        let Statement::FunctionDefine(inferred) = &ast.statements[0] else {
            panic!("expected inferred function");
        };
        assert!(inferred.return_type.is_none());

        let Statement::FunctionDefine(broken) = &ast.statements[1] else {
            panic!("expected recovered function");
        };
        assert!(broken.return_type.is_none());
        assert!(matches!(ast.statements[2], Statement::LetStatement(_)));
    }
}
