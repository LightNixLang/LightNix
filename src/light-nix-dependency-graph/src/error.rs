use std::ops::Range;

use light_nix_name_resolver::ModuleId;

use crate::{DependencyCycle, DependencyNodeId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyGraphError {
    pub kind: DependencyGraphErrorKind,
    pub locations: Vec<DependencyLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyGraphErrorKind {
    Cycle { nodes: Vec<DependencyNodeId> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyLocation {
    pub module: ModuleId,
    pub span: Range<usize>,
}

impl From<&DependencyCycle> for DependencyGraphError {
    fn from(cycle: &DependencyCycle) -> Self {
        Self {
            kind: DependencyGraphErrorKind::Cycle {
                nodes: cycle.nodes.clone(),
            },
            locations: cycle
                .edges
                .iter()
                .map(|edge| DependencyLocation {
                    module: edge.source.module(),
                    span: edge.span.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationOrderError {
    pub cycles: Vec<DependencyCycle>,
}
