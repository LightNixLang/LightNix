use light_nix_ir::{Constant, ExpressionId, OutputPath, SourceOrigin, VariableId, VariableSource};

#[derive(Debug, Clone, PartialEq)]
pub struct VariableChange {
    pub variable: VariableId,
    pub source: VariableSource,
    pub before: Option<Constant>,
    pub after: Constant,
    pub cost: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputChange {
    pub path: OutputPath,
    pub before: Option<Constant>,
    pub after: Option<Constant>,
    pub cost: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpaqueImpact {
    pub boundary: ExpressionId,
    pub origin: Option<SourceOrigin>,
    pub changed_variables: Vec<VariableId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    pub cost: u64,
    pub variables: Vec<VariableChange>,
    pub outputs: Vec<OutputChange>,
    pub opaque_impacts: Vec<OpaqueImpact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownReason(pub String);

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SolveOutcome {
    Sat(Solution),
    Unsat,
    Unknown(UnknownReason),
}
