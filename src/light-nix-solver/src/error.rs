use std::{error::Error, fmt};

use light_nix_ir::{ExpressionId, OutputPath, VariableId};
use light_nix_type_checker::{BuiltinMethod, Type};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolveError {
    pub kind: SolveErrorKind,
}

impl fmt::Display for SolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", self.kind)
    }
}

impl Error for SolveError {}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SolveErrorKind {
    UnknownExpression(ExpressionId),
    UnknownVariable(VariableId),
    UnknownOutput(OutputPath),
    MissingInput(VariableId),
    MissingInitialValue(VariableId),
    TypeMismatch { expected: Type, found: Type },
    InvalidConstant { expected: Type },
    UnsupportedType(Type),
    UnsupportedExpression(ExpressionId),
    UnsupportedBuiltin(BuiltinMethod),
    CyclicExpression(ExpressionId),
    CyclicVariable(VariableId),
    CyclicOutput(OutputPath),
    IntegerCostOverflow,
    InvalidFloat,
    ModelValueUnavailable,
}
