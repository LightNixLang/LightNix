use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    ast::{
        AssertStatement, AssignStatement, Block, EnumDefine, EnumVariant, Expression,
        FunctionArgument, FunctionArguments, FunctionAttribute, FunctionDefine, GenericParameter,
        GenericParameters, ImplementsDefine, ImportElement, ImportKind, ImportStatement,
        InterfaceDefine, LetStatement, Literal, MutationPolicy, MutationPolicyKind, Source,
        Spanned, Statement, Statements, StringLiteral, TypeDefine, TypeInfo, Typedef, TypedefBlock,
        TypedefValue, UseDeclare, WhereClause, WherePredicate,
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
                    && (is_statement_start(kind)
                        || matches!(
                            kind,
                            TokenKind::LineFeed
                                | TokenKind::Semicolon
                                | TokenKind::BraceRight
                                | TokenKind::None
                        ))
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
        TokenKind::Export => parse_exported_statement(lexer, errors, allocator),
        TokenKind::Enum => parse_enum_define(lexer, errors, allocator).map(Statement::EnumDefine),
        TokenKind::Type => parse_type_define(lexer, errors, allocator).map(Statement::TypeDefine),
        TokenKind::Interface => {
            parse_interface_define(lexer, errors, allocator).map(Statement::InterfaceDefine)
        }
        TokenKind::Implements => {
            parse_implements_define(lexer, errors, allocator).map(Statement::ImplementsDefine)
        }
        TokenKind::Use => parse_use_declare(lexer, errors, allocator).map(Statement::UseDeclare),
        TokenKind::Let | TokenKind::Declare => {
            parse_let_statement(lexer, errors, allocator).map(Statement::LetStatement)
        }
        TokenKind::Assert => {
            parse_assert_statement(lexer, errors, allocator).map(Statement::AssertStatement)
        }
        TokenKind::Inline | TokenKind::Opaque => {
            parse_function_define(lexer, errors, allocator).map(Statement::FunctionDefine)
        }
        _ => parse_assign_or_expression_statement(lexer, errors, allocator),
    }
}

fn parse_exported_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<Statement<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    lexer.next();
    let declaration_kind = current_kind(lexer);
    lexer.back_to_anchor(anchor);

    match declaration_kind {
        TokenKind::Enum => parse_enum_define(lexer, errors, allocator).map(Statement::EnumDefine),
        TokenKind::Type => parse_type_define(lexer, errors, allocator).map(Statement::TypeDefine),
        TokenKind::Interface => {
            parse_interface_define(lexer, errors, allocator).map(Statement::InterfaceDefine)
        }
        TokenKind::Declare | TokenKind::Let => {
            parse_let_statement(lexer, errors, allocator).map(Statement::LetStatement)
        }
        TokenKind::Inline | TokenKind::Opaque => {
            parse_function_define(lexer, errors, allocator).map(Statement::FunctionDefine)
        }
        _ => {
            lexer.next();
            let error = recover_until(
                ParseErrorKind::InvalidExportStatement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::ExportableDeclaration,
                Scope::ExportStatement,
                allocator,
            );
            errors.push(error);
            None
        }
    }
}

fn parse_import_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator ImportStatement<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Import {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    let kind = match current_kind(lexer) {
        TokenKind::StringLiteral => ImportKind::SideEffect,
        TokenKind::BraceLeft => ImportKind::Named(parse_import_elements(lexer, errors, allocator)),
        TokenKind::Asterisk => {
            lexer.next();

            if current_kind(lexer) != TokenKind::As {
                errors.push(error_here(
                    ParseErrorKind::InvalidImportStatement,
                    lexer,
                    Expected::Token(TokenKind::As),
                    Scope::ImportStatement,
                ));
                return None;
            }
            lexer.next();

            let Some(alias) = parse_literal(lexer) else {
                errors.push(error_here(
                    ParseErrorKind::InvalidImportStatement,
                    lexer,
                    Expected::Literal,
                    Scope::ImportStatement,
                ));
                return None;
            };

            ImportKind::Namespace { alias }
        }
        _ => {
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
    };

    if matches!(&kind, ImportKind::SideEffect) {
        let path_token = lexer.next().unwrap();
        let path = StringLiteral::new(path_token.text, path_token.span);
        return Some(allocator.alloc(ImportStatement {
            kind,
            path,
            span: anchor.elapsed(lexer),
        }));
    }

    if current_kind(lexer) != TokenKind::From {
        errors.push(error_here(
            ParseErrorKind::InvalidImportStatement,
            lexer,
            Expected::Token(TokenKind::From),
            Scope::ImportStatement,
        ));
        if current_kind(lexer) != TokenKind::StringLiteral {
            return None;
        }
    } else {
        lexer.next();
    }

    if current_kind(lexer) != TokenKind::StringLiteral {
        errors.push(error_here(
            ParseErrorKind::InvalidImportStatement,
            lexer,
            Expected::StringLiteral,
            Scope::ImportStatement,
        ));
        return None;
    }
    let path_token = lexer.next().unwrap();
    let path = StringLiteral::new(path_token.text, path_token.span);

    Some(allocator.alloc(ImportStatement {
        kind,
        path,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_import_elements<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> &'allocator [ImportElement<'input>] {
    lexer.next();
    skip_line_feed(lexer);
    let mut elements = Vec::new_in(allocator);

    loop {
        if matches!(
            current_kind(lexer),
            TokenKind::BraceRight | TokenKind::From | TokenKind::None
        ) || is_statement_start(current_kind(lexer))
        {
            break;
        }

        let element_anchor = lexer.cast_anchor();
        let Some(name) = parse_literal(lexer) else {
            let error = recover_until(
                ParseErrorKind::InvalidImportElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::BraceRight,
                    TokenKind::From,
                ],
                Expected::ImportElement,
                Scope::ImportElement,
                allocator,
            );
            errors.push(error);
            if matches!(current_kind(lexer), TokenKind::BraceRight | TokenKind::From) {
                break;
            }
            skip_list_separator(lexer);
            continue;
        };

        let alias = if current_kind(lexer) == TokenKind::As {
            lexer.next();
            let alias = parse_literal(lexer);
            if alias.is_none() {
                errors.push(error_here(
                    ParseErrorKind::InvalidImportElement,
                    lexer,
                    Expected::Literal,
                    Scope::ImportElement,
                ));
            }
            alias
        } else {
            None
        };

        elements.push(ImportElement {
            name,
            alias,
            span: element_anchor.elapsed(lexer),
        });

        if matches!(current_kind(lexer), TokenKind::BraceRight | TokenKind::From) {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidImportElement,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::BraceRight,
                    TokenKind::From,
                ],
                Expected::Token(TokenKind::Comma),
                Scope::ImportElement,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    if current_kind(lexer) != TokenKind::BraceRight {
        errors.push(error_here(
            ParseErrorKind::NonClosedBrace,
            lexer,
            Expected::Token(TokenKind::BraceRight),
            Scope::ImportStatement,
        ));
    } else {
        lexer.next();
    }

    allocator.alloc_slice_fill_iter(elements)
}

fn parse_enum_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator EnumDefine<'input, 'allocator>> {
    if !matches!(current_kind(lexer), TokenKind::Export | TokenKind::Enum) {
        return None;
    }

    let anchor = lexer.cast_anchor();
    let exported = parse_export_modifier(lexer);
    if current_kind(lexer) != TokenKind::Enum {
        return None;
    }
    lexer.next();

    let Some(name) = parse_literal(lexer) else {
        errors.push(error_here(
            ParseErrorKind::InvalidEnumDefine,
            lexer,
            Expected::Literal,
            Scope::EnumDefine,
        ));
        return None;
    };

    let represented = current_kind(lexer) == TokenKind::Colon;
    let representation_type = if represented {
        lexer.next();

        let representation_type = parse_type_info(lexer, errors, allocator);
        if representation_type.is_none() {
            errors.push(error_here(
                ParseErrorKind::InvalidEnumDefine,
                lexer,
                Expected::TypeInfo,
                Scope::EnumDefine,
            ));
        }

        representation_type
    } else {
        None
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
            _ => {}
        }

        let variant_anchor = lexer.cast_anchor();
        let Some(name) = parse_literal(lexer) else {
            let error = recover_until(
                ParseErrorKind::InvalidEnumVariant,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                Expected::EnumVariant,
                Scope::EnumVariant,
                allocator,
            );
            errors.push(error);
            if current_kind(lexer) == TokenKind::BraceRight {
                break;
            }
            skip_list_separator(lexer);
            continue;
        };

        let value = if represented {
            if current_kind(lexer) != TokenKind::Equal {
                let error = recover_until(
                    ParseErrorKind::InvalidEnumVariant,
                    lexer,
                    &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                    Expected::Token(TokenKind::Equal),
                    Scope::EnumVariant,
                    allocator,
                );
                errors.push(error);
                None
            } else {
                lexer.next();

                let value = parse_expression(lexer, errors, allocator);
                if value.is_none() {
                    let error = recover_until(
                        ParseErrorKind::InvalidEnumVariant,
                        lexer,
                        &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                        Expected::Expression,
                        Scope::EnumVariant,
                        allocator,
                    );
                    errors.push(error);
                }
                value
            }
        } else {
            None
        };

        variants.push(EnumVariant {
            name,
            value,
            span: variant_anchor.elapsed(lexer),
        });

        if current_kind(lexer) == TokenKind::BraceRight {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidEnumVariant,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceRight],
                Expected::Token(TokenKind::Comma),
                Scope::EnumVariant,
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
        exported,
        name,
        representation_type,
        variants,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_type_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator TypeDefine<'input, 'allocator>> {
    if !matches!(current_kind(lexer), TokenKind::Export | TokenKind::Type) {
        return None;
    }

    let anchor = lexer.cast_anchor();
    let exported = parse_export_modifier(lexer);
    if current_kind(lexer) != TokenKind::Type {
        return None;
    }
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
        exported,
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

fn parse_interface_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator InterfaceDefine<'input, 'allocator>> {
    if !matches!(
        current_kind(lexer),
        TokenKind::Export | TokenKind::Interface
    ) {
        return None;
    }

    let anchor = lexer.cast_anchor();
    let exported = parse_export_modifier(lexer);
    if current_kind(lexer) != TokenKind::Interface {
        return None;
    }
    lexer.next();

    let Some(name) = parse_literal(lexer) else {
        errors.push(error_here(
            ParseErrorKind::InvalidInterfaceDefine,
            lexer,
            Expected::Literal,
            Scope::InterfaceDefine,
        ));
        return None;
    };

    let generic_parameters = parse_generic_parameters(lexer, errors, allocator);
    skip_line_feed(lexer);
    let where_clause = parse_where_clause(lexer, errors, allocator);
    let methods = parse_methods_block(
        lexer,
        errors,
        allocator,
        ParseErrorKind::InvalidInterfaceDefine,
        Scope::InterfaceDefine,
    )?;

    Some(allocator.alloc(InterfaceDefine {
        exported,
        name,
        generic_parameters,
        where_clause,
        methods,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_implements_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator ImplementsDefine<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Implements {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    let generic_parameters = parse_generic_parameters(lexer, errors, allocator);

    let Some(interface) = parse_type_info(lexer, errors, allocator) else {
        errors.push(error_here(
            ParseErrorKind::InvalidImplementsDefine,
            lexer,
            Expected::TypeInfo,
            Scope::ImplementsDefine,
        ));
        return None;
    };

    if current_kind(lexer) != TokenKind::For {
        let error = recover_until(
            ParseErrorKind::InvalidImplementsDefine,
            lexer,
            &[TokenKind::For, TokenKind::BraceLeft, TokenKind::LineFeed],
            Expected::Token(TokenKind::For),
            Scope::ImplementsDefine,
            allocator,
        );
        errors.push(error);
    }
    if current_kind(lexer) != TokenKind::For {
        return None;
    }
    lexer.next();

    let Some(target) = parse_type_info(lexer, errors, allocator) else {
        errors.push(error_here(
            ParseErrorKind::InvalidImplementsDefine,
            lexer,
            Expected::TypeInfo,
            Scope::ImplementsDefine,
        ));
        return None;
    };

    skip_line_feed(lexer);
    let where_clause = parse_where_clause(lexer, errors, allocator);
    let methods = parse_methods_block(
        lexer,
        errors,
        allocator,
        ParseErrorKind::InvalidImplementsDefine,
        Scope::ImplementsDefine,
    )?;

    Some(allocator.alloc(ImplementsDefine {
        generic_parameters,
        interface,
        target,
        where_clause,
        methods,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_methods_block<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
    invalid_kind: ParseErrorKind,
    scope: Scope,
) -> Option<&'allocator [&'allocator FunctionDefine<'input, 'allocator>]> {
    if current_kind(lexer) != TokenKind::BraceLeft {
        errors.push(error_here(
            invalid_kind,
            lexer,
            Expected::Token(TokenKind::BraceLeft),
            scope,
        ));
        return None;
    }
    lexer.next();
    skip_statement_separator(lexer);

    let mut methods = Vec::new_in(allocator);
    while !matches!(current_kind(lexer), TokenKind::BraceRight | TokenKind::None) {
        if matches!(current_kind(lexer), TokenKind::Inline | TokenKind::Opaque) {
            if let Some(method) = parse_function_define(lexer, errors, allocator) {
                methods.push(method);
            }
        } else {
            let error = recover_until(
                invalid_kind,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::Method,
                scope,
                allocator,
            );
            errors.push(error);
        }

        if current_kind(lexer) == TokenKind::BraceRight {
            break;
        }
        if !skip_statement_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidStatementSeparator,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Semicolon,
                    TokenKind::BraceRight,
                ],
                Expected::StatementSeparator,
                scope,
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
        scope,
    );

    Some(allocator.alloc_slice_fill_iter(methods))
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
    if !matches!(
        current_kind(lexer),
        TokenKind::Export | TokenKind::Declare | TokenKind::Let
    ) {
        return None;
    }

    let anchor = lexer.cast_anchor();
    let exported = parse_export_modifier(lexer);
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
        exported,
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
        if matches!(
            target,
            Expression::If(_) | Expression::Match(_) | Expression::Return(_) | Expression::Throw(_)
        ) {
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

fn parse_assert_statement<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator AssertStatement<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Assert {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();

    let Some(condition) = parse_expression(lexer, errors, allocator) else {
        let error = recover_until(
            ParseErrorKind::InvalidAssertStatement,
            lexer,
            &[
                TokenKind::LineFeed,
                TokenKind::Semicolon,
                TokenKind::BraceRight,
            ],
            Expected::Expression,
            Scope::AssertStatement,
            allocator,
        );
        errors.push(error);
        return None;
    };

    if current_kind(lexer) != TokenKind::Comma {
        let error = recover_until(
            ParseErrorKind::InvalidAssertStatement,
            lexer,
            &[
                TokenKind::Comma,
                TokenKind::LineFeed,
                TokenKind::Semicolon,
                TokenKind::BraceRight,
            ],
            Expected::Token(TokenKind::Comma),
            Scope::AssertStatement,
            allocator,
        );
        errors.push(error);
    }

    if current_kind(lexer) != TokenKind::Comma {
        return Some(allocator.alloc(AssertStatement {
            condition,
            message: None,
            span: anchor.elapsed(lexer),
        }));
    }
    lexer.next();

    let message_anchor = lexer.cast_anchor();
    let message_follows_separator = matches!(
        current_kind(lexer),
        TokenKind::LineFeed | TokenKind::Document
    );
    skip_line_feed(lexer);
    let message = parse_expression(lexer, errors, allocator);
    if message.is_none() {
        if message_follows_separator && is_statement_start(current_kind(lexer)) {
            lexer.back_to_anchor(message_anchor);
        }
        errors.push(error_here(
            ParseErrorKind::InvalidAssertStatement,
            lexer,
            Expected::Expression,
            Scope::AssertStatement,
        ));
    }

    Some(allocator.alloc(AssertStatement {
        condition,
        message,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_function_define<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator FunctionDefine<'input, 'allocator>> {
    if !matches!(
        current_kind(lexer),
        TokenKind::Export | TokenKind::Inline | TokenKind::Opaque
    ) {
        return None;
    }

    let anchor = lexer.cast_anchor();
    let exported = parse_export_modifier(lexer);

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

    let generic_parameters = parse_generic_parameters(lexer, errors, allocator);
    let arguments = parse_function_arguments(lexer, errors, allocator)?;
    let return_type = parse_function_return_type(lexer, errors, allocator);
    skip_line_feed(lexer);
    let where_clause = parse_where_clause(lexer, errors, allocator);
    let Some(body) = parse_block(lexer, errors, allocator) else {
        errors.push(error_here(
            ParseErrorKind::InvalidFunctionDefine,
            lexer,
            Expected::Block,
            Scope::FunctionDefine,
        ));
        return None;
    };

    Some(allocator.alloc(FunctionDefine {
        exported,
        attribute,
        name,
        generic_parameters,
        arguments,
        return_type,
        where_clause,
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
    let receiver = if current_kind(lexer) == TokenKind::This {
        let token = lexer.next().unwrap();
        let receiver = Some(Literal::new(token.text, token.span));
        if current_kind(lexer) != TokenKind::ParenthesisRight && !skip_list_separator(lexer) {
            errors.push(error_here(
                ParseErrorKind::InvalidFunctionArgument,
                lexer,
                Expected::Token(TokenKind::Comma),
                Scope::FunctionArguments,
            ));
        }
        receiver
    } else {
        None
    };

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
        receiver,
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

fn parse_generic_parameters<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator GenericParameters<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::LessThan {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    skip_line_feed(lexer);
    let mut parameters = Vec::new_in(allocator);

    while !matches!(
        current_kind(lexer),
        TokenKind::GreaterThan | TokenKind::None
    ) {
        let parameter_anchor = lexer.cast_anchor();
        let Some(name) = parse_literal(lexer) else {
            let error = recover_until(
                ParseErrorKind::InvalidGenericParameter,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::GreaterThan,
                ],
                Expected::GenericParameter,
                Scope::GenericParameters,
                allocator,
            );
            errors.push(error);
            if current_kind(lexer) == TokenKind::GreaterThan {
                break;
            }
            skip_list_separator(lexer);
            continue;
        };

        let bounds = if current_kind(lexer) == TokenKind::Colon {
            lexer.next();
            parse_type_bounds(
                lexer,
                errors,
                allocator,
                Scope::GenericParameters,
                ParseErrorKind::InvalidGenericParameter,
            )
        } else {
            allocator.alloc_slice_copy(&[])
        };
        parameters.push(GenericParameter {
            name,
            bounds,
            span: parameter_anchor.elapsed(lexer),
        });

        if current_kind(lexer) == TokenKind::GreaterThan {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidGenericParameter,
                lexer,
                &[
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::GreaterThan,
                ],
                Expected::Token(TokenKind::Comma),
                Scope::GenericParameters,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    if parameters.is_empty() {
        errors.push(error_here(
            ParseErrorKind::InvalidGenericParameter,
            lexer,
            Expected::GenericParameter,
            Scope::GenericParameters,
        ));
    }
    close_delimiter(
        lexer,
        errors,
        allocator,
        TokenKind::GreaterThan,
        ParseErrorKind::NonClosedTypeParameter,
        Scope::GenericParameters,
    );

    let parameters = allocator.alloc_slice_fill_iter(parameters);
    Some(allocator.alloc(GenericParameters {
        parameters,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_where_clause<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator WhereClause<'input, 'allocator>> {
    if current_kind(lexer) != TokenKind::Where {
        return None;
    }

    let anchor = lexer.cast_anchor();
    lexer.next();
    skip_line_feed(lexer);
    let mut predicates = Vec::new_in(allocator);

    while !matches!(current_kind(lexer), TokenKind::BraceLeft | TokenKind::None)
        && !is_statement_start(current_kind(lexer))
    {
        let predicate_anchor = lexer.cast_anchor();
        let Some(ty) = parse_type_info(lexer, errors, allocator) else {
            let error = recover_until(
                ParseErrorKind::InvalidWhereClause,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceLeft],
                Expected::WherePredicate,
                Scope::WhereClause,
                allocator,
            );
            errors.push(error);
            if current_kind(lexer) == TokenKind::BraceLeft {
                break;
            }
            skip_list_separator(lexer);
            continue;
        };

        if current_kind(lexer) != TokenKind::Colon {
            let error = recover_until(
                ParseErrorKind::InvalidWhereClause,
                lexer,
                &[
                    TokenKind::Colon,
                    TokenKind::LineFeed,
                    TokenKind::Comma,
                    TokenKind::BraceLeft,
                ],
                Expected::Token(TokenKind::Colon),
                Scope::WhereClause,
                allocator,
            );
            errors.push(error);
        }
        let bounds = if current_kind(lexer) == TokenKind::Colon {
            lexer.next();
            parse_type_bounds(
                lexer,
                errors,
                allocator,
                Scope::WhereClause,
                ParseErrorKind::InvalidWhereClause,
            )
        } else {
            allocator.alloc_slice_copy(&[])
        };
        predicates.push(WherePredicate {
            ty,
            bounds,
            span: predicate_anchor.elapsed(lexer),
        });

        if current_kind(lexer) == TokenKind::BraceLeft {
            break;
        }
        if !skip_list_separator(lexer) {
            let error = recover_until(
                ParseErrorKind::InvalidWhereClause,
                lexer,
                &[TokenKind::LineFeed, TokenKind::Comma, TokenKind::BraceLeft],
                Expected::Token(TokenKind::Comma),
                Scope::WhereClause,
                allocator,
            );
            errors.push(error);
            skip_list_separator(lexer);
        }
    }

    if predicates.is_empty() {
        errors.push(error_here(
            ParseErrorKind::InvalidWhereClause,
            lexer,
            Expected::WherePredicate,
            Scope::WhereClause,
        ));
    }
    let predicates = allocator.alloc_slice_fill_iter(predicates);
    Some(allocator.alloc(WhereClause {
        predicates,
        span: anchor.elapsed(lexer),
    }))
}

fn parse_type_bounds<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
    scope: Scope,
    error_kind: ParseErrorKind,
) -> &'allocator [&'allocator TypeInfo<'input, 'allocator>] {
    let mut bounds = Vec::new_in(allocator);
    loop {
        let Some(bound) = parse_type_info(lexer, errors, allocator) else {
            errors.push(error_here(error_kind, lexer, Expected::TypeInfo, scope));
            break;
        };
        bounds.push(bound);

        if current_kind(lexer) != TokenKind::Plus {
            break;
        }
        lexer.next();
        skip_line_feed(lexer);
    }
    allocator.alloc_slice_fill_iter(bounds)
}

pub(super) fn parse_type_info<'input, 'allocator>(
    lexer: &mut Lexer<'input>,
    errors: &mut ParseErrors<'input, 'allocator>,
    allocator: &'allocator Bump,
) -> Option<&'allocator TypeInfo<'input, 'allocator>> {
    let anchor = lexer.cast_anchor();
    let name = parse_literal(lexer)?;

    let mut parameters = Vec::new_in(allocator);
    if current_kind(lexer) == TokenKind::LessThan {
        lexer.next();
        skip_line_feed(lexer);

        while !matches!(
            current_kind(lexer),
            TokenKind::GreaterThan | TokenKind::None
        ) {
            if let Some(parameter) = parse_type_info(lexer, errors, allocator) {
                parameters.push(parameter);
            } else {
                let error = recover_until(
                    ParseErrorKind::InvalidTypeInfo,
                    lexer,
                    &[
                        TokenKind::LineFeed,
                        TokenKind::Comma,
                        TokenKind::GreaterThan,
                    ],
                    Expected::TypeInfo,
                    Scope::TypeInfo,
                    allocator,
                );
                errors.push(error);
            }

            if current_kind(lexer) == TokenKind::GreaterThan {
                break;
            }
            if !skip_list_separator(lexer) {
                let error = recover_until(
                    ParseErrorKind::InvalidTypeInfo,
                    lexer,
                    &[
                        TokenKind::LineFeed,
                        TokenKind::Comma,
                        TokenKind::GreaterThan,
                    ],
                    Expected::Token(TokenKind::Comma),
                    Scope::TypeInfo,
                    allocator,
                );
                errors.push(error);
                skip_list_separator(lexer);
            }
        }

        if parameters.is_empty() {
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
    }

    let optional = if current_kind(lexer) == TokenKind::Question {
        lexer.next();
        true
    } else {
        false
    };

    if optional && current_kind(lexer) == TokenKind::Question {
        let error = recover_until(
            ParseErrorKind::InvalidTypeInfo,
            lexer,
            &[
                TokenKind::GreaterThan,
                TokenKind::Equal,
                TokenKind::LineFeed,
                TokenKind::Comma,
                TokenKind::BraceRight,
                TokenKind::ParenthesisRight,
            ],
            Expected::TypeInfo,
            Scope::TypeInfo,
            allocator,
        );
        errors.push(error);
    }

    let parameters = allocator.alloc_slice_fill_iter(parameters);
    Some(allocator.alloc(TypeInfo {
        name,
        parameters,
        optional,
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

fn parse_export_modifier(lexer: &mut Lexer<'_>) -> bool {
    if current_kind(lexer) != TokenKind::Export {
        return false;
    }

    lexer.next();
    true
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
        ast::{
            AccessOperator, BinaryOperator, Expression, ImportKind, MutationPolicyKind, Pattern,
            Statement, TypedefValue, Value,
        },
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
    fn parses_interfaces_generic_implements_where_clauses_and_type_arguments() {
        let source = r#"
export interface Container<T: Comparable> where This: Sized {
    inline function contains<U>(this, value: T) -> Bool
    where U: TestInterface<T> {
        return true
    }
}

implements<T> Container<T> for Set<T>
where T: Comparable {
    opaque function contains(this, value: T) -> Bool {
        return true
    }
}

inline function test<T, U>() -> U
where T: TestInterface<U> {
    return fallback
}

let value = test:<Test, _>()
let mapped = values.map:<String>(convert:<String>)
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 5);

        let Statement::InterfaceDefine(interface) = &ast.statements[0] else {
            panic!("expected interface definition");
        };
        assert!(interface.exported);
        assert_eq!(interface.name.value, "Container");
        let generic_parameters = interface.generic_parameters.unwrap();
        assert_eq!(generic_parameters.parameters.len(), 1);
        assert_eq!(generic_parameters.parameters[0].name.value, "T");
        assert_eq!(
            generic_parameters.parameters[0].bounds[0].name.value,
            "Comparable"
        );
        assert_eq!(
            interface.where_clause.unwrap().predicates[0].ty.name.value,
            "This"
        );
        assert_eq!(interface.methods.len(), 1);
        let method = interface.methods[0];
        assert_eq!(method.arguments.receiver.as_ref().unwrap().value, "this");
        assert_eq!(
            method.generic_parameters.unwrap().parameters[0].name.value,
            "U"
        );
        assert_eq!(method.where_clause.unwrap().predicates.len(), 1);

        let Statement::ImplementsDefine(implements) = &ast.statements[1] else {
            panic!("expected implements definition");
        };
        assert_eq!(implements.interface.name.value, "Container");
        assert_eq!(implements.interface.parameters[0].name.value, "T");
        assert_eq!(implements.target.name.value, "Set");
        assert_eq!(implements.target.parameters[0].name.value, "T");
        assert_eq!(implements.methods.len(), 1);

        let Statement::FunctionDefine(function) = &ast.statements[2] else {
            panic!("expected generic function");
        };
        assert_eq!(function.generic_parameters.unwrap().parameters.len(), 2);
        assert_eq!(function.return_type.unwrap().name.value, "U");
        assert_eq!(function.where_clause.unwrap().predicates.len(), 1);

        let Statement::LetStatement(value) = &ast.statements[3] else {
            panic!("expected explicit generic call");
        };
        let Expression::Primary(value) = value.value.unwrap() else {
            panic!("expected primary call");
        };
        let Value::Literal(value) = &value.value else {
            panic!("expected literal callee");
        };
        let type_arguments = value.type_arguments.unwrap();
        assert_eq!(type_arguments.arguments.len(), 2);
        assert!(matches!(
            type_arguments.arguments[0],
            crate::ast::ExplicitTypeArgument::Type(_)
        ));
        assert!(matches!(
            type_arguments.arguments[1],
            crate::ast::ExplicitTypeArgument::Infer(_)
        ));

        let Statement::LetStatement(mapped) = &ast.statements[4] else {
            panic!("expected generic method call");
        };
        let Expression::Primary(mapped) = mapped.value.unwrap() else {
            panic!("expected method primary");
        };
        assert_eq!(mapped.accesses[0].member.value, "map");
        assert_eq!(
            mapped.accesses[0].type_arguments.unwrap().arguments.len(),
            1
        );
        let call = mapped.accesses[0].call.unwrap();
        let Expression::Primary(converter) = &call.arguments[0] else {
            panic!("expected generic function value");
        };
        let Value::Literal(converter) = &converter.value else {
            panic!("expected converter literal");
        };
        assert!(converter.call.is_none());
        assert_eq!(converter.type_arguments.unwrap().arguments.len(), 1);
    }

    #[test]
    fn parses_multiple_nested_type_parameters() {
        let source = "let value: Result<Map<String, List<Int?>>, Error?>?";
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        let Statement::LetStatement(value) = ast.statements[0] else {
            panic!("expected value binding");
        };
        let result = value.type_info.unwrap();
        assert!(result.optional);
        assert_eq!(result.parameters.len(), 2);
        let map = result.parameters[0];
        assert_eq!(map.name.value, "Map");
        assert_eq!(map.parameters.len(), 2);
        assert_eq!(map.parameters[0].name.value, "String");
        assert!(map.parameters[1].parameters[0].optional);
        assert!(result.parameters[1].optional);
    }

    #[test]
    fn generic_syntax_errors_recover_to_following_statements() {
        let source = r#"
interface Broken<T: Comparable +> {}
let broken = test:<>();
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.iter().any(|error| matches!(
            error.kind,
            ParseErrorKind::InvalidGenericParameter | ParseErrorKind::InvalidTypeArgument
        )));
        assert_eq!(ast.statements.len(), 3, "parse errors: {errors:#?}");
        assert!(matches!(ast.statements[2], Statement::LetStatement(_)));
    }

    #[test]
    fn missing_generic_function_body_preserves_the_following_statement() {
        let source = r#"
inline function broken<T>() -> T
where T: Comparable
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.iter().any(|error| {
            error.kind == ParseErrorKind::InvalidFunctionDefine
                && error.expected == crate::error::Expected::Block
        }));
        assert_eq!(ast.statements.len(), 1, "parse errors: {errors:#?}");
        assert!(matches!(ast.statements[0], Statement::LetStatement(_)));
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
        assert!(profile.representation_type.is_none());
        assert_eq!(profile.variants.len(), 2);
        assert_eq!(profile.variants[0].name.value, "Desktop");
        assert!(profile.variants[0].value.is_none());
        assert_eq!(profile.variants[1].name.value, "Laptop");
        assert!(profile.variants[1].value.is_none());
        assert!(matches!(ast.statements[1], Statement::LetStatement(_)));
    }

    #[test]
    fn parses_enum_with_a_representation_type_and_values() {
        let source = r#"
enum Desktop: string {
    KDE = "kde plasma"
    GNOME = "gnome"
}
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 1);

        let Statement::EnumDefine(desktop) = &ast.statements[0] else {
            panic!("expected represented enum");
        };
        assert_eq!(desktop.representation_type.unwrap().name.value, "string");

        let [kde, gnome] = desktop.variants else {
            panic!("expected two represented variants");
        };
        assert_eq!(kde.name.value, "KDE");
        let Expression::Primary(kde_value) = kde.value.unwrap() else {
            panic!("expected KDE primary value");
        };
        let Value::String(kde_value) = &kde_value.value else {
            panic!("expected KDE string value");
        };
        assert_eq!(kde_value.value, "\"kde plasma\"");

        assert_eq!(gnome.name.value, "GNOME");
        let Expression::Primary(gnome_value) = gnome.value.unwrap() else {
            panic!("expected GNOME primary value");
        };
        let Value::String(gnome_value) = &gnome_value.value else {
            panic!("expected GNOME string value");
        };
        assert_eq!(gnome_value.value, "\"gnome\"");
    }

    #[test]
    fn parses_optional_types_safe_access_elvis_some_and_match() {
        let source = r#"
let parsed: Desktop? = Desktop::from_repr(input)
let name: string = parsed?.profile?.name ?: "unknown"
let wrapped: List<string?>? = some([])
let selected: string = match parsed {
    some(Desktop::KDE) => "kde",
    some(value) => value.to_repr()
    null => "none"
}
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 4);

        let Statement::LetStatement(parsed) = &ast.statements[0] else {
            panic!("expected parsed binding");
        };
        assert!(parsed.type_info.unwrap().optional);
        let Expression::Primary(from_repr) = parsed.value.unwrap() else {
            panic!("expected from_repr call");
        };
        assert_eq!(from_repr.accesses.len(), 1);
        assert_eq!(
            from_repr.accesses[0].operator.value,
            AccessOperator::DoubleColon
        );
        assert_eq!(from_repr.accesses[0].member.value, "from_repr");

        let Statement::LetStatement(name) = &ast.statements[1] else {
            panic!("expected name binding");
        };
        assert!(!name.type_info.unwrap().optional);
        let Expression::Elvis(name) = name.value.unwrap() else {
            panic!("expected Elvis expression");
        };
        let Expression::Primary(safe_chain) = name.optional else {
            panic!("expected safe access chain");
        };
        assert_eq!(safe_chain.accesses.len(), 2);
        assert!(
            safe_chain
                .accesses
                .iter()
                .all(|access| access.operator.value == AccessOperator::SafeDot)
        );
        let Expression::Primary(fallback) = name.fallback else {
            panic!("expected string fallback");
        };
        assert!(matches!(fallback.value, Value::String(_)));

        let Statement::LetStatement(wrapped) = &ast.statements[2] else {
            panic!("expected wrapped binding");
        };
        let wrapped_type = wrapped.type_info.unwrap();
        assert!(wrapped_type.optional);
        assert!(wrapped_type.parameters[0].optional);
        let Expression::Primary(wrapped) = wrapped.value.unwrap() else {
            panic!("expected some value");
        };
        let Value::Some(wrapped) = wrapped.value else {
            panic!("expected some constructor");
        };
        assert!(wrapped.value.is_some());

        let Statement::LetStatement(selected) = &ast.statements[3] else {
            panic!("expected selected binding");
        };
        let Expression::Match(selected) = selected.value.unwrap() else {
            panic!("expected match expression");
        };
        let [kde, value, null] = selected.arms else {
            panic!("expected three match arms");
        };
        let Pattern::Some(kde) = &kde.pattern else {
            panic!("expected some enum pattern");
        };
        let Pattern::EnumVariant(kde) = kde.pattern else {
            panic!("expected KDE enum pattern");
        };
        assert_eq!(kde.enum_name.value, "Desktop");
        assert_eq!(kde.variant.value, "KDE");

        let Pattern::Some(value) = &value.pattern else {
            panic!("expected some binding pattern");
        };
        let Pattern::Binding(value) = value.pattern else {
            panic!("expected value binding");
        };
        assert_eq!(value.value, "value");
        assert!(matches!(null.pattern, Pattern::Null(_)));
    }

    #[test]
    fn elvis_operator_is_right_associative() {
        let source = "let value = first ?: second ?: third";
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        let Statement::LetStatement(value) = &ast.statements[0] else {
            panic!("expected value binding");
        };
        let Expression::Elvis(value) = value.value.unwrap() else {
            panic!("expected outer Elvis expression");
        };
        assert!(matches!(value.fallback, Expression::Elvis(_)));
    }

    #[test]
    fn optional_and_match_errors_recover_to_following_constructs() {
        let source = r#"
let broken: string?? = null
let selected = match broken {
    some(value) value
    null => "none"
}
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
                .any(|error| error.kind == ParseErrorKind::InvalidTypeInfo)
        );
        assert!(
            errors
                .iter()
                .any(|error| error.kind == ParseErrorKind::InvalidMatchArm)
        );
        assert_eq!(ast.statements.len(), 3);

        let Statement::LetStatement(selected) = &ast.statements[1] else {
            panic!("expected recovered match binding");
        };
        let Expression::Match(selected) = selected.value.unwrap() else {
            panic!("expected recovered match expression");
        };
        assert_eq!(selected.arms.len(), 1);
        assert!(matches!(selected.arms[0].pattern, Pattern::Null(_)));
        assert!(matches!(ast.statements[2], Statement::LetStatement(_)));
    }

    #[test]
    fn represented_enum_recovers_from_a_missing_variant_value_separator() {
        let source = r#"
enum Desktop: string {
    KDE "kde plasma"
    GNOME = "gnome"
}
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert_eq!(errors.len(), 1, "parse errors: {errors:#?}");
        assert_eq!(errors[0].kind, ParseErrorKind::InvalidEnumVariant);
        assert_eq!(ast.statements.len(), 2);

        let Statement::EnumDefine(desktop) = &ast.statements[0] else {
            panic!("expected recovered enum");
        };
        assert_eq!(desktop.variants.len(), 2);
        assert!(desktop.variants[0].value.is_none());
        assert!(desktop.variants[1].value.is_some());
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
            complete.type_info.unwrap().parameters[0].name.value,
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
        assert!(matches!(first.kind, ImportKind::SideEffect));
        assert_eq!(first.path.value, r#""./modules/programs.lnix""#);
        assert_eq!(&source[first.path.span()], first.path.value);

        let Statement::ImportStatement(second) = &ast.statements[1] else {
            panic!("expected second import statement");
        };
        assert!(matches!(second.kind, ImportKind::SideEffect));
        assert_eq!(second.path.value, "'catalog/firefox.lnix'");
        assert_eq!(&source[second.path.span()], second.path.value);
        assert!(matches!(ast.statements[2], Statement::LetStatement(_)));
    }

    #[test]
    fn parses_typescript_style_imports_and_exported_declarations() {
        let source = r#"
import "./common.lnix"
import { Programs, helper as desktop_helper } from "./programs.lnix"
import * as desktop from "./desktop.lnix"
export enum Profile { Desktop, Laptop }
export type Programs { enabled: Bool }
export declare let programs: Programs
export let public_value = true
export inline function identity(value: String) -> String {
    return value
}
let local_value = false
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 9);

        let Statement::ImportStatement(side_effect) = &ast.statements[0] else {
            panic!("expected side-effect import");
        };
        assert!(matches!(side_effect.kind, ImportKind::SideEffect));

        let Statement::ImportStatement(named) = &ast.statements[1] else {
            panic!("expected named import");
        };
        let ImportKind::Named(elements) = named.kind else {
            panic!("expected named import elements");
        };
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].name.value, "Programs");
        assert!(elements[0].alias.is_none());
        assert_eq!(elements[1].name.value, "helper");
        assert_eq!(elements[1].alias.as_ref().unwrap().value, "desktop_helper");
        assert_eq!(named.path.value, r#""./programs.lnix""#);

        let Statement::ImportStatement(namespace) = &ast.statements[2] else {
            panic!("expected namespace import");
        };
        let ImportKind::Namespace { alias } = &namespace.kind else {
            panic!("expected namespace alias");
        };
        assert_eq!(alias.value, "desktop");
        assert_eq!(namespace.path.value, r#""./desktop.lnix""#);

        let Statement::EnumDefine(profile) = &ast.statements[3] else {
            panic!("expected exported enum");
        };
        assert!(profile.exported);

        let Statement::TypeDefine(programs_type) = &ast.statements[4] else {
            panic!("expected exported type");
        };
        assert!(programs_type.exported);

        let Statement::LetStatement(programs) = &ast.statements[5] else {
            panic!("expected exported declaration");
        };
        assert!(programs.exported);
        assert!(programs.declare);

        let Statement::LetStatement(public_value) = &ast.statements[6] else {
            panic!("expected exported binding");
        };
        assert!(public_value.exported);
        assert!(!public_value.declare);

        let Statement::FunctionDefine(identity) = &ast.statements[7] else {
            panic!("expected exported function");
        };
        assert!(identity.exported);

        let Statement::LetStatement(local_value) = &ast.statements[8] else {
            panic!("expected local binding");
        };
        assert!(!local_value.exported);
    }

    #[test]
    fn parses_throw_expressions_and_assert_statements() {
        let source = r#"
let required = optional ?: throw "value is required"
let selected = match optional {
    null => throw "missing value",
    some(value) => value
}
assert required != "", "required must not be empty"
assert true,
    "multiline message"
throw "top-level failure: " + required
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        assert_eq!(ast.statements.len(), 5);

        let Statement::LetStatement(required) = &ast.statements[0] else {
            panic!("expected required binding");
        };
        let Expression::Elvis(required) = required.value.unwrap() else {
            panic!("expected Elvis expression");
        };
        let Expression::Throw(missing_required) = required.fallback else {
            panic!("expected throw fallback");
        };
        assert!(missing_required.message.is_some());

        let Statement::LetStatement(selected) = &ast.statements[1] else {
            panic!("expected selected binding");
        };
        let Expression::Match(selected) = selected.value.unwrap() else {
            panic!("expected match expression");
        };
        assert!(matches!(selected.arms[0].value, Expression::Throw(_)));

        let Statement::AssertStatement(non_empty) = &ast.statements[2] else {
            panic!("expected assertion");
        };
        assert!(matches!(non_empty.condition, Expression::Binary(_)));
        assert!(non_empty.message.is_some());

        let Statement::AssertStatement(multiline) = &ast.statements[3] else {
            panic!("expected multiline assertion");
        };
        assert!(multiline.message.is_some());

        let Statement::Expression(Expression::Throw(top_level)) = &ast.statements[4] else {
            panic!("expected top-level throw expression");
        };
        assert!(matches!(top_level.message, Some(Expression::Binary(_))));
    }

    #[test]
    fn malformed_throw_and_assert_recover_to_following_statements() {
        let source = r#"
let broken = throw
let recovered = true
assert recovered
let after_missing_comma = false
assert recovered,
let after_missing_message = true
throw
let after_throw = false
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert_eq!(errors.len(), 4, "parse errors: {errors:#?}");
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.kind == ParseErrorKind::InvalidThrowExpression)
                .count(),
            2
        );
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.kind == ParseErrorKind::InvalidAssertStatement)
                .count(),
            2
        );
        assert_eq!(ast.statements.len(), 8);

        let Statement::LetStatement(broken) = &ast.statements[0] else {
            panic!("expected broken binding");
        };
        let Expression::Throw(broken_throw) = broken.value.unwrap() else {
            panic!("expected recovered throw expression");
        };
        assert!(broken_throw.message.is_none());
        assert!(matches!(ast.statements[1], Statement::LetStatement(_)));

        let Statement::AssertStatement(missing_comma) = &ast.statements[2] else {
            panic!("expected assertion recovered from a missing comma");
        };
        assert!(missing_comma.message.is_none());
        assert!(matches!(ast.statements[3], Statement::LetStatement(_)));

        let Statement::AssertStatement(missing_message) = &ast.statements[4] else {
            panic!("expected assertion recovered from a missing message");
        };
        assert!(missing_message.message.is_none());
        assert!(matches!(ast.statements[5], Statement::LetStatement(_)));

        assert!(matches!(
            ast.statements[6],
            Statement::Expression(Expression::Throw(_))
        ));
        assert!(matches!(ast.statements[7], Statement::LetStatement(_)));
    }

    #[test]
    fn malformed_imports_and_exports_recover_to_following_declarations() {
        let source = r#"
import {
    Good as,
    Other
} from "./module.lnix"
import { MissingBrace
from "./second.lnix"
export use [invalid]
let recovered = true
"#;
        let allocator = Bump::new();
        let mut lexer = Lexer::new(source);
        let mut errors = Vec::new_in(&allocator);

        let ast = parse_source(&mut lexer, &mut errors, &allocator);

        assert_eq!(errors.len(), 3, "parse errors: {errors:#?}");
        assert!(
            errors
                .iter()
                .any(|error| error.kind == ParseErrorKind::InvalidImportElement)
        );
        assert!(
            errors
                .iter()
                .any(|error| error.kind == ParseErrorKind::NonClosedBrace)
        );
        assert!(
            errors
                .iter()
                .any(|error| error.kind == ParseErrorKind::InvalidExportStatement)
        );
        assert_eq!(ast.statements.len(), 3);

        let Statement::ImportStatement(first) = &ast.statements[0] else {
            panic!("expected recovered named import");
        };
        let ImportKind::Named(elements) = first.kind else {
            panic!("expected named imports");
        };
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].name.value, "Good");
        assert!(elements[0].alias.is_none());
        assert_eq!(elements[1].name.value, "Other");

        let Statement::ImportStatement(second) = &ast.statements[1] else {
            panic!("expected import recovered from a missing brace");
        };
        let ImportKind::Named(elements) = second.kind else {
            panic!("expected named imports");
        };
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].name.value, "MissingBrace");
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
