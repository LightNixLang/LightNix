mod analysis;
mod planning;

pub use analysis::{AnalysisEnvironment, ModuleAnalysis, analyze_module};
pub use light_nix_dependency_graph as dependency_graph;
pub use light_nix_evaluator as evaluator;
pub use light_nix_ir as ir;
pub use light_nix_name_resolver as name_resolver;
pub use light_nix_parser as parser;
pub use light_nix_solver as solver;
pub use light_nix_type_checker as type_checker;
pub use planning::{
    ChangePlan, ExternalConstraint, Goal, PlanError, PlanOutcome, PlanningRequest, plan_changes,
};
