//! The write-back half of the lens: turns a solved [`ChangePlan`] into
//! source-text edits.
//!
//! Every change is a literal span replacement (applied back-to-front), plus
//! insertions appended at the end of the file — the canonical formatter then
//! carries an appended claim to its canonical position.  Changes that cannot
//! be expressed as a faithful span edit (conditional claims, nested
//! assignment rows, imported roots whose local names are unknown) are
//! reported as [`EditError::Unrepresentable`] instead of guessed at.

use light_nix_formatter::{FormatError, TextEdit, apply_edits, format_source};
use light_nix_ir::{
    Constant, ExpressionId, ExpressionKind, OutputPath, OutputPathSegment, SourceOrigin,
};
use light_nix_parser::ast::{AssignValue, Statement};

use crate::{ChangePlan, ModuleAnalysis};

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EditError {
    /// The change is real but cannot be written back as a span edit.
    Unrepresentable { reason: String },
    Format(FormatError),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unrepresentable { reason } => {
                write!(formatter, "change cannot be written back: {reason}")
            }
            Self::Format(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for EditError {}

impl From<FormatError> for EditError {
    fn from(error: FormatError) -> Self {
        Self::Format(error)
    }
}

fn unrepresentable(reason: impl Into<String>) -> EditError {
    EditError::Unrepresentable {
        reason: reason.into(),
    }
}

/// Computes the source edits realising a plan.  GetPut: a plan that changes
/// nothing produces no edits.
pub fn plan_edits(
    analysis: &ModuleAnalysis<'_, '_>,
    plan: &ChangePlan,
    source: &str,
) -> Result<Vec<TextEdit>, EditError> {
    let mut edits = Vec::new();

    for change in &plan.solution().variables {
        let variable = analysis
            .model()
            .variable(change.variable)
            .ok_or_else(|| unrepresentable("the solver changed an unknown variable"))?;
        let initial = variable.initial().ok_or_else(|| {
            unrepresentable("the changed binding has no initial value expression")
        })?;
        let span = expression_span(analysis, initial)?;
        edits.push(TextEdit {
            range: span,
            text: render_constant(analysis, &change.after)?,
        });
    }

    for change in &plan.solution().outputs {
        match &change.after {
            Some(value) => {
                if analysis.model().output(&change.path).is_some() {
                    let span = claim_value_span(analysis, &change.path)?;
                    edits.push(TextEdit {
                        range: span,
                        text: render_constant(analysis, value)?,
                    });
                } else {
                    let line = format!(
                        "{} = {}\n",
                        render_path(analysis, &change.path)?,
                        render_constant(analysis, value)?,
                    );
                    let text = if source.is_empty() || source.ends_with('\n') {
                        line
                    } else {
                        format!("\n{line}")
                    };
                    edits.push(TextEdit {
                        range: source.len()..source.len(),
                        text,
                    });
                }
            }
            None => {
                let span = claim_statement_span(analysis, &change.path)?;
                edits.push(TextEdit {
                    range: span,
                    text: String::new(),
                });
            }
        }
    }

    Ok(edits)
}

/// Applies a plan to the source text and returns the canonically formatted
/// result.
pub fn apply_plan(
    analysis: &ModuleAnalysis<'_, '_>,
    plan: &ChangePlan,
    source: &str,
) -> Result<String, EditError> {
    let edits = plan_edits(analysis, plan, source)?;
    let edited = apply_edits(source, &edits);
    Ok(format_source(&edited)?)
}

/// The span of the right-hand side of the single unconditional claim on a
/// path.  Conditional claims are not editable as span replacements — the
/// row's value would only change in some worlds — so they are refused.
fn claim_value_span(
    analysis: &ModuleAnalysis<'_, '_>,
    path: &OutputPath,
) -> Result<std::ops::Range<usize>, EditError> {
    let value = unconditional_claim_value(analysis, path)?;
    expression_span(analysis, value)
}

fn unconditional_claim_value(
    analysis: &ModuleAnalysis<'_, '_>,
    path: &OutputPath,
) -> Result<ExpressionId, EditError> {
    let definition = analysis
        .model()
        .output(path)
        .ok_or_else(|| unrepresentable("the edited path has no claim in this module"))?;
    let [case] = definition.cases() else {
        return Err(unrepresentable(
            "the edited path has multiple claims; editing one of them is ambiguous",
        ));
    };
    let guard = analysis
        .model()
        .expression(case.guard())
        .map(|expression| expression.kind());
    if !matches!(guard, Some(ExpressionKind::Constant(Constant::Bool(true)))) {
        return Err(unrepresentable(
            "the claim is conditional; edit its condition's inputs instead",
        ));
    }
    Ok(case.value())
}

fn expression_span(
    analysis: &ModuleAnalysis<'_, '_>,
    expression: ExpressionId,
) -> Result<std::ops::Range<usize>, EditError> {
    analysis
        .model()
        .expression(expression)
        .and_then(|expression| expression.origin())
        .map(SourceOrigin::span)
        .ok_or_else(|| unrepresentable("the edited expression has no source location"))
}

/// The span of the whole `a.b = c` statement carrying the claim, for row
/// deletion.  Only a top-level assignment with a plain expression value can
/// be deleted this way; a nested assignment block carries sibling claims.
fn claim_statement_span(
    analysis: &ModuleAnalysis<'_, '_>,
    path: &OutputPath,
) -> Result<std::ops::Range<usize>, EditError> {
    let value = unconditional_claim_value(analysis, path)?;
    let value_span = expression_span(analysis, value)?;
    for statement in analysis.source().statements {
        let Statement::AssignStatement(node) = statement else {
            continue;
        };
        let span = node.span.clone();
        if span.start > value_span.start || span.end < value_span.end {
            continue;
        }
        if !matches!(node.value, AssignValue::Expression(_)) {
            return Err(unrepresentable(
                "the claim lives in a nested assignment block; deleting it would touch siblings",
            ));
        }
        return Ok(span);
    }
    Err(unrepresentable(
        "no assignment statement carries the deleted claim",
    ))
}

/// Renders an output path the way a human would write it.
fn render_path(
    analysis: &ModuleAnalysis<'_, '_>,
    path: &OutputPath,
) -> Result<String, EditError> {
    let resolution = analysis.resolution();
    let root = resolution
        .symbols()
        .iter()
        .find(|symbol| symbol.id == path.root_symbol())
        .ok_or_else(|| {
            unrepresentable("the claim's root symbol is not declared in this module")
        })?;
    let mut text = resolution.name(root.name).to_owned();
    for segment in path.segments() {
        match segment {
            OutputPathSegment::Field(field) => {
                let field = resolution
                    .fields()
                    .iter()
                    .find(|candidate| candidate.id == *field)
                    .ok_or_else(|| {
                        unrepresentable("the claim's field is not declared in this module")
                    })?;
                text.push('.');
                text.push_str(resolution.name(field.name));
            }
            OutputPathSegment::Key(key) => {
                text.push_str(&format!("[\"{}\"]", escape_string(key)));
            }
        }
    }
    Ok(text)
}

/// Renders a constant as LightNix literal text.  Package atoms render as
/// plain string literals: the schema position they are written into gives
/// them back their package typing when the file is re-analysed.
fn render_constant(
    analysis: &ModuleAnalysis<'_, '_>,
    value: &Constant,
) -> Result<String, EditError> {
    Ok(match value {
        Constant::Bool(value) => value.to_string(),
        Constant::Int(value) => value.to_string(),
        Constant::Float(value) if value.is_finite() => format!("{value:?}"),
        Constant::String(value) | Constant::Package(value) => {
            format!("\"{}\"", escape_string(value))
        }
        Constant::Set(values) => format!("@set [{}]", render_elements(analysis, values)?),
        Constant::List(values) => format!("[{}]", render_elements(analysis, values)?),
        Constant::Optional(None) => "null".to_owned(),
        Constant::Optional(Some(value)) => {
            format!("some({})", render_constant(analysis, value)?)
        }
        Constant::Enum(variant) => {
            let resolution = analysis.resolution();
            let variant = resolution
                .variants()
                .iter()
                .find(|candidate| candidate.id == *variant)
                .ok_or_else(|| {
                    unrepresentable("the enum variant is not declared in this module")
                })?;
            let owner = resolution
                .types()
                .iter()
                .find(|candidate| candidate.id == variant.owner)
                .and_then(|candidate| candidate.name)
                .ok_or_else(|| unrepresentable("the enum type is not declared in this module"))?;
            format!(
                "{}::{}",
                resolution.name(owner),
                resolution.name(variant.name)
            )
        }
        value => {
            return Err(unrepresentable(format!(
                "no literal syntax renders the value {value:?}"
            )));
        }
    })
}

fn render_elements(
    analysis: &ModuleAnalysis<'_, '_>,
    values: &[Constant],
) -> Result<String, EditError> {
    Ok(values
        .iter()
        .map(|value| render_constant(analysis, value))
        .collect::<Result<Vec<_>, _>>()?
        .join(", "))
}

fn escape_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character => escaped.push(character),
        }
    }
    escaped
}
