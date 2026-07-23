use light_nix_name_resolver::{FieldId, ModuleId, SymbolId};

use crate::{OutputPath, RuntimeValue};

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluationError {
    pub kind: EvaluationErrorKind,
    pub module: ModuleId,
    pub span: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvaluationErrorKind {
    MissingInput { symbol: SymbolId },
    CyclicValue { symbol: SymbolId },
    InvalidNumber,
    InvalidStringEscape,
    ExpectedBoolean { found: RuntimeValue },
    ExpectedNumber { found: RuntimeValue },
    ExpectedString { found: RuntimeValue },
    ExpectedRecord { found: RuntimeValue },
    MissingField { field: FieldId },
    IntegerOverflow,
    DivisionByZero,
    NotCallable { found: RuntimeValue },
    ArgumentCount { expected: usize, found: usize },
    UnresolvedDispatch,
    InvalidAssignmentTarget,
    DuplicateAssignment { path: OutputPath },
    AssertionFailed { message: Option<String> },
    Thrown { message: Option<String> },
    NoMatchingPattern,
    InvalidBuiltinCall,
}
