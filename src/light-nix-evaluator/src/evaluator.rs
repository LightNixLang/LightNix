use std::collections::{BTreeSet, HashMap, HashSet};

use light_nix_name_resolver::{
    Declaration, ModuleId, NameResolution, Res, SymbolId, SymbolKind, VariantId,
};
use light_nix_parser::ast::{
    AST, AccessOperator, Array, AssignValue, BinaryOperator, Block, ClosureBody, ClosureExpression,
    CollectionKind, ElseBranchValue, Expression, FunctionAttribute, FunctionCall, FunctionDefine,
    LetStatement, Literal, MatchArm, MutationPolicyKind, Pattern, Primary, PrimaryAccess, Source,
    Statement, Statements, TypeOperator, UnaryOperator, Value,
};
use light_nix_type_checker::{BuiltinMethod, MemberResolution, Type, TypeCheckResult};

use crate::{
    EvaluationError, EvaluationErrorKind, EvaluationSnapshot, OutputEntry, OutputPath,
    RuntimeValue, SourceOrigin,
};

#[derive(Debug, Clone, Default)]
pub struct EvaluationInputs {
    values: HashMap<SymbolId, RuntimeValue>,
    tunable_overrides: HashMap<SymbolId, RuntimeValue>,
    output_overrides: HashMap<OutputPath, OutputOverride>,
}

impl EvaluationInputs {
    pub fn insert(&mut self, symbol: SymbolId, value: RuntimeValue) {
        self.values.insert(symbol, value);
    }

    pub fn override_tunable(&mut self, symbol: SymbolId, value: RuntimeValue) {
        self.tunable_overrides.insert(symbol, value);
    }

    pub fn override_output(
        &mut self,
        path: OutputPath,
        value: Option<RuntimeValue>,
        origin: SourceOrigin,
    ) {
        self.output_overrides
            .insert(path, OutputOverride { value, origin });
    }
}

#[derive(Debug, Clone)]
struct OutputOverride {
    value: Option<RuntimeValue>,
    origin: SourceOrigin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TunableValue {
    pub value: RuntimeValue,
    pub cost: u64,
    pub origin: SourceOrigin,
}

#[derive(Debug)]
pub struct EvaluationResult {
    snapshot: EvaluationSnapshot,
    symbol_values: HashMap<SymbolId, RuntimeValue>,
    tunables: HashMap<SymbolId, TunableValue>,
    errors: Vec<EvaluationError>,
}

impl EvaluationResult {
    pub fn snapshot(&self) -> &EvaluationSnapshot {
        &self.snapshot
    }

    pub fn symbol_value(&self, symbol: SymbolId) -> Option<&RuntimeValue> {
        self.symbol_values.get(&symbol)
    }

    pub fn tunable(&self, symbol: SymbolId) -> Option<&TunableValue> {
        self.tunables.get(&symbol)
    }

    pub fn tunables(&self) -> impl ExactSizeIterator<Item = (SymbolId, &TunableValue)> {
        self.tunables.iter().map(|(symbol, value)| (*symbol, value))
    }

    pub fn errors(&self) -> &[EvaluationError] {
        &self.errors
    }

    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn evaluate_module<'ast, 'input, 'allocator>(
    source: &'ast Source<'input, 'allocator>,
    resolution: &NameResolution<'ast>,
    types: &TypeCheckResult<'ast>,
    inputs: &EvaluationInputs,
) -> EvaluationResult {
    Evaluator::new(resolution, types, inputs).evaluate(source)
}

#[derive(Debug, Clone)]
struct TrackedValue {
    value: RuntimeValue,
    dependencies: BTreeSet<SymbolId>,
    opaque_dependencies: BTreeSet<SymbolId>,
    path: Option<OutputPath>,
}

impl TrackedValue {
    fn pure(value: RuntimeValue) -> Self {
        Self {
            value,
            dependencies: BTreeSet::new(),
            opaque_dependencies: BTreeSet::new(),
            path: None,
        }
    }

    fn error() -> Self {
        Self::pure(RuntimeValue::Error)
    }

    fn without_path(mut self) -> Self {
        self.path = None;
        self
    }
}

#[derive(Clone)]
struct ClosureInstance<'ast, 'input, 'allocator> {
    expression: &'ast ClosureExpression<'input, 'allocator>,
    captures: HashMap<SymbolId, TrackedValue>,
}

enum Signal {
    Return(Box<TrackedValue>),
    Error,
}

type Eval<T> = Result<T, Signal>;

struct Evaluator<'ast, 'input, 'allocator, 'context, 'inputs> {
    resolution: &'context NameResolution<'ast>,
    types: &'context TypeCheckResult<'ast>,
    inputs: &'inputs EvaluationInputs,
    module: ModuleId,
    top_level_lets: HashMap<SymbolId, &'ast LetStatement<'input, 'allocator>>,
    functions: HashMap<SymbolId, &'ast FunctionDefine<'input, 'allocator>>,
    closures: Vec<ClosureInstance<'ast, 'input, 'allocator>>,
    variants: HashMap<VariantId, Option<&'ast Expression<'input, 'allocator>>>,
    global_values: HashMap<SymbolId, TrackedValue>,
    frames: Vec<HashMap<SymbolId, TrackedValue>>,
    evaluating_values: HashSet<SymbolId>,
    calling_functions: HashSet<SymbolId>,
    control_dependencies: BTreeSet<SymbolId>,
    control_opaque_dependencies: BTreeSet<SymbolId>,
    snapshot: EvaluationSnapshot,
    tunables: HashMap<SymbolId, TunableValue>,
    errors: Vec<EvaluationError>,
}

impl<'ast, 'input, 'allocator, 'context, 'inputs>
    Evaluator<'ast, 'input, 'allocator, 'context, 'inputs>
{
    fn new(
        resolution: &'context NameResolution<'ast>,
        types: &'context TypeCheckResult<'ast>,
        inputs: &'inputs EvaluationInputs,
    ) -> Self {
        Self {
            resolution,
            types,
            inputs,
            module: resolution.module(),
            top_level_lets: HashMap::new(),
            functions: HashMap::new(),
            closures: Vec::new(),
            variants: HashMap::new(),
            global_values: HashMap::new(),
            frames: Vec::new(),
            evaluating_values: HashSet::new(),
            calling_functions: HashSet::new(),
            control_dependencies: BTreeSet::new(),
            control_opaque_dependencies: BTreeSet::new(),
            snapshot: EvaluationSnapshot::default(),
            tunables: HashMap::new(),
            errors: Vec::new(),
        }
    }

    fn evaluate(mut self, source: &'ast Source<'input, 'allocator>) -> EvaluationResult {
        self.register_module(source);
        for (path, output) in &self.inputs.output_overrides {
            let Some(value) = &output.value else {
                continue;
            };
            self.snapshot.insert(
                path.clone(),
                OutputEntry {
                    value: value.clone(),
                    dependencies: BTreeSet::new(),
                    opaque_dependencies: BTreeSet::new(),
                    origin: output.origin.clone(),
                },
            );
        }
        if let Err(Signal::Return(_)) = self.eval_statements(source, true) {
            // The type checker owns the diagnostic for a module-level return.
        }
        let mut symbol_values = self
            .global_values
            .iter()
            .map(|(symbol, value)| (*symbol, value.value.clone()))
            .collect::<HashMap<_, _>>();
        for (symbol, value) in &self.inputs.values {
            symbol_values
                .entry(*symbol)
                .or_insert_with(|| value.clone());
        }
        EvaluationResult {
            snapshot: self.snapshot,
            symbol_values,
            tunables: self.tunables,
            errors: self.errors,
        }
    }

    fn register_module(&mut self, statements: &'ast Statements<'input, 'allocator>) {
        for statement in statements.statements {
            match statement {
                Statement::LetStatement(node) => {
                    if let Some(symbol) = self.symbol_declaration(&node.name) {
                        self.top_level_lets.insert(symbol, node);
                    }
                }
                Statement::FunctionDefine(node) => self.register_function(node),
                Statement::EnumDefine(node) => {
                    for variant in node.variants {
                        if let Some(Declaration::EnumVariant(id)) =
                            self.resolution.declaration_of_literal(&variant.name)
                        {
                            self.variants.insert(id, variant.value);
                        }
                    }
                }
                Statement::InterfaceDefine(node) => {
                    for method in node.methods {
                        self.register_function(method);
                    }
                }
                Statement::ImplementsDefine(node) => {
                    for method in node.methods {
                        self.register_function(method);
                    }
                }
                _ => {}
            }
        }
    }

    fn register_function(&mut self, function: &'ast FunctionDefine<'input, 'allocator>) {
        if let Some(symbol) = self.symbol_declaration(&function.name) {
            self.functions.insert(symbol, function);
        }
    }

    fn register_block_functions(&mut self, statements: &'ast Statements<'input, 'allocator>) {
        for statement in statements.statements {
            match statement {
                Statement::FunctionDefine(function) => self.register_function(function),
                Statement::InterfaceDefine(interface) => {
                    for method in interface.methods {
                        self.register_function(method);
                    }
                }
                Statement::ImplementsDefine(implementation) => {
                    for method in implementation.methods {
                        self.register_function(method);
                    }
                }
                _ => {}
            }
        }
    }

    fn eval_statements(
        &mut self,
        statements: &'ast Statements<'input, 'allocator>,
        module_scope: bool,
    ) -> Eval<TrackedValue> {
        self.register_block_functions(statements);
        let mut result = TrackedValue::pure(RuntimeValue::Unit);
        for statement in statements.statements {
            result = self.eval_statement(statement, module_scope)?;
        }
        Ok(result)
    }

    fn eval_statement(
        &mut self,
        statement: &'ast Statement<'input, 'allocator>,
        module_scope: bool,
    ) -> Eval<TrackedValue> {
        match statement {
            Statement::ImportStatement(_) | Statement::TypeDefine(_) | Statement::UseDeclare(_) => {
                Ok(TrackedValue::pure(RuntimeValue::Unit))
            }
            Statement::EnumDefine(node) => {
                for variant in node.variants {
                    if let Some(Declaration::EnumVariant(id)) =
                        self.resolution.declaration_of_literal(&variant.name)
                    {
                        self.variants.insert(id, variant.value);
                    }
                }
                Ok(TrackedValue::pure(RuntimeValue::Unit))
            }
            Statement::InterfaceDefine(_) | Statement::ImplementsDefine(_) => {
                Ok(TrackedValue::pure(RuntimeValue::Unit))
            }
            Statement::LetStatement(node) => {
                let Some(symbol) = self.symbol_declaration(&node.name) else {
                    return Ok(TrackedValue::error());
                };
                if module_scope {
                    self.eval_symbol(symbol, node.name.span.clone())
                } else {
                    let value = self.eval_let(symbol, node)?;
                    self.bind_local(symbol, value.clone());
                    Ok(value)
                }
            }
            Statement::AssertStatement(node) => {
                let condition = self.eval_expression(node.condition)?;
                match condition.value {
                    RuntimeValue::Bool(true) => Ok(TrackedValue::pure(RuntimeValue::Unit)),
                    RuntimeValue::Bool(false) => {
                        let message = match node.message {
                            Some(message) => {
                                let value = self.eval_expression(message)?;
                                Some(self.expect_string(value, message.span())?)
                            }
                            None => None,
                        };
                        self.fail(
                            EvaluationErrorKind::AssertionFailed { message },
                            node.span.clone(),
                        )
                    }
                    found => self.fail(
                        EvaluationErrorKind::ExpectedBoolean { found },
                        node.condition.span(),
                    ),
                }
            }
            Statement::AssignStatement(node) => {
                let Some(path) = self.assignment_path(node.target) else {
                    return self.fail(
                        EvaluationErrorKind::InvalidAssignmentTarget,
                        node.target.span(),
                    );
                };
                let Some(target_type) = self.types.expression_type(node.target).cloned() else {
                    return self.fail(
                        EvaluationErrorKind::InvalidAssignmentTarget,
                        node.target.span(),
                    );
                };
                self.eval_assign_value(path, target_type, &node.value, node.span.clone())
            }
            Statement::FunctionDefine(_) => Ok(TrackedValue::pure(RuntimeValue::Unit)),
            Statement::Expression(expression) => self.eval_expression(expression),
        }
    }

    fn eval_assign_value(
        &mut self,
        path: OutputPath,
        target_type: Type,
        value: &'ast AssignValue<'input, 'allocator>,
        span: std::ops::Range<usize>,
    ) -> Eval<TrackedValue> {
        match value {
            AssignValue::Expression(expression) => {
                if self.inputs.output_overrides.contains_key(&path) {
                    return Ok(TrackedValue::pure(RuntimeValue::Unit));
                }
                let mut value = self.eval_expression(expression)?;
                value
                    .dependencies
                    .extend(self.control_dependencies.iter().copied());
                value
                    .opaque_dependencies
                    .extend(self.control_opaque_dependencies.iter().copied());
                let entry = OutputEntry {
                    value: value.value.clone(),
                    dependencies: value.dependencies.clone(),
                    opaque_dependencies: value.opaque_dependencies.clone(),
                    origin: SourceOrigin {
                        module: self.module,
                        span: span.clone(),
                    },
                };
                if self.snapshot.contains(&path) {
                    return self.fail(EvaluationErrorKind::DuplicateAssignment { path }, span);
                }
                self.snapshot.insert(path, entry);
            }
            AssignValue::Nested(nested) => {
                for field in nested.fields {
                    let Some((field_id, field_type)) =
                        self.types.named_field(&target_type, field.name.value)
                    else {
                        return self.fail(
                            EvaluationErrorKind::InvalidAssignmentTarget,
                            field.name.span.clone(),
                        );
                    };
                    self.eval_assign_value(
                        path.clone().field(field_id),
                        field_type,
                        &field.value,
                        field.span.clone(),
                    )?;
                }
            }
        }
        Ok(TrackedValue::pure(RuntimeValue::Unit))
    }

    fn eval_block(&mut self, block: &'ast Block<'input, 'allocator>) -> Eval<TrackedValue> {
        self.frames.push(HashMap::new());
        let result = self.eval_statements(&block.statements, false);
        self.frames.pop();
        result
    }

    fn eval_expression(
        &mut self,
        expression: &'ast Expression<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        match expression {
            Expression::If(node) => {
                let mut guards = BTreeSet::new();
                let mut opaque_guards = BTreeSet::new();
                let condition = self.eval_expression(node.branch.condition)?;
                guards.extend(condition.dependencies.iter().copied());
                opaque_guards.extend(condition.opaque_dependencies.iter().copied());
                if self.expect_bool(condition, node.branch.condition.span())? {
                    return self.eval_controlled_block(node.branch.body, &guards, &opaque_guards);
                }
                for branch in node.else_branches {
                    match branch.value {
                        ElseBranchValue::If(branch) => {
                            let condition = self.eval_expression(branch.condition)?;
                            guards.extend(condition.dependencies.iter().copied());
                            opaque_guards.extend(condition.opaque_dependencies.iter().copied());
                            if self.expect_bool(condition, branch.condition.span())? {
                                return self.eval_controlled_block(
                                    branch.body,
                                    &guards,
                                    &opaque_guards,
                                );
                            }
                        }
                        ElseBranchValue::Block(block) => {
                            return self.eval_controlled_block(block, &guards, &opaque_guards);
                        }
                    }
                }
                Ok(TrackedValue {
                    value: RuntimeValue::Unit,
                    dependencies: guards,
                    opaque_dependencies: BTreeSet::new(),
                    path: None,
                })
            }
            Expression::Match(node) => {
                let matched = self.eval_expression(node.value)?;
                for arm in node.arms {
                    if let Some(bindings) = self.match_pattern(&arm.pattern, &matched) {
                        return self.eval_match_arm(
                            arm,
                            bindings,
                            &matched.dependencies,
                            &matched.opaque_dependencies,
                        );
                    }
                }
                self.fail(EvaluationErrorKind::NoMatchingPattern, node.span.clone())
            }
            Expression::Return(node) => {
                let mut value = match node.value {
                    Some(value) => self.eval_expression(value)?,
                    None => TrackedValue::pure(RuntimeValue::Unit),
                };
                value
                    .dependencies
                    .extend(self.control_dependencies.iter().copied());
                value
                    .opaque_dependencies
                    .extend(self.control_opaque_dependencies.iter().copied());
                Err(Signal::Return(Box::new(value)))
            }
            Expression::Throw(node) => {
                let message = match node.message {
                    Some(message) => {
                        let value = self.eval_expression(message)?;
                        Some(self.expect_string(value, message.span())?)
                    }
                    None => None,
                };
                self.fail(EvaluationErrorKind::Thrown { message }, node.span.clone())
            }
            Expression::Closure(node) => self.eval_closure(node),
            Expression::Elvis(node) => {
                let optional = self.eval_expression(node.optional)?;
                match optional.value {
                    RuntimeValue::Optional(Some(value)) => Ok(TrackedValue {
                        value: *value,
                        dependencies: optional.dependencies,
                        opaque_dependencies: optional.opaque_dependencies,
                        path: optional.path,
                    }),
                    RuntimeValue::Optional(None) => {
                        let fallback = self.eval_expression(node.fallback)?;
                        Ok(merge_tracked(optional, fallback, |_, right| right))
                    }
                    _ => self.fail(
                        EvaluationErrorKind::InvalidBuiltinCall,
                        node.optional.span(),
                    ),
                }
            }
            Expression::TypeOperation(node) => {
                let value = self.eval_expression(node.value)?;
                let target = self
                    .types
                    .type_info_type(node.target)
                    .cloned()
                    .unwrap_or(Type::Error);
                let matches = self.runtime_matches_type(&value.value, &target);
                let result = match node.operator.value {
                    TypeOperator::Is => RuntimeValue::Bool(matches),
                    TypeOperator::SafeCast => {
                        RuntimeValue::optional(matches.then(|| value.value.clone()))
                    }
                };
                Ok(TrackedValue {
                    value: result,
                    dependencies: value.dependencies,
                    opaque_dependencies: value.opaque_dependencies,
                    path: value.path,
                })
            }
            Expression::Binary(node) => self.eval_binary(node),
            Expression::Unary(node) => {
                let operand = self.eval_expression(node.operand)?;
                let value = match (node.operator.value, operand.value) {
                    (UnaryOperator::Positive, RuntimeValue::Int(value)) => RuntimeValue::Int(value),
                    (UnaryOperator::Positive, RuntimeValue::Float(value)) => {
                        RuntimeValue::Float(value)
                    }
                    (UnaryOperator::Negate, RuntimeValue::Int(value)) => {
                        RuntimeValue::Int(value.checked_neg().ok_or_else(|| {
                            self.errors.push(EvaluationError {
                                kind: EvaluationErrorKind::IntegerOverflow,
                                module: self.module,
                                span: node.span.clone(),
                            });
                            Signal::Error
                        })?)
                    }
                    (UnaryOperator::Negate, RuntimeValue::Float(value)) => {
                        RuntimeValue::Float(-value)
                    }
                    (_, found) => {
                        return self.fail(
                            EvaluationErrorKind::ExpectedNumber { found },
                            node.operand.span(),
                        );
                    }
                };
                Ok(TrackedValue { value, ..operand })
            }
            Expression::Primary(primary) => self.eval_primary(primary),
        }
    }

    fn eval_closure(
        &mut self,
        expression: &'ast ClosureExpression<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        let mut captures = HashMap::new();
        for frame in &self.frames {
            captures.extend(frame.iter().map(|(symbol, value)| (*symbol, value.clone())));
        }
        let id = u32::try_from(self.closures.len()).map_err(|_| Signal::Error)?;
        self.closures.push(ClosureInstance {
            expression,
            captures,
        });
        Ok(TrackedValue::pure(RuntimeValue::Closure(id)))
    }

    fn runtime_matches_type(&self, value: &RuntimeValue, ty: &Type) -> bool {
        match (value, ty) {
            (RuntimeValue::Error, Type::Error) => true,
            (RuntimeValue::Unit, Type::Unit) => true,
            (RuntimeValue::Bool(_), Type::Bool) => true,
            (RuntimeValue::Int(_), Type::Int) => true,
            (RuntimeValue::Float(_), Type::Float) => true,
            (RuntimeValue::String(_), Type::String) => true,
            (RuntimeValue::List(values), Type::List(element)) => values
                .iter()
                .all(|value| self.runtime_matches_type(value, element)),
            (RuntimeValue::Set(values), Type::Set(element)) => values
                .iter()
                .all(|value| self.runtime_matches_type(value, element)),
            (RuntimeValue::AttrSet(values), Type::AttrSet(element)) => values
                .values()
                .all(|value| self.runtime_matches_type(value, element)),
            (RuntimeValue::Optional(None), Type::Optional(_)) => true,
            (RuntimeValue::Optional(Some(value)), Type::Optional(inner)) => {
                self.runtime_matches_type(value, inner)
            }
            (RuntimeValue::Record(record), Type::Named(expected, _)) => record.ty == *expected,
            (RuntimeValue::Enum(variant), Type::Named(expected, _)) => self
                .resolution
                .variants()
                .iter()
                .any(|candidate| candidate.id == *variant && candidate.owner == *expected),
            (RuntimeValue::Function(_), Type::Function(_))
            | (RuntimeValue::Closure(_), Type::Function(_)) => true,
            (_, Type::Union(alternatives)) => alternatives
                .iter()
                .any(|ty| self.runtime_matches_type(value, ty)),
            (_, Type::Never) => false,
            _ => false,
        }
    }

    fn eval_controlled_block(
        &mut self,
        block: &'ast Block<'input, 'allocator>,
        dependencies: &BTreeSet<SymbolId>,
        opaque_dependencies: &BTreeSet<SymbolId>,
    ) -> Eval<TrackedValue> {
        let previous = self.control_dependencies.clone();
        let previous_opaque = self.control_opaque_dependencies.clone();
        self.control_dependencies
            .extend(dependencies.iter().copied());
        self.control_opaque_dependencies
            .extend(opaque_dependencies.iter().copied());
        let result = self.eval_block(block);
        self.control_dependencies = previous;
        self.control_opaque_dependencies = previous_opaque;
        result.map(|mut value| {
            value.dependencies.extend(dependencies.iter().copied());
            value
                .opaque_dependencies
                .extend(opaque_dependencies.iter().copied());
            value
        })
    }

    fn eval_match_arm(
        &mut self,
        arm: &'ast MatchArm<'input, 'allocator>,
        bindings: Vec<(SymbolId, TrackedValue)>,
        dependencies: &BTreeSet<SymbolId>,
        opaque_dependencies: &BTreeSet<SymbolId>,
    ) -> Eval<TrackedValue> {
        let previous_control = self.control_dependencies.clone();
        let previous_opaque = self.control_opaque_dependencies.clone();
        self.control_dependencies
            .extend(dependencies.iter().copied());
        self.control_opaque_dependencies
            .extend(opaque_dependencies.iter().copied());
        self.frames.push(bindings.into_iter().collect());
        let result = self.eval_expression(arm.value);
        self.frames.pop();
        self.control_dependencies = previous_control;
        self.control_opaque_dependencies = previous_opaque;
        result.map(|mut value| {
            value.dependencies.extend(dependencies.iter().copied());
            value
                .opaque_dependencies
                .extend(opaque_dependencies.iter().copied());
            value
        })
    }

    fn eval_binary(
        &mut self,
        node: &'ast light_nix_parser::ast::BinaryExpression<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        let left = self.eval_expression(node.left)?;
        if node.operator.value == BinaryOperator::And
            && !self.expect_bool(left.clone(), node.left.span())?
        {
            return Ok(TrackedValue {
                value: RuntimeValue::Bool(false),
                dependencies: left.dependencies,
                opaque_dependencies: left.opaque_dependencies,
                path: None,
            });
        }
        if node.operator.value == BinaryOperator::Or
            && self.expect_bool(left.clone(), node.left.span())?
        {
            return Ok(TrackedValue {
                value: RuntimeValue::Bool(true),
                dependencies: left.dependencies,
                opaque_dependencies: left.opaque_dependencies,
                path: None,
            });
        }
        let right = self.eval_expression(node.right)?;
        let operator = node.operator.value;
        merge_tracked_result(left, right, |left, right| match operator {
            BinaryOperator::Or => bool_binary(left, right, |a, b| a || b),
            BinaryOperator::And => bool_binary(left, right, |a, b| a && b),
            BinaryOperator::Equal => Ok(RuntimeValue::Bool(left == right)),
            BinaryOperator::NotEqual => Ok(RuntimeValue::Bool(left != right)),
            BinaryOperator::LessThan => compare_binary(left, right, |ordering| ordering.is_lt()),
            BinaryOperator::GreaterThan => compare_binary(left, right, |ordering| ordering.is_gt()),
            BinaryOperator::LessThanOrEqual => {
                compare_binary(left, right, |ordering| !ordering.is_gt())
            }
            BinaryOperator::GreaterThanOrEqual => {
                compare_binary(left, right, |ordering| !ordering.is_lt())
            }
            BinaryOperator::Add => match (left, right) {
                (RuntimeValue::Int(left), RuntimeValue::Int(right)) => left
                    .checked_add(right)
                    .map(RuntimeValue::Int)
                    .ok_or(EvaluationErrorKind::IntegerOverflow),
                (RuntimeValue::Float(left), RuntimeValue::Float(right)) => {
                    finite_float(left + right)
                }
                (RuntimeValue::String(left), RuntimeValue::String(right)) => {
                    Ok(RuntimeValue::String(left + &right))
                }
                (left, _) => Err(EvaluationErrorKind::ExpectedNumber { found: left }),
            },
            BinaryOperator::Subtract => numeric_binary(left, right, i64::checked_sub, |a, b| a - b),
            BinaryOperator::Multiply => numeric_binary(left, right, i64::checked_mul, |a, b| a * b),
            BinaryOperator::Divide => match (left, right) {
                (_, RuntimeValue::Int(0)) | (_, RuntimeValue::Float(0.0)) => {
                    Err(EvaluationErrorKind::DivisionByZero)
                }
                (RuntimeValue::Int(left), RuntimeValue::Int(right)) => left
                    .checked_div(right)
                    .map(RuntimeValue::Int)
                    .ok_or(EvaluationErrorKind::IntegerOverflow),
                (RuntimeValue::Float(left), RuntimeValue::Float(right)) => {
                    finite_float(left / right)
                }
                (left, _) => Err(EvaluationErrorKind::ExpectedNumber { found: left }),
            },
        })
        .map_err(|kind| {
            self.errors.push(EvaluationError {
                kind,
                module: self.module,
                span: node.span.clone(),
            });
            Signal::Error
        })
    }

    fn eval_primary(&mut self, primary: &'ast Primary<'input, 'allocator>) -> Eval<TrackedValue> {
        let mut current = self.eval_value(&primary.value)?;
        for access in primary.accesses {
            current = self.eval_access(current, access)?;
        }
        Ok(current)
    }

    fn eval_value(&mut self, value: &'ast Value<'input, 'allocator>) -> Eval<TrackedValue> {
        match value {
            Value::Array(array) => self.eval_array(array),
            Value::Literal(literal) => {
                let mut value = match self.resolution.resolve_literal(&literal.literal) {
                    Some(Res::Symbol(symbol)) => {
                        let mut value = self.eval_symbol(symbol, literal.literal.span.clone())?;
                        let path = OutputPath::root(symbol);
                        if let Some(entry) = self.snapshot.get(&path) {
                            value.value = entry.value.clone();
                            value
                                .dependencies
                                .extend(entry.dependencies.iter().copied());
                        }
                        value.path = Some(path);
                        value
                    }
                    Some(Res::Type(ty)) => TrackedValue::pure(RuntimeValue::Type(ty)),
                    Some(Res::Module(module)) => TrackedValue::pure(RuntimeValue::Module(module)),
                    Some(Res::EnumVariant(variant)) => self.eval_variant(variant)?,
                    _ => TrackedValue::error(),
                };
                if let Some(call) = literal.call {
                    value = self.eval_call(value, None, call, literal.literal.span.clone())?;
                }
                Ok(value)
            }
            Value::Some(some) => {
                let value = match some.value {
                    Some(value) => self.eval_expression(value)?,
                    None => TrackedValue::error(),
                };
                Ok(TrackedValue {
                    value: RuntimeValue::optional(Some(value.value)),
                    dependencies: value.dependencies,
                    opaque_dependencies: value.opaque_dependencies,
                    path: None,
                })
            }
            Value::Numeric(number) => {
                parse_number(number.value)
                    .map(TrackedValue::pure)
                    .map_err(|kind| {
                        self.errors.push(EvaluationError {
                            kind,
                            module: self.module,
                            span: number.span.clone(),
                        });
                        Signal::Error
                    })
            }
            Value::String(string) => decode_string(string.value)
                .map(|value| TrackedValue::pure(RuntimeValue::String(value)))
                .map_err(|kind| {
                    self.errors.push(EvaluationError {
                        kind,
                        module: self.module,
                        span: string.span.clone(),
                    });
                    Signal::Error
                }),
            Value::Boolean(boolean) => Ok(TrackedValue::pure(RuntimeValue::Bool(boolean.value))),
            Value::Null(_) => Ok(TrackedValue::pure(RuntimeValue::optional(None))),
        }
    }

    fn eval_array(&mut self, array: &'ast Array<'input, 'allocator>) -> Eval<TrackedValue> {
        let mut values = Vec::new();
        let mut dependencies = BTreeSet::new();
        let mut opaque_dependencies = BTreeSet::new();
        for value in array.values {
            let value = self.eval_value(value)?;
            dependencies.extend(value.dependencies);
            opaque_dependencies.extend(value.opaque_dependencies);
            match array.kind {
                CollectionKind::List => values.push(value.value),
                CollectionKind::Set if !values.contains(&value.value) => values.push(value.value),
                CollectionKind::Set => {}
            }
        }
        Ok(TrackedValue {
            value: match array.kind {
                CollectionKind::List => RuntimeValue::List(values),
                CollectionKind::Set => RuntimeValue::Set(values),
            },
            dependencies,
            opaque_dependencies,
            path: None,
        })
    }

    fn eval_access(
        &mut self,
        receiver: TrackedValue,
        access: &'ast PrimaryAccess<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        if access.operator.value == AccessOperator::SafeDot {
            return match receiver.value {
                RuntimeValue::Optional(None) => Ok(TrackedValue {
                    value: RuntimeValue::optional(None),
                    dependencies: receiver.dependencies,
                    opaque_dependencies: receiver.opaque_dependencies,
                    path: None,
                }),
                RuntimeValue::Optional(Some(value)) => {
                    let inner = TrackedValue {
                        value: *value,
                        dependencies: receiver.dependencies,
                        opaque_dependencies: receiver.opaque_dependencies,
                        path: receiver.path,
                    };
                    let value = self.eval_regular_access(inner, access)?;
                    Ok(match value.value {
                        RuntimeValue::Optional(_) => value,
                        other => TrackedValue {
                            value: RuntimeValue::optional(Some(other)),
                            dependencies: value.dependencies,
                            opaque_dependencies: value.opaque_dependencies,
                            path: None,
                        },
                    })
                }
                _ => self.eval_regular_access(receiver, access),
            };
        }
        self.eval_regular_access(receiver, access)
    }

    fn eval_regular_access(
        &mut self,
        receiver: TrackedValue,
        access: &'ast PrimaryAccess<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        if matches!(
            self.types.member_resolution(access),
            Some(MemberResolution::AttrSetKey)
        ) {
            return self.eval_attr_set_key(receiver, access);
        }
        let Some(member) = access.named_member() else {
            return self.fail(
                EvaluationErrorKind::InvalidBuiltinCall,
                access.member_span(),
            );
        };
        match self.resolution.resolve_literal(member) {
            Some(Res::EnumVariant(variant)) => return self.eval_variant(variant),
            Some(Res::Symbol(symbol)) => {
                let mut value = self.eval_symbol(symbol, member.span.clone())?;
                if let Some(call) = access.call {
                    value = self.eval_call(value, None, call, member.span.clone())?;
                }
                return Ok(value);
            }
            _ => {}
        }
        match self.types.member_resolution(access) {
            Some(MemberResolution::Field(field)) => self.eval_field(receiver, *field, access),
            Some(MemberResolution::AttrSetKey) => unreachable!("handled before named access"),
            Some(MemberResolution::Builtin(method)) => {
                let Some(call) = access.call else {
                    return self.fail(EvaluationErrorKind::InvalidBuiltinCall, member.span.clone());
                };
                self.eval_builtin(receiver, *method, call, member.span.clone())
            }
            Some(MemberResolution::InterfaceMethod {
                implementation: Some(method),
                ..
            }) => {
                let Some(call) = access.call else {
                    return self.fail(EvaluationErrorKind::UnresolvedDispatch, member.span.clone());
                };
                let function = TrackedValue::pure(RuntimeValue::Function(*method));
                self.eval_call(function, Some(receiver), call, member.span.clone())
            }
            Some(MemberResolution::InterfaceMethod {
                implementation: None,
                ..
            }) => self.fail(EvaluationErrorKind::UnresolvedDispatch, member.span.clone()),
            None => self.fail(EvaluationErrorKind::InvalidBuiltinCall, member.span.clone()),
        }
    }

    fn eval_attr_set_key(
        &mut self,
        receiver: TrackedValue,
        access: &'ast PrimaryAccess<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        let Some(key) = access.key() else {
            return self.fail(
                EvaluationErrorKind::InvalidBuiltinCall,
                access.member_span(),
            );
        };
        let key = decode_string(key.value).map_err(|kind| {
            self.errors.push(EvaluationError {
                kind,
                module: self.module,
                span: key.span.clone(),
            });
            Signal::Error
        })?;
        let path = receiver.path.clone().map(|path| path.key(key.clone()));
        if let Some(entry) = path.as_ref().and_then(|path| self.snapshot.get(path)) {
            let mut dependencies = receiver.dependencies;
            dependencies.extend(entry.dependencies.iter().copied());
            let mut opaque_dependencies = receiver.opaque_dependencies;
            opaque_dependencies.extend(entry.opaque_dependencies.iter().copied());
            return Ok(TrackedValue {
                value: entry.value.clone(),
                dependencies,
                opaque_dependencies,
                path,
            });
        }
        let RuntimeValue::AttrSet(values) = receiver.value else {
            return self.fail(
                EvaluationErrorKind::ExpectedAttrSet {
                    found: receiver.value,
                },
                access.member_span(),
            );
        };
        let value = values
            .get(&key)
            .cloned()
            .or_else(|| self.types.member_type(access).and_then(structural_default));
        let Some(value) = value else {
            return self.fail(
                EvaluationErrorKind::MissingAttrSetKey { key },
                access.member_span(),
            );
        };
        Ok(TrackedValue {
            value,
            dependencies: receiver.dependencies,
            opaque_dependencies: receiver.opaque_dependencies,
            path,
        })
    }

    fn eval_field(
        &mut self,
        receiver: TrackedValue,
        field: light_nix_name_resolver::FieldId,
        access: &'ast PrimaryAccess<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        let path = receiver.path.clone().map(|path| path.field(field));
        if let Some(entry) = path.as_ref().and_then(|path| self.snapshot.get(path)) {
            let mut dependencies = receiver.dependencies;
            dependencies.extend(entry.dependencies.iter().copied());
            let mut opaque_dependencies = receiver.opaque_dependencies;
            opaque_dependencies.extend(entry.opaque_dependencies.iter().copied());
            return Ok(TrackedValue {
                value: entry.value.clone(),
                dependencies,
                opaque_dependencies,
                path,
            });
        }
        let RuntimeValue::Record(record) = receiver.value else {
            return self.fail(
                EvaluationErrorKind::ExpectedRecord {
                    found: receiver.value,
                },
                access.member_span(),
            );
        };
        let value = record
            .fields
            .get(&field)
            .cloned()
            .or_else(|| self.types.field_type(field).and_then(structural_default));
        let Some(value) = value else {
            return self.fail(
                EvaluationErrorKind::MissingField { field },
                access.member_span(),
            );
        };
        Ok(TrackedValue {
            value,
            dependencies: receiver.dependencies,
            opaque_dependencies: receiver.opaque_dependencies,
            path,
        })
    }

    fn eval_call(
        &mut self,
        callee: TrackedValue,
        receiver: Option<TrackedValue>,
        call: &'ast FunctionCall<'input, 'allocator>,
        _span: std::ops::Range<usize>,
    ) -> Eval<TrackedValue> {
        let mut arguments = Vec::with_capacity(call.arguments.len());
        for argument in call.arguments {
            arguments.push(self.eval_expression(argument)?.without_path());
        }
        let mut result = self.call_callable(callee, receiver, arguments, call.span.clone())?;
        result.path = None;
        Ok(result)
    }

    fn call_callable(
        &mut self,
        callable: TrackedValue,
        receiver: Option<TrackedValue>,
        arguments: Vec<TrackedValue>,
        span: std::ops::Range<usize>,
    ) -> Eval<TrackedValue> {
        let mut exact_inputs = callable.dependencies.clone();
        let mut opaque_inputs = callable.opaque_dependencies.clone();
        if let Some(receiver) = &receiver {
            exact_inputs.extend(receiver.dependencies.iter().copied());
            opaque_inputs.extend(receiver.opaque_dependencies.iter().copied());
        }
        for argument in &arguments {
            exact_inputs.extend(argument.dependencies.iter().copied());
            opaque_inputs.extend(argument.opaque_dependencies.iter().copied());
        }
        let (mut result, mode) = match callable.value {
            RuntimeValue::Function(symbol) => {
                let mode = self
                    .functions
                    .get(&symbol)
                    .map(|function| function.attribute.value)
                    .unwrap_or(FunctionAttribute::Opaque);
                (self.call_function(symbol, receiver, arguments, span)?, mode)
            }
            RuntimeValue::Closure(id) => {
                let expression = self
                    .closures
                    .get(id as usize)
                    .map(|closure| closure.expression)
                    .ok_or(Signal::Error)?;
                (
                    self.call_closure(id, arguments, span)?,
                    expression.attribute.value,
                )
            }
            found => {
                return self.fail(EvaluationErrorKind::NotCallable { found }, span);
            }
        };
        match mode {
            FunctionAttribute::Inline => {
                result.dependencies.extend(exact_inputs);
                result.opaque_dependencies.extend(opaque_inputs);
            }
            FunctionAttribute::Opaque => {
                result.opaque_dependencies.extend(opaque_inputs);
                result.opaque_dependencies.extend(exact_inputs);
                result
                    .opaque_dependencies
                    .extend(result.dependencies.iter().copied());
                result.dependencies.clear();
            }
        }
        Ok(result)
    }

    fn call_closure(
        &mut self,
        id: u32,
        arguments: Vec<TrackedValue>,
        span: std::ops::Range<usize>,
    ) -> Eval<TrackedValue> {
        let Some(closure) = self.closures.get(id as usize).cloned() else {
            return Err(Signal::Error);
        };
        if arguments.len() != closure.expression.parameters.len() {
            return self.fail(
                EvaluationErrorKind::ArgumentCount {
                    expected: closure.expression.parameters.len(),
                    found: arguments.len(),
                },
                span,
            );
        }
        let mut frame = closure.captures;
        for (parameter, value) in closure.expression.parameters.iter().zip(arguments) {
            if let Some(symbol) = self.symbol_declaration(&parameter.name) {
                frame.insert(symbol, value);
            }
        }
        self.frames.push(frame);
        let result = match closure.expression.body {
            ClosureBody::Expression(expression) => self.eval_expression(expression),
            ClosureBody::Block(block) => match self.eval_block(block) {
                Ok(value) => Ok(value),
                Err(Signal::Return(value)) => Ok(*value),
                Err(Signal::Error) => Err(Signal::Error),
            },
        };
        self.frames.pop();
        result
    }

    fn call_function(
        &mut self,
        symbol: SymbolId,
        receiver: Option<TrackedValue>,
        arguments: Vec<TrackedValue>,
        span: std::ops::Range<usize>,
    ) -> Eval<TrackedValue> {
        let Some(function) = self.functions.get(&symbol).copied() else {
            return self.fail(EvaluationErrorKind::MissingInput { symbol }, span);
        };
        if !self.calling_functions.insert(symbol) {
            return self.fail(EvaluationErrorKind::CyclicValue { symbol }, span);
        }
        if arguments.len() != function.arguments.arguments.len() {
            self.calling_functions.remove(&symbol);
            return self.fail(
                EvaluationErrorKind::ArgumentCount {
                    expected: function.arguments.arguments.len(),
                    found: arguments.len(),
                },
                span,
            );
        }
        let mut frame = HashMap::new();
        if let (Some(receiver_name), Some(receiver)) = (&function.arguments.receiver, receiver)
            && let Some(symbol) = self.symbol_declaration(receiver_name)
        {
            frame.insert(symbol, receiver.without_path());
        }
        for (argument, value) in function.arguments.arguments.iter().zip(arguments) {
            if let Some(symbol) = self.symbol_declaration(&argument.name) {
                frame.insert(symbol, value);
            }
        }
        self.frames.push(frame);
        let result = match self.eval_block(function.body) {
            Ok(value) => Ok(value),
            Err(Signal::Return(value)) => Ok(*value),
            Err(Signal::Error) => Err(Signal::Error),
        };
        self.frames.pop();
        self.calling_functions.remove(&symbol);
        result
    }

    fn eval_builtin(
        &mut self,
        receiver: TrackedValue,
        method: BuiltinMethod,
        call: &'ast FunctionCall<'input, 'allocator>,
        span: std::ops::Range<usize>,
    ) -> Eval<TrackedValue> {
        let mut arguments = Vec::with_capacity(call.arguments.len());
        for argument in call.arguments {
            arguments.push(self.eval_expression(argument)?.without_path());
        }
        let mut dependencies = receiver.dependencies.clone();
        let mut opaque_dependencies = receiver.opaque_dependencies.clone();
        for argument in &arguments {
            dependencies.extend(argument.dependencies.iter().copied());
            opaque_dependencies.extend(argument.opaque_dependencies.iter().copied());
        }
        let value = match method {
            BuiltinMethod::Contains => {
                let [needle] = arguments.as_slice() else {
                    return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span);
                };
                let values = match receiver.value {
                    RuntimeValue::List(values) | RuntimeValue::Set(values) => values,
                    _ => return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span),
                };
                RuntimeValue::Bool(values.contains(&needle.value))
            }
            BuiltinMethod::Filter => {
                let [predicate] = arguments.as_slice() else {
                    return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span);
                };
                let (values, is_list) = match receiver.value {
                    RuntimeValue::List(values) => (values, true),
                    RuntimeValue::Set(values) => (values, false),
                    _ => return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span),
                };
                let mut filtered = Vec::new();
                for value in values {
                    let argument = TrackedValue {
                        value: value.clone(),
                        dependencies: receiver.dependencies.clone(),
                        opaque_dependencies: receiver.opaque_dependencies.clone(),
                        path: None,
                    };
                    let result = self.call_runtime_function(
                        predicate.clone(),
                        vec![argument],
                        span.clone(),
                    )?;
                    dependencies.extend(result.dependencies.iter().copied());
                    opaque_dependencies.extend(result.opaque_dependencies.iter().copied());
                    if self.expect_bool(result, span.clone())? {
                        filtered.push(value);
                    }
                }
                if is_list {
                    RuntimeValue::List(filtered)
                } else {
                    RuntimeValue::Set(filtered)
                }
            }
            BuiltinMethod::Map => {
                let [mapper] = arguments.as_slice() else {
                    return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span);
                };
                let (values, is_list) = match receiver.value {
                    RuntimeValue::List(values) => (values, true),
                    RuntimeValue::Set(values) => (values, false),
                    _ => return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span),
                };
                let mut mapped = Vec::new();
                for value in values {
                    let argument = TrackedValue {
                        value,
                        dependencies: receiver.dependencies.clone(),
                        opaque_dependencies: receiver.opaque_dependencies.clone(),
                        path: None,
                    };
                    let result =
                        self.call_runtime_function(mapper.clone(), vec![argument], span.clone())?;
                    dependencies.extend(result.dependencies.iter().copied());
                    opaque_dependencies.extend(result.opaque_dependencies.iter().copied());
                    if is_list || !mapped.contains(&result.value) {
                        mapped.push(result.value);
                    }
                }
                if is_list {
                    RuntimeValue::List(mapped)
                } else {
                    RuntimeValue::Set(mapped)
                }
            }
            BuiltinMethod::ToFloat => match receiver.value {
                RuntimeValue::Int(value) if arguments.is_empty() => {
                    RuntimeValue::Float(value as f64)
                }
                _ => return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span),
            },
            BuiltinMethod::TryToInt => match receiver.value {
                RuntimeValue::Float(value)
                    if arguments.is_empty()
                        && value.is_finite()
                        && value.fract() == 0.0
                        && value >= i64::MIN as f64
                        && value <= i64::MAX as f64 =>
                {
                    RuntimeValue::optional(Some(RuntimeValue::Int(value as i64)))
                }
                RuntimeValue::Float(_) if arguments.is_empty() => RuntimeValue::optional(None),
                _ => return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span),
            },
            BuiltinMethod::ToString if arguments.is_empty() => match receiver.value {
                RuntimeValue::Int(value) => RuntimeValue::String(value.to_string()),
                RuntimeValue::Float(value) => RuntimeValue::String(value.to_string()),
                RuntimeValue::Bool(value) => RuntimeValue::String(value.to_string()),
                RuntimeValue::String(value) => RuntimeValue::String(value),
                _ => return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span),
            },
            BuiltinMethod::ToString => {
                return self.fail(EvaluationErrorKind::InvalidBuiltinCall, span);
            }
        };
        Ok(TrackedValue {
            value,
            dependencies,
            opaque_dependencies,
            path: None,
        })
    }

    fn call_runtime_function(
        &mut self,
        function: TrackedValue,
        arguments: Vec<TrackedValue>,
        span: std::ops::Range<usize>,
    ) -> Eval<TrackedValue> {
        self.call_callable(function, None, arguments, span)
    }

    fn eval_symbol(
        &mut self,
        symbol: SymbolId,
        span: std::ops::Range<usize>,
    ) -> Eval<TrackedValue> {
        for frame in self.frames.iter().rev() {
            if let Some(value) = frame.get(&symbol) {
                return Ok(value.clone());
            }
        }
        if let Some(value) = self.global_values.get(&symbol) {
            return Ok(value.clone());
        }
        if self.functions.contains_key(&symbol) {
            return Ok(TrackedValue::pure(RuntimeValue::Function(symbol)));
        }
        let Some(node) = self.top_level_lets.get(&symbol).copied() else {
            if let Some(value) = self.inputs.values.get(&symbol) {
                return Ok(TrackedValue::pure(value.clone()));
            }
            return self.fail(EvaluationErrorKind::MissingInput { symbol }, span);
        };
        if !self.evaluating_values.insert(symbol) {
            return self.fail(EvaluationErrorKind::CyclicValue { symbol }, span);
        }
        let value = self.eval_let(symbol, node);
        self.evaluating_values.remove(&symbol);
        let value = value?;
        self.global_values.insert(symbol, value.clone());
        Ok(value)
    }

    fn eval_let(
        &mut self,
        symbol: SymbolId,
        node: &'ast LetStatement<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        let tunable = matches!(
            node.policy.as_ref().map(|policy| &policy.kind),
            Some(MutationPolicyKind::Tunable { .. })
        );
        let mut value = if tunable {
            match self.inputs.tunable_overrides.get(&symbol) {
                Some(value) => TrackedValue::pure(value.clone()),
                None => self.eval_let_default(symbol, node)?,
            }
        } else {
            self.eval_let_default(symbol, node)?
        };
        value.path = None;
        if tunable {
            value.dependencies.insert(symbol);
            let cost = match node.policy.as_ref().map(|policy| &policy.kind) {
                Some(MutationPolicyKind::Tunable { cost }) => {
                    cost.as_ref().map_or(1, |cost| cost.value)
                }
                _ => 1,
            };
            self.tunables.insert(
                symbol,
                TunableValue {
                    value: value.value.clone(),
                    cost,
                    origin: SourceOrigin {
                        module: self.module,
                        span: node.span.clone(),
                    },
                },
            );
        }
        Ok(value)
    }

    fn eval_let_default(
        &mut self,
        symbol: SymbolId,
        node: &'ast LetStatement<'input, 'allocator>,
    ) -> Eval<TrackedValue> {
        match node.value {
            Some(value) => self.eval_expression(value),
            None => self
                .inputs
                .values
                .get(&symbol)
                .cloned()
                .or_else(|| {
                    self.types
                        .symbol_type(symbol)
                        .and_then(|scheme| structural_default(&scheme.ty))
                })
                .map(TrackedValue::pure)
                .ok_or_else(|| {
                    self.errors.push(EvaluationError {
                        kind: EvaluationErrorKind::MissingInput { symbol },
                        module: self.module,
                        span: node.span.clone(),
                    });
                    Signal::Error
                }),
        }
    }

    fn eval_variant(&mut self, variant: VariantId) -> Eval<TrackedValue> {
        let mut dependencies = BTreeSet::new();
        if let Some(Some(value)) = self.variants.get(&variant).copied() {
            dependencies.extend(self.eval_expression(value)?.dependencies);
        }
        Ok(TrackedValue {
            value: RuntimeValue::Enum(variant),
            dependencies,
            opaque_dependencies: BTreeSet::new(),
            path: None,
        })
    }

    fn match_pattern(
        &self,
        pattern: &'ast Pattern<'input, 'allocator>,
        value: &TrackedValue,
    ) -> Option<Vec<(SymbolId, TrackedValue)>> {
        match pattern {
            Pattern::Some(pattern) => {
                let RuntimeValue::Optional(Some(inner)) = &value.value else {
                    return None;
                };
                self.match_pattern(
                    pattern.pattern,
                    &TrackedValue {
                        value: (**inner).clone(),
                        dependencies: value.dependencies.clone(),
                        opaque_dependencies: value.opaque_dependencies.clone(),
                        path: None,
                    },
                )
            }
            Pattern::Null(_) => matches!(value.value, RuntimeValue::Optional(None)).then(Vec::new),
            Pattern::Wildcard(_) => Some(Vec::new()),
            Pattern::Binding(binding) => self
                .symbol_declaration(binding)
                .map(|symbol| vec![(symbol, value.clone().without_path())]),
            Pattern::EnumVariant(pattern) => {
                let Some(Res::EnumVariant(expected)) =
                    self.resolution.resolve_literal(&pattern.variant)
                else {
                    return None;
                };
                matches!(value.value, RuntimeValue::Enum(found) if found == expected).then(Vec::new)
            }
        }
    }

    fn assignment_path(
        &self,
        expression: &'ast Expression<'input, 'allocator>,
    ) -> Option<OutputPath> {
        let Expression::Primary(primary) = expression else {
            return None;
        };
        let Value::Literal(root) = &primary.value else {
            return None;
        };
        if root.call.is_some() {
            return None;
        }
        let Some(Res::Symbol(root)) = self.resolution.resolve_literal(&root.literal) else {
            return None;
        };
        if self
            .resolution
            .symbols()
            .iter()
            .find(|symbol| symbol.id == root)
            .is_some_and(|symbol| symbol.kind != SymbolKind::Let)
        {
            return None;
        }
        let mut path = OutputPath::root(root);
        for access in primary.accesses {
            if access.call.is_some() {
                return None;
            }
            match self.types.member_resolution(access) {
                Some(MemberResolution::Field(field)) => {
                    path = path.field(*field);
                }
                Some(MemberResolution::AttrSetKey) => {
                    let key = access.key().and_then(|key| decode_string(key.value).ok())?;
                    path = path.key(key);
                }
                _ => return None,
            }
        }
        Some(path)
    }

    fn bind_local(&mut self, symbol: SymbolId, value: TrackedValue) {
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(symbol, value);
        } else {
            self.global_values.insert(symbol, value);
        }
    }

    fn expect_bool(&mut self, value: TrackedValue, span: std::ops::Range<usize>) -> Eval<bool> {
        match value.value {
            RuntimeValue::Bool(value) => Ok(value),
            found => self.fail(EvaluationErrorKind::ExpectedBoolean { found }, span),
        }
    }

    fn expect_string(&mut self, value: TrackedValue, span: std::ops::Range<usize>) -> Eval<String> {
        match value.value {
            RuntimeValue::String(value) => Ok(value),
            found => self.fail(EvaluationErrorKind::ExpectedString { found }, span),
        }
    }

    fn symbol_declaration(&self, literal: &'ast Literal<'input>) -> Option<SymbolId> {
        match self.resolution.declaration_of_literal(literal) {
            Some(Declaration::Symbol(symbol)) => Some(symbol),
            _ => None,
        }
    }

    fn fail<T>(&mut self, kind: EvaluationErrorKind, span: std::ops::Range<usize>) -> Eval<T> {
        self.errors.push(EvaluationError {
            kind,
            module: self.module,
            span,
        });
        Err(Signal::Error)
    }
}

fn merge_tracked(
    left: TrackedValue,
    right: TrackedValue,
    merge: impl FnOnce(RuntimeValue, RuntimeValue) -> RuntimeValue,
) -> TrackedValue {
    let mut dependencies = left.dependencies;
    dependencies.extend(right.dependencies);
    let mut opaque_dependencies = left.opaque_dependencies;
    opaque_dependencies.extend(right.opaque_dependencies);
    TrackedValue {
        value: merge(left.value, right.value),
        dependencies,
        opaque_dependencies,
        path: None,
    }
}

fn merge_tracked_result(
    left: TrackedValue,
    right: TrackedValue,
    merge: impl FnOnce(RuntimeValue, RuntimeValue) -> Result<RuntimeValue, EvaluationErrorKind>,
) -> Result<TrackedValue, EvaluationErrorKind> {
    let mut dependencies = left.dependencies;
    dependencies.extend(right.dependencies);
    let mut opaque_dependencies = left.opaque_dependencies;
    opaque_dependencies.extend(right.opaque_dependencies);
    Ok(TrackedValue {
        value: merge(left.value, right.value)?,
        dependencies,
        opaque_dependencies,
        path: None,
    })
}

fn bool_binary(
    left: RuntimeValue,
    right: RuntimeValue,
    operation: impl FnOnce(bool, bool) -> bool,
) -> Result<RuntimeValue, EvaluationErrorKind> {
    match (left, right) {
        (RuntimeValue::Bool(left), RuntimeValue::Bool(right)) => {
            Ok(RuntimeValue::Bool(operation(left, right)))
        }
        (found, _) => Err(EvaluationErrorKind::ExpectedBoolean { found }),
    }
}

fn numeric_binary(
    left: RuntimeValue,
    right: RuntimeValue,
    integer: impl FnOnce(i64, i64) -> Option<i64>,
    float: impl FnOnce(f64, f64) -> f64,
) -> Result<RuntimeValue, EvaluationErrorKind> {
    match (left, right) {
        (RuntimeValue::Int(left), RuntimeValue::Int(right)) => integer(left, right)
            .map(RuntimeValue::Int)
            .ok_or(EvaluationErrorKind::IntegerOverflow),
        (RuntimeValue::Float(left), RuntimeValue::Float(right)) => finite_float(float(left, right)),
        (found, _) => Err(EvaluationErrorKind::ExpectedNumber { found }),
    }
}

fn compare_binary(
    left: RuntimeValue,
    right: RuntimeValue,
    compare: impl FnOnce(std::cmp::Ordering) -> bool,
) -> Result<RuntimeValue, EvaluationErrorKind> {
    let ordering = match (left, right) {
        (RuntimeValue::Int(left), RuntimeValue::Int(right)) => left.cmp(&right),
        (RuntimeValue::Float(left), RuntimeValue::Float(right)) => left
            .partial_cmp(&right)
            .ok_or(EvaluationErrorKind::InvalidNumber)?,
        (found, _) => return Err(EvaluationErrorKind::ExpectedNumber { found }),
    };
    Ok(RuntimeValue::Bool(compare(ordering)))
}

fn parse_number(value: &str) -> Result<RuntimeValue, EvaluationErrorKind> {
    let value = value.replace('_', "");
    if value.contains(['.', 'e', 'E']) {
        value
            .parse::<f64>()
            .map_err(|_| EvaluationErrorKind::InvalidNumber)
            .and_then(finite_float)
    } else {
        value
            .parse::<i64>()
            .map(RuntimeValue::Int)
            .map_err(|_| EvaluationErrorKind::InvalidNumber)
    }
}

fn finite_float(value: f64) -> Result<RuntimeValue, EvaluationErrorKind> {
    value
        .is_finite()
        .then_some(RuntimeValue::Float(value))
        .ok_or(EvaluationErrorKind::InvalidNumber)
}

fn decode_string(value: &str) -> Result<String, EvaluationErrorKind> {
    let mut chars = value.chars();
    let quote = chars
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))
        .ok_or(EvaluationErrorKind::InvalidStringEscape)?;
    let mut result = String::new();
    let mut escaped = false;
    while let Some(character) = chars.next() {
        if escaped {
            result.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '\'' => '\'',
                '"' => '"',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            if chars.next().is_some() {
                return Err(EvaluationErrorKind::InvalidStringEscape);
            }
            return Ok(result);
        } else {
            result.push(character);
        }
    }
    Err(EvaluationErrorKind::InvalidStringEscape)
}

fn structural_default(ty: &Type) -> Option<RuntimeValue> {
    match ty {
        Type::Named(id, _) => Some(RuntimeValue::record(*id)),
        Type::AttrSet(_) => Some(RuntimeValue::attr_set()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use light_nix_name_resolver::{
        Declaration, FieldId, ImportEnvironment, ModuleId, NameResolution, SymbolId, TypeDefId,
        collect_module,
    };
    use light_nix_parser::{
        ast::{AstArena, Literal, Source, Statement},
        lexer::Lexer,
        parser::{ParseErrors, parse_source},
    };
    use light_nix_type_checker::{TypeEnvironment, check_module};

    use super::*;

    fn parse<'input, 'allocator>(
        source: &'input str,
        arena: &'allocator AstArena,
    ) -> &'allocator Source<'input, 'allocator> {
        let mut lexer = Lexer::new(source);
        let mut errors = ParseErrors::new_in(arena);
        let ast = parse_source(&mut lexer, &mut errors, arena);
        assert!(errors.is_empty(), "parse errors: {errors:#?}");
        ast
    }

    fn symbol_of(resolution: &NameResolution<'_>, literal: &Literal<'_>) -> SymbolId {
        let Some(Declaration::Symbol(symbol)) = resolution.declaration_of_literal(literal) else {
            panic!("expected symbol declaration");
        };
        symbol
    }

    fn type_of(resolution: &NameResolution<'_>, literal: &Literal<'_>) -> TypeDefId {
        let Some(Declaration::Type(ty)) = resolution.declaration_of_literal(literal) else {
            panic!("expected type declaration");
        };
        ty
    }

    fn field_named(resolution: &NameResolution<'_>, owner: TypeDefId, name: &str) -> FieldId {
        resolution
            .fields()
            .iter()
            .find(|field| field.owner == owner && resolution.name(field.name) == name)
            .map(|field| field.id)
            .unwrap_or_else(|| panic!("missing field {name}"))
    }

    #[test]
    fn tunable_override_reports_all_control_dependent_output_changes() {
        let source = r#"
type Firefox {
    enable: Bool
}
type Hyprland {
    enable: Bool
}
type Programs {
    firefox: Firefox
    hyprland: Hyprland
}

let tunable(cost = 7) n = 0
declare let programs: Programs

if n == 100 {
    programs.firefox.enable = true
    programs.hyprland.enable = true
}
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(
            resolution.errors().is_empty(),
            "name errors: {:#?}",
            resolution.errors()
        );
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(
            types.errors().is_empty(),
            "type errors: {:#?}",
            types.errors()
        );

        let Statement::TypeDefine(firefox_type) = ast.statements[0] else {
            panic!("expected Firefox type");
        };
        let Statement::TypeDefine(hyprland_type) = ast.statements[1] else {
            panic!("expected Hyprland type");
        };
        let Statement::TypeDefine(programs_type) = ast.statements[2] else {
            panic!("expected Programs type");
        };
        let Statement::LetStatement(n_binding) = ast.statements[3] else {
            panic!("expected n binding");
        };
        let Statement::LetStatement(programs_binding) = ast.statements[4] else {
            panic!("expected programs binding");
        };

        let n = symbol_of(&resolution, &n_binding.name);
        let programs = symbol_of(&resolution, &programs_binding.name);
        let firefox = field_named(
            &resolution,
            type_of(&resolution, &programs_type.name),
            "firefox",
        );
        let firefox_enable = field_named(
            &resolution,
            type_of(&resolution, &firefox_type.name),
            "enable",
        );
        let hyprland = field_named(
            &resolution,
            type_of(&resolution, &programs_type.name),
            "hyprland",
        );
        let hyprland_enable = field_named(
            &resolution,
            type_of(&resolution, &hyprland_type.name),
            "enable",
        );

        let before = evaluate_module(ast, &resolution, &types, &EvaluationInputs::default());
        assert!(
            before.is_success(),
            "evaluation errors: {:#?}",
            before.errors()
        );
        assert_eq!(before.snapshot().outputs().len(), 0);
        assert_eq!(before.tunable(n).map(|value| value.cost), Some(7));
        assert_eq!(
            before.tunable(n).map(|value| &value.value),
            Some(&RuntimeValue::Int(0))
        );

        let mut inputs = EvaluationInputs::default();
        inputs.override_tunable(n, RuntimeValue::Int(100));
        let after = evaluate_module(ast, &resolution, &types, &inputs);
        assert!(
            after.is_success(),
            "evaluation errors: {:#?}",
            after.errors()
        );
        assert_eq!(
            after.tunable(n).map(|value| &value.value),
            Some(&RuntimeValue::Int(100))
        );

        let firefox_path = OutputPath::root(programs)
            .field(firefox)
            .field(firefox_enable);
        let hyprland_path = OutputPath::root(programs)
            .field(hyprland)
            .field(hyprland_enable);
        let expected_dependencies = BTreeSet::from([n]);
        let firefox_output = after.snapshot().get(&firefox_path).expect("firefox output");
        assert_eq!(
            (&firefox_output.value, &firefox_output.dependencies),
            (&RuntimeValue::Bool(true), &expected_dependencies)
        );
        assert_eq!(firefox_output.origin.module, ModuleId(0));
        assert_eq!(
            after
                .snapshot()
                .get(&hyprland_path)
                .map(|entry| (&entry.value, &entry.dependencies)),
            Some((&RuntimeValue::Bool(true), &expected_dependencies))
        );

        let changes = before.snapshot().diff(after.snapshot());
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().all(|change| change.before.is_none()));
        assert!(
            changes
                .iter()
                .all(|change| change.after.as_ref().map(|entry| &entry.value)
                    == Some(&RuntimeValue::Bool(true)))
        );
        assert!(
            changes
                .iter()
                .all(|change| change.dependencies == expected_dependencies)
        );
        assert!(changes.iter().any(|change| change.path == firefox_path));
        assert!(changes.iter().any(|change| change.path == hyprland_path));
    }

    #[test]
    fn evaluates_functions_and_polymorphic_set_builtins() {
        let source = r#"
type Programs {
    values: Set<String>
    enabled: Bool
}
opaque function stringify(value: Int) -> String {
    return value.to_string()
}
let tunable n = 1
let values = @set [n, 2, 2]
let mapped = values.map(stringify)
declare let programs: Programs
programs.values = mapped
programs.enabled = values.contains(n)
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(
            resolution.errors().is_empty(),
            "name errors: {:#?}",
            resolution.errors()
        );
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(
            types.errors().is_empty(),
            "type errors: {:#?}",
            types.errors()
        );
        let result = evaluate_module(ast, &resolution, &types, &EvaluationInputs::default());
        assert!(
            result.is_success(),
            "evaluation errors: {:#?}",
            result.errors()
        );

        let Statement::TypeDefine(programs_type) = ast.statements[0] else {
            panic!("expected Programs type");
        };
        let Statement::LetStatement(n_binding) = ast.statements[2] else {
            panic!("expected n binding");
        };
        let Statement::LetStatement(mapped_binding) = ast.statements[4] else {
            panic!("expected mapped binding");
        };
        let Statement::LetStatement(programs_binding) = ast.statements[5] else {
            panic!("expected programs binding");
        };
        let n = symbol_of(&resolution, &n_binding.name);
        let mapped = symbol_of(&resolution, &mapped_binding.name);
        let programs = symbol_of(&resolution, &programs_binding.name);
        let programs_type = type_of(&resolution, &programs_type.name);
        let values = field_named(&resolution, programs_type, "values");
        let enabled = field_named(&resolution, programs_type, "enabled");

        assert_eq!(
            result.symbol_value(mapped),
            Some(&RuntimeValue::Set(vec![
                RuntimeValue::String("1".to_owned()),
                RuntimeValue::String("2".to_owned()),
            ]))
        );
        assert_eq!(
            result
                .snapshot()
                .get(&OutputPath::root(programs).field(values))
                .map(|entry| (&entry.value, &entry.dependencies)),
            Some((
                &RuntimeValue::Set(vec![
                    RuntimeValue::String("1".to_owned()),
                    RuntimeValue::String("2".to_owned()),
                ]),
                &BTreeSet::from([n]),
            ))
        );
        assert_eq!(
            result
                .snapshot()
                .get(&OutputPath::root(programs).field(enabled))
                .map(|entry| (&entry.value, &entry.dependencies)),
            Some((&RuntimeValue::Bool(true), &BTreeSet::from([n])))
        );
    }

    #[test]
    fn lists_preserve_order_and_duplicates_while_sets_deduplicate() {
        let source = r#"
let values = [1, 1, 2]
let mapped = values.map(inline |value| => value + 1)
let filtered = values.filter(inline |value| => value == 1)
let unique = @set [1, 1, 2]
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(types.errors().is_empty(), "{:#?}", types.errors());
        let result = evaluate_module(ast, &resolution, &types, &EvaluationInputs::default());
        assert!(result.is_success(), "{:#?}", result.errors());

        let symbols = ast
            .statements
            .iter()
            .map(|statement| {
                let Statement::LetStatement(binding) = statement else {
                    panic!("expected let binding");
                };
                symbol_of(&resolution, &binding.name)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            result.symbol_value(symbols[0]),
            Some(&RuntimeValue::List(vec![
                RuntimeValue::Int(1),
                RuntimeValue::Int(1),
                RuntimeValue::Int(2),
            ]))
        );
        assert_eq!(
            result.symbol_value(symbols[1]),
            Some(&RuntimeValue::List(vec![
                RuntimeValue::Int(2),
                RuntimeValue::Int(2),
                RuntimeValue::Int(3),
            ]))
        );
        assert_eq!(
            result.symbol_value(symbols[2]),
            Some(&RuntimeValue::List(vec![
                RuntimeValue::Int(1),
                RuntimeValue::Int(1),
            ]))
        );
        assert_eq!(
            result.symbol_value(symbols[3]),
            Some(&RuntimeValue::Set(vec![
                RuntimeValue::Int(1),
                RuntimeValue::Int(2),
            ]))
        );
    }

    #[test]
    fn evaluates_closures_and_distinguishes_exact_from_opaque_dependencies() {
        let source = r#"
type Programs {
    inline_values: Set<Int>
    opaque_values: Set<Int>
}
let tunable threshold = 1
let values = @set [1, 2, 3]
let inline_values = values.filter(inline |value| => value > threshold)
let opaque_values = values.filter(opaque |value: Int| -> Bool => {
    return value > threshold
})
declare let programs: Programs
programs.inline_values = inline_values
programs.opaque_values = opaque_values
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(types.errors().is_empty(), "{:#?}", types.errors());
        let result = evaluate_module(ast, &resolution, &types, &EvaluationInputs::default());
        assert!(result.is_success(), "{:#?}", result.errors());

        let Statement::TypeDefine(programs_type) = ast.statements[0] else {
            panic!("expected Programs type");
        };
        let Statement::LetStatement(threshold_binding) = ast.statements[1] else {
            panic!("expected threshold binding");
        };
        let Statement::LetStatement(programs_binding) = ast.statements[5] else {
            panic!("expected programs binding");
        };
        let threshold = symbol_of(&resolution, &threshold_binding.name);
        let programs = symbol_of(&resolution, &programs_binding.name);
        let programs_type = type_of(&resolution, &programs_type.name);
        let inline_values = field_named(&resolution, programs_type, "inline_values");
        let opaque_values = field_named(&resolution, programs_type, "opaque_values");
        let expected = RuntimeValue::Set(vec![RuntimeValue::Int(2), RuntimeValue::Int(3)]);

        let inline = result
            .snapshot()
            .get(&OutputPath::root(programs).field(inline_values))
            .expect("inline output");
        assert_eq!(inline.value, expected);
        assert_eq!(inline.dependencies, BTreeSet::from([threshold]));
        assert!(inline.opaque_dependencies.is_empty());

        let opaque = result
            .snapshot()
            .get(&OutputPath::root(programs).field(opaque_values))
            .expect("opaque output");
        assert_eq!(opaque.value, expected);
        assert!(opaque.dependencies.is_empty());
        assert_eq!(opaque.opaque_dependencies, BTreeSet::from([threshold]));
    }

    #[test]
    fn evaluates_concrete_static_interface_dispatch() {
        let source = r#"
interface Flag {
    inline function enabled(this) -> Bool { throw "abstract" }
}
type Firefox {}
implements Flag for Firefox {
    inline function enabled(this) -> Bool { return true }
}
type Programs {
    enabled: Bool
}
declare let firefox: Firefox
declare let programs: Programs
programs.enabled = firefox.enabled()
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(
            resolution.errors().is_empty(),
            "name errors: {:#?}",
            resolution.errors()
        );
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(
            types.errors().is_empty(),
            "type errors: {:#?}",
            types.errors()
        );
        let result = evaluate_module(ast, &resolution, &types, &EvaluationInputs::default());
        assert!(
            result.is_success(),
            "evaluation errors: {:#?}",
            result.errors()
        );

        let Statement::TypeDefine(programs_type) = ast.statements[3] else {
            panic!("expected Programs type");
        };
        let Statement::LetStatement(programs_binding) = ast.statements[5] else {
            panic!("expected programs binding");
        };
        let programs = symbol_of(&resolution, &programs_binding.name);
        let enabled = field_named(
            &resolution,
            type_of(&resolution, &programs_type.name),
            "enabled",
        );
        assert_eq!(
            result
                .snapshot()
                .get(&OutputPath::root(programs).field(enabled))
                .map(|entry| &entry.value),
            Some(&RuntimeValue::Bool(true))
        );
    }
}
