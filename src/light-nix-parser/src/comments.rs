//! Comment ownership.
//!
//! Comments are not trivia in LightNix: the canonical formatter reorders
//! statements, so every comment must have an owner that carries it along.
//! Ownership is decided purely by syntax:
//!
//! - `/* ... */` before the first statement is the module header, owned by
//!   the file.  Anywhere else it is a parse error.
//! - `# ...` on the same line after a statement trails that statement.
//! - Any other `# ...` leads the next statement below it, even when blank
//!   lines separate them.  With no statement below, it is the module footer.
//! - A `# ...` inside a statement's span (e.g. inside an `if` block) is
//!   interior: it travels inside the statement's own text and needs no
//!   separate owner.

use std::ops::Range;

use crate::ast::{AST, Source};

/// Where every comment of a module lives.  Indices into `leading` and
/// `trailing` parallel the module's top-level statement list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommentMap {
    pub header: Vec<Range<usize>>,
    pub footer: Vec<Range<usize>>,
    pub leading: Vec<Vec<Range<usize>>>,
    pub trailing: Vec<Option<Range<usize>>>,
}

/// Attaches every recorded comment to its owner.  Misplaced block comments
/// attach nowhere; [`misplaced_block_comments`] reports them.
pub fn attach_comments(
    text: &str,
    source: &Source<'_, '_>,
    comments: &[Range<usize>],
) -> CommentMap {
    let spans = statement_spans(source);
    let mut map = CommentMap {
        header: Vec::new(),
        footer: Vec::new(),
        leading: vec![Vec::new(); spans.len()],
        trailing: vec![None; spans.len()],
    };
    for comment in normalized(comments) {
        if is_block(text, &comment) {
            if in_header_position(&spans, &comment) {
                map.header.push(comment);
            }
            continue;
        }
        if spans
            .iter()
            .any(|span| span.start < comment.start && comment.end <= span.end)
        {
            continue; // interior: preserved inside the statement's own text
        }
        if let Some(index) = spans.iter().position(|span| {
            span.end <= comment.start && !text[span.end..comment.start].contains(['\n', '\r'])
        }) {
            map.trailing[index].get_or_insert(comment);
            continue;
        }
        match spans.iter().position(|span| comment.end <= span.start) {
            Some(index) => map.leading[index].push(comment),
            None => map.footer.push(comment),
        }
    }
    map
}

/// Block comments that are not in header position: everywhere but before the
/// first statement they are a parse error, with `#` as the suggested fix.
pub fn misplaced_block_comments(
    text: &str,
    source: &Source<'_, '_>,
    comments: &[Range<usize>],
) -> Vec<Range<usize>> {
    let spans = statement_spans(source);
    normalized(comments)
        .into_iter()
        .filter(|comment| is_block(text, comment) && !in_header_position(&spans, comment))
        .collect()
}

/// The lexer records comments during lookahead as well, so the raw list can
/// contain duplicates and out-of-order entries.
fn normalized(comments: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut comments = comments.to_vec();
    comments.sort_by_key(|span| (span.start, span.end));
    comments.dedup();
    comments
}

fn statement_spans(source: &Source<'_, '_>) -> Vec<Range<usize>> {
    source
        .statements
        .iter()
        .map(|statement| statement.span())
        .collect()
}

fn is_block(text: &str, comment: &Range<usize>) -> bool {
    text[comment.clone()].starts_with("/*")
}

fn in_header_position(spans: &[Range<usize>], comment: &Range<usize>) -> bool {
    spans
        .first()
        .is_none_or(|first| comment.end <= first.start)
}
