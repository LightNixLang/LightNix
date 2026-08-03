//! Z3-backed optimization for LightNix constraint models.

mod error;
mod request;
mod result;
mod solver;

pub use error::{SolveError, SolveErrorKind};
pub use request::{OutputConstraint, OutputGoal, SolveRequest, VirtualOutput};
pub use result::{
    OpaqueImpact, OutputChange, Solution, SolveOutcome, UnknownReason, UnsatItem, VariableChange,
};
pub use solver::solve;
