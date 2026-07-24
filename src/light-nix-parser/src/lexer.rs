use std::ops::Range;

use extension_fn::extension_fn;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// import
    Import,
    /// export
    Export,
    /// from
    From,
    /// as
    As,
    /// is
    Is,
    /// enum
    Enum,
    /// type
    Type,
    /// interface
    Interface,
    /// implements
    Implements,
    /// where
    Where,
    /// for
    For,
    /// this
    This,
    /// use
    Use,
    /// tunable
    Tunable,
    /// readonly
    Readonly,
    /// declare
    Declare,
    /// cost
    Cost,
    /// let
    Let,
    /// assert
    Assert,
    /// function
    Function,
    /// inline
    Inline,
    /// opaque
    Opaque,
    /// if
    If,
    /// else
    Else,
    /// return
    Return,
    /// throw
    Throw,
    /// match
    Match,
    /// some
    Some,
    /// or
    Or,
    /// and
    And,
    /// true
    True,
    /// false
    False,
    /// null
    Null,
    /// ==
    DoubleEquals,
    /// !=
    NotEquals,
    /// <
    LessThan,
    /// >
    GreaterThan,
    /// <=
    LessThanOrEqual,
    /// >=
    GreaterThanOrEqual,
    /// +
    Plus,
    /// -
    Minus,
    /// *
    Asterisk,
    /// /
    Slash,
    /// .
    Dot,
    /// ->
    ThinArrow,
    /// =>
    FatArrow,
    /// ?.
    SafeDot,
    /// ?:
    Elvis,
    /// ::
    DoubleColon,
    /// :
    Colon,
    /// =
    Equal,
    /// ,
    Comma,
    /// ;
    Semicolon,
    /// (
    ParenthesisLeft,
    /// )
    ParenthesisRight,
    /// {
    BraceLeft,
    /// }
    BraceRight,
    /// [
    BracketLeft,
    /// ]
    BracketRight,
    /// |
    VerticalLine,
    /// ?
    Question,
    /// e.g. 42, 6.3, 5E+2
    NumericLiteral,
    /// e.g. literal
    Literal,
    /// e.g. "literal"
    StringLiteral,
    /// e.g. '\n'
    LineFeed,
    /// ' '
    Whitespace,
    /// // comment or /* comment */
    Comment,
    /// /// document
    Document,
    UnexpectedCharacter,
    None,
}

static TOKENIZERS: &[Tokenizer] = &[
    // Keywords
    Tokenizer::Keyword(TokenKind::Import, "import"),
    Tokenizer::Keyword(TokenKind::Export, "export"),
    Tokenizer::Keyword(TokenKind::From, "from"),
    Tokenizer::Keyword(TokenKind::As, "as"),
    Tokenizer::Keyword(TokenKind::Is, "is"),
    Tokenizer::Keyword(TokenKind::Enum, "enum"),
    Tokenizer::Keyword(TokenKind::Type, "type"),
    Tokenizer::Keyword(TokenKind::Interface, "interface"),
    Tokenizer::Keyword(TokenKind::Implements, "implements"),
    Tokenizer::Keyword(TokenKind::Where, "where"),
    Tokenizer::Keyword(TokenKind::For, "for"),
    Tokenizer::Keyword(TokenKind::This, "this"),
    Tokenizer::Keyword(TokenKind::Use, "use"),
    Tokenizer::Keyword(TokenKind::Tunable, "tunable"),
    Tokenizer::Keyword(TokenKind::Readonly, "readonly"),
    Tokenizer::Keyword(TokenKind::Declare, "declare"),
    Tokenizer::Keyword(TokenKind::Cost, "cost"),
    Tokenizer::Keyword(TokenKind::Let, "let"),
    Tokenizer::Keyword(TokenKind::Assert, "assert"),
    Tokenizer::Keyword(TokenKind::Function, "function"),
    Tokenizer::Keyword(TokenKind::Inline, "inline"),
    Tokenizer::Keyword(TokenKind::Opaque, "opaque"),
    Tokenizer::Keyword(TokenKind::If, "if"),
    Tokenizer::Keyword(TokenKind::Else, "else"),
    Tokenizer::Keyword(TokenKind::Return, "return"),
    Tokenizer::Keyword(TokenKind::Throw, "throw"),
    Tokenizer::Keyword(TokenKind::Match, "match"),
    Tokenizer::Keyword(TokenKind::Some, "some"),
    Tokenizer::Keyword(TokenKind::Or, "or"),
    Tokenizer::Keyword(TokenKind::And, "and"),
    Tokenizer::Keyword(TokenKind::True, "true"),
    Tokenizer::Keyword(TokenKind::False, "false"),
    Tokenizer::Keyword(TokenKind::Null, "null"),
    // Multi-character operators
    Tokenizer::Keyword(TokenKind::DoubleEquals, "=="),
    Tokenizer::Keyword(TokenKind::NotEquals, "!="),
    Tokenizer::Keyword(TokenKind::LessThanOrEqual, "<="),
    Tokenizer::Keyword(TokenKind::GreaterThanOrEqual, ">="),
    Tokenizer::Keyword(TokenKind::ThinArrow, "->"),
    Tokenizer::Keyword(TokenKind::FatArrow, "=>"),
    Tokenizer::Keyword(TokenKind::SafeDot, "?."),
    Tokenizer::Keyword(TokenKind::Elvis, "?:"),
    Tokenizer::Keyword(TokenKind::DoubleColon, "::"),
    // Single-character operators
    Tokenizer::Keyword(TokenKind::LessThan, "<"),
    Tokenizer::Keyword(TokenKind::GreaterThan, ">"),
    Tokenizer::Keyword(TokenKind::Plus, "+"),
    Tokenizer::Keyword(TokenKind::Minus, "-"),
    Tokenizer::Keyword(TokenKind::Asterisk, "*"),
    Tokenizer::Keyword(TokenKind::Slash, "/"),
    Tokenizer::Keyword(TokenKind::Dot, "."),
    Tokenizer::Keyword(TokenKind::Colon, ":"),
    Tokenizer::Keyword(TokenKind::Equal, "="),
    Tokenizer::Keyword(TokenKind::Comma, ","),
    Tokenizer::Keyword(TokenKind::Semicolon, ";"),
    Tokenizer::Keyword(TokenKind::VerticalLine, "|"),
    Tokenizer::Keyword(TokenKind::Question, "?"),
    // Delimiters
    Tokenizer::Keyword(TokenKind::ParenthesisLeft, "("),
    Tokenizer::Keyword(TokenKind::ParenthesisRight, ")"),
    Tokenizer::Keyword(TokenKind::BraceLeft, "{"),
    Tokenizer::Keyword(TokenKind::BraceRight, "}"),
    Tokenizer::Keyword(TokenKind::BracketLeft, "["),
    Tokenizer::Keyword(TokenKind::BracketRight, "]"),
    // Numeric literal
    //
    // Examples:
    // 123
    // -123
    // +123
    // 1.25
    // -6.3
    // 5e2
    // +5E+2
    // -5e-2
    // 1_000.25
    Tokenizer::Regex(
        TokenKind::NumericLiteral,
        r"[\d_]+(?:\.[\d_]+)?(?:[eE][+-]?[\d_]+)?",
    ),
    // Identifier
    Tokenizer::Regex(TokenKind::Literal, r"\w+"),
    // String literals
    Tokenizer::Regex(TokenKind::StringLiteral, r#""(?:[^"\\]|\\.)*""#),
    Tokenizer::Regex(TokenKind::StringLiteral, r"'(?:[^'\\]|\\.)*'"),
    // Document comment
    Tokenizer::Regex(TokenKind::Document, r"///[^\n\r]*(?:\r\n|\n|\r|$)"),
    // Comments
    Tokenizer::Regex(TokenKind::Comment, r"//[^\n\r]*"),
    Tokenizer::Regex(TokenKind::Comment, r"(?s:/\*.*?\*/)"),
    // Line feeds and whitespace
    Tokenizer::Regex(TokenKind::LineFeed, r"\r\n|\n|\r"),
    Tokenizer::Regex(TokenKind::Whitespace, r"[ \u{3000}\t]+"),
];

enum Tokenizer {
    Keyword(TokenKind, &'static str),
    Regex(TokenKind, &'static str),
}

impl Tokenizer {
    fn tokenize(
        &self,
        current_input: &str,
        index: usize,
        regex_cache: &mut [Option<Regex>],
    ) -> (TokenKind, usize) {
        return match self {
            Tokenizer::Keyword(kind, keyword) => {
                let mut input_chars = current_input.chars();
                let mut keyword_chars = keyword.chars();
                let mut current_byte_length = 0;
                loop {
                    let keyword_char = match keyword_chars.next() {
                        Some(c) => c,
                        _ => break,
                    };
                    let current_char = match input_chars.next() {
                        Some(c) => c,
                        _ => return (kind.clone(), 0), // reject
                    };
                    if current_char != keyword_char {
                        return (kind.clone(), 0); // reject
                    }

                    current_byte_length += current_char.len_utf8();
                }
                (kind.clone(), current_byte_length) // accept
            }
            Tokenizer::Regex(kind, regex) => {
                let regex = (&mut regex_cache[index])
                    .get_or_insert_with(|| Regex::new(format!("^({})", regex).as_str()).unwrap());

                let length = match regex.find(current_input) {
                    Some(matched) => matched.end(),
                    None => 0,
                };

                (kind.clone(), length)
            }
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token<'input> {
    pub kind: TokenKind,
    pub text: &'input str,
    pub span: Range<usize>,
}

#[extension_fn(Option<Token<'_>>)]
pub fn get_kind(&self) -> TokenKind {
    self.as_ref()
        .map(|token| token.kind)
        .unwrap_or(TokenKind::None)
}

pub struct Lexer<'input> {
    source: &'input str,
    current_byte_position: usize,
    regex_cache: Box<[Option<Regex>]>,
    current_token_cache: Option<Token<'input>>,
    pub comments: Vec<Range<usize>>,
    pub ignore_whitespace: bool,
    pub ignore_comment: bool,
}

impl<'input> Lexer<'input> {
    pub fn new(source: &'input str) -> Self {
        Self {
            source,
            current_byte_position: 0,
            regex_cache: vec![None; TOKENIZERS.len()].into_boxed_slice(),
            current_token_cache: None,
            comments: Vec::new(),
            ignore_whitespace: true,
            ignore_comment: true,
        }
    }

    pub fn current(&mut self) -> Option<Token<'input>> {
        let anchor = self.cast_anchor();

        // move to next temporarily
        self.current_token_cache = self.next();

        // back to anchor position
        self.current_byte_position = anchor.byte_position;

        self.current_token_cache.clone()
    }

    pub fn cast_anchor(&self) -> Anchor {
        Anchor {
            byte_position: self.current_byte_position,
        }
    }

    pub fn skip_line_feed(&mut self) {
        loop {
            if let TokenKind::LineFeed = self.current().get_kind() {
                self.next();
                continue;
            } else {
                return;
            }
        }
    }

    pub fn back_to_anchor(&mut self, anchor: Anchor) {
        self.current_byte_position = anchor.byte_position;
        self.current_token_cache = None;
    }

    pub fn enable_comment_token(mut self) -> Self {
        self.ignore_comment = false;
        self
    }
}

impl<'input> Iterator for Lexer<'input> {
    type Item = Token<'input>;

    fn next(&mut self) -> Option<Self::Item> {
        // take cache
        if let Some(token) = self.current_token_cache.take() {
            self.current_byte_position = token.span.end;
            return Some(token);
        }

        loop {
            if self.current_byte_position == self.source.len() {
                return None;
            }

            let current_input = &self.source[self.current_byte_position..self.source.len()];

            let mut current_max_length = 0;
            let mut current_token_kind = TokenKind::Whitespace;

            for (index, tokenizer) in TOKENIZERS.iter().enumerate() {
                let result = tokenizer.tokenize(current_input, index, &mut self.regex_cache);
                let token_kind = result.0;
                let byte_length = result.1;

                if byte_length > current_max_length {
                    current_max_length = byte_length;
                    current_token_kind = token_kind;
                }
            }

            let start_position = self.current_byte_position;

            let token = if current_max_length == 0 {
                let char_length = self.source[start_position..]
                    .chars()
                    .next()
                    .unwrap()
                    .len_utf8();

                self.current_byte_position += char_length;
                let end_position = start_position + char_length;

                Token {
                    kind: TokenKind::UnexpectedCharacter,
                    text: &self.source[start_position..end_position],
                    span: start_position..end_position,
                }
            } else {
                self.current_byte_position += current_max_length;

                if current_token_kind == TokenKind::Whitespace && self.ignore_whitespace {
                    continue;
                }

                if current_token_kind == TokenKind::Comment && self.ignore_comment {
                    self.comments
                        .push(start_position..self.current_byte_position);
                    continue;
                }

                let end_position = self.current_byte_position;

                Token {
                    kind: current_token_kind,
                    text: &self.source[start_position..end_position],
                    span: start_position..end_position,
                }
            };

            return Some(token);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    byte_position: usize,
}

impl Anchor {
    pub fn elapsed(&self, lexer: &Lexer) -> Range<usize> {
        // skip until not whitespace
        let floor = lexer.source[self.byte_position..]
            .chars()
            .take_while(|char| char.is_whitespace())
            .map(|char| char.len_utf8())
            .sum::<usize>();

        let start = self.byte_position + floor;
        let end = lexer.current_byte_position.max(start);

        start..end
    }
}

#[cfg(test)]
mod test {
    use crate::lexer::{Lexer, TokenKind};

    #[test]
    fn lexer() {
        let source = "
import \"./common.lnix\"

enum Profile { Desktop, Laptop }

let tunable profile = Profile::Desktop;

use [shojiwm];

programs.shojiwm.enable = true;
programs.shojiwm.init_config.users = [ \"bea\" ];

if profile == Profile::Desktop {
    programs.firefox.enable = true;
}
            ";

        for token in Lexer::new(source) {
            println!("{:?} : {:?}", token.kind, token.text);
        }
    }

    #[test]
    fn inputs_and_host_are_regular_literals() {
        let kinds = Lexer::new("inputs host")
            .map(|token| token.kind)
            .collect::<Vec<_>>();

        assert_eq!(kinds, [TokenKind::Literal, TokenKind::Literal]);
    }

    #[test]
    fn tokenizes_optional_and_match_syntax() {
        let kinds = Lexer::new("match value?.field ?: some(value) { null => value } T?")
            .map(|token| token.kind)
            .collect::<Vec<_>>();

        assert_eq!(
            kinds,
            [
                TokenKind::Match,
                TokenKind::Literal,
                TokenKind::SafeDot,
                TokenKind::Literal,
                TokenKind::Elvis,
                TokenKind::Some,
                TokenKind::ParenthesisLeft,
                TokenKind::Literal,
                TokenKind::ParenthesisRight,
                TokenKind::BraceLeft,
                TokenKind::Null,
                TokenKind::FatArrow,
                TokenKind::Literal,
                TokenKind::BraceRight,
                TokenKind::Literal,
                TokenKind::Question,
            ]
        );
    }

    #[test]
    fn tokenizes_explicit_import_and_export_syntax() {
        let kinds = Lexer::new(
            r#"import { Programs as Config } from "./module.lnix" export declare let value"#,
        )
        .map(|token| token.kind)
        .collect::<Vec<_>>();

        assert_eq!(
            kinds,
            [
                TokenKind::Import,
                TokenKind::BraceLeft,
                TokenKind::Literal,
                TokenKind::As,
                TokenKind::Literal,
                TokenKind::BraceRight,
                TokenKind::From,
                TokenKind::StringLiteral,
                TokenKind::Export,
                TokenKind::Declare,
                TokenKind::Let,
                TokenKind::Literal,
            ]
        );
    }

    #[test]
    fn tokenizes_throw_and_assert_syntax() {
        let kinds = Lexer::new(r#"assert enabled, "must be enabled" throw "invalid""#)
            .map(|token| token.kind)
            .collect::<Vec<_>>();

        assert_eq!(
            kinds,
            [
                TokenKind::Assert,
                TokenKind::Literal,
                TokenKind::Comma,
                TokenKind::StringLiteral,
                TokenKind::Throw,
                TokenKind::StringLiteral,
            ]
        );
    }

    #[test]
    fn finite_numbers_are_numeric_but_inf_and_nan_are_literals() {
        let kinds = Lexer::new("inf nan 1 1.5 5E+2")
            .map(|token| token.kind)
            .collect::<Vec<_>>();

        assert_eq!(
            kinds,
            [
                TokenKind::Literal,
                TokenKind::Literal,
                TokenKind::NumericLiteral,
                TokenKind::NumericLiteral,
                TokenKind::NumericLiteral,
            ]
        );
    }
}
