use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
};

use light_nix_name_resolver::{FieldId, ModuleId, SymbolId, VariantId};
use light_nix_type_checker::{BuiltinMethod, Type};

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
    };
}

define_id!(VariableId);
define_id!(ExpressionId);
define_id!(ConstraintId);
define_id!(ObjectiveId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOrigin {
    pub module: ModuleId,
    pub span: Range<usize>,
}

impl SourceOrigin {
    pub fn new(module: ModuleId, span: Range<usize>) -> Self {
        Self { module, span }
    }

    pub fn module(&self) -> ModuleId {
        self.module
    }

    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputPath {
    pub root: SymbolId,
    pub fields: Vec<FieldId>,
}

impl OutputPath {
    pub fn root(root: SymbolId) -> Self {
        Self {
            root,
            fields: Vec::new(),
        }
    }

    pub fn field(mut self, field: FieldId) -> Self {
        self.fields.push(field);
        self
    }

    pub fn root_symbol(&self) -> SymbolId {
        self.root
    }

    pub fn fields(&self) -> &[FieldId] {
        &self.fields
    }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Constant {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Set(Vec<Constant>),
    Optional(Option<Box<Constant>>),
    Enum(VariantId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MutationPolicy {
    Readonly,
    Tunable { cost: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VariableSource {
    Symbol(SymbolId),
    Output(OutputPath),
    Synthetic(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VariableKind {
    Input,
    Tunable { cost: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    id: VariableId,
    source: VariableSource,
    ty: Type,
    kind: VariableKind,
    initial: Option<ExpressionId>,
    origin: Option<SourceOrigin>,
}

impl Variable {
    pub fn id(&self) -> VariableId {
        self.id
    }

    pub fn source(&self) -> &VariableSource {
        &self.source
    }

    pub fn ty(&self) -> &Type {
        &self.ty
    }

    pub fn kind(&self) -> VariableKind {
        self.kind
    }

    pub fn initial(&self) -> Option<ExpressionId> {
        self.initial
    }

    pub fn origin(&self) -> Option<&SourceOrigin> {
        self.origin.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnaryOperation {
    Positive,
    Negate,
    Not,
    IsNull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BinaryOperation {
    Or,
    And,
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LessThanOrEqual,
    GreaterThanOrEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CallTarget {
    Function(SymbolId),
    Builtin(BuiltinMethod),
    Interface {
        declaration: SymbolId,
        implementation: Option<SymbolId>,
    },
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ExpressionKind {
    Unreachable,
    Constant(Constant),
    Variable(VariableId),
    Output(OutputPath),
    Function(SymbolId),
    Set(Vec<ExpressionId>),
    Some(ExpressionId),
    Null,
    OptionalValue(ExpressionId),
    Unary {
        operation: UnaryOperation,
        operand: ExpressionId,
    },
    Binary {
        operation: BinaryOperation,
        left: ExpressionId,
        right: ExpressionId,
    },
    If {
        condition: ExpressionId,
        then_value: ExpressionId,
        else_value: ExpressionId,
    },
    Elvis {
        optional: ExpressionId,
        fallback: ExpressionId,
    },
    Field {
        receiver: ExpressionId,
        field: FieldId,
        safe: bool,
    },
    Call {
        target: CallTarget,
        receiver: Option<ExpressionId>,
        arguments: Vec<ExpressionId>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expression {
    id: ExpressionId,
    ty: Type,
    kind: ExpressionKind,
    origin: Option<SourceOrigin>,
}

impl Expression {
    pub fn id(&self) -> ExpressionId {
        self.id
    }

    pub fn ty(&self) -> &Type {
        &self.ty
    }

    pub fn kind(&self) -> &ExpressionKind {
        &self.kind
    }

    pub fn origin(&self) -> Option<&SourceOrigin> {
        self.origin.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConstraintKind {
    Assert,
    Target,
    Validity,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Constraint {
    id: ConstraintId,
    condition: ExpressionId,
    kind: ConstraintKind,
    origin: Option<SourceOrigin>,
}

impl Constraint {
    pub fn id(&self) -> ConstraintId {
        self.id
    }

    pub fn condition(&self) -> ExpressionId {
        self.condition
    }

    pub fn kind(&self) -> ConstraintKind {
        self.kind
    }

    pub fn origin(&self) -> Option<&SourceOrigin> {
        self.origin.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WeightedVariable {
    variable: VariableId,
    cost: u64,
}

impl WeightedVariable {
    pub fn new(variable: VariableId, cost: u64) -> Self {
        Self { variable, cost }
    }

    pub fn variable(self) -> VariableId {
        self.variable
    }

    pub fn cost(self) -> u64 {
        self.cost
    }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ObjectiveKind {
    Minimize(ExpressionId),
    MinimizeChanges(Vec<WeightedVariable>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Objective {
    id: ObjectiveId,
    kind: ObjectiveKind,
    origin: Option<SourceOrigin>,
}

impl Objective {
    pub fn id(&self) -> ObjectiveId {
        self.id
    }

    pub fn kind(&self) -> &ObjectiveKind {
        &self.kind
    }

    pub fn origin(&self) -> Option<&SourceOrigin> {
        self.origin.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputCase {
    guard: ExpressionId,
    value: ExpressionId,
    origin: SourceOrigin,
}

impl OutputCase {
    pub fn guard(&self) -> ExpressionId {
        self.guard
    }

    pub fn value(&self) -> ExpressionId {
        self.value
    }

    pub fn origin(&self) -> &SourceOrigin {
        &self.origin
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputDefinition {
    path: OutputPath,
    ty: Type,
    policy: MutationPolicy,
    cases: Vec<OutputCase>,
}

impl OutputDefinition {
    pub fn path(&self) -> &OutputPath {
        &self.path
    }

    pub fn ty(&self) -> &Type {
        &self.ty
    }

    pub fn policy(&self) -> MutationPolicy {
        self.policy
    }

    pub fn cases(&self) -> &[OutputCase] {
        &self.cases
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathDeclaration {
    path: OutputPath,
    ty: Type,
    policy: MutationPolicy,
    origin: SourceOrigin,
}

impl PathDeclaration {
    pub fn path(&self) -> &OutputPath {
        &self.path
    }

    pub fn ty(&self) -> &Type {
        &self.ty
    }

    pub fn policy(&self) -> MutationPolicy {
        self.policy
    }

    pub fn origin(&self) -> &SourceOrigin {
        &self.origin
    }
}

#[derive(Debug, Clone)]
pub struct ConstraintModel {
    module: ModuleId,
    variables: Vec<Variable>,
    expressions: Vec<Expression>,
    constraints: Vec<Constraint>,
    objectives: Vec<Objective>,
    paths: Vec<PathDeclaration>,
    outputs: Vec<OutputDefinition>,
    variable_sources: HashMap<VariableSource, VariableId>,
    output_indices: BTreeMap<OutputPath, usize>,
    path_indices: BTreeMap<OutputPath, usize>,
}

impl ConstraintModel {
    pub fn module(&self) -> ModuleId {
        self.module
    }

    pub fn variables(&self) -> impl ExactSizeIterator<Item = &Variable> {
        self.variables.iter()
    }

    pub fn expressions(&self) -> impl ExactSizeIterator<Item = &Expression> {
        self.expressions.iter()
    }

    pub fn constraints(&self) -> impl ExactSizeIterator<Item = &Constraint> {
        self.constraints.iter()
    }

    pub fn objectives(&self) -> impl ExactSizeIterator<Item = &Objective> {
        self.objectives.iter()
    }

    pub fn outputs(&self) -> impl ExactSizeIterator<Item = &OutputDefinition> {
        self.outputs.iter()
    }

    pub fn paths(&self) -> impl ExactSizeIterator<Item = &PathDeclaration> {
        self.paths.iter()
    }

    pub fn variable(&self, id: VariableId) -> Option<&Variable> {
        self.variables.get(id.0 as usize)
    }

    pub fn expression(&self, id: ExpressionId) -> Option<&Expression> {
        self.expressions.get(id.0 as usize)
    }

    pub fn constraint(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraints.get(id.0 as usize)
    }

    pub fn objective(&self, id: ObjectiveId) -> Option<&Objective> {
        self.objectives.get(id.0 as usize)
    }

    pub fn variable_for_source(&self, source: &VariableSource) -> Option<VariableId> {
        self.variable_sources.get(source).copied()
    }

    pub fn output(&self, path: &OutputPath) -> Option<&OutputDefinition> {
        self.output_indices
            .get(path)
            .and_then(|index| self.outputs.get(*index))
    }

    pub fn path(&self, path: &OutputPath) -> Option<&PathDeclaration> {
        self.path_indices
            .get(path)
            .and_then(|index| self.paths.get(*index))
    }

    pub(crate) fn new(module: ModuleId) -> Self {
        Self {
            module,
            variables: Vec::new(),
            expressions: Vec::new(),
            constraints: Vec::new(),
            objectives: Vec::new(),
            paths: Vec::new(),
            outputs: Vec::new(),
            variable_sources: HashMap::new(),
            output_indices: BTreeMap::new(),
            path_indices: BTreeMap::new(),
        }
    }

    pub(crate) fn push_variable(&mut self, variable: Variable) {
        self.variable_sources
            .insert(variable.source.clone(), variable.id);
        self.variables.push(variable);
    }

    pub(crate) fn set_variable_initial(&mut self, variable: VariableId, initial: ExpressionId) {
        self.variables[variable.0 as usize].initial = Some(initial);
    }

    pub(crate) fn push_expression(&mut self, expression: Expression) {
        self.expressions.push(expression);
    }

    pub(crate) fn push_constraint(&mut self, constraint: Constraint) {
        self.constraints.push(constraint);
    }

    pub(crate) fn push_objective(&mut self, objective: Objective) {
        self.objectives.push(objective);
    }

    pub(crate) fn push_output_case(
        &mut self,
        path: OutputPath,
        ty: Type,
        policy: MutationPolicy,
        case: OutputCase,
    ) {
        if let Some(index) = self.output_indices.get(&path).copied() {
            self.outputs[index].cases.push(case);
        } else {
            let index = self.outputs.len();
            self.output_indices.insert(path.clone(), index);
            self.outputs.push(OutputDefinition {
                path,
                ty,
                policy,
                cases: vec![case],
            });
        }
    }

    pub(crate) fn push_path_declaration(&mut self, declaration: PathDeclaration) {
        let index = self.paths.len();
        self.path_indices.insert(declaration.path.clone(), index);
        self.paths.push(declaration);
    }

    pub(crate) fn variable_count(&self) -> usize {
        self.variables.len()
    }

    pub(crate) fn expression_count(&self) -> usize {
        self.expressions.len()
    }

    pub(crate) fn constraint_count(&self) -> usize {
        self.constraints.len()
    }

    pub(crate) fn objective_count(&self) -> usize {
        self.objectives.len()
    }
}

pub(crate) fn variable(
    id: VariableId,
    source: VariableSource,
    ty: Type,
    kind: VariableKind,
    initial: Option<ExpressionId>,
    origin: Option<SourceOrigin>,
) -> Variable {
    Variable {
        id,
        source,
        ty,
        kind,
        initial,
        origin,
    }
}

pub(crate) fn expression(
    id: ExpressionId,
    ty: Type,
    kind: ExpressionKind,
    origin: Option<SourceOrigin>,
) -> Expression {
    Expression {
        id,
        ty,
        kind,
        origin,
    }
}

pub(crate) fn constraint(
    id: ConstraintId,
    condition: ExpressionId,
    kind: ConstraintKind,
    origin: Option<SourceOrigin>,
) -> Constraint {
    Constraint {
        id,
        condition,
        kind,
        origin,
    }
}

pub(crate) fn objective(
    id: ObjectiveId,
    kind: ObjectiveKind,
    origin: Option<SourceOrigin>,
) -> Objective {
    Objective { id, kind, origin }
}

pub(crate) fn output_case(
    guard: ExpressionId,
    value: ExpressionId,
    origin: SourceOrigin,
) -> OutputCase {
    OutputCase {
        guard,
        value,
        origin,
    }
}

pub(crate) fn path_declaration(
    path: OutputPath,
    ty: Type,
    policy: MutationPolicy,
    origin: SourceOrigin,
) -> PathDeclaration {
    PathDeclaration {
        path,
        ty,
        policy,
        origin,
    }
}
