//! The canonical LightNix formatter.
//!
//! This crate is a pure library — text in, edits out, no I/O — so the same
//! implementation serves the CLI (format on save), the machine-editing
//! pipeline (append a claim roughly, let the formatter carry it to its
//! canonical position), and an LSP server (`textDocument/formatting` returns
//! the [`TextEdit`]s directly; converting byte offsets to UTF-16 positions is
//! the transport layer's job).
//!
//! Guarantees:
//! - Sources that fail to parse are never touched ([`format_source`] returns
//!   an error and no edits), so a half-typed buffer cannot be mangled.
//! - Formatting is idempotent: `format(format(x)) == format(x)`.
//! - Statement interiors are reproduced verbatim from the source; the
//!   formatter only owns statement order and the blank lines between groups.

use std::ops::Range;

use light_nix_parser::{
    ast::{AST, AssignValue, AstArena, Statement},
    lexer::Lexer,
    parser::{ParseErrors, parse_source},
};

/// A replacement of one byte range of the input with new text.  Ranges of
/// distinct edits never overlap; apply them back-to-front (see
/// [`apply_edits`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatError {
    /// The input does not parse; it is left untouched.
    Parse { messages: Vec<String> },
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { messages } => {
                write!(formatter, "input does not parse: {}", messages.join("; "))
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// The statement groups of the canonical layout, in file order.  Groups are
/// separated by exactly one blank line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    /// `import` statements, sorted by their text.
    Imports,
    /// `use` declarations, sorted by their text.
    Uses,
    /// Type-level items (`enum` / `type` / `interface` / `implements`),
    /// original order preserved.
    Definitions,
    /// `let` / `function` / `assert` items.  Original order is preserved
    /// because lexical declaration-before-use is part of the semantics.
    Bindings,
    /// Option claims (`a.b = c`), sorted by target path so that claims about
    /// the same subtree end up adjacent — this is what carries a roughly
    /// appended machine edit to its canonical position.
    Claims,
    /// Top-level `if` / `match` blocks (conditional claim containers),
    /// original order preserved.
    ControlFlow,
}

/// Formats a whole source file into its canonical form.
pub fn format_source(input: &str) -> Result<String, FormatError> {
    let arena = AstArena::new();
    let mut lexer = Lexer::new(input);
    let mut errors = ParseErrors::new_in(&arena);
    let source = parse_source(&mut lexer, &mut errors, &arena);
    if !errors.is_empty() {
        return Err(FormatError::Parse {
            messages: errors.iter().map(|error| format!("{error:?}")).collect(),
        });
    }

    let mut entries = Vec::new();
    for statement in source.statements {
        let span = statement.span();
        let text = input[span].trim().to_owned();
        let (group, key) = match statement {
            Statement::ImportStatement(_) => (Group::Imports, Some(text.clone())),
            Statement::UseDeclare(_) => (Group::Uses, Some(text.clone())),
            Statement::EnumDefine(_)
            | Statement::TypeDefine(_)
            | Statement::InterfaceDefine(_)
            | Statement::ImplementsDefine(_) => (Group::Definitions, None),
            Statement::LetStatement(_)
            | Statement::FunctionDefine(_)
            | Statement::AssertStatement(_) => (Group::Bindings, None),
            Statement::AssignStatement(node) => {
                let key = claim_sort_key(input, &node.target.span(), &node.value);
                (Group::Claims, Some(key))
            }
            Statement::Expression(_) => (Group::ControlFlow, None),
        };
        entries.push((group, key, text));
    }
    // A stable sort on (group, key) keeps the original order wherever the
    // key is `None` and wherever keys tie.
    entries.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));

    let mut output = String::new();
    let mut previous_group = None;
    for (group, _, text) in &entries {
        match previous_group {
            None => {}
            Some(previous) if previous == *group => output.push('\n'),
            Some(_) => output.push_str("\n\n"),
        }
        output.push_str(text);
        previous_group = Some(*group);
    }
    if !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

/// The sort key of a claim: the target path with whitespace removed, so that
/// `programs .firefox.enable` and `programs.firefox.enable` collate the same
/// way, and claims about one subtree become adjacent.
fn claim_sort_key(input: &str, target: &Range<usize>, value: &AssignValue<'_, '_>) -> String {
    let mut key: String = input[target.clone()]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    // A nested assignment covers a whole subtree; suffix the key so the
    // block sorts right after a plain claim on the same path prefix.
    if matches!(value, AssignValue::Nested(_)) {
        key.push('{');
    }
    key
}

/// Formats the input and reports the result as a minimal single edit —
/// empty when the file is already canonical.  This is the shape an LSP
/// formatting handler forwards directly.
pub fn format_edits(input: &str) -> Result<Vec<TextEdit>, FormatError> {
    let formatted = format_source(input)?;
    if formatted == input {
        return Ok(Vec::new());
    }
    let input_bytes = input.as_bytes();
    let formatted_bytes = formatted.as_bytes();
    let mut prefix = input_bytes
        .iter()
        .zip(formatted_bytes)
        .take_while(|(left, right)| left == right)
        .count();
    while !input.is_char_boundary(prefix) || !formatted.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = input_bytes
        .iter()
        .rev()
        .zip(formatted_bytes.iter().rev())
        .take_while(|(left, right)| left == right)
        .count()
        .min(input.len() - prefix)
        .min(formatted.len() - prefix);
    while !input.is_char_boundary(input.len() - suffix)
        || !formatted.is_char_boundary(formatted.len() - suffix)
    {
        suffix -= 1;
    }
    Ok(vec![TextEdit {
        range: prefix..input.len() - suffix,
        text: formatted[prefix..formatted.len() - suffix].to_owned(),
    }])
}

/// Applies non-overlapping edits to a source text.
pub fn apply_edits(input: &str, edits: &[TextEdit]) -> String {
    let mut edits = edits.to_vec();
    edits.sort_by(|left, right| right.range.start.cmp(&left.range.start));
    let mut output = input.to_owned();
    for edit in &edits {
        output.replace_range(edit.range.clone(), &edit.text);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_are_grouped_and_claims_sorted_by_path() {
        let input = r#"programs.zsh.enable = true
type Firefox { enable: Bool }
import { programs } from "@lnix/nixos"
let value = 1
programs.firefox.enable = true
"#;
        let formatted = format_source(input).unwrap();
        assert_eq!(
            formatted,
            r#"import { programs } from "@lnix/nixos"

type Firefox { enable: Bool }

let value = 1

programs.firefox.enable = true
programs.zsh.enable = true
"#
        );
    }

    #[test]
    fn formatting_is_idempotent() {
        let input = r#"
let b = 2
let a = 1
audio.pulseaudio = true
audio.pipewire = false
"#;
        let once = format_source(input).unwrap();
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn binding_order_is_preserved_for_lexical_scoping() {
        let input = r#"let first = 1
let second = first
"#;
        let formatted = format_source(input).unwrap();
        assert!(formatted.find("let first").unwrap() < formatted.find("let second").unwrap());
    }

    #[test]
    fn unparseable_input_is_left_untouched() {
        assert!(matches!(
            format_source("let = = ="),
            Err(FormatError::Parse { .. })
        ));
    }

    #[test]
    fn canonical_input_produces_no_edits() {
        let input = "programs.firefox.enable = true\n";
        assert_eq!(format_edits(input).unwrap(), Vec::new());
    }

    #[test]
    fn edits_reproduce_the_formatted_output() {
        let input = "let a = 1\nprograms.zsh.enable = true\nprograms.firefox.enable = true\n";
        let edits = format_edits(input).unwrap();
        assert_eq!(apply_edits(input, &edits), format_source(input).unwrap());
    }
}
