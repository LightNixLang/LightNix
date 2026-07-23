//! Solver-independent, typed intermediate representation for LightNix.
//!
//! The public IR deliberately contains no Z3 values or contexts. Solver
//! backends can translate the same validated model to Z3 or another engine.

mod builder;
mod error;
mod lower;
mod model;

pub use builder::ModelBuilder;
pub use error::{BuildError, BuildErrorKind, LowerError, LowerErrorKind};
pub use lower::{LowerResult, lower_module};
pub use model::{
    BinaryOperation, CallTarget, Constant, Constraint, ConstraintId, ConstraintKind,
    ConstraintModel, Expression, ExpressionId, ExpressionKind, MutationPolicy, Objective,
    ObjectiveId, ObjectiveKind, OutputCase, OutputDefinition, OutputPath, PathDeclaration,
    SourceOrigin, UnaryOperation, Variable, VariableId, VariableKind, VariableSource,
    WeightedVariable,
};
