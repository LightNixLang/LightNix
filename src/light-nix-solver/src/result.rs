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

/// One requirement that participates in an unsatisfiable core, identified by
/// its index in the [`SolveRequest`](crate::SolveRequest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnsatItem {
    Goal(usize),
    Constraint(usize),
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SolveOutcome {
    Sat(Solution),
    /// No assignment satisfies the request.  `core` names a subset of the
    /// request's goals and constraints that is already contradictory on its
    /// own (empty when the contradiction lies outside tracked requirements,
    /// e.g. in candidate exclusions).
    Unsat { core: Vec<UnsatItem> },
    Unknown(UnknownReason),
}
