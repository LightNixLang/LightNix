use std::collections::{BTreeMap, HashMap, HashSet};

use light_nix_name_resolver::{
    Declaration, FieldId, GenericParameterId, ModuleId, NameResolution, Res, SymbolId, TypeDefId,
    TypeDefKind,
};
use light_nix_parser::ast::{
    AST, AccessOperator, BinaryOperator, Block, ClosureBody, ClosureExpression, ElseBranchValue,
    Expression, FunctionAttribute, LetStatement, Literal, MutationPolicyKind, Pattern, Primary,
    Source, Statement, Statements, UnaryOperator, Value,
};
use light_nix_type_checker::{MemberResolution, Type, TypeCheckResult};

use crate::{
    BinaryOperation, BuildError, CallTarget, ClosureParameter, Constant, ConstraintKind,
    ConstraintModel, ExpressionId, FunctionMode, LowerError, LowerErrorKind, ModelBuilder,
    MutationPolicy, ObjectiveKind, OutputPath, SourceOrigin, UnaryOperation, VariableId,
    VariableKind, VariableSource, WeightedVariable,
};

#[derive(Debug)]
pub struct LowerResult {
    model: ConstraintModel,
    errors: Vec<LowerError>,
}

impl LowerResult {
    pub fn model(&self) -> &ConstraintModel {
        &self.model
    }

    pub fn into_model(self) -> ConstraintModel {
        self.model
    }

    pub fn errors(&self) -> &[LowerError] {
        &self.errors
    }

    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn lower_module<'ast, 'input, 'allocator>(
    source: &'ast Source<'input, 'allocator>,
    resolution: &'ast NameResolution<'ast>,
    types: &'ast TypeCheckResult<'ast>,
) -> LowerResult {
    Lowerer::new(resolution, types).lower(source)
}

#[derive(Debug, Clone)]
struct LoweredValue {
    expression: ExpressionId,
    path: Option<OutputPath>,
}

struct Lowerer<'ast, 'input, 'allocator> {
    resolution: &'ast NameResolution<'ast>,
    types: &'ast TypeCheckResult<'ast>,
    module: ModuleId,
    builder: ModelBuilder,
    top_level_lets: BTreeMap<SymbolId, &'ast LetStatement<'input, 'allocator>>,
    variables: HashMap<SymbolId, VariableId>,
    lowered_symbols: HashMap<SymbolId, ExpressionId>,
    lowering_symbols: HashSet<SymbolId>,
    frames: Vec<HashMap<SymbolId, ExpressionId>>,
    field_policies: HashMap<FieldId, MutationPolicy>,
    errors: Vec<LowerError>,
    true_expression: Option<ExpressionId>,
}

impl<'ast, 'input, 'allocator> Lowerer<'ast, 'input, 'allocator> {
    fn new(resolution: &'ast NameResolution<'ast>, types: &'ast TypeCheckResult<'ast>) -> Self {
        let module = resolution.module();
        Self {
            resolution,
            types,
            module,
            builder: ModelBuilder::new(module),
            top_level_lets: BTreeMap::new(),
            variables: HashMap::new(),
            lowered_symbols: HashMap::new(),
            lowering_symbols: HashSet::new(),
            frames: Vec::new(),
            field_policies: HashMap::new(),
            errors: Vec::new(),
            true_expression: None,
        }
    }

    fn lower(mut self, source: &'ast Source<'input, 'allocator>) -> LowerResult {
        self.collect_declarations(source);
        self.declare_variables();
        self.declare_output_paths();
        let root_guard = self.true_expression();
        self.initialize_variables(root_guard);
        self.lower_statements(source, root_guard, true);
        self.add_change_objective();
        LowerResult {
            model: self.builder.finish(),
            errors: self.errors,
        }
    }

    fn collect_declarations(&mut self, statements: &'ast Statements<'input, 'allocator>) {
        for statement in statements.statements {
            match statement {
                Statement::LetStatement(node) => {
                    if let Some(symbol) = self.symbol_declaration(&node.name) {
                        self.top_level_lets.insert(symbol, node);
                    }
                }
                Statement::TypeDefine(node) => self.collect_field_policies(node.body),
                _ => {}
            }
        }
    }

    fn collect_field_policies(
        &mut self,
        block: &'ast light_nix_parser::ast::TypedefBlock<'input, 'allocator>,
    ) {
        for field in block.fields {
            if let Some(Declaration::Field(id)) =
                self.resolution.declaration_of_literal(&field.name)
                && let Some(policy) = field.policy.as_ref()
            {
                self.field_policies
                    .insert(id, mutation_policy(&policy.kind));
            }
            if let light_nix_parser::ast::TypedefValue::Block(nested) = field.value {
                self.collect_field_policies(nested);
            }
        }
    }

    fn declare_variables(&mut self) {
        let declarations = self
            .top_level_lets
            .iter()
            .map(|(symbol, node)| (*symbol, *node))
            .collect::<Vec<_>>();
        for (symbol, node) in declarations {
            let Some(scheme) = self.types.symbol_type(symbol) else {
                self.error(LowerErrorKind::MissingType, node.name.span.clone());
                continue;
            };
            let kind = match node.policy.as_ref().map(|policy| &policy.kind) {
                Some(MutationPolicyKind::Tunable { .. })
                    if node.value.is_none() && matches!(scheme.ty, Type::Named(_, _)) =>
                {
                    Some(VariableKind::Input)
                }
                Some(MutationPolicyKind::Tunable { cost }) => Some(VariableKind::Tunable {
                    cost: cost.as_ref().map_or(1, |cost| cost.value),
                }),
                _ if node.value.is_none() => Some(VariableKind::Input),
                _ => None,
            };
            let Some(kind) = kind else {
                continue;
            };
            let result = self.builder.add_variable(
                VariableSource::Symbol(symbol),
                scheme.ty.clone(),
                kind,
                None,
                Some(self.origin(node.span.clone())),
            );
            match result {
                Ok(variable) => {
                    self.variables.insert(symbol, variable);
                }
                Err(error) => self.build_error(error, node.span.clone()),
            }
        }
    }

    fn declare_output_paths(&mut self) {
        let roots = self
            .top_level_lets
            .iter()
            .filter_map(|(symbol, node)| {
                if node.value.is_some() {
                    return None;
                }
                let Type::Named(owner, arguments) = &self.types.symbol_type(*symbol)?.ty else {
                    return None;
                };
                let policy = node
                    .policy
                    .as_ref()
                    .map_or(MutationPolicy::Readonly, |policy| {
                        mutation_policy(&policy.kind)
                    });
                Some((
                    *symbol,
                    *owner,
                    arguments.clone(),
                    policy,
                    node.span.clone(),
                ))
            })
            .collect::<Vec<_>>();
        for (symbol, owner, arguments, policy, span) in roots {
            let mut visiting = HashSet::new();
            self.declare_record_fields(
                OutputPath::root(symbol),
                owner,
                &arguments,
                policy,
                span,
                &mut visiting,
            );
        }
    }

    fn declare_record_fields(
        &mut self,
        root: OutputPath,
        owner: TypeDefId,
        arguments: &[Type],
        inherited_policy: MutationPolicy,
        root_span: std::ops::Range<usize>,
        visiting: &mut HashSet<TypeDefId>,
    ) {
        if !visiting.insert(owner) {
            let result = self.builder.declare_output_path(
                root,
                Type::Named(owner, arguments.to_vec()),
                inherited_policy,
                self.origin(root_span.clone()),
            );
            if let Err(error) = result {
                self.build_error(error, root_span);
            }
            return;
        }
        let substitutions = self
            .types
            .type_parameters(owner)
            .iter()
            .copied()
            .zip(arguments.iter().cloned())
            .collect::<HashMap<GenericParameterId, Type>>();
        let fields = self
            .resolution
            .types()
            .iter()
            .find(|ty| ty.id == owner)
            .map(|ty| ty.fields.clone())
            .unwrap_or_default();
        if fields.is_empty() {
            visiting.remove(&owner);
            return;
        }
        for field_id in fields {
            let Some(field) = self
                .resolution
                .fields()
                .iter()
                .find(|field| field.id == field_id)
            else {
                continue;
            };
            let field_span = field.span.clone();
            let ty = self
                .types
                .field_type(field_id)
                .map(|ty| substitute_type(ty, &substitutions))
                .unwrap_or(Type::Error);
            let policy = self
                .field_policies
                .get(&field_id)
                .copied()
                .unwrap_or(inherited_policy);
            let path = root.clone().field(field_id);
            let child = match &ty {
                Type::Named(child, child_arguments)
                    if self.resolution.types().iter().any(|candidate| {
                        candidate.id == *child && candidate.kind == TypeDefKind::Record
                    }) =>
                {
                    Some((*child, child_arguments.clone()))
                }
                _ => None,
            };
            if let Some((child, child_arguments)) = child {
                self.declare_record_fields(
                    path,
                    child,
                    &child_arguments,
                    policy,
                    field_span,
                    visiting,
                );
            } else {
                let result = self.builder.declare_output_path(
                    path,
                    ty,
                    policy,
                    self.origin(field_span.clone()),
                );
                if let Err(error) = result {
                    self.build_error(error, field_span);
                }
            }
        }
        visiting.remove(&owner);
    }

    fn initialize_variables(&mut self, guard: ExpressionId) {
        let tunables = self
            .top_level_lets
            .iter()
            .filter_map(|(symbol, node)| self.variables.get(symbol).copied().zip(node.value))
            .collect::<Vec<_>>();
        for (variable, value) in tunables {
            let initial = self.lower_expression(value, guard).expression;
            if let Err(error) = self.builder.set_variable_initial(variable, initial) {
                self.build_error(error, value.span());
            }
        }
    }

    fn add_change_objective(&mut self) {
        let variables = self
            .builder
            .model()
            .variables()
            .filter_map(|variable| match variable.kind() {
                VariableKind::Tunable { cost } => Some(WeightedVariable::new(variable.id(), cost)),
                VariableKind::Input => None,
            })
            .collect::<Vec<_>>();
        if variables.is_empty() {
            return;
        }
        if let Err(error) = self
            .builder
            .add_objective(ObjectiveKind::MinimizeChanges(variables), None)
        {
            self.build_error(error, 0..0);
        }
    }

    fn lower_statements(
        &mut self,
        statements: &'ast Statements<'input, 'allocator>,
        guard: ExpressionId,
        module_scope: bool,
    ) -> Option<ExpressionId> {
        let mut result = None;
        for statement in statements.statements {
            match statement {
                Statement::ImportStatement(_)
                | Statement::EnumDefine(_)
                | Statement::TypeDefine(_)
                | Statement::InterfaceDefine(_)
                | Statement::ImplementsDefine(_)
                | Statement::UseDeclare(_)
                | Statement::FunctionDefine(_) => {}
                Statement::LetStatement(node) => {
                    if module_scope {
                        if let Some(symbol) = self.symbol_declaration(&node.name)
                            && !self.variables.contains_key(&symbol)
                        {
                            let _ = self.lower_symbol(symbol, node.name.span.clone(), guard);
                        }
                    } else if let Some(symbol) = self.symbol_declaration(&node.name) {
                        let value = match node.value {
                            Some(value) => self.lower_expression(value, guard).expression,
                            None => self.input_for_local(symbol, node),
                        };
                        if let Some(frame) = self.frames.last_mut() {
                            frame.insert(symbol, value);
                        }
                    }
                }
                Statement::AssertStatement(node) => {
                    let condition = self.lower_expression(node.condition, guard).expression;
                    let implication = self.implies(guard, condition, node.condition.span());
                    let add = self.builder.add_constraint(
                        implication,
                        ConstraintKind::Assert,
                        Some(self.origin(node.span.clone())),
                    );
                    if let Err(error) = add {
                        self.build_error(error, node.span.clone());
                    }
                }
                Statement::AssignStatement(node) => {
                    let Some((path, ty, policy)) = self.assignment_path(node.target) else {
                        self.error(LowerErrorKind::InvalidAssignmentTarget, node.target.span());
                        continue;
                    };
                    let value = self.lower_expression(node.value, guard).expression;
                    let add = self.builder.add_output_case(
                        path,
                        ty,
                        policy,
                        guard,
                        value,
                        self.origin(node.span.clone()),
                    );
                    if let Err(error) = add {
                        self.build_error(error, node.span.clone());
                    }
                }
                Statement::Expression(expression) => {
                    if let Expression::Return(node) = expression {
                        result = Some(match node.value {
                            Some(value) => self.lower_expression(value, guard).expression,
                            None => self.unit(expression.span()),
                        });
                        break;
                    }
                    result = Some(self.lower_expression(expression, guard).expression);
                }
            }
        }
        result
    }

    fn lower_block(
        &mut self,
        block: &'ast Block<'input, 'allocator>,
        guard: ExpressionId,
        expected: &Type,
    ) -> ExpressionId {
        self.frames.push(HashMap::new());
        let result = self.lower_statements(&block.statements, guard, false);
        self.frames.pop();
        let result = result.unwrap_or_else(|| {
            if expected == &Type::Unit {
                self.unit(block.span.clone())
            } else {
                self.unreachable(expected.clone(), block.span.clone())
            }
        });
        if expected != &Type::Never
            && self
                .builder
                .model()
                .expression(result)
                .is_some_and(|expression| expression.ty() == &Type::Never)
        {
            self.unreachable(expected.clone(), block.span.clone())
        } else {
            result
        }
    }

    fn lower_expression(
        &mut self,
        expression: &'ast Expression<'input, 'allocator>,
        guard: ExpressionId,
    ) -> LoweredValue {
        let ty = self
            .types
            .expression_type(expression)
            .cloned()
            .unwrap_or_else(|| {
                self.error(LowerErrorKind::MissingType, expression.span());
                Type::Error
            });
        let origin = Some(self.origin(expression.span()));
        let expression_id = match expression {
            Expression::If(node) => self.lower_if(node, guard, &ty),
            Expression::Match(node) => self.lower_match(node, guard, &ty),
            Expression::Return(node) => match node.value {
                Some(value) => self.lower_expression(value, guard).expression,
                None => self.unit(node.span.clone()),
            },
            Expression::Throw(node) => {
                let not_guard = self.not(guard, node.span.clone());
                let add = self.builder.add_constraint(
                    not_guard,
                    ConstraintKind::Validity,
                    Some(self.origin(node.span.clone())),
                );
                if let Err(error) = add {
                    self.build_error(error, node.span.clone());
                }
                self.unreachable(ty, node.span.clone())
            }
            Expression::Closure(node) => self.lower_closure(node, guard, &ty),
            Expression::Elvis(node) => {
                let optional = self.lower_expression(node.optional, guard).expression;
                let is_null = self.is_null(optional, node.optional.span());
                let fallback_guard = self.and(guard, is_null, node.fallback.span());
                let fallback = self
                    .lower_expression(node.fallback, fallback_guard)
                    .expression;
                let result = self.builder.elvis(optional, fallback, ty.clone(), origin);
                self.finish_expression(result, ty, expression.span())
            }
            Expression::Binary(node) => {
                let left = self.lower_expression(node.left, guard).expression;
                let right_guard = match node.operator.value {
                    BinaryOperator::And => self.and(guard, left, node.left.span()),
                    BinaryOperator::Or => {
                        let not_left = self.not(left, node.left.span());
                        self.and(guard, not_left, node.left.span())
                    }
                    _ => guard,
                };
                let right = self.lower_expression(node.right, right_guard).expression;
                let result = self.builder.binary(
                    binary_operation(node.operator.value),
                    left,
                    right,
                    ty.clone(),
                    origin,
                );
                self.finish_expression(result, ty, expression.span())
            }
            Expression::Unary(node) => {
                let operand = self.lower_expression(node.operand, guard).expression;
                let operation = match node.operator.value {
                    UnaryOperator::Positive => UnaryOperation::Positive,
                    UnaryOperator::Negate => UnaryOperation::Negate,
                };
                let result = self.builder.unary(operation, operand, ty.clone(), origin);
                self.finish_expression(result, ty, expression.span())
            }
            Expression::Primary(primary) => {
                return self.lower_primary(primary, guard, ty);
            }
        };
        LoweredValue {
            expression: expression_id,
            path: None,
        }
    }

    fn lower_closure(
        &mut self,
        closure: &'ast ClosureExpression<'input, 'allocator>,
        guard: ExpressionId,
        ty: &Type,
    ) -> ExpressionId {
        let Type::Function(function_type) = ty else {
            return self.unreachable(ty.clone(), closure.span.clone());
        };
        let mut frame = HashMap::new();
        let mut parameters = Vec::new();
        for (parameter, parameter_type) in closure.parameters.iter().zip(&function_type.parameters)
        {
            let Some(symbol) = self.symbol_declaration(&parameter.name) else {
                continue;
            };
            let reference = self.builder.parameter_reference(
                symbol,
                parameter_type.clone(),
                Some(self.origin(parameter.span.clone())),
            );
            let reference =
                self.finish_expression(reference, parameter_type.clone(), parameter.span.clone());
            frame.insert(symbol, reference);
            parameters.push(ClosureParameter::new(symbol, parameter_type.clone()));
        }
        self.frames.push(frame);
        let body = match closure.body {
            ClosureBody::Expression(expression) => {
                self.lower_expression(expression, guard).expression
            }
            ClosureBody::Block(block) => self.lower_block(block, guard, &function_type.return_type),
        };
        self.frames.pop();
        let mode = match closure.attribute.value {
            FunctionAttribute::Inline => FunctionMode::Inline,
            FunctionAttribute::Opaque => FunctionMode::Opaque,
        };
        let result = self.builder.closure(
            mode,
            parameters,
            body,
            Some(self.origin(closure.span.clone())),
        );
        self.finish_expression(result, ty.clone(), closure.span.clone())
    }

    fn lower_if(
        &mut self,
        node: &'ast light_nix_parser::ast::IfExpression<'input, 'allocator>,
        outer_guard: ExpressionId,
        ty: &Type,
    ) -> ExpressionId {
        let condition = self
            .lower_expression(node.branch.condition, outer_guard)
            .expression;
        let then_guard = self.and(outer_guard, condition, node.branch.span.clone());
        let then_value = self.lower_block(node.branch.body, then_guard, ty);
        let not_condition = self.not(condition, node.branch.span.clone());
        let mut remaining_guard = self.and(outer_guard, not_condition, node.span.clone());
        let mut branches = Vec::new();
        for branch in node.else_branches {
            match branch.value {
                ElseBranchValue::If(if_branch) => {
                    let condition = self
                        .lower_expression(if_branch.condition, remaining_guard)
                        .expression;
                    let branch_guard = self.and(remaining_guard, condition, if_branch.span.clone());
                    let value = self.lower_block(if_branch.body, branch_guard, ty);
                    branches.push((condition, value, branch.span.clone()));
                    let not_condition = self.not(condition, branch.span.clone());
                    remaining_guard = self.and(remaining_guard, not_condition, branch.span.clone());
                }
                ElseBranchValue::Block(block) => {
                    let value = self.lower_block(block, remaining_guard, ty);
                    branches.push((self.true_expression(), value, branch.span.clone()));
                }
            }
        }
        let mut otherwise = if ty == &Type::Unit {
            self.unit(node.span.clone())
        } else {
            self.unreachable(ty.clone(), node.span.clone())
        };
        for (condition, value, span) in branches.into_iter().rev() {
            let result = self.builder.if_then_else(
                condition,
                value,
                otherwise,
                Some(self.origin(span.clone())),
            );
            otherwise = self.finish_expression(result, ty.clone(), span);
        }
        let result = self.builder.if_then_else(
            condition,
            then_value,
            otherwise,
            Some(self.origin(node.span.clone())),
        );
        self.finish_expression(result, ty.clone(), node.span.clone())
    }

    fn lower_match(
        &mut self,
        node: &'ast light_nix_parser::ast::MatchExpression<'input, 'allocator>,
        outer_guard: ExpressionId,
        ty: &Type,
    ) -> ExpressionId {
        let matched = self.lower_expression(node.value, outer_guard).expression;
        let mut remaining_guard = outer_guard;
        let mut arms = Vec::with_capacity(node.arms.len());
        for arm in node.arms {
            self.frames.push(HashMap::new());
            let condition = self.lower_pattern(&arm.pattern, matched, node.value.span());
            let arm_guard = self.and(remaining_guard, condition, arm.span.clone());
            let value = self.lower_expression(arm.value, arm_guard).expression;
            self.frames.pop();
            arms.push((condition, value, arm.span.clone()));
            let not_condition = self.not(condition, arm.span.clone());
            remaining_guard = self.and(remaining_guard, not_condition, arm.span.clone());
        }
        let exhaustive = self.not(remaining_guard, node.span.clone());
        let validity = self.builder.add_constraint(
            exhaustive,
            ConstraintKind::Validity,
            Some(self.origin(node.span.clone())),
        );
        if let Err(error) = validity {
            self.build_error(error, node.span.clone());
        }
        let mut result = self.unreachable(ty.clone(), node.span.clone());
        for (condition, value, span) in arms.into_iter().rev() {
            let expression = self.builder.if_then_else(
                condition,
                value,
                result,
                Some(self.origin(span.clone())),
            );
            result = self.finish_expression(expression, ty.clone(), span);
        }
        result
    }

    fn lower_pattern(
        &mut self,
        pattern: &'ast Pattern<'input, 'allocator>,
        value: ExpressionId,
        span: std::ops::Range<usize>,
    ) -> ExpressionId {
        match pattern {
            Pattern::Some(pattern) => {
                let is_null = self.is_null(value, pattern.span.clone());
                let present = self.not(is_null, pattern.span.clone());
                let inner_result = self
                    .builder
                    .optional_value(value, Some(self.origin(pattern.span.clone())));
                let inner_type = match self
                    .builder
                    .model()
                    .expression(value)
                    .map(|expression| expression.ty())
                {
                    Some(Type::Optional(inner)) => inner.as_ref().clone(),
                    _ => Type::Error,
                };
                let inner = self.finish_expression(inner_result, inner_type, pattern.span.clone());
                let nested = self.lower_pattern(pattern.pattern, inner, pattern.span.clone());
                self.and(present, nested, pattern.span.clone())
            }
            Pattern::Null(node) => self.is_null(value, node.span()),
            Pattern::Wildcard(_) => self.true_expression(),
            Pattern::Binding(binding) => {
                if let Some(symbol) = self.symbol_declaration(binding)
                    && let Some(frame) = self.frames.last_mut()
                {
                    frame.insert(symbol, value);
                }
                self.true_expression()
            }
            Pattern::EnumVariant(pattern) => {
                let Some(Res::EnumVariant(variant)) =
                    self.resolution.resolve_literal(&pattern.variant)
                else {
                    self.error(
                        LowerErrorKind::MissingResolution,
                        pattern.variant.span.clone(),
                    );
                    return self.false_expression();
                };
                let ty = self
                    .builder
                    .model()
                    .expression(value)
                    .map(|expression| expression.ty().clone())
                    .unwrap_or(Type::Error);
                let constant_result = self.builder.constant(
                    ty.clone(),
                    Constant::Enum(variant),
                    Some(self.origin(pattern.span.clone())),
                );
                let constant = self.finish_expression(constant_result, ty, pattern.span.clone());
                let equal_result = self.builder.binary(
                    BinaryOperation::Equal,
                    value,
                    constant,
                    Type::Bool,
                    Some(self.origin(span.clone())),
                );
                self.finish_expression(equal_result, Type::Bool, span)
            }
        }
    }

    fn lower_primary(
        &mut self,
        primary: &'ast Primary<'input, 'allocator>,
        guard: ExpressionId,
        final_type: Type,
    ) -> LoweredValue {
        let root_type = self
            .types
            .value_type(&primary.value)
            .cloned()
            .unwrap_or_else(|| {
                self.error(LowerErrorKind::MissingType, primary.value.span());
                Type::Error
            });
        let mut current = self.lower_value(&primary.value, guard, root_type);
        for access in primary.accesses {
            let result_type = self.types.member_type(access).cloned().unwrap_or_else(|| {
                self.error(LowerErrorKind::MissingType, access.span.clone());
                Type::Error
            });
            match self.types.member_resolution(access) {
                Some(MemberResolution::Field(field)) if access.call.is_none() => {
                    let safe = access.operator.value == AccessOperator::SafeDot;
                    if !safe && let Some(path) = current.path.take() {
                        let path = path.field(*field);
                        let result = self.builder.output_reference(
                            path.clone(),
                            result_type.clone(),
                            Some(self.origin(access.span.clone())),
                        );
                        current = LoweredValue {
                            expression: self.finish_expression(
                                result,
                                result_type,
                                access.span.clone(),
                            ),
                            path: Some(path),
                        };
                    } else {
                        let result = self.builder.field(
                            current.expression,
                            *field,
                            safe,
                            result_type.clone(),
                            Some(self.origin(access.span.clone())),
                        );
                        current = LoweredValue {
                            expression: self.finish_expression(
                                result,
                                result_type,
                                access.span.clone(),
                            ),
                            path: None,
                        };
                    }
                }
                Some(MemberResolution::Builtin(method)) => {
                    let arguments = access.call.map_or_else(Vec::new, |call| {
                        call.arguments
                            .iter()
                            .map(|argument| self.lower_expression(argument, guard).expression)
                            .collect()
                    });
                    let result = self.builder.call(
                        CallTarget::Builtin(*method),
                        Some(current.expression),
                        arguments,
                        result_type.clone(),
                        Some(self.origin(access.span.clone())),
                    );
                    current = LoweredValue {
                        expression: self.finish_expression(
                            result,
                            result_type,
                            access.span.clone(),
                        ),
                        path: None,
                    };
                }
                Some(MemberResolution::InterfaceMethod {
                    declaration,
                    implementation,
                    ..
                }) => {
                    let arguments = access.call.map_or_else(Vec::new, |call| {
                        call.arguments
                            .iter()
                            .map(|argument| self.lower_expression(argument, guard).expression)
                            .collect()
                    });
                    let result = self.builder.call(
                        CallTarget::Interface {
                            declaration: *declaration,
                            implementation: *implementation,
                        },
                        Some(current.expression),
                        arguments,
                        result_type.clone(),
                        Some(self.origin(access.span.clone())),
                    );
                    current = LoweredValue {
                        expression: self.finish_expression(
                            result,
                            result_type,
                            access.span.clone(),
                        ),
                        path: None,
                    };
                }
                _ => {
                    if let Some(Res::EnumVariant(variant)) =
                        self.resolution.resolve_literal(&access.member)
                    {
                        let result = self.builder.constant(
                            result_type.clone(),
                            Constant::Enum(variant),
                            Some(self.origin(access.span.clone())),
                        );
                        current = LoweredValue {
                            expression: self.finish_expression(
                                result,
                                result_type,
                                access.span.clone(),
                            ),
                            path: None,
                        };
                    } else if let Some(Res::Symbol(symbol)) =
                        self.resolution.resolve_literal(&access.member)
                    {
                        current = self.lower_symbol_call(
                            symbol,
                            access.call,
                            None,
                            guard,
                            result_type,
                            access.span.clone(),
                        );
                    } else {
                        self.error(LowerErrorKind::MissingResolution, access.span.clone());
                        current = LoweredValue {
                            expression: self.unreachable(result_type, access.span.clone()),
                            path: None,
                        };
                    }
                }
            }
        }
        if self
            .builder
            .model()
            .expression(current.expression)
            .is_some_and(|expression| expression.ty() != &final_type)
        {
            self.error(LowerErrorKind::MissingType, primary.span.clone());
        }
        current
    }

    fn lower_value(
        &mut self,
        value: &'ast Value<'input, 'allocator>,
        guard: ExpressionId,
        ty: Type,
    ) -> LoweredValue {
        let origin = Some(self.origin(value.span()));
        match value {
            Value::Array(array) => {
                let Type::Set(element) = &ty else {
                    self.error(LowerErrorKind::UnsupportedExpression, value.span());
                    return LoweredValue {
                        expression: self.unreachable(ty, value.span()),
                        path: None,
                    };
                };
                let values = array
                    .values
                    .iter()
                    .map(|value| {
                        self.lower_value(value, guard, element.as_ref().clone())
                            .expression
                    })
                    .collect();
                let result = self.builder.set(element.as_ref().clone(), values, origin);
                LoweredValue {
                    expression: self.finish_expression(result, ty, value.span()),
                    path: None,
                }
            }
            Value::Literal(literal) => {
                let Some(resolution) = self.resolution.resolve_literal(&literal.literal) else {
                    self.error(
                        LowerErrorKind::MissingResolution,
                        literal.literal.span.clone(),
                    );
                    return LoweredValue {
                        expression: self.unreachable(ty, value.span()),
                        path: None,
                    };
                };
                match resolution {
                    Res::Symbol(symbol) if literal.call.is_some() => self.lower_symbol_call(
                        symbol,
                        literal.call,
                        None,
                        guard,
                        ty,
                        literal.span.clone(),
                    ),
                    Res::Symbol(symbol) => LoweredValue {
                        expression: self.lower_symbol(symbol, literal.literal.span.clone(), guard),
                        path: Some(OutputPath::root(symbol)),
                    },
                    Res::EnumVariant(variant) => {
                        let result =
                            self.builder
                                .constant(ty.clone(), Constant::Enum(variant), origin);
                        LoweredValue {
                            expression: self.finish_expression(result, ty, value.span()),
                            path: None,
                        }
                    }
                    Res::Type(_) | Res::Module(_) => LoweredValue {
                        expression: self.unreachable(ty, value.span()),
                        path: None,
                    },
                    _ => {
                        self.error(LowerErrorKind::MissingResolution, value.span());
                        LoweredValue {
                            expression: self.unreachable(ty, value.span()),
                            path: None,
                        }
                    }
                }
            }
            Value::Some(some) => {
                let Some(inner) = some.value else {
                    self.error(LowerErrorKind::UnsupportedExpression, some.span.clone());
                    return LoweredValue {
                        expression: self.unreachable(ty, value.span()),
                        path: None,
                    };
                };
                let inner = self.lower_expression(inner, guard).expression;
                let result = self.builder.some(inner, origin);
                LoweredValue {
                    expression: self.finish_expression(result, ty, value.span()),
                    path: None,
                }
            }
            Value::Numeric(number) => match parse_number(number.value) {
                Ok(constant) => {
                    let result = self.builder.constant(ty.clone(), constant, origin);
                    LoweredValue {
                        expression: self.finish_expression(result, ty, value.span()),
                        path: None,
                    }
                }
                Err(()) => {
                    self.error(LowerErrorKind::InvalidNumber, number.span.clone());
                    LoweredValue {
                        expression: self.unreachable(ty, value.span()),
                        path: None,
                    }
                }
            },
            Value::String(string) => match decode_string(string.value) {
                Ok(string) => {
                    let result =
                        self.builder
                            .constant(ty.clone(), Constant::String(string), origin);
                    LoweredValue {
                        expression: self.finish_expression(result, ty, value.span()),
                        path: None,
                    }
                }
                Err(()) => {
                    self.error(LowerErrorKind::InvalidString, string.span.clone());
                    LoweredValue {
                        expression: self.unreachable(ty, value.span()),
                        path: None,
                    }
                }
            },
            Value::Boolean(boolean) => {
                let result =
                    self.builder
                        .constant(ty.clone(), Constant::Bool(boolean.value), origin);
                LoweredValue {
                    expression: self.finish_expression(result, ty, value.span()),
                    path: None,
                }
            }
            Value::Null(_) => {
                let result = self.builder.null(ty.clone(), origin);
                LoweredValue {
                    expression: self.finish_expression(result, ty, value.span()),
                    path: None,
                }
            }
        }
    }

    fn lower_symbol_call(
        &mut self,
        symbol: SymbolId,
        call: Option<&'ast light_nix_parser::ast::FunctionCall<'input, 'allocator>>,
        receiver: Option<ExpressionId>,
        guard: ExpressionId,
        result_type: Type,
        span: std::ops::Range<usize>,
    ) -> LoweredValue {
        let arguments = call.map_or_else(Vec::new, |call| {
            call.arguments
                .iter()
                .map(|argument| self.lower_expression(argument, guard).expression)
                .collect()
        });
        let result = self.builder.call(
            CallTarget::Function(symbol),
            receiver,
            arguments,
            result_type.clone(),
            Some(self.origin(span.clone())),
        );
        LoweredValue {
            expression: self.finish_expression(result, result_type, span),
            path: None,
        }
    }

    fn lower_symbol(
        &mut self,
        symbol: SymbolId,
        span: std::ops::Range<usize>,
        guard: ExpressionId,
    ) -> ExpressionId {
        for frame in self.frames.iter().rev() {
            if let Some(expression) = frame.get(&symbol) {
                return *expression;
            }
        }
        if let Some(expression) = self.lowered_symbols.get(&symbol) {
            return *expression;
        }
        if let Some(variable) = self.variables.get(&symbol).copied() {
            let result = self
                .builder
                .variable_reference(variable, Some(self.origin(span.clone())));
            let ty = self
                .types
                .symbol_type(symbol)
                .map(|scheme| scheme.ty.clone())
                .unwrap_or(Type::Error);
            return self.finish_expression(result, ty, span);
        }
        let Some(node) = self.top_level_lets.get(&symbol).copied() else {
            let ty = self
                .types
                .symbol_type(symbol)
                .map(|scheme| scheme.ty.clone())
                .unwrap_or(Type::Error);
            if matches!(ty, Type::Function(_)) {
                let result = self.builder.function_reference(
                    symbol,
                    ty.clone(),
                    Some(self.origin(span.clone())),
                );
                return self.finish_expression(result, ty, span);
            }
            self.error(LowerErrorKind::UnknownSymbol(symbol), span.clone());
            return self.unreachable(ty, span);
        };
        if !self.lowering_symbols.insert(symbol) {
            self.error(LowerErrorKind::CyclicBinding(symbol), span.clone());
            let ty = self
                .types
                .symbol_type(symbol)
                .map(|scheme| scheme.ty.clone())
                .unwrap_or(Type::Error);
            return self.unreachable(ty, span);
        }
        let ty = self
            .types
            .symbol_type(symbol)
            .map(|scheme| scheme.ty.clone())
            .unwrap_or(Type::Error);
        let expression = match node.value {
            Some(value) => self.lower_expression(value, guard).expression,
            None => self.unreachable(ty, node.span.clone()),
        };
        self.lowering_symbols.remove(&symbol);
        self.lowered_symbols.insert(symbol, expression);
        expression
    }

    fn input_for_local(
        &mut self,
        symbol: SymbolId,
        node: &'ast LetStatement<'input, 'allocator>,
    ) -> ExpressionId {
        let ty = self
            .types
            .symbol_type(symbol)
            .map(|scheme| scheme.ty.clone())
            .unwrap_or(Type::Error);
        let result = self.builder.add_variable(
            VariableSource::Symbol(symbol),
            ty.clone(),
            VariableKind::Input,
            None,
            Some(self.origin(node.span.clone())),
        );
        match result {
            Ok(variable) => {
                let reference = self
                    .builder
                    .variable_reference(variable, Some(self.origin(node.span.clone())));
                self.finish_expression(reference, ty, node.span.clone())
            }
            Err(error) => {
                self.build_error(error, node.span.clone());
                self.unreachable(ty, node.span.clone())
            }
        }
    }

    fn assignment_path(
        &self,
        expression: &'ast Expression<'input, 'allocator>,
    ) -> Option<(OutputPath, Type, MutationPolicy)> {
        let Expression::Primary(primary) = expression else {
            return None;
        };
        let Value::Literal(root) = &primary.value else {
            return None;
        };
        if root.call.is_some() {
            return None;
        }
        let Some(Res::Symbol(root_symbol)) = self.resolution.resolve_literal(&root.literal) else {
            return None;
        };
        let root_policy = self
            .top_level_lets
            .get(&root_symbol)
            .and_then(|node| node.policy.as_ref())
            .map_or(MutationPolicy::Readonly, |policy| {
                mutation_policy(&policy.kind)
            });
        let mut policy = root_policy;
        let mut path = OutputPath::root(root_symbol);
        for access in primary.accesses {
            if access.call.is_some() {
                return None;
            }
            let Some(MemberResolution::Field(field)) = self.types.member_resolution(access) else {
                return None;
            };
            path = path.field(*field);
            if let Some(field_policy) = self.field_policies.get(field) {
                policy = *field_policy;
            }
        }
        let ty = self.types.expression_type(expression)?.clone();
        Some((path, ty, policy))
    }

    fn true_expression(&mut self) -> ExpressionId {
        if let Some(expression) = self.true_expression {
            return expression;
        }
        let expression = self
            .builder
            .constant(Type::Bool, Constant::Bool(true), None)
            .expect("fresh IR table cannot overflow");
        self.true_expression = Some(expression);
        expression
    }

    fn false_expression(&mut self) -> ExpressionId {
        self.builder
            .constant(Type::Bool, Constant::Bool(false), None)
            .expect("fresh IR table cannot overflow")
    }

    fn unit(&mut self, span: std::ops::Range<usize>) -> ExpressionId {
        let result =
            self.builder
                .constant(Type::Unit, Constant::Unit, Some(self.origin(span.clone())));
        self.finish_expression(result, Type::Unit, span)
    }

    fn not(&mut self, value: ExpressionId, span: std::ops::Range<usize>) -> ExpressionId {
        let result = self.builder.unary(
            UnaryOperation::Not,
            value,
            Type::Bool,
            Some(self.origin(span.clone())),
        );
        self.finish_expression(result, Type::Bool, span)
    }

    fn is_null(&mut self, value: ExpressionId, span: std::ops::Range<usize>) -> ExpressionId {
        let result = self.builder.unary(
            UnaryOperation::IsNull,
            value,
            Type::Bool,
            Some(self.origin(span.clone())),
        );
        self.finish_expression(result, Type::Bool, span)
    }

    fn and(
        &mut self,
        left: ExpressionId,
        right: ExpressionId,
        span: std::ops::Range<usize>,
    ) -> ExpressionId {
        let result = self.builder.binary(
            BinaryOperation::And,
            left,
            right,
            Type::Bool,
            Some(self.origin(span.clone())),
        );
        self.finish_expression(result, Type::Bool, span)
    }

    fn implies(
        &mut self,
        condition: ExpressionId,
        value: ExpressionId,
        span: std::ops::Range<usize>,
    ) -> ExpressionId {
        let not_condition = self.not(condition, span.clone());
        let result = self.builder.binary(
            BinaryOperation::Or,
            not_condition,
            value,
            Type::Bool,
            Some(self.origin(span.clone())),
        );
        self.finish_expression(result, Type::Bool, span)
    }

    fn unreachable(&mut self, ty: Type, span: std::ops::Range<usize>) -> ExpressionId {
        self.builder
            .unreachable(ty, Some(self.origin(span)))
            .expect("fresh IR table cannot overflow")
    }

    fn finish_expression(
        &mut self,
        result: Result<ExpressionId, BuildError>,
        fallback_type: Type,
        span: std::ops::Range<usize>,
    ) -> ExpressionId {
        match result {
            Ok(expression) => expression,
            Err(error) => {
                self.build_error(error, span.clone());
                self.unreachable(fallback_type, span)
            }
        }
    }

    fn build_error(&mut self, error: BuildError, span: std::ops::Range<usize>) {
        self.error(LowerErrorKind::Build(error.kind), span);
    }

    fn error(&mut self, kind: LowerErrorKind, span: std::ops::Range<usize>) {
        self.errors.push(LowerError {
            kind,
            module: self.module,
            span,
        });
    }

    fn origin(&self, span: std::ops::Range<usize>) -> SourceOrigin {
        SourceOrigin::new(self.module, span)
    }

    fn symbol_declaration(&self, literal: &'ast Literal<'input>) -> Option<SymbolId> {
        match self.resolution.declaration_of_literal(literal) {
            Some(Declaration::Symbol(symbol)) => Some(symbol),
            _ => None,
        }
    }
}

fn mutation_policy(policy: &MutationPolicyKind) -> MutationPolicy {
    match policy {
        MutationPolicyKind::Readonly => MutationPolicy::Readonly,
        MutationPolicyKind::Tunable { cost } => MutationPolicy::Tunable {
            cost: cost.as_ref().map_or(1, |cost| cost.value),
        },
    }
}

fn binary_operation(operation: BinaryOperator) -> BinaryOperation {
    match operation {
        BinaryOperator::Or => BinaryOperation::Or,
        BinaryOperator::And => BinaryOperation::And,
        BinaryOperator::Equal => BinaryOperation::Equal,
        BinaryOperator::NotEqual => BinaryOperation::NotEqual,
        BinaryOperator::LessThan => BinaryOperation::LessThan,
        BinaryOperator::GreaterThan => BinaryOperation::GreaterThan,
        BinaryOperator::LessThanOrEqual => BinaryOperation::LessThanOrEqual,
        BinaryOperator::GreaterThanOrEqual => BinaryOperation::GreaterThanOrEqual,
        BinaryOperator::Add => BinaryOperation::Add,
        BinaryOperator::Subtract => BinaryOperation::Subtract,
        BinaryOperator::Multiply => BinaryOperation::Multiply,
        BinaryOperator::Divide => BinaryOperation::Divide,
    }
}

fn substitute_type(ty: &Type, substitutions: &HashMap<GenericParameterId, Type>) -> Type {
    match ty {
        Type::Set(element) => Type::Set(Box::new(substitute_type(element, substitutions))),
        Type::List(element) => Type::List(Box::new(substitute_type(element, substitutions))),
        Type::Optional(inner) => Type::optional(substitute_type(inner, substitutions)),
        Type::Named(id, arguments) => Type::Named(
            *id,
            arguments
                .iter()
                .map(|argument| substitute_type(argument, substitutions))
                .collect(),
        ),
        Type::Function(function) => Type::function(
            function
                .parameters
                .iter()
                .map(|parameter| substitute_type(parameter, substitutions))
                .collect(),
            substitute_type(&function.return_type, substitutions),
        ),
        Type::Parameter(parameter) => substitutions
            .get(parameter)
            .cloned()
            .unwrap_or(Type::Parameter(*parameter)),
        other => other.clone(),
    }
}

fn parse_number(value: &str) -> Result<Constant, ()> {
    let value = value.replace('_', "");
    if value.contains(['.', 'e', 'E']) {
        let value = value.parse::<f64>().map_err(|_| ())?;
        value
            .is_finite()
            .then_some(Constant::Float(value))
            .ok_or(())
    } else {
        value.parse::<i64>().map(Constant::Int).map_err(|_| ())
    }
}

fn decode_string(value: &str) -> Result<String, ()> {
    let mut chars = value.chars();
    let quote = chars
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))
        .ok_or(())?;
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
            return chars.next().is_none().then_some(result).ok_or(());
        } else {
            result.push(character);
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use light_nix_name_resolver::{
        Declaration, ImportEnvironment, ModuleId, NameResolution, TypeDefId, collect_module,
    };
    use light_nix_parser::{
        ast::{AstArena, Literal, Source, Statement},
        lexer::Lexer,
        parser::{ParseErrors, parse_source},
    };
    use light_nix_type_checker::{BuiltinMethod, TypeEnvironment, check_module};

    use super::*;
    use crate::{ExpressionKind, ObjectiveKind};

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

    fn contains_variable(
        model: &ConstraintModel,
        expression: ExpressionId,
        variable: VariableId,
    ) -> bool {
        let Some(expression) = model.expression(expression) else {
            return false;
        };
        match expression.kind() {
            ExpressionKind::Variable(found) => *found == variable,
            ExpressionKind::Set(values) => values
                .iter()
                .any(|value| contains_variable(model, *value, variable)),
            ExpressionKind::Some(value)
            | ExpressionKind::OptionalValue(value)
            | ExpressionKind::Unary { operand: value, .. } => {
                contains_variable(model, *value, variable)
            }
            ExpressionKind::Binary { left, right, .. } => {
                contains_variable(model, *left, variable)
                    || contains_variable(model, *right, variable)
            }
            ExpressionKind::If {
                condition,
                then_value,
                else_value,
            } => {
                contains_variable(model, *condition, variable)
                    || contains_variable(model, *then_value, variable)
                    || contains_variable(model, *else_value, variable)
            }
            ExpressionKind::Elvis { optional, fallback } => {
                contains_variable(model, *optional, variable)
                    || contains_variable(model, *fallback, variable)
            }
            ExpressionKind::Field { receiver, .. } => contains_variable(model, *receiver, variable),
            ExpressionKind::Call {
                receiver,
                arguments,
                ..
            } => {
                receiver.is_some_and(|receiver| contains_variable(model, receiver, variable))
                    || arguments
                        .iter()
                        .any(|argument| contains_variable(model, *argument, variable))
            }
            ExpressionKind::Unreachable
            | ExpressionKind::Constant(_)
            | ExpressionKind::Output(_)
            | ExpressionKind::Function(_)
            | ExpressionKind::Parameter(_)
            | ExpressionKind::Null => false,
            ExpressionKind::Closure { body, .. } => contains_variable(model, *body, variable),
        }
    }

    #[test]
    fn lowers_tunables_guards_outputs_policies_and_change_costs() {
        let source = r#"
type Firefox {
    enable: Bool
}
type Hyprland {
    enable: Bool
}
type Programs {
    readonly firefox: Firefox
    tunable(cost = 200) hyprland: Hyprland
}
let tunable(cost = 7) n = 0
declare let tunable(cost = 1) programs: Programs

if n == 100 {
    programs.firefox.enable = true
    programs.hyprland.enable = true
}
assert n >= 0, "n must stay non-negative"
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
        let lowered = lower_module(ast, &resolution, &types);
        assert!(
            lowered.errors().is_empty(),
            "lower errors: {:#?}",
            lowered.errors()
        );
        let model = lowered.model();

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
        let n_variable = model
            .variable_for_source(&VariableSource::Symbol(n))
            .expect("n variable");
        let programs_variable = model
            .variable_for_source(&VariableSource::Symbol(programs))
            .expect("programs variable");

        assert_eq!(
            model.variable(n_variable).map(|variable| variable.kind()),
            Some(VariableKind::Tunable { cost: 7 })
        );
        assert_eq!(
            model
                .variable(programs_variable)
                .map(|variable| variable.kind()),
            Some(VariableKind::Input)
        );
        let initial = model
            .variable(n_variable)
            .and_then(|variable| variable.initial())
            .and_then(|expression| model.expression(expression))
            .expect("n initial expression");
        assert_eq!(initial.kind(), &ExpressionKind::Constant(Constant::Int(0)));

        let objectives = model.objectives().collect::<Vec<_>>();
        let [objective] = objectives.as_slice() else {
            panic!("expected one change objective");
        };
        let ObjectiveKind::MinimizeChanges(changes) = objective.kind() else {
            panic!("expected weighted change objective");
        };
        assert!(changes.contains(&WeightedVariable::new(n_variable, 7)));
        assert_eq!(changes.len(), 1);

        let programs_type = type_of(&resolution, &programs_type.name);
        let firefox = field_named(&resolution, programs_type, "firefox");
        let hyprland = field_named(&resolution, programs_type, "hyprland");
        let firefox_enable = field_named(
            &resolution,
            type_of(&resolution, &firefox_type.name),
            "enable",
        );
        let hyprland_enable = field_named(
            &resolution,
            type_of(&resolution, &hyprland_type.name),
            "enable",
        );
        let firefox_output = model
            .output(
                &OutputPath::root(programs)
                    .field(firefox)
                    .field(firefox_enable),
            )
            .expect("firefox output");
        let hyprland_output = model
            .output(
                &OutputPath::root(programs)
                    .field(hyprland)
                    .field(hyprland_enable),
            )
            .expect("hyprland output");
        assert_eq!(firefox_output.policy(), MutationPolicy::Readonly);
        assert_eq!(
            hyprland_output.policy(),
            MutationPolicy::Tunable { cost: 200 }
        );
        assert_eq!(firefox_output.cases().len(), 1);
        assert_eq!(hyprland_output.cases().len(), 1);
        assert_eq!(model.paths().len(), 2);
        assert!(contains_variable(
            model,
            firefox_output.cases()[0].guard(),
            n_variable
        ));
        assert!(contains_variable(
            model,
            hyprland_output.cases()[0].guard(),
            n_variable
        ));
        assert_eq!(model.constraints().len(), 1);
    }

    #[test]
    fn lowers_sets_builtins_and_optional_match_to_typed_nodes() {
        let source = r#"
type Programs {
    enabled: Bool
    selected: Int
}
let tunable packages = ["firefox", "kitty"]
let has_firefox = packages.contains("firefox")
let optional: Int? = some(1)
let selected = match optional {
    some(value) => value
    null => 0
}
declare let programs: Programs
programs.enabled = has_firefox
programs.selected = selected
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(types.errors().is_empty(), "{:#?}", types.errors());
        let lowered = lower_module(ast, &resolution, &types);
        assert!(
            lowered.errors().is_empty(),
            "lower errors: {:#?}",
            lowered.errors()
        );
        let model = lowered.model();
        assert!(model.expressions().any(|expression| matches!(
            expression.kind(),
            ExpressionKind::Call {
                target: CallTarget::Builtin(BuiltinMethod::Contains),
                ..
            }
        )));
        assert!(
            model
                .expressions()
                .any(|expression| matches!(expression.kind(), ExpressionKind::OptionalValue(_)))
        );
        assert!(
            model
                .expressions()
                .any(|expression| matches!(expression.kind(), ExpressionKind::If { .. }))
        );
        assert!(
            model
                .constraints()
                .any(|constraint| constraint.kind() == ConstraintKind::Validity)
        );
        assert_eq!(model.outputs().len(), 2);
    }

    #[test]
    fn lowers_closures_to_typed_parameters_and_preserves_their_modes() {
        let source = r#"
type Programs {
    filtered: Set<Int>
    mapped: Set<Int>
}
let values = [1, 2, 3]
let filtered = values.filter(inline |value| => value > 1)
let mapped = values.map(opaque |value: Int| -> Int => {
    return value + 1
})
declare let programs: Programs
programs.filtered = filtered
programs.mapped = mapped
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(types.errors().is_empty(), "{:#?}", types.errors());
        let lowered = lower_module(ast, &resolution, &types);
        assert!(lowered.errors().is_empty(), "{:#?}", lowered.errors());
        let model = lowered.model();

        let closures = model
            .expressions()
            .filter_map(|expression| match expression.kind() {
                ExpressionKind::Closure {
                    mode,
                    parameters,
                    body,
                } => Some((*mode, parameters, *body)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(closures.len(), 2);
        assert_eq!(closures[0].0, FunctionMode::Inline);
        assert_eq!(closures[1].0, FunctionMode::Opaque);
        for (_, parameters, body) in closures {
            assert_eq!(parameters.len(), 1);
            assert_eq!(parameters[0].ty(), &light_nix_type_checker::Type::Int);
            assert!(model.expressions().any(|expression| {
                matches!(
                    expression.kind(),
                    ExpressionKind::Parameter(symbol)
                        if *symbol == parameters[0].symbol()
                )
            }));
            assert!(model.expression(body).is_some());
        }
    }

    #[test]
    fn declares_unassigned_generic_leaf_paths_with_substituted_types() {
        let source = r#"
type Box<T> {
    tunable(cost = 9) value: T
}
declare let boxed: Box<String>
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(types.errors().is_empty(), "{:#?}", types.errors());
        let lowered = lower_module(ast, &resolution, &types);
        assert!(
            lowered.errors().is_empty(),
            "lower errors: {:#?}",
            lowered.errors()
        );

        let Statement::TypeDefine(box_type) = ast.statements[0] else {
            panic!("expected Box type");
        };
        let Statement::LetStatement(boxed) = ast.statements[1] else {
            panic!("expected boxed declaration");
        };
        let boxed = symbol_of(&resolution, &boxed.name);
        let value = field_named(&resolution, type_of(&resolution, &box_type.name), "value");
        let path = lowered
            .model()
            .path(&OutputPath::root(boxed).field(value))
            .expect("declared value path");
        assert_eq!(path.ty(), &Type::String);
        assert_eq!(path.policy(), MutationPolicy::Tunable { cost: 9 });
        assert_eq!(lowered.model().outputs().len(), 0);
    }
}
