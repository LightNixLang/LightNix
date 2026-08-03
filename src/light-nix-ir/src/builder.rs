use light_nix_name_resolver::ModuleId;
use light_nix_type_checker::Type;

use crate::{
    BinaryOperation, BuildError, BuildErrorKind, CallTarget, ClosureParameter, Constant,
    ConstraintId, ConstraintKind, ConstraintModel, ExpressionId, ExpressionKind, FunctionMode,
    MutationPolicy, ObjectiveId, ObjectiveKind, OutputPath, SourceOrigin, UnaryOperation,
    VariableId, VariableKind, VariableSource,
    model::{constraint, expression, objective, output_case, path_declaration, variable},
};

#[derive(Debug)]
pub struct ModelBuilder {
    model: ConstraintModel,
}

impl ModelBuilder {
    pub fn new(module: ModuleId) -> Self {
        Self {
            model: ConstraintModel::new(module),
        }
    }

    pub fn from_model(model: ConstraintModel) -> Self {
        Self { model }
    }

    pub fn module(&self) -> ModuleId {
        self.model.module()
    }

    pub fn model(&self) -> &ConstraintModel {
        &self.model
    }

    pub fn finish(self) -> ConstraintModel {
        self.model
    }

    pub fn add_variable(
        &mut self,
        source: VariableSource,
        ty: Type,
        kind: VariableKind,
        initial: Option<ExpressionId>,
        origin: Option<SourceOrigin>,
    ) -> Result<VariableId, BuildError> {
        if self.model.variable_for_source(&source).is_some() {
            return Err(error(BuildErrorKind::DuplicateVariableSource));
        }
        if let Some(initial) = initial {
            self.require_type(initial, &ty)?;
        }
        let id = VariableId(index(self.model.variable_count())?);
        self.model
            .push_variable(variable(id, source, ty, kind, initial, origin));
        Ok(id)
    }

    pub fn set_variable_initial(
        &mut self,
        variable: VariableId,
        initial: ExpressionId,
    ) -> Result<(), BuildError> {
        let Some(value) = self.model.variable(variable) else {
            return Err(error(BuildErrorKind::UnknownVariable(variable)));
        };
        if value.initial().is_some() {
            return Err(error(BuildErrorKind::DuplicateInitialValue(variable)));
        }
        let ty = value.ty().clone();
        self.require_type(initial, &ty)?;
        self.model.set_variable_initial(variable, initial);
        Ok(())
    }

    pub fn constant(
        &mut self,
        ty: Type,
        value: Constant,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        if !constant_matches(&value, &ty) {
            return Err(error(BuildErrorKind::InvalidConstant { expected: ty }));
        }
        self.push_expression(ty, ExpressionKind::Constant(value), origin)
    }

    pub fn unreachable(
        &mut self,
        ty: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        self.push_expression(ty, ExpressionKind::Unreachable, origin)
    }

    pub fn variable_reference(
        &mut self,
        variable: VariableId,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let Some(variable_value) = self.model.variable(variable) else {
            return Err(error(BuildErrorKind::UnknownVariable(variable)));
        };
        self.push_expression(
            variable_value.ty().clone(),
            ExpressionKind::Variable(variable),
            origin,
        )
    }

    pub fn output_reference(
        &mut self,
        path: OutputPath,
        ty: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        self.push_expression(ty, ExpressionKind::Output(path), origin)
    }

    pub fn function_reference(
        &mut self,
        symbol: light_nix_name_resolver::SymbolId,
        ty: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        self.push_expression(ty, ExpressionKind::Function(symbol), origin)
    }

    pub fn parameter_reference(
        &mut self,
        symbol: light_nix_name_resolver::SymbolId,
        ty: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        self.push_expression(ty, ExpressionKind::Parameter(symbol), origin)
    }

    pub fn closure(
        &mut self,
        mode: FunctionMode,
        parameters: Vec<ClosureParameter>,
        body: ExpressionId,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let return_type = self.expression_type(body)?.clone();
        let ty = Type::function(
            parameters
                .iter()
                .map(|parameter| parameter.ty().clone())
                .collect(),
            return_type,
        );
        self.push_expression(
            ty,
            ExpressionKind::Closure {
                mode,
                parameters,
                body,
            },
            origin,
        )
    }

    pub fn set(
        &mut self,
        element_type: Type,
        values: Vec<ExpressionId>,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        for value in &values {
            self.require_type(*value, &element_type)?;
        }
        self.push_expression(
            Type::Set(Box::new(element_type)),
            ExpressionKind::Set(values),
            origin,
        )
    }

    pub fn list(
        &mut self,
        element_type: Type,
        values: Vec<ExpressionId>,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        for value in &values {
            self.require_type(*value, &element_type)?;
        }
        self.push_expression(
            Type::List(Box::new(element_type)),
            ExpressionKind::List(values),
            origin,
        )
    }

    pub fn some(
        &mut self,
        value: ExpressionId,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let ty = self.expression_type(value)?.clone();
        self.push_expression(Type::optional(ty), ExpressionKind::Some(value), origin)
    }

    pub fn null(
        &mut self,
        ty: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        if !matches!(ty, Type::Optional(_)) {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.push_expression(ty, ExpressionKind::Null, origin)
    }

    pub fn optional_value(
        &mut self,
        optional: ExpressionId,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let Type::Optional(inner) = self.expression_type(optional)? else {
            return Err(error(BuildErrorKind::InvalidOperation));
        };
        let result_type = inner.as_ref().clone();
        self.push_expression(result_type, ExpressionKind::OptionalValue(optional), origin)
    }

    pub fn union_inject(
        &mut self,
        value: ExpressionId,
        union: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let found = self.expression_type(value)?;
        if !matches!(union, Type::Union(_)) || !union.accepts(found) {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.push_expression(
            union.clone(),
            ExpressionKind::UnionInject { value, union },
            origin,
        )
    }

    pub fn type_is(
        &mut self,
        value: ExpressionId,
        target: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let source = self.expression_type(value)?;
        if !source.contains_union_alternative(&target) {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.push_expression(Type::Bool, ExpressionKind::TypeIs { value, target }, origin)
    }

    pub fn union_project(
        &mut self,
        value: ExpressionId,
        target: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let source = self.expression_type(value)?;
        if !matches!(source, Type::Union(_)) || !source.contains_union_alternative(&target) {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.push_expression(
            target.clone(),
            ExpressionKind::UnionProject { value, target },
            origin,
        )
    }

    pub fn safe_cast(
        &mut self,
        value: ExpressionId,
        target: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let source = self.expression_type(value)?;
        if !source.contains_union_alternative(&target) {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.push_expression(
            Type::optional(target.clone()),
            ExpressionKind::SafeCast { value, target },
            origin,
        )
    }

    pub fn unary(
        &mut self,
        operation: UnaryOperation,
        operand: ExpressionId,
        result_type: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let operand_type = self.expression_type(operand)?;
        let valid = match operation {
            UnaryOperation::Positive | UnaryOperation::Negate => {
                is_numeric(operand_type) && operand_type == &result_type
            }
            UnaryOperation::Not => operand_type == &Type::Bool && result_type == Type::Bool,
            UnaryOperation::IsNull => {
                matches!(operand_type, Type::Optional(_)) && result_type == Type::Bool
            }
        };
        if !valid {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.push_expression(
            result_type,
            ExpressionKind::Unary { operation, operand },
            origin,
        )
    }

    pub fn binary(
        &mut self,
        operation: BinaryOperation,
        left: ExpressionId,
        right: ExpressionId,
        result_type: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let left_type = self.expression_type(left)?;
        let right_type = self.expression_type(right)?;
        let same = left_type == right_type;
        let valid = match operation {
            BinaryOperation::Or | BinaryOperation::And => {
                same && left_type == &Type::Bool && result_type == Type::Bool
            }
            BinaryOperation::Equal | BinaryOperation::NotEqual => same && result_type == Type::Bool,
            BinaryOperation::LessThan
            | BinaryOperation::GreaterThan
            | BinaryOperation::LessThanOrEqual
            | BinaryOperation::GreaterThanOrEqual => {
                same && is_numeric(left_type) && result_type == Type::Bool
            }
            BinaryOperation::Add => {
                same && (is_numeric(left_type) || left_type == &Type::String)
                    && left_type == &result_type
            }
            BinaryOperation::Subtract | BinaryOperation::Multiply | BinaryOperation::Divide => {
                same && is_numeric(left_type) && left_type == &result_type
            }
        };
        if !valid {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.push_expression(
            result_type,
            ExpressionKind::Binary {
                operation,
                left,
                right,
            },
            origin,
        )
    }

    pub fn if_then_else(
        &mut self,
        condition: ExpressionId,
        then_value: ExpressionId,
        else_value: ExpressionId,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        self.require_boolean(condition)?;
        let ty = self.expression_type(then_value)?.clone();
        self.require_type(else_value, &ty)?;
        self.push_expression(
            ty,
            ExpressionKind::If {
                condition,
                then_value,
                else_value,
            },
            origin,
        )
    }

    pub fn elvis(
        &mut self,
        optional: ExpressionId,
        fallback: ExpressionId,
        result_type: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let Type::Optional(inner) = self.expression_type(optional)? else {
            return Err(error(BuildErrorKind::InvalidOperation));
        };
        if inner.as_ref() != &result_type {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.require_type(fallback, &result_type)?;
        self.push_expression(
            result_type,
            ExpressionKind::Elvis { optional, fallback },
            origin,
        )
    }

    pub fn field(
        &mut self,
        receiver: ExpressionId,
        field: light_nix_name_resolver::FieldId,
        safe: bool,
        result_type: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        self.expression_type(receiver)?;
        self.push_expression(
            result_type,
            ExpressionKind::Field {
                receiver,
                field,
                safe,
            },
            origin,
        )
    }

    pub fn attr_set_key(
        &mut self,
        receiver: ExpressionId,
        key: String,
        result_type: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let Type::AttrSet(element) = self.expression_type(receiver)? else {
            return Err(error(BuildErrorKind::InvalidOperation));
        };
        if element.as_ref() != &result_type {
            return Err(error(BuildErrorKind::InvalidOperation));
        }
        self.push_expression(
            result_type,
            ExpressionKind::AttrSetKey { receiver, key },
            origin,
        )
    }

    pub fn call(
        &mut self,
        target: CallTarget,
        receiver: Option<ExpressionId>,
        arguments: Vec<ExpressionId>,
        result_type: Type,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        if let Some(receiver) = receiver {
            self.expression_type(receiver)?;
        }
        for argument in &arguments {
            self.expression_type(*argument)?;
        }
        self.push_expression(
            result_type,
            ExpressionKind::Call {
                target,
                receiver,
                arguments,
            },
            origin,
        )
    }

    pub fn add_constraint(
        &mut self,
        condition: ExpressionId,
        kind: ConstraintKind,
        origin: Option<SourceOrigin>,
    ) -> Result<ConstraintId, BuildError> {
        self.require_boolean(condition)?;
        let id = ConstraintId(index(self.model.constraint_count())?);
        self.model
            .push_constraint(constraint(id, condition, kind, origin));
        Ok(id)
    }

    pub fn add_output_case(
        &mut self,
        path: OutputPath,
        ty: Type,
        policy: MutationPolicy,
        guard: ExpressionId,
        value: ExpressionId,
        origin: SourceOrigin,
    ) -> Result<(), BuildError> {
        self.require_boolean(guard)?;
        self.require_type(value, &ty)?;
        if let Some(declaration) = self.model.path(&path) {
            if declaration.ty() != &ty {
                return Err(error(BuildErrorKind::OutputTypeMismatch {
                    path,
                    expected: declaration.ty().clone(),
                    found: ty,
                }));
            }
            if declaration.policy() != policy {
                return Err(error(BuildErrorKind::OutputPolicyMismatch { path }));
            }
        } else {
            self.model.push_path_declaration(path_declaration(
                path.clone(),
                ty.clone(),
                policy,
                origin.clone(),
            ));
        }
        if let Some(existing) = self.model.output(&path) {
            if existing.ty() != &ty {
                return Err(error(BuildErrorKind::OutputTypeMismatch {
                    path,
                    expected: existing.ty().clone(),
                    found: ty,
                }));
            }
            if existing.policy() != policy {
                return Err(error(BuildErrorKind::OutputPolicyMismatch { path }));
            }
        }
        self.model
            .push_output_case(path, ty, policy, output_case(guard, value, origin));
        Ok(())
    }

    pub fn declare_output_path(
        &mut self,
        path: OutputPath,
        ty: Type,
        policy: MutationPolicy,
        origin: SourceOrigin,
    ) -> Result<(), BuildError> {
        if let Some(existing) = self.model.path(&path) {
            if existing.ty() != &ty {
                return Err(error(BuildErrorKind::OutputTypeMismatch {
                    path,
                    expected: existing.ty().clone(),
                    found: ty,
                }));
            }
            if existing.policy() != policy {
                return Err(error(BuildErrorKind::OutputPolicyMismatch { path }));
            }
            return Ok(());
        }
        self.model
            .push_path_declaration(path_declaration(path, ty, policy, origin));
        Ok(())
    }

    pub fn add_objective(
        &mut self,
        kind: ObjectiveKind,
        origin: Option<SourceOrigin>,
    ) -> Result<ObjectiveId, BuildError> {
        match &kind {
            ObjectiveKind::Minimize(expression) => {
                if !is_numeric(self.expression_type(*expression)?) {
                    return Err(error(BuildErrorKind::InvalidOperation));
                }
            }
            ObjectiveKind::MinimizeChanges(variables) => {
                for weighted in variables {
                    let Some(variable) = self.model.variable(weighted.variable()) else {
                        return Err(error(BuildErrorKind::UnknownVariable(weighted.variable())));
                    };
                    if !matches!(variable.kind(), VariableKind::Tunable { .. }) {
                        return Err(error(BuildErrorKind::InvalidOperation));
                    }
                }
            }
        }
        let id = ObjectiveId(index(self.model.objective_count())?);
        self.model.push_objective(objective(id, kind, origin));
        Ok(id)
    }

    fn push_expression(
        &mut self,
        ty: Type,
        kind: ExpressionKind,
        origin: Option<SourceOrigin>,
    ) -> Result<ExpressionId, BuildError> {
        let id = ExpressionId(index(self.model.expression_count())?);
        self.model.push_expression(expression(id, ty, kind, origin));
        Ok(id)
    }

    fn expression_type(&self, expression: ExpressionId) -> Result<&Type, BuildError> {
        self.model
            .expression(expression)
            .map(|expression| expression.ty())
            .ok_or_else(|| error(BuildErrorKind::UnknownExpression(expression)))
    }

    fn require_type(&self, expression: ExpressionId, expected: &Type) -> Result<(), BuildError> {
        let found = self.expression_type(expression)?;
        if !expected.accepts(found) {
            return Err(error(BuildErrorKind::TypeMismatch {
                expected: expected.clone(),
                found: found.clone(),
            }));
        }
        Ok(())
    }

    fn require_boolean(&self, expression: ExpressionId) -> Result<(), BuildError> {
        let found = self.expression_type(expression)?;
        if found != &Type::Bool {
            return Err(error(BuildErrorKind::ExpectedBoolean {
                found: found.clone(),
            }));
        }
        Ok(())
    }
}

fn index(length: usize) -> Result<u32, BuildError> {
    u32::try_from(length).map_err(|_| error(BuildErrorKind::TableOverflow))
}

fn error(kind: BuildErrorKind) -> BuildError {
    BuildError { kind }
}

fn is_numeric(ty: &Type) -> bool {
    matches!(ty, Type::Int | Type::Float)
}

fn constant_matches(value: &Constant, ty: &Type) -> bool {
    match (value, ty) {
        (Constant::Unit, Type::Unit) => true,
        (Constant::Bool(_), Type::Bool) => true,
        (Constant::Int(_), Type::Int) => true,
        (Constant::Float(value), Type::Float) => value.is_finite(),
        (Constant::String(_), Type::String) => true,
        (Constant::Package(_), Type::Package) => true,
        (Constant::List(values), Type::List(element)) => {
            values.iter().all(|value| constant_matches(value, element))
        }
        (Constant::Set(values), Type::Set(element)) => {
            values.iter().all(|value| constant_matches(value, element))
        }
        (Constant::AttrSet(values), Type::AttrSet(element)) => values
            .values()
            .all(|value| constant_matches(value, element)),
        (Constant::Optional(None), Type::Optional(_)) => true,
        (Constant::Optional(Some(value)), Type::Optional(inner)) => constant_matches(value, inner),
        (Constant::Enum(_), Type::Named(_, _)) => true,
        (value, Type::Union(alternatives)) => alternatives
            .iter()
            .any(|alternative| constant_matches(value, alternative)),
        _ => false,
    }
}
