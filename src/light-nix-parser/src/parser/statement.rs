use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        AssignStatement, Block, EnumDefine, Expression, FunctionArgument, FunctionArguments,
        FunctionAttribute, FunctionDefine, ImportStatement, LetStatement, Literal, MutationPolicy,
        MutationPolicyKind, Source, Spanned, Statement, Statements, StringLiteral, TypeDefine,
        TypeInfo, Typedef, TypedefBlock, TypedefValue, UseDeclare,
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

        let error_count = errors.len();
        let statement = match parse_statement(lexer, errors, allocator) {
            Some(statement) => statement,
            None => {
                let kind = current_kind(lexer);
                if errors.len() > error_count
                    && matches!(
                        kind,
                        TokenKind::LineFeed
                            | TokenKind::Semicolon
                            | TokenKind::BraceRight
                            | TokenKind::None
                    )
                {
                    if stop_at_brace && kind == TokenKind::BraceRight {
                        break;
                    }
                    if !stop_at_brace && kind == TokenKind::BraceRight {
                        lexer.next();
                    }
                    skip_statement_separator(lexer);
                    continue;
                }

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
        TokenKind::Import => {
            parse_import_statement(lexer, errors, allocator).map(Statement::ImportStatement)
        }
        TokenKind::Enum => parse_enum_define(lexer, errors, allocator).map(Statement::EnumDefine),
        TokenKind::Type => parse_type_define(lexer, errors, allocator).map(Statement::TypeDefine),
        TokenKind::Use => parse_use_declare(lexer, errors, allocator).map(Statement::UseDeclare),
        TokenKind::Let | TokenKind::Declare => {
            parse_let_statement(lexer, errors, allocator).map(Statement::LetStatement)
        }
        TokenKind::Inline | TokenKind::Opaque => {
            parse_function_define(lexer, errors, allocator).map(Statement::FunctionDefine)
        }
        _ => parse_assign_or_expression_statement(lexer, errors, allocator),
    }
}

fn parse_import_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator ImportStatement<'input>> {
    if current_kind(lexer) != TokenKind::Import {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    if current_kind(lexer) != TokenKind::StringLiteral {
        let error = recover_until(
            ParseErrorKind::InvalidImportStatement,
            lexer,
            &[
                TokenKind::LineFeed,
                TokenKind::Semicolon,
                TokenKind::BraceRight,
            ],
            Expected::StringLiteral,
            Scope::ImportStatement,
            allocator,
        );
        errors.push(error);
        return None;
    }

    let path_token = lexer.next().unwrap();
    let path = StringLiteral::new(path_token.text, path_token.span);

    Some(allocator.alloc(ImportStatement {
        path,
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
        let policy = parse_mutation_policy(lexer, errors, allocator);
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
            policy,
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

fn parse_let_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator LetStatement<'input, 'allocator>> {
    if !matches!(current_kind(lexer), TokenKind::Declare | TokenKind::Let) {
        return None;
    }

    let anchor = lexer.cast_anchor();
    let declare = if current_kind(lexer) == TokenKind::Declare {
        lexer.next();
        true
    } else {
        false
    };

    if current_kind(lexer) != TokenKind::Let {
        errors.push(error_here(
            ParseErrorKind::InvalidLetStatement,
            lexer,
            Expected::Token(TokenKind::Let),
            Scope::LetStatement,
        ));
        return None;
    }
    lexer.next();

    let policy = parse_mutation_policy(lexer, errors, allocator);

    let Some(name) = parse_literal(lexer) else {
        errors.push(error_here(
            ParseErrorKind::InvalidLetStatement,
            lexer,
            Expected::Literal,
            Scope::LetStatement,
        ));
        return None;
    };

    let type_info = if current_kind(lexer) == TokenKind::Colon {
        lexer.next();

        let type_info = parse_type_info(lexer, errors, allocator);
        if type_info.is_none() {
            let error = recover_until(
                ParseErrorKind::InvalidLetStatement,
                lexer,
                &[
                    TokenKind::Equal,
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::TypeInfo,
                Scope::LetStatement,
                allocator,
            );
            errors.push(error);
        }

        type_info
    } else {
        None
    };

    let value = if current_kind(lexer) == TokenKind::Equal {
        lexer.next();
        skip_line_feed(lexer);

        let value = parse_expression(lexer, errors, allocator);
        if value.is_none() {
            errors.push(error_here(
                ParseErrorKind::InvalidLetStatement,
                lexer,
                Expected::Expression,
                Scope::LetStatement,
            ));
        }

        value
    } else {
        None
    };

    Some(allocator.alloc(LetStatement {
        declare,
        policy,
        name,
        type_info,
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

fn parse_mutation_policy<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<MutationPolicy> {
    let anchor = lexer.cast_anchor();

    if current_kind(lexer) == TokenKind::Readonly {
        lexer.next();
        return Some(MutationPolicy {
            kind: MutationPolicyKind::Readonly,
            span: anchor.elapsed(lexer),
        });
    }

    if current_kind(lexer) != TokenKind::Tunable {
        return None;
    }
    lexer.next();

    let mut cost = None;

    if current_kind(lexer) == TokenKind::ParenthesisLeft {
        lexer.next();

        if current_kind(lexer) != TokenKind::Cost {
            let error = recover_until(
                ParseErrorKind::InvalidMutationPolicy,
                lexer,
                &[
                    TokenKind::Cost,
                    TokenKind::Equal,
                    TokenKind::NumericLiteral,
                    TokenKind::ParenthesisRight,
                ],
                Expected::Token(TokenKind::Cost),
                Scope::MutationPolicy,
                allocator,
            );
            errors.push(error);
        }
        if current_kind(lexer) == TokenKind::Cost {
            lexer.next();
        }

        if current_kind(lexer) != TokenKind::Equal {
            let error = recover_until(
                ParseErrorKind::InvalidMutationPolicy,
                lexer,
                &[
                    TokenKind::Equal,
                    TokenKind::NumericLiteral,
                    TokenKind::ParenthesisRight,
                ],
                Expected::Token(TokenKind::Equal),
                Scope::MutationPolicy,
                allocator,
            );
            errors.push(error);
        }
        if current_kind(lexer) == TokenKind::Equal {
            lexer.next();
        }

        if current_kind(lexer) == TokenKind::NumericLiteral {
            let token = lexer.current().unwrap();
            if let Some(value) = parse_policy_cost(token.text) {
                let token = lexer.next().unwrap();
                cost = Some(Spanned::new(value, token.span));
            } else {
                let error = recover_until(
                    ParseErrorKind::InvalidMutationPolicy,
                    lexer,
                    &[
                        TokenKind::ParenthesisRight,
                        TokenKind::Literal,
                        TokenKind::LineFeed,
                        TokenKind::Comma,
                        TokenKind::BraceRight,
                    ],
                    Expected::IntegerLiteral,
                    Scope::MutationPolicy,
                    allocator,
                );
                errors.push(error);
            }
        } else {
            errors.push(error_here(
                ParseErrorKind::InvalidMutationPolicy,
                lexer,
                Expected::IntegerLiteral,
                Scope::MutationPolicy,
            ));
        }

        if current_kind(lexer) != TokenKind::ParenthesisRight {
            let error = recover_until(
                ParseErrorKind::NonClosedParenthesis,
                lexer,
                &[
                    TokenKind::ParenthesisRight,
                    TokenKind::Literal,
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::BraceRight,
                ],
                Expected::Token(TokenKind::ParenthesisRight),
                Scope::MutationPolicy,
                allocator,
            );
            errors.push(error);
        }
        if current_kind(lexer) == TokenKind::ParenthesisRight {
            lexer.next();
        }
    }

    Some(MutationPolicy {
        kind: MutationPolicyKind::Tunable { cost },
        span: anchor.elapsed(lexer),
    })
}

fn parse_policy_cost(text: &str) -> Option<u64> {
    let mut value = 0_u64;
    let mut found_digit = false;

    for byte in text.bytes() {
        if byte == b'_' {
            continue;
        }
        if !byte.is_ascii_digit() {
            return None;
        }

        found_digit = true;
        value = value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
    }

    found_digit.then_some(value)
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
        ast::{BinaryOperator, Expression, MutationPolicyKind, Statement, TypedefValue},
        error::ParseErrorKind,
        lexer::Lexer,
        parser::parse_source,
    };

    #[test]
    fn parses_the_complete_grammar() {
        let source = r#"
import "./common.lnix"

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

let tunable profile = Profile::Desktop
programs.shojiwm.enable = true
if profile == Profile::Desktop {
    programs.firefox.enable = true
} else {
    programs.firefox.enable = false
}

inline function calculate(left: Number, right: Number) -> Number {
    let result: Number = left + right * 2
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
        assert_eq!(ast.statements.len(), 8);

        let Statement::TypeDefine(ty) = &ast.statements[2] else {
            panic!("expected type definition");
        };
        assert_eq!(ty.body.fields.len(), 3);
        assert!(matches!(ty.body.fields[2].value, TypedefValue::Block(_)));

        let Statement::FunctionDefine(function) = &ast.statements[7] else {
            panic!("expected function definition");
        };
        assert_eq!(function.return_type.unwrap().name.value, "Number");
        let Statement::LetStatement(result) = &function.body.statements.statements[0] else {
            panic!("expected let statement");
        };
        assert_eq!(result.type_info.unwrap().name.value, "Number");
        let Expression::Binary(add) = result.value.unwrap() else {
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
        let Expression::Binary(subtract) = difference.value.unwrap() else {
            panic!("expected subtraction");
        };
        assert_eq!(subtract.operator.value, BinaryOperator::Subtract);

        let Statement::LetStatement(called) = &function.body.statements.statements[2] else {
            panic!("expected function call");
        };
        let Expression::Primary(primary) = called.value.unwrap() else {
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
enum Profile { Desktop, =, Laptop }
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(!errors.is_empty(), "expected a recoverable error");
        assert_eq!(ast.statements.len(), 2);

        let Statement::EnumDefine(profile) = &ast.statements[0] else {
            panic!("expected enum");
        };
        assert_eq!(profile.variants.len(), 2);
        assert_eq!(profile.variants[0].value, "Desktop");
        assert_eq!(profile.variants[1].value, "Laptop");
        assert!(matches!(ast.statements[1], Statement::LetStatement(_)));
    }

    #[test]
    fn inputs_and_host_can_be_used_as_identifiers() {
        let source = r#"
inputs = true
host = false
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 2);
        assert!(
            ast.statements
                .iter()
                .all(|statement| matches!(statement, Statement::AssignStatement(_)))
        );
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

    #[test]
    fn let_type_and_initializer_are_independently_optional() {
        let source = r#"
let untyped
let typed: String
let initialized = "value"
let tunable complete: List<String> = []
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 4);

        let Statement::LetStatement(untyped) = &ast.statements[0] else {
            panic!("expected untyped let");
        };
        assert!(untyped.type_info.is_none());
        assert!(untyped.value.is_none());

        let Statement::LetStatement(typed) = &ast.statements[1] else {
            panic!("expected typed let");
        };
        assert_eq!(typed.type_info.unwrap().name.value, "String");
        assert!(typed.value.is_none());

        let Statement::LetStatement(initialized) = &ast.statements[2] else {
            panic!("expected initialized let");
        };
        assert!(initialized.type_info.is_none());
        assert!(initialized.value.is_some());

        let Statement::LetStatement(complete) = &ast.statements[3] else {
            panic!("expected complete let");
        };
        assert!(matches!(
            complete.policy.as_ref().map(|policy| &policy.kind),
            Some(MutationPolicyKind::Tunable { cost: None })
        ));
        assert_eq!(complete.type_info.unwrap().name.value, "List");
        assert_eq!(
            complete.type_info.unwrap().parameter.unwrap().name.value,
            "String"
        );
        assert!(complete.value.is_some());
    }

    #[test]
    fn let_recovers_from_a_missing_type_before_the_initializer() {
        let source = r#"
let broken: = true
let recovered
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert_eq!(errors.len(), 1, "parse errors: {errors:#?}");
        assert_eq!(errors[0].kind, ParseErrorKind::InvalidLetStatement);
        assert_eq!(ast.statements.len(), 2);

        let Statement::LetStatement(broken) = &ast.statements[0] else {
            panic!("expected recovered let");
        };
        assert!(broken.type_info.is_none());
        assert!(broken.value.is_some());
        assert!(matches!(ast.statements[1], Statement::LetStatement(_)));
    }

    #[test]
    fn parses_import_statements_with_zero_copy_paths() {
        let source = r#"
import "./modules/programs.lnix"
import 'catalog/firefox.lnix';
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 3);

        let Statement::ImportStatement(first) = &ast.statements[0] else {
            panic!("expected first import statement");
        };
        assert_eq!(first.path.value, r#""./modules/programs.lnix""#);
        assert_eq!(&source[first.path.span()], first.path.value);

        let Statement::ImportStatement(second) = &ast.statements[1] else {
            panic!("expected second import statement");
        };
        assert_eq!(second.path.value, "'catalog/firefox.lnix'");
        assert_eq!(&source[second.path.span()], second.path.value);
        assert!(matches!(ast.statements[2], Statement::LetStatement(_)));
    }

    #[test]
    fn invalid_import_path_recovers_to_the_following_statement() {
        let source = r#"
import nixpkgs
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert_eq!(errors.len(), 1, "parse errors: {errors:#?}");
        assert_eq!(errors[0].kind, ParseErrorKind::InvalidImportStatement);
        assert_eq!(errors[0].scope, crate::error::Scope::ImportStatement);
        assert_eq!(ast.statements.len(), 1);
        assert!(matches!(ast.statements[0], Statement::LetStatement(_)));
    }

    #[test]
    fn parses_binding_and_field_mutation_policies() {
        let source = r#"
type Programs {
    readonly example0: Example0,
    tunable(cost = 200) example1: Example1,
    example2: Example2
}
declare let tunable(cost = 1) programs: Programs
let readonly snapshot: Programs
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 3);

        let Statement::TypeDefine(programs_type) = &ast.statements[0] else {
            panic!("expected Programs type");
        };
        let [example0, example1, example2] = programs_type.body.fields else {
            panic!("expected three fields");
        };
        assert!(matches!(
            example0.policy.as_ref().map(|policy| &policy.kind),
            Some(MutationPolicyKind::Readonly)
        ));
        let Some(MutationPolicyKind::Tunable { cost: Some(cost) }) =
            example1.policy.as_ref().map(|policy| &policy.kind)
        else {
            panic!("expected costed tunable field");
        };
        assert_eq!(cost.value, 200);
        assert!(example2.policy.is_none());

        let Statement::LetStatement(programs) = &ast.statements[1] else {
            panic!("expected programs declaration");
        };
        assert!(programs.declare);
        let Some(MutationPolicyKind::Tunable { cost: Some(cost) }) =
            programs.policy.as_ref().map(|policy| &policy.kind)
        else {
            panic!("expected costed tunable declaration");
        };
        assert_eq!(cost.value, 1);

        let Statement::LetStatement(snapshot) = &ast.statements[2] else {
            panic!("expected snapshot binding");
        };
        assert!(!snapshot.declare);
        assert!(matches!(
            snapshot.policy.as_ref().map(|policy| &policy.kind),
            Some(MutationPolicyKind::Readonly)
        ));
    }

    #[test]
    fn invalid_policy_cost_recovers_to_the_field_and_binding() {
        let source = r#"
type Programs {
    tunable(cost = 1.5) example: Example,
    readonly safe: Safe
}
declare let tunable(cost =) programs: Programs
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert_eq!(errors.len(), 2, "parse errors: {errors:#?}");
        assert!(
            errors
                .iter()
                .all(|error| error.kind == ParseErrorKind::InvalidMutationPolicy)
        );
        assert_eq!(ast.statements.len(), 3);

        let Statement::TypeDefine(programs_type) = &ast.statements[0] else {
            panic!("expected recovered Programs type");
        };
        assert_eq!(programs_type.body.fields.len(), 2);
        assert!(matches!(
            programs_type.body.fields[1]
                .policy
                .as_ref()
                .map(|policy| &policy.kind),
            Some(MutationPolicyKind::Readonly)
        ));

        let Statement::LetStatement(programs) = &ast.statements[1] else {
            panic!("expected recovered declaration");
        };
        assert!(programs.declare);
        assert!(matches!(
            programs.policy.as_ref().map(|policy| &policy.kind),
            Some(MutationPolicyKind::Tunable { cost: None })
        ));
        assert!(matches!(ast.statements[2], Statement::LetStatement(_)));
    }
}
