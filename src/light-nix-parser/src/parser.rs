use allocator_api2::vec::Vec;
use bumpalo::Bump;

use crate::{
    error::ParseError,
    lexer::{Lexer, TokenKind},
};

mod expression;
pub mod statement;

pub use statement::parse_source;

pub type ParseErrors<'input, 'allocator> = Vec<ParseError<'input, 'allocator>, &'allocator Bump>;

pub(crate) fn current_kind(lexer: &mut Lexer<'_>) -> TokenKind {
    lexer
        .current()
        .map(|token| token.kind)
        .unwrap_or(TokenKind::None)
}

pub(crate) fn is_statement_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Inputs
            | TokenKind::Enum
            | TokenKind::Type
            | TokenKind::Use
            | TokenKind::Host
            | TokenKind::Let
            | TokenKind::Inline
            | TokenKind::Opaque
            | TokenKind::If
            | TokenKind::Return
    )
}

pub(crate) fn skip_line_feed(lexer: &mut Lexer<'_>) {
    while matches!(
        current_kind(lexer),
        TokenKind::LineFeed | TokenKind::Document
    ) {
        lexer.next();
    }
}

pub(crate) fn skip_statement_separator(lexer: &mut Lexer<'_>) -> bool {
    let mut skipped = false;

    while matches!(
        current_kind(lexer),
        TokenKind::LineFeed | TokenKind::Semicolon | TokenKind::Document
    ) {
        skipped = true;
        lexer.next();
    }

    skipped
}

pub(crate) fn skip_list_separator(lexer: &mut Lexer<'_>) -> bool {
    let mut skipped = false;

    while matches!(
        current_kind(lexer),
        TokenKind::LineFeed | TokenKind::Comma | TokenKind::Document
    ) {
        skipped = true;
        lexer.next();
    }

    skipped
}
