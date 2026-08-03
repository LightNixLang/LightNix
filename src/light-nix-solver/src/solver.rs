use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

use light_nix_ir::{
    BinaryOperation, CallTarget, ClosureParameter, Constant, ConstraintModel, ExpressionId,
    ExpressionKind, FunctionMode, MutationPolicy, ObjectiveKind, OutputPath, OutputPathSegment,
    SourceOrigin, UnaryOperation, VariableId, VariableKind,
};
use light_nix_name_resolver::{ModuleId, VariantId};
use light_nix_type_checker::{BuiltinMethod, Type};
use z3::{
    Model, Optimize, Params, SatResult,
    ast::{Bool, Int, Real, String as Z3String},
};

use crate::{
    OpaqueImpact, OutputChange, OutputConstraint, OutputGoal, Solution, SolveError, SolveErrorKind,
    SolveOutcome, SolveRequest, UnknownReason, VariableChange,
};

pub fn solve(model: &ConstraintModel, request: &SolveRequest) -> Result<SolveOutcome, SolveError> {
    let mut encoder = Encoder::new(model, request);
    encoder.encode()
}

#[derive(Clone)]
enum EncodedValue {
    Unit,
    Bool(Bool),
    Int(Int),
    Real(Real),
    String(Z3String),
    Package(Z3String),
    Enum(Int),
    List {
        element_type: Type,
        elements: Vec<EncodedListElement>,
    },
    Set {
        element_type: Type,
        universe: Vec<Constant>,
        members: Vec<Bool>,
    },
    Optional {
        present: Bool,
        value: Box<EncodedValue>,
    },
    Union {
        alternatives: Vec<Type>,
        selected: Int,
        values: Vec<EncodedValue>,
    },
    Closure {
        mode: FunctionMode,
        parameters: Vec<ClosureParameter>,
        body: ExpressionId,
        ty: Type,
    },
}

#[derive(Clone)]
struct EncodedListElement {
    present: Bool,
    value: EncodedValue,
}

#[derive(Clone)]
struct VariableState {
    edit: Bool,
    base: EncodedValue,
    actual: EncodedValue,
}

#[derive(Clone)]
struct OutputState {
    derived_present: Bool,
    derived_value: EncodedValue,
    actual_present: Bool,
    actual_value: EncodedValue,
    edit: Option<Bool>,
    cost: u64,
}

#[derive(Clone)]
struct OpaqueBoundaryState {
    boundary: ExpressionId,
    origin: Option<SourceOrigin>,
    variables: HashSet<VariableId>,
}

struct Encoder<'a> {
    source: &'a ConstraintModel,
    request: &'a SolveRequest,
    optimize: Optimize,
    universes: Vec<(Type, Vec<Constant>)>,
    enum_universes: Vec<(Type, Vec<VariantId>)>,
    expressions: HashMap<ExpressionId, EncodedValue>,
    variables: HashMap<VariableId, EncodedValue>,
    variable_states: HashMap<VariableId, VariableState>,
    outputs: HashMap<OutputPath, OutputState>,
    visiting_expressions: HashSet<ExpressionId>,
    visiting_variables: HashSet<VariableId>,
    visiting_outputs: HashSet<OutputPath>,
    parameter_bindings: Vec<HashMap<light_nix_name_resolver::SymbolId, EncodedValue>>,
    opaque_boundaries: Vec<OpaqueBoundaryState>,
    next_opaque_value: u32,
}

impl<'a> Encoder<'a> {
    fn new(source: &'a ConstraintModel, request: &'a SolveRequest) -> Self {
        Self {
            source,
            request,
            optimize: Optimize::new(),
            universes: collect_universes(source, request),
            enum_universes: collect_enum_universes(source, request),
            expressions: HashMap::new(),
            variables: HashMap::new(),
            variable_states: HashMap::new(),
            outputs: HashMap::new(),
            visiting_expressions: HashSet::new(),
            visiting_variables: HashSet::new(),
            visiting_outputs: HashSet::new(),
            parameter_bindings: Vec::new(),
            opaque_boundaries: Vec::new(),
            next_opaque_value: 0,
        }
    }

    fn encode(&mut self) -> Result<SolveOutcome, SolveError> {
        if let Some(timeout) = self.request.timeout() {
            let mut params = Params::new();
            let milliseconds = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
            params.set_u32("timeout", milliseconds);
            self.optimize.set_params(&params);
        }

        for variable in self.source.variables() {
            if matches!(variable.kind(), VariableKind::Tunable { .. }) {
                self.encode_variable(variable.id())?;
            }
        }
        for path in self
            .source
            .paths()
            .map(|declaration| declaration.path().clone())
            .collect::<Vec<_>>()
        {
            self.encode_output(&path)?;
        }
        for constraint in self.source.constraints() {
            let condition = self.encode_expression(constraint.condition())?;
            let condition = self.expect_bool(condition)?;
            self.optimize.assert(condition);
        }
        for goal in self.request.goals() {
            self.encode_goal(goal)?;
        }
        for constraint in self.request.constraints() {
            self.encode_external_constraint(constraint)?;
        }
        self.encode_exclusions()?;

        let costs = self.change_costs()?;
        let total_cost = if costs.is_empty() {
            Int::from_i64(0)
        } else {
            Int::add(&costs)
        };
        self.optimize.minimize(&total_cost);
        for objective in self.source.objectives() {
            if let ObjectiveKind::Minimize(expression) = objective.kind() {
                match self.encode_expression(*expression)? {
                    EncodedValue::Int(value) => self.optimize.minimize(&value),
                    EncodedValue::Real(value) => self.optimize.minimize(&value),
                    other => {
                        return Err(self.type_error(&Type::Int, encoded_type(&other)));
                    }
                }
            }
        }
        let distances = self.change_distances()?;
        let total_distance = if distances.is_empty() {
            Int::from_i64(0)
        } else {
            Int::add(&distances)
        };
        self.optimize.minimize(&total_distance);

        match self.optimize.check(&[]) {
            SatResult::Unsat => Ok(SolveOutcome::Unsat),
            SatResult::Unknown => Ok(SolveOutcome::Unknown(UnknownReason(
                self.optimize
                    .get_reason_unknown()
                    .unwrap_or_else(|| "unknown".to_owned()),
            ))),
            SatResult::Sat => {
                let model = self
                    .optimize
                    .get_model()
                    .ok_or_else(|| error(SolveErrorKind::ModelValueUnavailable))?;
                Ok(SolveOutcome::Sat(self.decode_solution(&model)?))
            }
        }
    }

    fn encode_goal(&mut self, goal: &OutputGoal) -> Result<(), SolveError> {
        let predicate = self.encode_output_predicate(goal)?;
        self.optimize.assert(predicate);
        Ok(())
    }

    fn encode_external_constraint(
        &mut self,
        constraint: &OutputConstraint,
    ) -> Result<(), SolveError> {
        let condition = match constraint {
            OutputConstraint::Required(predicate) => self.encode_output_predicate(predicate)?,
            OutputConstraint::Implies {
                condition,
                consequence,
            } => {
                let condition = self.encode_output_predicate(condition)?;
                let consequence = self.encode_output_predicate(consequence)?;
                condition.implies(&consequence)
            }
            OutputConstraint::Conflicts { left, right } => {
                let left = self.encode_output_predicate(left)?;
                let right = self.encode_output_predicate(right)?;
                Bool::and(&[left, right]).not()
            }
        };
        self.optimize.assert(condition);
        Ok(())
    }

    fn encode_output_predicate(&mut self, goal: &OutputGoal) -> Result<Bool, SolveError> {
        Ok(match goal {
            OutputGoal::Equals { path, value } => {
                let (output_type, _) = self.output_info(path)?;
                let output = self.encode_output(path)?;
                let expected = self.encode_constant(&output_type, value)?;
                Bool::and(&[
                    output.actual_present,
                    self.value_eq(&output.actual_value, &expected)?,
                ])
            }
            OutputGoal::Absent { path } => {
                self.output_info(path)?;
                let output = self.encode_output(path)?;
                output.actual_present.not()
            }
            OutputGoal::Contains { path, value } | OutputGoal::NotContains { path, value } => {
                let (output_type, _) = self.output_info(path)?;
                let element_type = match &output_type {
                    Type::List(element_type) | Type::Set(element_type) => {
                        element_type.as_ref().clone()
                    }
                    found => {
                        return Err(
                            self.type_error(&Type::Set(Box::new(Type::Error)), found.clone())
                        );
                    }
                };
                let output = self.encode_output(path)?;
                let member =
                    self.collection_contains(&output.actual_value, &element_type, value)?;
                if matches!(goal, OutputGoal::Contains { .. }) {
                    Bool::and(&[output.actual_present, member])
                } else {
                    Bool::and(&[output.actual_present, member.not()])
                }
            }
        })
    }

    fn encode_exclusions(&mut self) -> Result<(), SolveError> {
        for exclusion in self.request.excluded_candidates() {
            let mut differences = Vec::new();
            for (variable, value) in &exclusion.variable_values {
                let declaration = self
                    .source
                    .variable(*variable)
                    .ok_or_else(|| error(SolveErrorKind::UnknownVariable(*variable)))?;
                let actual = self.encode_variable(*variable)?;
                let excluded = self.encode_constant(declaration.ty(), value)?;
                differences.push(self.value_eq(&actual, &excluded)?.not());
            }
            for (path, value) in &exclusion.output_values {
                let (output_type, _) = self.output_info(path)?;
                let output = self.encode_output(path)?;
                match value {
                    Some(value) => {
                        let excluded = self.encode_constant(&output_type, value)?;
                        let same = Bool::and(&[
                            output.actual_present,
                            self.value_eq(&output.actual_value, &excluded)?,
                        ]);
                        differences.push(same.not());
                    }
                    None => differences.push(output.actual_present),
                }
            }
            self.optimize.assert(bool_or(differences));
        }
        Ok(())
    }

    fn collection_contains(
        &self,
        collection: &EncodedValue,
        element_type: &Type,
        value: &Constant,
    ) -> Result<Bool, SolveError> {
        let value = self.encode_constant(element_type, value)?;
        let alternatives = match collection {
            EncodedValue::List { elements, .. } => elements
                .iter()
                .map(|element| {
                    let equal = self.value_eq(&value, &element.value)?;
                    Ok(Bool::and(&[element.present.clone(), equal]))
                })
                .collect::<Result<Vec<_>, SolveError>>()?,
            EncodedValue::Set {
                universe, members, ..
            } => universe
                .iter()
                .zip(members)
                .map(|(candidate, member)| {
                    let candidate = self.encode_constant(element_type, candidate)?;
                    let equal = self.value_eq(&value, &candidate)?;
                    Ok(Bool::and(&[member.clone(), equal]))
                })
                .collect::<Result<Vec<_>, SolveError>>()?,
            other => {
                return Err(self.type_error(
                    &Type::Set(Box::new(element_type.clone())),
                    encoded_type(other),
                ));
            }
        };
        Ok(bool_or(alternatives))
    }

    fn change_costs(&self) -> Result<Vec<Int>, SolveError> {
        let variable_costs = self.variable_costs();

        let mut costs = Vec::new();
        for (variable, cost) in variable_costs {
            let state = self
                .variable_states
                .get(&variable)
                .ok_or_else(|| error(SolveErrorKind::UnknownVariable(variable)))?;
            costs.push(state.edit.ite(&Int::from_u64(cost), &Int::from_i64(0)));
        }
        for state in self.outputs.values() {
            if let Some(edit) = &state.edit {
                costs.push(edit.ite(&Int::from_u64(state.cost), &Int::from_i64(0)));
            }
        }
        Ok(costs)
    }

    fn variable_costs(&self) -> HashMap<VariableId, u64> {
        let mut costs = self
            .source
            .variables()
            .filter_map(|variable| match variable.kind() {
                VariableKind::Input => None,
                VariableKind::Tunable { cost } => Some((variable.id(), cost)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        for objective in self.source.objectives() {
            if let ObjectiveKind::MinimizeChanges(variables) = objective.kind() {
                for variable in variables {
                    costs.insert(variable.variable(), variable.cost());
                }
            }
        }
        costs
    }

    fn change_distances(&self) -> Result<Vec<Int>, SolveError> {
        let mut distances = Vec::new();
        for state in self.variable_states.values() {
            let distance = self.value_distance(&state.actual, &state.base)?;
            distances.push(state.edit.ite(&distance, &Int::from_i64(0)));
        }
        for state in self.outputs.values() {
            let Some(edit) = &state.edit else {
                continue;
            };
            let presence_distance = state
                .actual_present
                .eq(&state.derived_present)
                .ite(&Int::from_i64(0), &Int::from_i64(1));
            let value_distance = self.value_distance(&state.actual_value, &state.derived_value)?;
            let both_present =
                Bool::and(&[state.actual_present.clone(), state.derived_present.clone()]);
            let distance = Int::add(&[
                presence_distance,
                both_present.ite(&value_distance, &Int::from_i64(0)),
            ]);
            distances.push(edit.ite(&distance, &Int::from_i64(0)));
        }
        Ok(distances)
    }

    fn encode_expression(&mut self, id: ExpressionId) -> Result<EncodedValue, SolveError> {
        let cacheable = self.parameter_bindings.is_empty();
        if cacheable && let Some(value) = self.expressions.get(&id) {
            return Ok(value.clone());
        }
        if !self.visiting_expressions.insert(id) {
            return Err(error(SolveErrorKind::CyclicExpression(id)));
        }
        let expression = self
            .source
            .expression(id)
            .ok_or_else(|| error(SolveErrorKind::UnknownExpression(id)))?;
        let result = match expression.kind() {
            ExpressionKind::Unreachable => {
                self.fresh_value(expression.ty(), &format!("e{}_never", id.0))
            }
            ExpressionKind::Constant(value) => self.encode_constant(expression.ty(), value),
            ExpressionKind::Variable(variable) => self.encode_variable(*variable),
            ExpressionKind::Parameter(symbol) => self
                .parameter_bindings
                .iter()
                .rev()
                .find_map(|bindings| bindings.get(symbol).cloned())
                .ok_or_else(|| error(SolveErrorKind::UnsupportedExpression(id))),
            ExpressionKind::Closure {
                mode,
                parameters,
                body,
            } => Ok(EncodedValue::Closure {
                mode: *mode,
                parameters: parameters.clone(),
                body: *body,
                ty: expression.ty().clone(),
            }),
            ExpressionKind::Output(path) => {
                let output = self.encode_output(path)?;
                self.optimize.assert(&output.actual_present);
                Ok(output.actual_value)
            }
            ExpressionKind::List(values) => self.encode_list_expression(expression.ty(), values),
            ExpressionKind::Set(values) => self.encode_set_expression(expression.ty(), values),
            ExpressionKind::Some(value) => Ok(EncodedValue::Optional {
                present: Bool::from_bool(true),
                value: Box::new(self.encode_expression(*value)?),
            }),
            ExpressionKind::Null => {
                let Type::Optional(inner) = expression.ty() else {
                    return Err(self.type_error(
                        &Type::Optional(Box::new(Type::Error)),
                        expression.ty().clone(),
                    ));
                };
                Ok(EncodedValue::Optional {
                    present: Bool::from_bool(false),
                    value: Box::new(self.default_value(inner)?),
                })
            }
            ExpressionKind::OptionalValue(optional) => {
                let optional = self.encode_expression(*optional)?;
                let EncodedValue::Optional { present, value } = optional else {
                    return Err(self.type_error(
                        &Type::Optional(Box::new(Type::Error)),
                        encoded_type(&optional),
                    ));
                };
                self.optimize.assert(present);
                Ok(*value)
            }
            ExpressionKind::UnionInject { value, union } => {
                let value = self.encode_expression(*value)?;
                self.inject_union(value, union)
            }
            ExpressionKind::UnionProject { value, target } => {
                let value = self.encode_expression(*value)?;
                self.project_union(value, target)
            }
            ExpressionKind::TypeIs { value, target } => {
                let value = self.encode_expression(*value)?;
                self.encode_type_is(value, target).map(EncodedValue::Bool)
            }
            ExpressionKind::SafeCast { value, target } => {
                let value = self.encode_expression(*value)?;
                self.encode_safe_cast(value, target)
            }
            ExpressionKind::Unary { operation, operand } => {
                let operand = self.encode_expression(*operand)?;
                self.encode_unary(*operation, operand)
            }
            ExpressionKind::Binary {
                operation,
                left,
                right,
            } => {
                let left = self.encode_expression(*left)?;
                let right = self.encode_expression(*right)?;
                self.encode_binary(*operation, left, right)
            }
            ExpressionKind::If {
                condition,
                then_value,
                else_value,
            } => {
                let condition = self.encode_expression(*condition)?;
                let condition = self.expect_bool(condition)?;
                let then_value = self.encode_expression(*then_value)?;
                let else_value = self.encode_expression(*else_value)?;
                self.value_ite(&condition, &then_value, &else_value)
            }
            ExpressionKind::Elvis { optional, fallback } => {
                let optional = self.encode_expression(*optional)?;
                let EncodedValue::Optional { present, value } = optional else {
                    return Err(self.type_error(
                        &Type::Optional(Box::new(Type::Error)),
                        encoded_type(&optional),
                    ));
                };
                let fallback = self.encode_expression(*fallback)?;
                self.value_ite(&present, &value, &fallback)
            }
            ExpressionKind::Call {
                target,
                receiver,
                arguments,
            } => self.encode_call(id, expression.ty(), *target, *receiver, arguments),
            ExpressionKind::Function(_)
            | ExpressionKind::Field { .. }
            | ExpressionKind::AttrSetKey { .. } => {
                Err(error(SolveErrorKind::UnsupportedExpression(id)))
            }
            _ => Err(error(SolveErrorKind::UnsupportedExpression(id))),
        };
        self.visiting_expressions.remove(&id);
        let value = result?;
        if cacheable {
            self.expressions.insert(id, value.clone());
        }
        Ok(value)
    }

    fn encode_variable(&mut self, id: VariableId) -> Result<EncodedValue, SolveError> {
        if let Some(value) = self.variables.get(&id) {
            return Ok(value.clone());
        }
        if !self.visiting_variables.insert(id) {
            return Err(error(SolveErrorKind::CyclicVariable(id)));
        }
        let variable = self
            .source
            .variable(id)
            .ok_or_else(|| error(SolveErrorKind::UnknownVariable(id)))?;
        let value = match variable.kind() {
            VariableKind::Input => {
                let current = self
                    .request
                    .variable_values()
                    .get(&id)
                    .ok_or_else(|| error(SolveErrorKind::MissingInput(id)))?;
                self.encode_constant(variable.ty(), current)?
            }
            VariableKind::Tunable { .. } => {
                let base = if let Some(initial) = variable.initial() {
                    self.encode_expression(initial)?
                } else {
                    let current = self
                        .request
                        .variable_values()
                        .get(&id)
                        .ok_or_else(|| error(SolveErrorKind::MissingInitialValue(id)))?;
                    self.encode_constant(variable.ty(), current)?
                };
                let edit = Bool::new_const(format!("variable_{}_edit", id.0));
                let candidate =
                    self.fresh_value(variable.ty(), &format!("variable_{}_candidate", id.0))?;
                let changed = self.value_eq(&candidate, &base)?.not();
                self.optimize.assert(edit.implies(&changed));
                let actual = self.value_ite(&edit, &candidate, &base)?;
                self.variable_states.insert(
                    id,
                    VariableState {
                        edit,
                        base,
                        actual: actual.clone(),
                    },
                );
                actual
            }
            _ => {
                return Err(error(SolveErrorKind::UnsupportedType(
                    variable.ty().clone(),
                )));
            }
        };
        self.visiting_variables.remove(&id);
        self.variables.insert(id, value.clone());
        Ok(value)
    }

    /// The declared type and mutation policy of an output path, answered from
    /// the lowered model or, for virtual claims, from the request.
    fn output_info(&self, path: &OutputPath) -> Result<(Type, MutationPolicy), SolveError> {
        if let Some(declaration) = self.source.path(path) {
            return Ok((declaration.ty().clone(), declaration.policy()));
        }
        if let Some(virtual_output) = self.request.virtual_outputs().get(path) {
            return Ok((virtual_output.ty.clone(), virtual_output.policy));
        }
        Err(error(SolveErrorKind::UnknownOutput(path.clone())))
    }

    fn encode_output(&mut self, path: &OutputPath) -> Result<OutputState, SolveError> {
        if let Some(output) = self.outputs.get(path) {
            return Ok(output.clone());
        }
        if !self.visiting_outputs.insert(path.clone()) {
            return Err(error(SolveErrorKind::CyclicOutput(path.clone())));
        }
        let (output_type, policy) = self.output_info(path)?;
        let definition = self.source.output(path);

        let (derived_present, derived_value) = if let Some(definition) = definition {
            let mut guards = Vec::new();
            let mut values = Vec::new();
            for case in definition.cases() {
                let guard = self.encode_expression(case.guard())?;
                guards.push(self.expect_bool(guard)?);
                values.push(self.encode_expression(case.value())?);
            }
            for left in 0..guards.len() {
                for right in left + 1..guards.len() {
                    self.optimize
                        .assert(Bool::and(&[guards[left].clone(), guards[right].clone()]).not());
                }
            }
            let present = if guards.is_empty() {
                Bool::from_bool(false)
            } else {
                Bool::or(&guards)
            };
            let mut value = self.default_value(&output_type)?;
            for (guard, candidate) in guards.iter().zip(values.iter()).rev() {
                value = self.value_ite(guard, candidate, &value)?;
            }
            (present, value)
        } else if let Some(current) = self.request.output_values().get(path) {
            match current {
                Some(value) => (
                    Bool::from_bool(true),
                    self.encode_constant(&output_type, value)?,
                ),
                None => (Bool::from_bool(false), self.default_value(&output_type)?),
            }
        } else {
            (Bool::from_bool(false), self.default_value(&output_type)?)
        };

        let state = match policy {
            MutationPolicy::Readonly => OutputState {
                derived_present: derived_present.clone(),
                derived_value: derived_value.clone(),
                actual_present: derived_present,
                actual_value: derived_value,
                edit: None,
                cost: 0,
            },
            MutationPolicy::Tunable { cost } => {
                let edit = Bool::new_const(output_name(path, "edit"));
                let candidate_present = Bool::new_const(output_name(path, "present"));
                let candidate_value =
                    self.fresh_value(&output_type, &output_name(path, "candidate"))?;
                let presence_changed = candidate_present.eq(&derived_present).not();
                let both_present = Bool::and(&[candidate_present.clone(), derived_present.clone()]);
                let value_changed = self.value_eq(&candidate_value, &derived_value)?.not();
                let changed =
                    Bool::or(&[presence_changed, Bool::and(&[both_present, value_changed])]);
                self.optimize.assert(edit.implies(&changed));
                OutputState {
                    derived_present: derived_present.clone(),
                    derived_value: derived_value.clone(),
                    actual_present: edit.ite(&candidate_present, &derived_present),
                    actual_value: self.value_ite(&edit, &candidate_value, &derived_value)?,
                    edit: Some(edit),
                    cost,
                }
            }
            _ => {
                return Err(error(SolveErrorKind::UnsupportedType(output_type)));
            }
        };
        self.visiting_outputs.remove(path);
        self.outputs.insert(path.clone(), state.clone());
        Ok(state)
    }

    fn encode_list_expression(
        &mut self,
        ty: &Type,
        values: &[ExpressionId],
    ) -> Result<EncodedValue, SolveError> {
        let Type::List(element_type) = ty else {
            return Err(self.type_error(&Type::List(Box::new(Type::Error)), ty.clone()));
        };
        let elements = values
            .iter()
            .map(|value| {
                Ok(EncodedListElement {
                    present: Bool::from_bool(true),
                    value: self.encode_expression(*value)?,
                })
            })
            .collect::<Result<Vec<_>, SolveError>>()?;
        Ok(EncodedValue::List {
            element_type: element_type.as_ref().clone(),
            elements,
        })
    }

    fn encode_set_expression(
        &mut self,
        ty: &Type,
        values: &[ExpressionId],
    ) -> Result<EncodedValue, SolveError> {
        let Type::Set(element_type) = ty else {
            return Err(self.type_error(&Type::Set(Box::new(Type::Error)), ty.clone()));
        };
        let universe = self.universe(element_type);
        let encoded = values
            .iter()
            .map(|value| self.encode_expression(*value))
            .collect::<Result<Vec<_>, _>>()?;
        for value in &encoded {
            let alternatives = universe
                .iter()
                .map(|candidate| {
                    let candidate = self.encode_constant(element_type, candidate)?;
                    self.value_eq(value, &candidate)
                })
                .collect::<Result<Vec<_>, SolveError>>()?;
            self.optimize.assert(bool_or(alternatives));
        }
        let members = universe
            .iter()
            .map(|candidate| {
                let candidate = self.encode_constant(element_type, candidate)?;
                let equalities = encoded
                    .iter()
                    .map(|value| self.value_eq(value, &candidate))
                    .collect::<Result<Vec<_>, SolveError>>()?;
                Ok(bool_or(equalities))
            })
            .collect::<Result<Vec<_>, SolveError>>()?;
        Ok(EncodedValue::Set {
            element_type: element_type.as_ref().clone(),
            universe,
            members,
        })
    }

    fn encode_call(
        &mut self,
        call: ExpressionId,
        result_type: &Type,
        target: CallTarget,
        receiver: Option<ExpressionId>,
        arguments: &[ExpressionId],
    ) -> Result<EncodedValue, SolveError> {
        let CallTarget::Builtin(method) = target else {
            return Err(error(SolveErrorKind::UnsupportedExpression(
                receiver.unwrap_or(ExpressionId(u32::MAX)),
            )));
        };
        let receiver_id =
            receiver.ok_or_else(|| error(SolveErrorKind::UnsupportedBuiltin(method)))?;
        let receiver = self.encode_expression(receiver_id)?;
        match method {
            BuiltinMethod::Contains if arguments.len() == 1 => {
                let argument = self.encode_expression(arguments[0])?;
                let alternatives = match receiver {
                    EncodedValue::List { elements, .. } => elements
                        .into_iter()
                        .map(|element| {
                            let equal = self.value_eq(&argument, &element.value)?;
                            Ok(Bool::and(&[element.present, equal]))
                        })
                        .collect::<Result<Vec<_>, SolveError>>()?,
                    EncodedValue::Set {
                        element_type,
                        universe,
                        members,
                    } => universe
                        .iter()
                        .zip(members)
                        .map(|(candidate, member)| {
                            let candidate = self.encode_constant(&element_type, candidate)?;
                            let equal = self.value_eq(&argument, &candidate)?;
                            Ok(Bool::and(&[member, equal]))
                        })
                        .collect::<Result<Vec<_>, SolveError>>()?,
                    _ => return Err(error(SolveErrorKind::UnsupportedBuiltin(method))),
                };
                Ok(EncodedValue::Bool(bool_or(alternatives)))
            }
            BuiltinMethod::Filter if arguments.len() == 1 => {
                let closure = self.encode_expression(arguments[0])?;
                self.encode_filter(call, receiver_id, receiver, closure)
            }
            BuiltinMethod::Map if arguments.len() == 1 => {
                let closure = self.encode_expression(arguments[0])?;
                self.encode_map(call, receiver_id, receiver, closure, result_type)
            }
            BuiltinMethod::ToFloat if arguments.is_empty() => {
                let EncodedValue::Int(value) = receiver else {
                    return Err(error(SolveErrorKind::UnsupportedBuiltin(method)));
                };
                Ok(EncodedValue::Real(value.to_real()))
            }
            BuiltinMethod::TryToInt if arguments.is_empty() => {
                let EncodedValue::Real(value) = receiver else {
                    return Err(error(SolveErrorKind::UnsupportedBuiltin(method)));
                };
                Ok(EncodedValue::Optional {
                    present: value.is_int(),
                    value: Box::new(EncodedValue::Int(value.to_int())),
                })
            }
            BuiltinMethod::ToString if arguments.is_empty() => match receiver {
                EncodedValue::String(value) => Ok(EncodedValue::String(value)),
                EncodedValue::Bool(value) => Ok(EncodedValue::String(value.ite(
                    &Z3String::from_str("true").expect("valid Z3 string"),
                    &Z3String::from_str("false").expect("valid Z3 string"),
                ))),
                _ => Err(error(SolveErrorKind::UnsupportedBuiltin(method))),
            },
            _ => Err(error(SolveErrorKind::UnsupportedBuiltin(method))),
        }
    }

    fn encode_filter(
        &mut self,
        call: ExpressionId,
        receiver_id: ExpressionId,
        receiver: EncodedValue,
        closure: EncodedValue,
    ) -> Result<EncodedValue, SolveError> {
        let dependencies = self.opaque_dependencies(call, receiver_id, &closure);
        match receiver {
            EncodedValue::List {
                element_type,
                elements,
            } => {
                let mut filtered = Vec::with_capacity(elements.len());
                for (index, element) in elements.into_iter().enumerate() {
                    let predicate = self.apply_closure(
                        call,
                        &closure,
                        vec![element.value.clone()],
                        &dependencies,
                        index,
                    )?;
                    let predicate = self.expect_bool(predicate)?;
                    filtered.push(EncodedListElement {
                        present: Bool::and(&[element.present, predicate]),
                        value: element.value,
                    });
                }
                Ok(EncodedValue::List {
                    element_type,
                    elements: filtered,
                })
            }
            EncodedValue::Set {
                element_type,
                universe,
                members,
            } => {
                let mut filtered = Vec::with_capacity(universe.len());
                for (index, (candidate, member)) in universe.iter().zip(members).enumerate() {
                    let candidate = self.encode_constant(&element_type, candidate)?;
                    let predicate =
                        self.apply_closure(call, &closure, vec![candidate], &dependencies, index)?;
                    let predicate = self.expect_bool(predicate)?;
                    filtered.push(Bool::and(&[member, predicate]));
                }
                Ok(EncodedValue::Set {
                    element_type,
                    universe,
                    members: filtered,
                })
            }
            _ => Err(error(SolveErrorKind::UnsupportedBuiltin(
                BuiltinMethod::Filter,
            ))),
        }
    }

    fn encode_map(
        &mut self,
        call: ExpressionId,
        receiver_id: ExpressionId,
        receiver: EncodedValue,
        closure: EncodedValue,
        result_type: &Type,
    ) -> Result<EncodedValue, SolveError> {
        if let (EncodedValue::List { elements, .. }, Type::List(result_element)) =
            (&receiver, result_type)
        {
            let dependencies = self.opaque_dependencies(call, receiver_id, &closure);
            let mut mapped = Vec::with_capacity(elements.len());
            for (index, element) in elements.iter().enumerate() {
                mapped.push(EncodedListElement {
                    present: element.present.clone(),
                    value: self.apply_closure(
                        call,
                        &closure,
                        vec![element.value.clone()],
                        &dependencies,
                        index,
                    )?,
                });
            }
            return Ok(EncodedValue::List {
                element_type: result_element.as_ref().clone(),
                elements: mapped,
            });
        }

        let EncodedValue::Set {
            element_type,
            universe: input_universe,
            members: input_members,
        } = receiver
        else {
            return Err(error(SolveErrorKind::UnsupportedBuiltin(
                BuiltinMethod::Map,
            )));
        };
        let Type::Set(result_element) = result_type else {
            return Err(error(SolveErrorKind::UnsupportedBuiltin(
                BuiltinMethod::Map,
            )));
        };
        let result_universe = self.universe(result_element);
        let dependencies = self.opaque_dependencies(call, receiver_id, &closure);
        let mut mapped_values = Vec::with_capacity(input_universe.len());
        for (index, (candidate, member)) in input_universe.iter().zip(&input_members).enumerate() {
            let candidate = self.encode_constant(&element_type, candidate)?;
            let mapped =
                self.apply_closure(call, &closure, vec![candidate], &dependencies, index)?;
            let alternatives = result_universe
                .iter()
                .map(|candidate| {
                    let candidate = self.encode_constant(result_element, candidate)?;
                    self.value_eq(&mapped, &candidate)
                })
                .collect::<Result<Vec<_>, SolveError>>()?;
            self.optimize.assert(member.implies(bool_or(alternatives)));
            mapped_values.push(mapped);
        }
        let result_members = result_universe
            .iter()
            .map(|candidate| {
                let candidate = self.encode_constant(result_element, candidate)?;
                let sources = input_members
                    .iter()
                    .zip(&mapped_values)
                    .map(|(member, mapped)| {
                        let equal = self.value_eq(mapped, &candidate)?;
                        Ok(Bool::and(&[member.clone(), equal]))
                    })
                    .collect::<Result<Vec<_>, SolveError>>()?;
                Ok(bool_or(sources))
            })
            .collect::<Result<Vec<_>, SolveError>>()?;
        Ok(EncodedValue::Set {
            element_type: result_element.as_ref().clone(),
            universe: result_universe,
            members: result_members,
        })
    }

    fn apply_closure(
        &mut self,
        call: ExpressionId,
        closure: &EncodedValue,
        arguments: Vec<EncodedValue>,
        dependencies: &HashSet<VariableId>,
        application: usize,
    ) -> Result<EncodedValue, SolveError> {
        let EncodedValue::Closure {
            mode,
            parameters,
            body,
            ty,
            ..
        } = closure
        else {
            return Err(error(SolveErrorKind::UnsupportedExpression(call)));
        };
        if parameters.len() != arguments.len() {
            return Err(error(SolveErrorKind::UnsupportedExpression(call)));
        }
        match mode {
            FunctionMode::Inline => {
                let bindings = parameters
                    .iter()
                    .zip(arguments)
                    .map(|(parameter, value)| (parameter.symbol(), value))
                    .collect();
                self.parameter_bindings.push(bindings);
                let result = self.encode_expression(*body);
                self.parameter_bindings.pop();
                result
            }
            FunctionMode::Opaque => {
                self.record_opaque_boundary(call, dependencies.clone());
                let Type::Function(function) = ty else {
                    return Err(error(SolveErrorKind::UnsupportedExpression(call)));
                };
                let name = format!(
                    "opaque_{}_{}_{}",
                    call.0, application, self.next_opaque_value
                );
                self.next_opaque_value = self.next_opaque_value.wrapping_add(1);
                self.fresh_value(&function.return_type, &name)
            }
            _ => Err(error(SolveErrorKind::UnsupportedExpression(call))),
        }
    }

    fn opaque_dependencies(
        &self,
        call: ExpressionId,
        receiver: ExpressionId,
        closure: &EncodedValue,
    ) -> HashSet<VariableId> {
        let mut dependencies = HashSet::new();
        let mut visiting = HashSet::new();
        self.collect_expression_variables(receiver, &mut dependencies, &mut visiting);
        if let EncodedValue::Closure { body, .. } = closure {
            self.collect_expression_variables(*body, &mut dependencies, &mut visiting);
        } else {
            self.collect_expression_variables(call, &mut dependencies, &mut visiting);
        }
        dependencies
    }

    fn collect_expression_variables(
        &self,
        expression: ExpressionId,
        dependencies: &mut HashSet<VariableId>,
        visiting: &mut HashSet<ExpressionId>,
    ) {
        if !visiting.insert(expression) {
            return;
        }
        let Some(expression) = self.source.expression(expression) else {
            return;
        };
        match expression.kind() {
            ExpressionKind::Variable(variable) => {
                dependencies.insert(*variable);
            }
            ExpressionKind::List(values) | ExpressionKind::Set(values) => {
                for value in values {
                    self.collect_expression_variables(*value, dependencies, visiting);
                }
            }
            ExpressionKind::Some(value)
            | ExpressionKind::OptionalValue(value)
            | ExpressionKind::UnionInject { value, .. }
            | ExpressionKind::UnionProject { value, .. }
            | ExpressionKind::TypeIs { value, .. }
            | ExpressionKind::SafeCast { value, .. }
            | ExpressionKind::Unary { operand: value, .. } => {
                self.collect_expression_variables(*value, dependencies, visiting);
            }
            ExpressionKind::Closure { body, .. } => {
                self.collect_expression_variables(*body, dependencies, visiting);
            }
            ExpressionKind::Binary { left, right, .. } => {
                self.collect_expression_variables(*left, dependencies, visiting);
                self.collect_expression_variables(*right, dependencies, visiting);
            }
            ExpressionKind::If {
                condition,
                then_value,
                else_value,
            } => {
                self.collect_expression_variables(*condition, dependencies, visiting);
                self.collect_expression_variables(*then_value, dependencies, visiting);
                self.collect_expression_variables(*else_value, dependencies, visiting);
            }
            ExpressionKind::Elvis { optional, fallback } => {
                self.collect_expression_variables(*optional, dependencies, visiting);
                self.collect_expression_variables(*fallback, dependencies, visiting);
            }
            ExpressionKind::Field { receiver, .. } => {
                self.collect_expression_variables(*receiver, dependencies, visiting);
            }
            ExpressionKind::Call {
                receiver,
                arguments,
                ..
            } => {
                if let Some(receiver) = receiver {
                    self.collect_expression_variables(*receiver, dependencies, visiting);
                }
                for argument in arguments {
                    self.collect_expression_variables(*argument, dependencies, visiting);
                }
            }
            _ => {}
        }
        visiting.remove(&expression.id());
    }

    fn record_opaque_boundary(&mut self, boundary: ExpressionId, variables: HashSet<VariableId>) {
        if let Some(existing) = self
            .opaque_boundaries
            .iter_mut()
            .find(|candidate| candidate.boundary == boundary)
        {
            existing.variables.extend(variables);
            return;
        }
        let origin = self
            .source
            .expression(boundary)
            .and_then(|expression| expression.origin().cloned());
        self.opaque_boundaries.push(OpaqueBoundaryState {
            boundary,
            origin,
            variables,
        });
    }

    fn encode_unary(
        &self,
        operation: UnaryOperation,
        operand: EncodedValue,
    ) -> Result<EncodedValue, SolveError> {
        match (operation, operand) {
            (UnaryOperation::Positive, value @ (EncodedValue::Int(_) | EncodedValue::Real(_))) => {
                Ok(value)
            }
            (UnaryOperation::Negate, EncodedValue::Int(value)) => {
                Ok(EncodedValue::Int(value.unary_minus()))
            }
            (UnaryOperation::Negate, EncodedValue::Real(value)) => {
                Ok(EncodedValue::Real(value.unary_minus()))
            }
            (UnaryOperation::Not, EncodedValue::Bool(value)) => Ok(EncodedValue::Bool(value.not())),
            (UnaryOperation::IsNull, EncodedValue::Optional { present, .. }) => {
                Ok(EncodedValue::Bool(present.not()))
            }
            (_, value) => Err(self.type_error(&Type::Error, encoded_type(&value))),
        }
    }

    fn encode_binary(
        &mut self,
        operation: BinaryOperation,
        left: EncodedValue,
        right: EncodedValue,
    ) -> Result<EncodedValue, SolveError> {
        match operation {
            BinaryOperation::Equal | BinaryOperation::NotEqual => {
                let equal = self.value_eq(&left, &right)?;
                Ok(EncodedValue::Bool(if operation == BinaryOperation::Equal {
                    equal
                } else {
                    equal.not()
                }))
            }
            BinaryOperation::Or | BinaryOperation::And => {
                let left = self.expect_bool(left)?;
                let right = self.expect_bool(right)?;
                Ok(EncodedValue::Bool(if operation == BinaryOperation::Or {
                    Bool::or(&[left, right])
                } else {
                    Bool::and(&[left, right])
                }))
            }
            BinaryOperation::LessThan
            | BinaryOperation::GreaterThan
            | BinaryOperation::LessThanOrEqual
            | BinaryOperation::GreaterThanOrEqual => {
                let result = match (left, right) {
                    (EncodedValue::Int(left), EncodedValue::Int(right)) => {
                        compare_int(operation, &left, &right)
                    }
                    (EncodedValue::Real(left), EncodedValue::Real(right)) => {
                        compare_real(operation, &left, &right)
                    }
                    (left, right) => {
                        return Err(self.type_error(&encoded_type(&left), encoded_type(&right)));
                    }
                };
                Ok(EncodedValue::Bool(result))
            }
            BinaryOperation::Add
            | BinaryOperation::Subtract
            | BinaryOperation::Multiply
            | BinaryOperation::Divide => self.encode_arithmetic(operation, left, right),
            _ => Err(error(SolveErrorKind::UnsupportedType(encoded_type(&left)))),
        }
    }

    fn encode_arithmetic(
        &mut self,
        operation: BinaryOperation,
        left: EncodedValue,
        right: EncodedValue,
    ) -> Result<EncodedValue, SolveError> {
        match (left, right) {
            (EncodedValue::Int(left), EncodedValue::Int(right)) => {
                let result = match operation {
                    BinaryOperation::Add => Int::add(&[left, right]),
                    BinaryOperation::Subtract => Int::sub(&[left, right]),
                    BinaryOperation::Multiply => Int::mul(&[left, right]),
                    BinaryOperation::Divide => {
                        self.optimize.assert(right.eq(0).not());
                        truncating_integer_division(&left, &right)
                    }
                    _ => unreachable!(),
                };
                Ok(EncodedValue::Int(result))
            }
            (EncodedValue::Real(left), EncodedValue::Real(right)) => {
                let result = match operation {
                    BinaryOperation::Add => Real::add(&[left, right]),
                    BinaryOperation::Subtract => Real::sub(&[left, right]),
                    BinaryOperation::Multiply => Real::mul(&[left, right]),
                    BinaryOperation::Divide => {
                        self.optimize
                            .assert(right.eq(Real::from_rational(0, 1)).not());
                        left.div(&right)
                    }
                    _ => unreachable!(),
                };
                Ok(EncodedValue::Real(result))
            }
            (EncodedValue::String(left), EncodedValue::String(right))
                if operation == BinaryOperation::Add =>
            {
                Ok(EncodedValue::String(Z3String::concat(&[left, right])))
            }
            (left, right) => Err(self.type_error(&encoded_type(&left), encoded_type(&right))),
        }
    }

    fn inject_union(&self, value: EncodedValue, union: &Type) -> Result<EncodedValue, SolveError> {
        let Type::Union(alternatives) = union else {
            return Err(error(SolveErrorKind::UnsupportedType(union.clone())));
        };
        let found = encoded_type(&value);
        let Some(selected) = alternatives
            .iter()
            .position(|alternative| alternative.accepts(&found))
        else {
            return Err(self.type_error(union, found));
        };
        let mut value = Some(value);
        let values = alternatives
            .iter()
            .enumerate()
            .map(|(index, alternative)| {
                if index == selected {
                    Ok(value.take().expect("selected union value is consumed once"))
                } else {
                    self.default_value(alternative)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EncodedValue::Union {
            alternatives: alternatives.clone(),
            selected: Int::from_u64(selected as u64),
            values,
        })
    }

    fn encode_type_is(&self, value: EncodedValue, target: &Type) -> Result<Bool, SolveError> {
        match value {
            EncodedValue::Union {
                alternatives,
                selected,
                ..
            } => {
                let Some(index) = alternatives.iter().position(|ty| ty == target) else {
                    return Err(self.type_error(&Type::union(alternatives), target.clone()));
                };
                Ok(selected.eq(Int::from_u64(index as u64)))
            }
            value if &encoded_type(&value) == target => Ok(Bool::from_bool(true)),
            value => Err(self.type_error(target, encoded_type(&value))),
        }
    }

    fn project_union(
        &self,
        value: EncodedValue,
        target: &Type,
    ) -> Result<EncodedValue, SolveError> {
        let EncodedValue::Union {
            alternatives,
            values,
            ..
        } = value
        else {
            return Err(self.type_error(
                &Type::union([target.clone(), Type::Error]),
                encoded_type(&value),
            ));
        };
        let Some(index) = alternatives.iter().position(|ty| ty == target) else {
            return Err(self.type_error(&Type::union(alternatives), target.clone()));
        };
        Ok(values[index].clone())
    }

    fn encode_safe_cast(
        &self,
        value: EncodedValue,
        target: &Type,
    ) -> Result<EncodedValue, SolveError> {
        match value {
            EncodedValue::Union {
                alternatives,
                selected,
                values,
            } => {
                let Some(index) = alternatives.iter().position(|ty| ty == target) else {
                    return Err(self.type_error(&Type::union(alternatives), target.clone()));
                };
                Ok(EncodedValue::Optional {
                    present: selected.eq(Int::from_u64(index as u64)),
                    value: Box::new(values[index].clone()),
                })
            }
            value if &encoded_type(&value) == target => Ok(EncodedValue::Optional {
                present: Bool::from_bool(true),
                value: Box::new(value),
            }),
            value => Err(self.type_error(target, encoded_type(&value))),
        }
    }

    fn encode_constant(&self, ty: &Type, value: &Constant) -> Result<EncodedValue, SolveError> {
        let encoded = match (ty, value) {
            (Type::Unit, Constant::Unit) => EncodedValue::Unit,
            (Type::Bool, Constant::Bool(value)) => EncodedValue::Bool(Bool::from_bool(*value)),
            (Type::Int, Constant::Int(value)) => EncodedValue::Int(Int::from_i64(*value)),
            (Type::Float, Constant::Float(value)) if value.is_finite() => {
                EncodedValue::Real(real_from_f64(*value)?)
            }
            (Type::String, Constant::String(value)) => {
                EncodedValue::String(Z3String::from_str(value).map_err(|_| {
                    error(SolveErrorKind::InvalidConstant {
                        expected: ty.clone(),
                    })
                })?)
            }
            (Type::Package, Constant::Package(value)) => {
                EncodedValue::Package(Z3String::from_str(value).map_err(|_| {
                    error(SolveErrorKind::InvalidConstant {
                        expected: ty.clone(),
                    })
                })?)
            }
            (Type::Named(_, _), Constant::Enum(variant)) => {
                EncodedValue::Enum(Int::from_u64(variant_number(*variant)))
            }
            (Type::List(element_type), Constant::List(values)) => {
                let elements = values
                    .iter()
                    .map(|value| {
                        Ok(EncodedListElement {
                            present: Bool::from_bool(true),
                            value: self.encode_constant(element_type, value)?,
                        })
                    })
                    .collect::<Result<Vec<_>, SolveError>>()?;
                EncodedValue::List {
                    element_type: element_type.as_ref().clone(),
                    elements,
                }
            }
            (Type::Set(element_type), Constant::Set(values)) => {
                if !values
                    .iter()
                    .all(|value| constant_matches(value, element_type))
                {
                    return Err(error(SolveErrorKind::InvalidConstant {
                        expected: ty.clone(),
                    }));
                }
                let universe = self.universe(element_type);
                let members = universe
                    .iter()
                    .map(|candidate| {
                        Bool::from_bool(
                            values.iter().any(|value| constants_equal(value, candidate)),
                        )
                    })
                    .collect();
                EncodedValue::Set {
                    element_type: element_type.as_ref().clone(),
                    universe,
                    members,
                }
            }
            (Type::Optional(inner), Constant::Optional(value)) => {
                let (present, value) = match value {
                    Some(value) => (true, self.encode_constant(inner, value)?),
                    None => (false, self.default_value(inner)?),
                };
                EncodedValue::Optional {
                    present: Bool::from_bool(present),
                    value: Box::new(value),
                }
            }
            (Type::Union(alternatives), value) => {
                let Some(selected) = alternatives
                    .iter()
                    .position(|alternative| constant_matches(value, alternative))
                else {
                    return Err(error(SolveErrorKind::InvalidConstant {
                        expected: ty.clone(),
                    }));
                };
                let values = alternatives
                    .iter()
                    .enumerate()
                    .map(|(index, alternative)| {
                        if index == selected {
                            self.encode_constant(alternative, value)
                        } else {
                            self.default_value(alternative)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                EncodedValue::Union {
                    alternatives: alternatives.clone(),
                    selected: Int::from_u64(selected as u64),
                    values,
                }
            }
            _ => {
                return Err(error(SolveErrorKind::InvalidConstant {
                    expected: ty.clone(),
                }));
            }
        };
        Ok(encoded)
    }

    fn list_capacity(&self, ty: &Type) -> usize {
        let Type::List(element_type) = ty else {
            return 0;
        };
        let mut capacity = self
            .source
            .expressions()
            .filter(|expression| expression.ty() == ty)
            .filter_map(|expression| match expression.kind() {
                ExpressionKind::List(values) => Some(values.len()),
                ExpressionKind::Constant(Constant::List(values)) => Some(values.len()),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let mut observable_values = Vec::new();
        for expression in self.source.expressions() {
            if expression.ty() == element_type.as_ref()
                && let ExpressionKind::Constant(value) = expression.kind()
                && !observable_values
                    .iter()
                    .any(|candidate| constants_equal(candidate, value))
            {
                observable_values.push(value.clone());
            }
        }
        capacity = capacity.max(observable_values.len());
        for value in self.request.variable_values().values().chain(
            self.request
                .output_values()
                .values()
                .filter_map(Option::as_ref),
        ) {
            if let Constant::List(values) = value {
                capacity = capacity.max(values.len());
            }
        }
        for goal in self.request.goals() {
            if let OutputGoal::Equals {
                value: Constant::List(values),
                ..
            } = goal
            {
                capacity = capacity.max(values.len());
            }
        }
        capacity.max(1)
    }

    fn fresh_value(&self, ty: &Type, name: &str) -> Result<EncodedValue, SolveError> {
        match ty {
            Type::Unit => Ok(EncodedValue::Unit),
            Type::Bool => Ok(EncodedValue::Bool(Bool::new_const(name))),
            Type::Int => Ok(EncodedValue::Int(Int::new_const(name))),
            Type::Float => Ok(EncodedValue::Real(Real::new_const(name))),
            Type::String => Ok(EncodedValue::String(Z3String::new_const(name))),
            Type::Package => Ok(EncodedValue::Package(Z3String::new_const(name))),
            Type::Named(_, _) => {
                let value = Int::new_const(name);
                let variants = self.enum_universe(ty);
                if variants.is_empty() {
                    return Err(error(SolveErrorKind::UnsupportedType(ty.clone())));
                }
                self.optimize.assert(bool_or(
                    variants
                        .iter()
                        .map(|variant| value.eq(Int::from_u64(variant_number(*variant))))
                        .collect(),
                ));
                Ok(EncodedValue::Enum(value))
            }
            Type::List(element_type) => {
                let capacity = self.list_capacity(ty);
                let mut elements = Vec::with_capacity(capacity);
                for index in 0..capacity {
                    let present = Bool::new_const(format!("{name}_present_{index}"));
                    if let Some(previous) = elements.last() {
                        let previous: &EncodedListElement = previous;
                        self.optimize.assert(present.implies(&previous.present));
                    }
                    elements.push(EncodedListElement {
                        present,
                        value: self.fresh_value(element_type, &format!("{name}_value_{index}"))?,
                    });
                }
                Ok(EncodedValue::List {
                    element_type: element_type.as_ref().clone(),
                    elements,
                })
            }
            Type::Set(element_type) => {
                let universe = self.universe(element_type);
                let members = (0..universe.len())
                    .map(|index| Bool::new_const(format!("{name}_member_{index}")))
                    .collect();
                Ok(EncodedValue::Set {
                    element_type: element_type.as_ref().clone(),
                    universe,
                    members,
                })
            }
            Type::Optional(inner) => Ok(EncodedValue::Optional {
                present: Bool::new_const(format!("{name}_present")),
                value: Box::new(self.fresh_value(inner, &format!("{name}_value"))?),
            }),
            Type::Union(alternatives) => {
                let selected = Int::new_const(format!("{name}_type"));
                self.optimize.assert(bool_or(
                    (0..alternatives.len())
                        .map(|index| selected.eq(Int::from_u64(index as u64)))
                        .collect(),
                ));
                let values = alternatives
                    .iter()
                    .enumerate()
                    .map(|(index, alternative)| {
                        self.fresh_value(alternative, &format!("{name}_variant_{index}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(EncodedValue::Union {
                    alternatives: alternatives.clone(),
                    selected,
                    values,
                })
            }
            Type::Error
            | Type::Never
            | Type::AttrSet(_)
            | Type::Function(_)
            | Type::Parameter(_)
            | Type::Variable(_) => Err(error(SolveErrorKind::UnsupportedType(ty.clone()))),
        }
    }

    fn default_value(&self, ty: &Type) -> Result<EncodedValue, SolveError> {
        match ty {
            Type::Unit => Ok(EncodedValue::Unit),
            Type::Bool => Ok(EncodedValue::Bool(Bool::from_bool(false))),
            Type::Int => Ok(EncodedValue::Int(Int::from_i64(0))),
            Type::Float => Ok(EncodedValue::Real(Real::from_rational(0, 1))),
            Type::String => Ok(EncodedValue::String(
                Z3String::from_str("").expect("empty Z3 string is valid"),
            )),
            Type::Package => Ok(EncodedValue::Package(
                Z3String::from_str("").expect("empty Z3 string is valid"),
            )),
            Type::Named(_, _) => Ok(EncodedValue::Enum(Int::from_i64(0))),
            Type::List(element_type) => Ok(EncodedValue::List {
                element_type: element_type.as_ref().clone(),
                elements: Vec::new(),
            }),
            Type::Set(element_type) => {
                let universe = self.universe(element_type);
                Ok(EncodedValue::Set {
                    element_type: element_type.as_ref().clone(),
                    members: vec![Bool::from_bool(false); universe.len()],
                    universe,
                })
            }
            Type::Optional(inner) => Ok(EncodedValue::Optional {
                present: Bool::from_bool(false),
                value: Box::new(self.default_value(inner)?),
            }),
            Type::Union(alternatives) => Ok(EncodedValue::Union {
                alternatives: alternatives.clone(),
                selected: Int::from_i64(0),
                values: alternatives
                    .iter()
                    .map(|alternative| self.default_value(alternative))
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Type::Error
            | Type::Never
            | Type::AttrSet(_)
            | Type::Function(_)
            | Type::Parameter(_)
            | Type::Variable(_) => Err(error(SolveErrorKind::UnsupportedType(ty.clone()))),
        }
    }

    fn value_eq(&self, left: &EncodedValue, right: &EncodedValue) -> Result<Bool, SolveError> {
        let result = match (left, right) {
            (EncodedValue::Unit, EncodedValue::Unit) => Bool::from_bool(true),
            (EncodedValue::Bool(left), EncodedValue::Bool(right)) => left.eq(right),
            (EncodedValue::Int(left), EncodedValue::Int(right)) => left.eq(right),
            (EncodedValue::Real(left), EncodedValue::Real(right)) => left.eq(right),
            (EncodedValue::String(left), EncodedValue::String(right)) => left.eq(right),
            (EncodedValue::Package(left), EncodedValue::Package(right)) => left.eq(right),
            (EncodedValue::Enum(left), EncodedValue::Enum(right)) => left.eq(right),
            (
                EncodedValue::List {
                    element_type: left_type,
                    elements: left,
                },
                EncodedValue::List {
                    element_type: right_type,
                    elements: right,
                },
            ) if left_type == right_type => self.list_eq(left, right)?,
            (
                EncodedValue::Set {
                    element_type: left_type,
                    universe: left_universe,
                    members: left,
                },
                EncodedValue::Set {
                    element_type: right_type,
                    universe: right_universe,
                    members: right,
                },
            ) if left_type == right_type
                && same_universe(left_universe, right_universe)
                && left.len() == right.len() =>
            {
                bool_and(
                    left.iter()
                        .zip(right)
                        .map(|(left, right)| left.eq(right))
                        .collect(),
                )
            }
            (
                EncodedValue::Optional {
                    present: left_present,
                    value: left_value,
                },
                EncodedValue::Optional {
                    present: right_present,
                    value: right_value,
                },
            ) => {
                let same_presence = left_present.eq(right_present);
                let values = self.value_eq(left_value, right_value)?;
                Bool::and(&[same_presence, Bool::or(&[left_present.not(), values])])
            }
            (
                EncodedValue::Union {
                    alternatives: left_alternatives,
                    selected: left_selected,
                    values: left_values,
                },
                EncodedValue::Union {
                    alternatives: right_alternatives,
                    selected: right_selected,
                    values: right_values,
                },
            ) if left_alternatives == right_alternatives
                && left_values.len() == right_values.len() =>
            {
                let same_tag = left_selected.eq(right_selected);
                let same_values = left_values
                    .iter()
                    .zip(right_values)
                    .enumerate()
                    .map(|(index, (left, right))| {
                        let selected = left_selected.eq(Int::from_u64(index as u64));
                        self.value_eq(left, right)
                            .map(|equal| selected.implies(&equal))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Bool::and(&[same_tag, bool_and(same_values)])
            }
            (left, right) => {
                return Err(self.type_error(&encoded_type(left), encoded_type(right)));
            }
        };
        Ok(result)
    }

    fn list_eq(
        &self,
        left: &[EncodedListElement],
        right: &[EncodedListElement],
    ) -> Result<Bool, SolveError> {
        let mut suffixes = vec![vec![Bool::from_bool(false); right.len() + 1]; left.len() + 1];
        suffixes[left.len()][right.len()] = Bool::from_bool(true);
        for index in (0..left.len()).rev() {
            suffixes[index][right.len()] = Bool::and(&[
                left[index].present.not(),
                suffixes[index + 1][right.len()].clone(),
            ]);
        }
        for index in (0..right.len()).rev() {
            suffixes[left.len()][index] = Bool::and(&[
                right[index].present.not(),
                suffixes[left.len()][index + 1].clone(),
            ]);
        }
        for left_index in (0..left.len()).rev() {
            for right_index in (0..right.len()).rev() {
                let skip_left = Bool::and(&[
                    left[left_index].present.not(),
                    suffixes[left_index + 1][right_index].clone(),
                ]);
                let skip_right = Bool::and(&[
                    right[right_index].present.not(),
                    suffixes[left_index][right_index + 1].clone(),
                ]);
                let values = self.value_eq(&left[left_index].value, &right[right_index].value)?;
                let matched = Bool::and(&[
                    left[left_index].present.clone(),
                    right[right_index].present.clone(),
                    values,
                    suffixes[left_index + 1][right_index + 1].clone(),
                ]);
                suffixes[left_index][right_index] = Bool::or(&[skip_left, skip_right, matched]);
            }
        }
        Ok(suffixes[0][0].clone())
    }

    fn value_ite(
        &self,
        condition: &Bool,
        then_value: &EncodedValue,
        else_value: &EncodedValue,
    ) -> Result<EncodedValue, SolveError> {
        let value = match (then_value, else_value) {
            (EncodedValue::Unit, EncodedValue::Unit) => EncodedValue::Unit,
            (EncodedValue::Bool(left), EncodedValue::Bool(right)) => {
                EncodedValue::Bool(condition.ite(left, right))
            }
            (EncodedValue::Int(left), EncodedValue::Int(right)) => {
                EncodedValue::Int(condition.ite(left, right))
            }
            (EncodedValue::Real(left), EncodedValue::Real(right)) => {
                EncodedValue::Real(condition.ite(left, right))
            }
            (EncodedValue::String(left), EncodedValue::String(right)) => {
                EncodedValue::String(condition.ite(left, right))
            }
            (EncodedValue::Package(left), EncodedValue::Package(right)) => {
                EncodedValue::Package(condition.ite(left, right))
            }
            (EncodedValue::Enum(left), EncodedValue::Enum(right)) => {
                EncodedValue::Enum(condition.ite(left, right))
            }
            (
                EncodedValue::List {
                    element_type: left_type,
                    elements: left,
                },
                EncodedValue::List {
                    element_type: right_type,
                    elements: right,
                },
            ) if left_type == right_type => {
                let capacity = left.len().max(right.len());
                let default = self.default_value(left_type)?;
                let mut elements = Vec::with_capacity(capacity);
                for index in 0..capacity {
                    let left_present = left
                        .get(index)
                        .map(|element| element.present.clone())
                        .unwrap_or_else(|| Bool::from_bool(false));
                    let right_present = right
                        .get(index)
                        .map(|element| element.present.clone())
                        .unwrap_or_else(|| Bool::from_bool(false));
                    let left_value = left
                        .get(index)
                        .map(|element| &element.value)
                        .unwrap_or(&default);
                    let right_value = right
                        .get(index)
                        .map(|element| &element.value)
                        .unwrap_or(&default);
                    elements.push(EncodedListElement {
                        present: condition.ite(&left_present, &right_present),
                        value: self.value_ite(condition, left_value, right_value)?,
                    });
                }
                EncodedValue::List {
                    element_type: left_type.clone(),
                    elements,
                }
            }
            (
                EncodedValue::Set {
                    element_type: left_type,
                    universe: left_universe,
                    members: left,
                },
                EncodedValue::Set {
                    element_type: right_type,
                    universe: right_universe,
                    members: right,
                },
            ) if left_type == right_type && same_universe(left_universe, right_universe) => {
                EncodedValue::Set {
                    element_type: left_type.clone(),
                    universe: left_universe.clone(),
                    members: left
                        .iter()
                        .zip(right)
                        .map(|(left, right)| condition.ite(left, right))
                        .collect(),
                }
            }
            (
                EncodedValue::Optional {
                    present: left_present,
                    value: left_value,
                },
                EncodedValue::Optional {
                    present: right_present,
                    value: right_value,
                },
            ) => EncodedValue::Optional {
                present: condition.ite(left_present, right_present),
                value: Box::new(self.value_ite(condition, left_value, right_value)?),
            },
            (
                EncodedValue::Union {
                    alternatives: left_alternatives,
                    selected: left_selected,
                    values: left_values,
                },
                EncodedValue::Union {
                    alternatives: right_alternatives,
                    selected: right_selected,
                    values: right_values,
                },
            ) if left_alternatives == right_alternatives
                && left_values.len() == right_values.len() =>
            {
                EncodedValue::Union {
                    alternatives: left_alternatives.clone(),
                    selected: condition.ite(left_selected, right_selected),
                    values: left_values
                        .iter()
                        .zip(right_values)
                        .map(|(left, right)| self.value_ite(condition, left, right))
                        .collect::<Result<Vec<_>, _>>()?,
                }
            }
            (left, right) => {
                return Err(self.type_error(&encoded_type(left), encoded_type(right)));
            }
        };
        Ok(value)
    }

    fn value_distance(&self, left: &EncodedValue, right: &EncodedValue) -> Result<Int, SolveError> {
        match (left, right) {
            (EncodedValue::Unit, EncodedValue::Unit) => Ok(Int::from_i64(0)),
            (
                EncodedValue::List {
                    element_type: left_type,
                    elements: left,
                },
                EncodedValue::List {
                    element_type: right_type,
                    elements: right,
                },
            ) if left_type == right_type => {
                let count = |elements: &[EncodedListElement]| {
                    let terms = elements
                        .iter()
                        .map(|element| element.present.ite(&Int::from_i64(1), &Int::from_i64(0)))
                        .collect::<Vec<_>>();
                    if terms.is_empty() {
                        Int::from_i64(0)
                    } else {
                        Int::add(&terms)
                    }
                };
                let difference = Int::sub(&[count(left), count(right)]);
                let absolute = difference
                    .ge(Int::from_i64(0))
                    .ite(&difference, &difference.unary_minus());
                let unequal = self
                    .list_eq(left, right)?
                    .ite(&Int::from_i64(0), &Int::from_i64(1));
                Ok(Int::add(&[absolute, unequal]))
            }
            (
                EncodedValue::Set {
                    element_type: left_type,
                    universe: left_universe,
                    members: left,
                },
                EncodedValue::Set {
                    element_type: right_type,
                    universe: right_universe,
                    members: right,
                },
            ) if left_type == right_type && same_universe(left_universe, right_universe) => {
                let terms = left
                    .iter()
                    .zip(right)
                    .map(|(left, right)| left.xor(right).ite(&Int::from_i64(1), &Int::from_i64(0)))
                    .collect::<Vec<_>>();
                Ok(if terms.is_empty() {
                    Int::from_i64(0)
                } else {
                    Int::add(&terms)
                })
            }
            (
                EncodedValue::Optional {
                    present: left_present,
                    value: left_value,
                },
                EncodedValue::Optional {
                    present: right_present,
                    value: right_value,
                },
            ) => {
                let presence = left_present
                    .xor(right_present)
                    .ite(&Int::from_i64(1), &Int::from_i64(0));
                let inner = self.value_distance(left_value, right_value)?;
                let both = Bool::and(&[left_present.clone(), right_present.clone()]);
                Ok(Int::add(&[presence, both.ite(&inner, &Int::from_i64(0))]))
            }
            _ => Ok(self
                .value_eq(left, right)?
                .ite(&Int::from_i64(0), &Int::from_i64(1))),
        }
    }

    fn expect_bool(&self, value: EncodedValue) -> Result<Bool, SolveError> {
        match value {
            EncodedValue::Bool(value) => Ok(value),
            value => Err(self.type_error(&Type::Bool, encoded_type(&value))),
        }
    }

    fn universe(&self, element_type: &Type) -> Vec<Constant> {
        self.universes
            .iter()
            .find(|(ty, _)| ty == element_type)
            .map(|(_, values)| values.clone())
            .unwrap_or_default()
    }

    fn enum_universe(&self, ty: &Type) -> Vec<VariantId> {
        self.enum_universes
            .iter()
            .find(|(candidate, _)| candidate == ty)
            .map(|(_, variants)| variants.clone())
            .unwrap_or_default()
    }

    fn decode_solution(&self, model: &Model) -> Result<Solution, SolveError> {
        let mut variables = Vec::new();
        let mut outputs = Vec::new();
        let mut cost = 0u64;
        let variable_costs = self.variable_costs();

        for variable in self.source.variables() {
            let Some(state) = self.variable_states.get(&variable.id()) else {
                continue;
            };
            if !decode_bool(model, &state.edit)? {
                continue;
            }
            let variable_cost = variable_costs
                .get(&variable.id())
                .copied()
                .unwrap_or_default();
            cost = cost
                .checked_add(variable_cost)
                .ok_or_else(|| error(SolveErrorKind::IntegerCostOverflow))?;
            let before = self
                .request
                .variable_values()
                .get(&variable.id())
                .cloned()
                .or_else(|| self.decode_value(model, &state.base).ok());
            variables.push(VariableChange {
                variable: variable.id(),
                source: variable.source().clone(),
                before,
                after: self.decode_value(model, &state.actual)?,
                cost: variable_cost,
            });
        }

        for (path, state) in &self.outputs {
            let Some(edit) = &state.edit else {
                continue;
            };
            if !decode_bool(model, edit)? {
                continue;
            }
            cost = cost
                .checked_add(state.cost)
                .ok_or_else(|| error(SolveErrorKind::IntegerCostOverflow))?;
            let before = match self.request.output_values().get(path) {
                Some(value) => value.clone(),
                None => self.decode_optional_output(
                    model,
                    &state.derived_present,
                    &state.derived_value,
                )?,
            };
            let after =
                self.decode_optional_output(model, &state.actual_present, &state.actual_value)?;
            outputs.push(OutputChange {
                path: path.clone(),
                before,
                after,
                cost: state.cost,
            });
        }

        variables.sort_by_key(|change| change.variable);
        outputs.sort_by(|left, right| left.path.cmp(&right.path));
        let changed_variables = variables
            .iter()
            .map(|change| change.variable)
            .collect::<HashSet<_>>();
        let mut opaque_impacts = self
            .opaque_boundaries
            .iter()
            .filter_map(|boundary| {
                let mut affected_variables = boundary
                    .variables
                    .intersection(&changed_variables)
                    .copied()
                    .collect::<Vec<_>>();
                affected_variables.sort();
                (!affected_variables.is_empty()).then(|| OpaqueImpact {
                    boundary: boundary.boundary,
                    origin: boundary.origin.clone(),
                    changed_variables: affected_variables,
                })
            })
            .collect::<Vec<_>>();
        opaque_impacts.sort_by_key(|impact| impact.boundary);
        Ok(Solution {
            cost,
            variables,
            outputs,
            opaque_impacts,
        })
    }

    fn decode_optional_output(
        &self,
        model: &Model,
        present: &Bool,
        value: &EncodedValue,
    ) -> Result<Option<Constant>, SolveError> {
        if decode_bool(model, present)? {
            Ok(Some(self.decode_value(model, value)?))
        } else {
            Ok(None)
        }
    }

    fn decode_value(&self, model: &Model, value: &EncodedValue) -> Result<Constant, SolveError> {
        match value {
            EncodedValue::Unit => Ok(Constant::Unit),
            EncodedValue::Bool(value) => Ok(Constant::Bool(decode_bool(model, value)?)),
            EncodedValue::Int(value) => Ok(Constant::Int(
                model
                    .eval(value, true)
                    .and_then(|value| value.as_i64())
                    .ok_or_else(|| error(SolveErrorKind::ModelValueUnavailable))?,
            )),
            EncodedValue::Real(value) => Ok(Constant::Float(
                model
                    .eval(value, true)
                    .map(|value| value.approx_f64())
                    .ok_or_else(|| error(SolveErrorKind::ModelValueUnavailable))?,
            )),
            EncodedValue::String(value) => Ok(Constant::String(
                model
                    .eval(value, true)
                    .and_then(|value| value.as_string())
                    .ok_or_else(|| error(SolveErrorKind::ModelValueUnavailable))?,
            )),
            EncodedValue::Package(value) => Ok(Constant::Package(
                model
                    .eval(value, true)
                    .and_then(|value| value.as_string())
                    .ok_or_else(|| error(SolveErrorKind::ModelValueUnavailable))?,
            )),
            EncodedValue::Enum(value) => {
                let number = model
                    .eval(value, true)
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| error(SolveErrorKind::ModelValueUnavailable))?;
                Ok(Constant::Enum(number_variant(number)))
            }
            EncodedValue::List { elements, .. } => {
                let values = elements
                    .iter()
                    .filter_map(|element| match decode_bool(model, &element.present) {
                        Ok(true) => Some(self.decode_value(model, &element.value)),
                        Ok(false) => None,
                        Err(error) => Some(Err(error)),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Constant::List(values))
            }
            EncodedValue::Set {
                universe, members, ..
            } => {
                let values = universe
                    .iter()
                    .zip(members)
                    .filter_map(|(value, member)| match decode_bool(model, member) {
                        Ok(true) => Some(Ok(value.clone())),
                        Ok(false) => None,
                        Err(error) => Some(Err(error)),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Constant::Set(values))
            }
            EncodedValue::Optional { present, value } => {
                let value = if decode_bool(model, present)? {
                    Some(Box::new(self.decode_value(model, value)?))
                } else {
                    None
                };
                Ok(Constant::Optional(value))
            }
            EncodedValue::Union {
                selected, values, ..
            } => {
                let index = model
                    .eval(selected, true)
                    .and_then(|value| value.as_u64())
                    .and_then(|value| usize::try_from(value).ok())
                    .filter(|index| *index < values.len())
                    .ok_or_else(|| error(SolveErrorKind::ModelValueUnavailable))?;
                self.decode_value(model, &values[index])
            }
            EncodedValue::Closure { .. } => Err(error(SolveErrorKind::ModelValueUnavailable)),
        }
    }

    fn type_error(&self, expected: &Type, found: Type) -> SolveError {
        error(SolveErrorKind::TypeMismatch {
            expected: expected.clone(),
            found,
        })
    }
}

fn collect_universes(
    model: &ConstraintModel,
    request: &SolveRequest,
) -> Vec<(Type, Vec<Constant>)> {
    let mut universes = Vec::new();
    for expression in model.expressions() {
        register_set_types(expression.ty(), &mut universes);
    }
    for variable in model.variables() {
        register_set_types(variable.ty(), &mut universes);
    }
    for declaration in model.paths() {
        register_set_types(declaration.ty(), &mut universes);
    }
    for virtual_output in request.virtual_outputs().values() {
        register_set_types(&virtual_output.ty, &mut universes);
    }
    for expression in model.expressions() {
        if let ExpressionKind::Constant(value) = expression.kind() {
            collect_constant_universe(expression.ty(), value, &mut universes);
        }
    }
    for (variable, value) in request.variable_values() {
        if let Some(variable) = model.variable(*variable) {
            collect_constant_universe(variable.ty(), value, &mut universes);
        }
    }
    for (path, value) in request.output_values() {
        if let (Some(declaration), Some(value)) = (model.path(path), value) {
            collect_constant_universe(declaration.ty(), value, &mut universes);
        }
    }
    for goal in request.goals() {
        collect_goal_constant_universe(model, request, goal, &mut universes);
    }
    for constraint in request.constraints() {
        let (first, second) = constraint_predicates(constraint);
        collect_goal_constant_universe(model, request, first, &mut universes);
        if let Some(second) = second {
            collect_goal_constant_universe(model, request, second, &mut universes);
        }
    }
    for exclusion in request.excluded_candidates() {
        for (variable, value) in &exclusion.variable_values {
            if let Some(variable) = model.variable(*variable) {
                collect_constant_universe(variable.ty(), value, &mut universes);
            }
        }
        for (path, value) in &exclusion.output_values {
            if let (Some(declaration), Some(value)) = (model.path(path), value) {
                collect_constant_universe(declaration.ty(), value, &mut universes);
            }
        }
    }
    universes
}

fn collect_enum_universes(
    model: &ConstraintModel,
    request: &SolveRequest,
) -> Vec<(Type, Vec<VariantId>)> {
    let mut universes = Vec::new();
    for expression in model.expressions() {
        if let ExpressionKind::Constant(value) = expression.kind() {
            collect_enum_constant(expression.ty(), value, &mut universes);
        }
    }
    for (variable, value) in request.variable_values() {
        if let Some(variable) = model.variable(*variable) {
            collect_enum_constant(variable.ty(), value, &mut universes);
        }
    }
    for (path, value) in request.output_values() {
        if let (Some(declaration), Some(value)) = (model.path(path), value) {
            collect_enum_constant(declaration.ty(), value, &mut universes);
        }
    }
    for goal in request.goals() {
        collect_goal_enum_universe(model, request, goal, &mut universes);
    }
    for constraint in request.constraints() {
        let (first, second) = constraint_predicates(constraint);
        collect_goal_enum_universe(model, request, first, &mut universes);
        if let Some(second) = second {
            collect_goal_enum_universe(model, request, second, &mut universes);
        }
    }
    for exclusion in request.excluded_candidates() {
        for (variable, value) in &exclusion.variable_values {
            if let Some(variable) = model.variable(*variable) {
                collect_enum_constant(variable.ty(), value, &mut universes);
            }
        }
        for (path, value) in &exclusion.output_values {
            if let (Some(declaration), Some(value)) = (model.path(path), value) {
                collect_enum_constant(declaration.ty(), value, &mut universes);
            }
        }
    }
    universes
}

fn constraint_predicates(constraint: &OutputConstraint) -> (&OutputGoal, Option<&OutputGoal>) {
    match constraint {
        OutputConstraint::Required(predicate) => (predicate, None),
        OutputConstraint::Implies {
            condition,
            consequence,
        } => (condition, Some(consequence)),
        OutputConstraint::Conflicts { left, right } => (left, Some(right)),
    }
}

fn goal_output_type<'model>(
    model: &'model ConstraintModel,
    request: &'model SolveRequest,
    path: &OutputPath,
) -> Option<&'model Type> {
    model
        .path(path)
        .map(|declaration| declaration.ty())
        .or_else(|| {
            request
                .virtual_outputs()
                .get(path)
                .map(|virtual_output| &virtual_output.ty)
        })
}

fn collect_goal_constant_universe(
    model: &ConstraintModel,
    request: &SolveRequest,
    goal: &OutputGoal,
    universes: &mut Vec<(Type, Vec<Constant>)>,
) {
    match goal {
        OutputGoal::Equals { path, value } => {
            if let Some(ty) = goal_output_type(model, request, path) {
                collect_constant_universe(ty, value, universes);
            }
        }
        OutputGoal::Contains { path, value } | OutputGoal::NotContains { path, value } => {
            if let Some(Type::List(element) | Type::Set(element)) =
                goal_output_type(model, request, path)
            {
                collect_constant_universe(element, value, universes);
            }
        }
        OutputGoal::Absent { .. } => {}
    }
}

fn collect_goal_enum_universe(
    model: &ConstraintModel,
    request: &SolveRequest,
    goal: &OutputGoal,
    universes: &mut Vec<(Type, Vec<VariantId>)>,
) {
    match goal {
        OutputGoal::Equals { path, value } => {
            if let Some(ty) = goal_output_type(model, request, path) {
                collect_enum_constant(ty, value, universes);
            }
        }
        OutputGoal::Contains { path, value } | OutputGoal::NotContains { path, value } => {
            if let Some(Type::List(element) | Type::Set(element)) =
                goal_output_type(model, request, path)
            {
                collect_enum_constant(element, value, universes);
            }
        }
        OutputGoal::Absent { .. } => {}
    }
}

fn collect_enum_constant(ty: &Type, value: &Constant, universes: &mut Vec<(Type, Vec<VariantId>)>) {
    match (ty, value) {
        (Type::Named(_, _), Constant::Enum(variant)) => {
            let index = universes
                .iter()
                .position(|(candidate, _)| candidate == ty)
                .unwrap_or_else(|| {
                    universes.push((ty.clone(), Vec::new()));
                    universes.len() - 1
                });
            if !universes[index].1.contains(variant) {
                universes[index].1.push(*variant);
            }
        }
        (Type::List(element), Constant::List(values))
        | (Type::Set(element), Constant::Set(values)) => {
            for value in values {
                collect_enum_constant(element, value, universes);
            }
        }
        (Type::Optional(inner), Constant::Optional(Some(value))) => {
            collect_enum_constant(inner, value, universes);
        }
        (Type::Union(alternatives), value) => {
            for alternative in alternatives {
                if constant_matches(value, alternative) {
                    collect_enum_constant(alternative, value, universes);
                }
            }
        }
        _ => {}
    }
}

fn register_set_types(ty: &Type, universes: &mut Vec<(Type, Vec<Constant>)>) {
    match ty {
        Type::Set(element) => {
            if !universes.iter().any(|(ty, _)| ty == element.as_ref()) {
                universes.push((element.as_ref().clone(), Vec::new()));
            }
            register_set_types(element, universes);
        }
        Type::Optional(inner) | Type::List(inner) | Type::AttrSet(inner) => {
            register_set_types(inner, universes)
        }
        Type::Union(alternatives) => {
            for alternative in alternatives {
                register_set_types(alternative, universes);
            }
        }
        Type::Named(_, arguments) => {
            for argument in arguments {
                register_set_types(argument, universes);
            }
        }
        Type::Function(function) => {
            for parameter in &function.parameters {
                register_set_types(parameter, universes);
            }
            register_set_types(&function.return_type, universes);
        }
        Type::Error
        | Type::Never
        | Type::Unit
        | Type::Bool
        | Type::Int
        | Type::Float
        | Type::String
        | Type::Package
        | Type::Parameter(_)
        | Type::Variable(_) => {}
    }
}

fn collect_constant_universe(
    ty: &Type,
    value: &Constant,
    universes: &mut Vec<(Type, Vec<Constant>)>,
) {
    if let Some((_, values)) = universes
        .iter_mut()
        .find(|(element_type, _)| element_type == ty)
        && !values
            .iter()
            .any(|candidate| constants_equal(candidate, value))
    {
        values.push(value.clone());
    }
    match (ty, value) {
        (Type::List(element_type), Constant::List(values))
        | (Type::Set(element_type), Constant::Set(values)) => {
            for value in values {
                collect_constant_universe(element_type, value, universes);
            }
        }
        (Type::Optional(inner), Constant::Optional(Some(value))) => {
            collect_constant_universe(inner, value, universes);
        }
        (Type::Union(alternatives), value) => {
            for alternative in alternatives {
                if constant_matches(value, alternative) {
                    collect_constant_universe(alternative, value, universes);
                }
            }
        }
        _ => {}
    }
}

fn compare_int(operation: BinaryOperation, left: &Int, right: &Int) -> Bool {
    match operation {
        BinaryOperation::LessThan => left.lt(right),
        BinaryOperation::GreaterThan => left.gt(right),
        BinaryOperation::LessThanOrEqual => left.le(right),
        BinaryOperation::GreaterThanOrEqual => left.ge(right),
        _ => unreachable!(),
    }
}

fn compare_real(operation: BinaryOperation, left: &Real, right: &Real) -> Bool {
    match operation {
        BinaryOperation::LessThan => left.lt(right),
        BinaryOperation::GreaterThan => left.gt(right),
        BinaryOperation::LessThanOrEqual => left.le(right),
        BinaryOperation::GreaterThanOrEqual => left.ge(right),
        _ => unreachable!(),
    }
}

fn truncating_integer_division(left: &Int, right: &Int) -> Int {
    let zero = Int::from_i64(0);
    let absolute_left = left.ge(&zero).ite(left, &left.unary_minus());
    let absolute_right = right.ge(&zero).ite(right, &right.unary_minus());
    let quotient = absolute_left.div(&absolute_right);
    left.lt(&zero)
        .xor(right.lt(&zero))
        .ite(&quotient.unary_minus(), &quotient)
}

fn real_from_f64(value: f64) -> Result<Real, SolveError> {
    if !value.is_finite() {
        return Err(error(SolveErrorKind::InvalidFloat));
    }
    let (numerator, denominator) = decimal_fraction(value);
    Real::from_rational_str(&numerator, &denominator)
        .ok_or_else(|| error(SolveErrorKind::InvalidFloat))
}

fn decimal_fraction(value: f64) -> (String, String) {
    let rendered = value.to_string();
    let (mantissa, exponent) = rendered
        .split_once(['e', 'E'])
        .map_or((rendered.as_str(), 0), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().unwrap_or(0))
        });
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.trim_start_matches(['-', '+']);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits = format!("{whole}{fraction}");
    while digits.starts_with('0') && digits.len() > 1 {
        digits.remove(0);
    }
    if negative && digits != "0" {
        digits.insert(0, '-');
    }
    let scale = i32::try_from(fraction.len()).unwrap_or(i32::MAX) - exponent;
    if scale <= 0 {
        digits.push_str(&"0".repeat(scale.unsigned_abs() as usize));
        (digits, "1".to_owned())
    } else {
        (digits, format!("1{}", "0".repeat(scale as usize)))
    }
}

fn variant_number(variant: VariantId) -> u64 {
    (u64::from(variant.module.0) << 32) | u64::from(variant.index)
}

fn number_variant(value: u64) -> VariantId {
    VariantId {
        module: ModuleId((value >> 32) as u32),
        index: value as u32,
    }
}

fn output_name(path: &OutputPath, suffix: &str) -> String {
    let segments = path
        .segments()
        .iter()
        .map(|segment| match segment {
            OutputPathSegment::Field(field) => format!("f{}_{}", field.module.0, field.index),
            OutputPathSegment::Key(key) => {
                let encoded = key
                    .as_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                format!("k{}_{}", key.len(), encoded)
            }
        })
        .collect::<Vec<_>>()
        .join("_");
    format!(
        "output_{}_{}_{}_{}",
        path.root_symbol().module.0,
        path.root_symbol().index,
        segments,
        suffix
    )
}

fn decode_bool(model: &Model, value: &Bool) -> Result<bool, SolveError> {
    model
        .eval(value, true)
        .and_then(|value| value.as_bool())
        .ok_or_else(|| error(SolveErrorKind::ModelValueUnavailable))
}

fn bool_and(values: Vec<Bool>) -> Bool {
    if values.is_empty() {
        Bool::from_bool(true)
    } else {
        Bool::and(&values)
    }
}

fn bool_or(values: Vec<Bool>) -> Bool {
    if values.is_empty() {
        Bool::from_bool(false)
    } else {
        Bool::or(&values)
    }
}

fn same_universe(left: &[Constant], right: &[Constant]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| constants_equal(left, right))
}

fn constants_equal(left: &Constant, right: &Constant) -> bool {
    match (left, right) {
        (Constant::Unit, Constant::Unit) => true,
        (Constant::Bool(left), Constant::Bool(right)) => left == right,
        (Constant::Int(left), Constant::Int(right)) => left == right,
        (Constant::Float(left), Constant::Float(right)) => left.to_bits() == right.to_bits(),
        (Constant::String(left), Constant::String(right)) => left == right,
        (Constant::Package(left), Constant::Package(right)) => left == right,
        (Constant::Enum(left), Constant::Enum(right)) => left == right,
        (Constant::Optional(left), Constant::Optional(right)) => match (left, right) {
            (None, None) => true,
            (Some(left), Some(right)) => constants_equal(left, right),
            _ => false,
        },
        (Constant::List(left), Constant::List(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| constants_equal(left, right))
        }
        (Constant::Set(left), Constant::Set(right)) => {
            left.iter().all(|value| {
                right
                    .iter()
                    .any(|candidate| constants_equal(value, candidate))
            }) && right.iter().all(|value| {
                left.iter()
                    .any(|candidate| constants_equal(value, candidate))
            })
        }
        _ => false,
    }
}

fn constant_matches(value: &Constant, ty: &Type) -> bool {
    match (value, ty) {
        (Constant::Unit, Type::Unit)
        | (Constant::Bool(_), Type::Bool)
        | (Constant::Int(_), Type::Int)
        | (Constant::String(_), Type::String)
        | (Constant::Package(_), Type::Package)
        | (Constant::Enum(_), Type::Named(_, _)) => true,
        (Constant::Float(value), Type::Float) => value.is_finite(),
        (Constant::List(values), Type::List(element)) => {
            values.iter().all(|value| constant_matches(value, element))
        }
        (Constant::Set(values), Type::Set(element)) => {
            values.iter().all(|value| constant_matches(value, element))
        }
        (Constant::Optional(None), Type::Optional(_)) => true,
        (Constant::Optional(Some(value)), Type::Optional(inner)) => constant_matches(value, inner),
        (value, Type::Union(alternatives)) => alternatives
            .iter()
            .any(|alternative| constant_matches(value, alternative)),
        _ => false,
    }
}

fn encoded_type(value: &EncodedValue) -> Type {
    match value {
        EncodedValue::Unit => Type::Unit,
        EncodedValue::Bool(_) => Type::Bool,
        EncodedValue::Int(_) => Type::Int,
        EncodedValue::Real(_) => Type::Float,
        EncodedValue::String(_) => Type::String,
        EncodedValue::Package(_) => Type::Package,
        EncodedValue::Enum(_) => Type::Error,
        EncodedValue::List { element_type, .. } => Type::List(Box::new(element_type.clone())),
        EncodedValue::Set { element_type, .. } => Type::Set(Box::new(element_type.clone())),
        EncodedValue::Optional { value, .. } => Type::optional(encoded_type(value)),
        EncodedValue::Union { alternatives, .. } => Type::union(alternatives.clone()),
        EncodedValue::Closure { ty, .. } => ty.clone(),
    }
}

fn error(kind: SolveErrorKind) -> SolveError {
    SolveError { kind }
}
