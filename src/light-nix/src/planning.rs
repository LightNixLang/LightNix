use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    fmt,
    time::Duration,
};

use light_nix_evaluator::{
    EvaluationError, EvaluationInputs, EvaluationSnapshot, OutputChange as EvaluatedOutputChange,
    RuntimeValue, evaluate_module,
};
use light_nix_ir::{Constant, OutputPath, VariableKind, VariableSource};
use light_nix_solver::{
    OutputConstraint, OutputGoal, Solution, SolveError, SolveOutcome, SolveRequest, UnknownReason,
    solve,
};

use crate::ModuleAnalysis;

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Goal {
    Equals { path: OutputPath, value: Constant },
    Absent { path: OutputPath },
    Contains { path: OutputPath, value: Constant },
    NotContains { path: OutputPath, value: Constant },
}

impl Goal {
    pub fn path(&self) -> &OutputPath {
        match self {
            Self::Equals { path, .. }
            | Self::Absent { path }
            | Self::Contains { path, .. }
            | Self::NotContains { path, .. } => path,
        }
    }
}

/// A catalog-provided invariant that is enforced without becoming part of the
/// user's primary request.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ExternalConstraint {
    Required(Goal),
    Implies { condition: Goal, consequence: Goal },
    Conflicts { left: Goal, right: Goal },
}

#[derive(Debug, Clone)]
pub struct PlanningRequest {
    goals: Vec<Goal>,
    constraints: Vec<ExternalConstraint>,
    timeout: Option<Duration>,
    max_candidates: usize,
}

impl Default for PlanningRequest {
    fn default() -> Self {
        Self {
            goals: Vec::new(),
            constraints: Vec::new(),
            timeout: None,
            max_candidates: 32,
        }
    }
}

impl PlanningRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn require(&mut self, goal: Goal) -> &mut Self {
        self.goals.push(goal);
        self
    }

    pub fn require_output(&mut self, path: OutputPath, value: Constant) -> &mut Self {
        self.require(Goal::Equals { path, value })
    }

    pub fn require_output_absent(&mut self, path: OutputPath) -> &mut Self {
        self.require(Goal::Absent { path })
    }

    pub fn require_member(&mut self, path: OutputPath, value: Constant) -> &mut Self {
        self.require(Goal::Contains { path, value })
    }

    pub fn forbid_member(&mut self, path: OutputPath, value: Constant) -> &mut Self {
        self.require(Goal::NotContains { path, value })
    }

    pub fn constrain(&mut self, constraint: ExternalConstraint) -> &mut Self {
        self.constraints.push(constraint);
        self
    }

    pub fn require_constraint(&mut self, predicate: Goal) -> &mut Self {
        self.constrain(ExternalConstraint::Required(predicate))
    }

    pub fn imply(&mut self, condition: Goal, consequence: Goal) -> &mut Self {
        self.constrain(ExternalConstraint::Implies {
            condition,
            consequence,
        })
    }

    pub fn conflict(&mut self, left: Goal, right: Goal) -> &mut Self {
        self.constrain(ExternalConstraint::Conflicts { left, right })
    }

    pub fn set_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn set_max_candidates(&mut self, max_candidates: usize) -> &mut Self {
        self.max_candidates = max_candidates;
        self
    }

    pub fn goals(&self) -> &[Goal] {
        &self.goals
    }

    pub fn constraints(&self) -> &[ExternalConstraint] {
        &self.constraints
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub fn max_candidates(&self) -> usize {
        self.max_candidates
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangePlan {
    solution: Solution,
    before: EvaluationSnapshot,
    after: EvaluationSnapshot,
    output_changes: Vec<EvaluatedOutputChange>,
    requested_paths: BTreeSet<OutputPath>,
    rejected_candidates: usize,
}

impl ChangePlan {
    pub fn solution(&self) -> &Solution {
        &self.solution
    }

    pub fn before(&self) -> &EvaluationSnapshot {
        &self.before
    }

    pub fn after(&self) -> &EvaluationSnapshot {
        &self.after
    }

    pub fn output_changes(&self) -> &[EvaluatedOutputChange] {
        &self.output_changes
    }

    pub fn side_effects(&self) -> impl Iterator<Item = &EvaluatedOutputChange> {
        self.output_changes
            .iter()
            .filter(|change| !self.requested_paths.contains(&change.path))
    }

    pub fn rejected_candidates(&self) -> usize {
        self.rejected_candidates
    }

    pub fn requires_confirmation(&self) -> bool {
        !self.solution.opaque_impacts.is_empty()
            || self.side_effects().next().is_some()
            || self
                .output_changes
                .iter()
                .any(|change| !change.opaque_dependencies.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PlanOutcome {
    Applicable(ChangePlan),
    Unsatisfiable { rejected_candidates: usize },
    Unknown(UnknownReason),
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PlanError {
    AnalysisFailed,
    EmptyRequest,
    BaselineEvaluation(Vec<EvaluationError>),
    Solve(SolveError),
    UnsupportedRuntimeValue,
    UnsupportedConstant,
    UnsupportedVariableSource(VariableSource),
    UnsupportedSolveOutcome,
    CandidateLimitReached { rejected_candidates: usize },
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Error for PlanError {}

impl From<SolveError> for PlanError {
    fn from(error: SolveError) -> Self {
        Self::Solve(error)
    }
}

impl<'input, 'allocator> ModuleAnalysis<'input, 'allocator> {
    pub fn plan(
        &self,
        inputs: &EvaluationInputs,
        request: &PlanningRequest,
    ) -> Result<PlanOutcome, PlanError> {
        plan_changes(self, inputs, request)
    }
}

pub fn plan_changes(
    analysis: &ModuleAnalysis<'_, '_>,
    inputs: &EvaluationInputs,
    request: &PlanningRequest,
) -> Result<PlanOutcome, PlanError> {
    if !analysis.is_success() {
        return Err(PlanError::AnalysisFailed);
    }
    if request.goals.is_empty() {
        return Err(PlanError::EmptyRequest);
    }
    if request.max_candidates == 0 {
        return Err(PlanError::CandidateLimitReached {
            rejected_candidates: 0,
        });
    }

    let baseline = evaluate_module(
        analysis.source(),
        analysis.resolution(),
        analysis.types(),
        inputs,
    );
    if !baseline.is_success() {
        return Err(PlanError::BaselineEvaluation(baseline.errors().to_vec()));
    }

    let mut solve_request = build_solve_request(analysis, &baseline, request)?;
    let before = baseline.snapshot().clone();
    let requested_paths = request
        .goals
        .iter()
        .map(|goal| goal.path().clone())
        .collect::<BTreeSet<_>>();
    let mut rejected_candidates = 0;

    loop {
        let outcome = solve(analysis.model(), &solve_request)?;
        let solution = match outcome {
            SolveOutcome::Sat(solution) => solution,
            SolveOutcome::Unsat => {
                return Ok(PlanOutcome::Unsatisfiable {
                    rejected_candidates,
                });
            }
            SolveOutcome::Unknown(reason) => return Ok(PlanOutcome::Unknown(reason)),
            _ => return Err(PlanError::UnsupportedSolveOutcome),
        };
        let candidate_inputs = apply_solution(analysis, inputs, &solution)?;
        let candidate = evaluate_module(
            analysis.source(),
            analysis.resolution(),
            analysis.types(),
            &candidate_inputs,
        );
        let valid = candidate.is_success()
            && request
                .goals
                .iter()
                .all(|goal| goal_is_satisfied(candidate.snapshot(), goal))
            && request
                .constraints
                .iter()
                .all(|constraint| constraint_is_satisfied(candidate.snapshot(), constraint));
        if valid {
            let after = candidate.snapshot().clone();
            let output_changes = before.diff(&after);
            return Ok(PlanOutcome::Applicable(ChangePlan {
                solution,
                before,
                after,
                output_changes,
                requested_paths,
                rejected_candidates,
            }));
        }

        rejected_candidates += 1;
        if rejected_candidates >= request.max_candidates {
            return Err(PlanError::CandidateLimitReached {
                rejected_candidates,
            });
        }
        exclude_solution(&mut solve_request, &solution);
    }
}

fn build_solve_request(
    analysis: &ModuleAnalysis<'_, '_>,
    baseline: &light_nix_evaluator::EvaluationResult,
    request: &PlanningRequest,
) -> Result<SolveRequest, PlanError> {
    let mut solve_request = SolveRequest::new();
    if let Some(timeout) = request.timeout {
        solve_request.set_timeout(timeout);
    }
    for variable in analysis.model().variables() {
        let VariableSource::Symbol(symbol) = variable.source() else {
            continue;
        };
        let runtime = baseline
            .tunable(*symbol)
            .map(|value| &value.value)
            .or_else(|| baseline.symbol_value(*symbol));
        let Some(runtime) = runtime else {
            continue;
        };
        match runtime_to_constant(runtime) {
            Some(value) => {
                solve_request.set_variable_value(variable.id(), value);
            }
            None if matches!(variable.kind(), VariableKind::Tunable { .. }) => {
                return Err(PlanError::UnsupportedRuntimeValue);
            }
            None => {}
        }
    }
    for declaration in analysis.model().paths() {
        let value = baseline
            .snapshot()
            .get(declaration.path())
            .map(|entry| {
                runtime_to_constant(&entry.value).ok_or(PlanError::UnsupportedRuntimeValue)
            })
            .transpose()?;
        solve_request.set_output_value(declaration.path().clone(), value);
    }
    for goal in &request.goals {
        add_solver_goal(&mut solve_request, goal);
    }
    for constraint in &request.constraints {
        solve_request.add_constraint(solver_constraint(constraint));
    }
    Ok(solve_request)
}

fn add_solver_goal(request: &mut SolveRequest, goal: &Goal) {
    match goal {
        Goal::Equals { path, value } => {
            request.require_output(path.clone(), value.clone());
        }
        Goal::Absent { path } => {
            request.require_output_absent(path.clone());
        }
        Goal::Contains { path, value } => {
            request.require_output_contains(path.clone(), value.clone());
        }
        Goal::NotContains { path, value } => {
            request.require_output_not_contains(path.clone(), value.clone());
        }
    }
}

fn solver_goal(goal: &Goal) -> OutputGoal {
    match goal {
        Goal::Equals { path, value } => OutputGoal::Equals {
            path: path.clone(),
            value: value.clone(),
        },
        Goal::Absent { path } => OutputGoal::Absent { path: path.clone() },
        Goal::Contains { path, value } => OutputGoal::Contains {
            path: path.clone(),
            value: value.clone(),
        },
        Goal::NotContains { path, value } => OutputGoal::NotContains {
            path: path.clone(),
            value: value.clone(),
        },
    }
}

fn solver_constraint(constraint: &ExternalConstraint) -> OutputConstraint {
    match constraint {
        ExternalConstraint::Required(predicate) => {
            OutputConstraint::Required(solver_goal(predicate))
        }
        ExternalConstraint::Implies {
            condition,
            consequence,
        } => OutputConstraint::Implies {
            condition: solver_goal(condition),
            consequence: solver_goal(consequence),
        },
        ExternalConstraint::Conflicts { left, right } => OutputConstraint::Conflicts {
            left: solver_goal(left),
            right: solver_goal(right),
        },
    }
}

fn apply_solution(
    analysis: &ModuleAnalysis<'_, '_>,
    inputs: &EvaluationInputs,
    solution: &Solution,
) -> Result<EvaluationInputs, PlanError> {
    let mut candidate = inputs.clone();
    for change in &solution.variables {
        let value = constant_to_runtime(&change.after)?;
        match &change.source {
            VariableSource::Symbol(symbol) => candidate.override_tunable(*symbol, value),
            source => return Err(PlanError::UnsupportedVariableSource(source.clone())),
        }
    }
    for change in &solution.outputs {
        let origin = analysis
            .model()
            .path(&change.path)
            .map(|declaration| declaration.origin().clone())
            .expect("solver only returns declared output paths");
        candidate.override_output(
            change.path.clone(),
            change.after.as_ref().map(constant_to_runtime).transpose()?,
            origin,
        );
    }
    Ok(candidate)
}

fn exclude_solution(request: &mut SolveRequest, solution: &Solution) {
    let mut variable_values = request.variable_values().clone();
    for change in &solution.variables {
        variable_values.insert(change.variable, change.after.clone());
    }
    // Derived outputs are recomputed by the concrete evaluator.  Excluding
    // their baseline value would not exclude the candidate that actually
    // satisfied the symbolic goal.  Only direct output edits form part of the
    // candidate assignment; all tunable/input variable values are included
    // above, including unchanged ones.
    let mut output_values = HashMap::new();
    for change in &solution.outputs {
        output_values.insert(change.path.clone(), change.after.clone());
    }
    request.exclude_candidate(variable_values, output_values);
}

fn goal_is_satisfied(snapshot: &EvaluationSnapshot, goal: &Goal) -> bool {
    match goal {
        Goal::Equals { path, value } => snapshot.get(path).is_some_and(|entry| {
            constant_to_runtime(value).is_ok_and(|value| entry.value == value)
        }),
        Goal::Absent { path } => snapshot.get(path).is_none(),
        Goal::Contains { path, value } => snapshot.get(path).is_some_and(|entry| {
            let values = match &entry.value {
                RuntimeValue::List(values) | RuntimeValue::Set(values) => values,
                _ => return false,
            };
            constant_to_runtime(value).is_ok_and(|value| values.contains(&value))
        }),
        Goal::NotContains { path, value } => snapshot.get(path).is_some_and(|entry| {
            let values = match &entry.value {
                RuntimeValue::List(values) | RuntimeValue::Set(values) => values,
                _ => return false,
            };
            constant_to_runtime(value).is_ok_and(|value| !values.contains(&value))
        }),
    }
}

fn constraint_is_satisfied(snapshot: &EvaluationSnapshot, constraint: &ExternalConstraint) -> bool {
    match constraint {
        ExternalConstraint::Required(predicate) => goal_is_satisfied(snapshot, predicate),
        ExternalConstraint::Implies {
            condition,
            consequence,
        } => !goal_is_satisfied(snapshot, condition) || goal_is_satisfied(snapshot, consequence),
        ExternalConstraint::Conflicts { left, right } => {
            !(goal_is_satisfied(snapshot, left) && goal_is_satisfied(snapshot, right))
        }
    }
}

fn runtime_to_constant(value: &RuntimeValue) -> Option<Constant> {
    match value {
        RuntimeValue::Unit => Some(Constant::Unit),
        RuntimeValue::Bool(value) => Some(Constant::Bool(*value)),
        RuntimeValue::Int(value) => Some(Constant::Int(*value)),
        RuntimeValue::Float(value) => Some(Constant::Float(*value)),
        RuntimeValue::String(value) => Some(Constant::String(value.clone())),
        RuntimeValue::List(values) => values
            .iter()
            .map(runtime_to_constant)
            .collect::<Option<Vec<_>>>()
            .map(Constant::List),
        RuntimeValue::Set(values) => values
            .iter()
            .map(runtime_to_constant)
            .collect::<Option<Vec<_>>>()
            .map(Constant::Set),
        RuntimeValue::AttrSet(values) => values
            .iter()
            .map(|(key, value)| Some((key.clone(), runtime_to_constant(value)?)))
            .collect::<Option<_>>()
            .map(Constant::AttrSet),
        RuntimeValue::Optional(None) => Some(Constant::Optional(None)),
        RuntimeValue::Optional(Some(value)) => {
            runtime_to_constant(value).map(|value| Constant::Optional(Some(Box::new(value))))
        }
        RuntimeValue::Enum(variant) => Some(Constant::Enum(*variant)),
        RuntimeValue::Error
        | RuntimeValue::Record(_)
        | RuntimeValue::Function(_)
        | RuntimeValue::Closure(_)
        | RuntimeValue::Type(_)
        | RuntimeValue::Module(_) => None,
    }
}

fn constant_to_runtime(value: &Constant) -> Result<RuntimeValue, PlanError> {
    Ok(match value {
        Constant::Unit => RuntimeValue::Unit,
        Constant::Bool(value) => RuntimeValue::Bool(*value),
        Constant::Int(value) => RuntimeValue::Int(*value),
        Constant::Float(value) => RuntimeValue::Float(*value),
        Constant::String(value) => RuntimeValue::String(value.clone()),
        Constant::List(values) => RuntimeValue::List(
            values
                .iter()
                .map(constant_to_runtime)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Constant::Set(values) => RuntimeValue::Set(
            values
                .iter()
                .map(constant_to_runtime)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Constant::AttrSet(values) => RuntimeValue::AttrSet(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), constant_to_runtime(value)?)))
                .collect::<Result<_, PlanError>>()?,
        ),
        Constant::Optional(value) => {
            RuntimeValue::optional(value.as_deref().map(constant_to_runtime).transpose()?)
        }
        Constant::Enum(variant) => RuntimeValue::Enum(*variant),
        _ => return Err(PlanError::UnsupportedConstant),
    })
}
