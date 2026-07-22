//! Value-level dependency graphs built from a parsed module and its resolved names.
//!
//! Edges point from a declaration or root statement to the values it uses. Type-only
//! references are intentionally excluded. References that require static dispatch are
//! added by [`refine_dependency_graph`] after type checking; the untyped graph still
//! retains their receiver dependencies.

mod builder;
pub mod error;
mod graph;

pub use builder::{build_dependency_graph, refine_dependency_graph};
pub use error::{
    DependencyGraphError, DependencyGraphErrorKind, DependencyLocation, EvaluationOrderError,
};
pub use graph::{
    DependencyCycle, DependencyEdge, DependencyEdgeKind, DependencyGraph, DependencyNode,
    DependencyNodeId, DependencyNodeKind, StatementId,
};
