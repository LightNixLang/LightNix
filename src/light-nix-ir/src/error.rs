use std::ops::Range;

use light_nix_name_resolver::{ModuleId, SymbolId};
use light_nix_type_checker::Type;

use crate::{ExpressionId, OutputPath, VariableId};

#[derive(Debug, Clone, PartialEq)]
pub struct BuildError {
    pub kind: BuildErrorKind,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum BuildErrorKind {
    TableOverflow,
    DuplicateVariableSource,
    UnknownVariable(VariableId),
    UnknownExpression(ExpressionId),
    TypeMismatch {
        expected: Type,
        found: Type,
    },
    ExpectedBoolean {
        found: Type,
    },
    InvalidConstant {
        expected: Type,
    },
    InvalidOperation,
    DuplicateInitialValue(VariableId),
    OutputTypeMismatch {
        path: OutputPath,
        expected: Type,
        found: Type,
    },
    OutputPolicyMismatch {
        path: OutputPath,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LowerError {
    pub kind: LowerErrorKind,
    pub module: ModuleId,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum LowerErrorKind {
    MissingType,
    MissingResolution,
    UnknownSymbol(SymbolId),
    InvalidNumber,
    InvalidString,
    InvalidAssignmentTarget,
    CyclicBinding(SymbolId),
    UnsupportedExpression,
    Build(BuildErrorKind),
}

impl From<BuildError> for LowerErrorKind {
    fn from(error: BuildError) -> Self {
        Self::Build(error.kind)
    }
}
