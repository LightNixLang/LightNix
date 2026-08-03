use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
};

use light_nix_name_resolver::{
    AstId, AstKind, BuiltinType, Declaration, FieldId, GenericParameterId, NameResolution, Res,
    SymbolId, SymbolKind, TypeDefId, TypeDefKind,
};
use light_nix_parser::ast::{
    AST, AccessOperator, Array, AssignValue, BinaryOperator, ClosureBody, ClosureExpression,
    CollectionKind, ElseBranchValue, ExplicitTypeArgument, ExplicitTypeArguments, Expression,
    FunctionCall, FunctionDefine, GenericParameters, ImplementsDefine, InterfaceDefine,
    LetStatement, NestedAssignment, Pattern, Primary, PrimaryAccess, Source, Statement, Statements,
    TypeDefine, TypeInfo, TypeOperator, TypedefBlock, TypedefValue, UnaryOperator, Value,
    WhereClause,
};

use crate::{
    BuiltinMethod, InterfaceBound, Type, TypeCheckError, TypeCheckErrorKind, TypeScheme,
    builtin::find_builtin_method, unify::Unifier,
};

#[derive(Debug, Clone, Default)]
pub struct TypeEnvironment {
    symbols: HashMap<SymbolId, TypeScheme>,
    fields: HashMap<FieldId, Type>,
    field_lookup: HashMap<(TypeDefId, String), FieldId>,
    generic_arities: HashMap<TypeDefId, usize>,
    type_parameters: HashMap<TypeDefId, Vec<GenericParameterId>>,
    type_bounds: HashMap<TypeDefId, Vec<InterfaceBound>>,
    interfaces: HashSet<TypeDefId>,
    implementations: Vec<ImplementationScheme>,
    interface_methods: Vec<InterfaceMethodScheme>,
}

impl TypeEnvironment {
    pub fn extend(&mut self, other: &Self) {
        self.symbols.extend(other.symbols.clone());
        self.fields.extend(other.fields.clone());
        self.field_lookup.extend(other.field_lookup.clone());
        self.generic_arities.extend(other.generic_arities.clone());
        self.type_parameters.extend(other.type_parameters.clone());
        self.type_bounds.extend(other.type_bounds.clone());
        self.interfaces.extend(&other.interfaces);
        self.implementations
            .extend(other.implementations.iter().cloned());
        self.interface_methods
            .extend(other.interface_methods.iter().cloned());
    }

    pub fn insert_symbol(&mut self, symbol: SymbolId, scheme: TypeScheme) {
        self.symbols.insert(symbol, scheme);
    }

    pub fn insert_field(&mut self, field: FieldId, ty: Type) {
        self.fields.insert(field, ty);
    }

    pub fn insert_named_field(
        &mut self,
        owner: TypeDefId,
        name: impl Into<String>,
        field: FieldId,
        ty: Type,
    ) {
        self.fields.insert(field, ty);
        self.field_lookup.insert((owner, name.into()), field);
    }

    pub fn insert_type(&mut self, ty: TypeDefId, generic_arity: usize) {
        self.generic_arities.insert(ty, generic_arity);
    }

    pub fn insert_generic_type(
        &mut self,
        ty: TypeDefId,
        parameters: Vec<GenericParameterId>,
        bounds: Vec<InterfaceBound>,
    ) {
        self.generic_arities.insert(ty, parameters.len());
        self.type_parameters.insert(ty, parameters);
        self.type_bounds.insert(ty, bounds);
    }

    pub fn insert_interface(&mut self, ty: TypeDefId, generic_arity: usize) {
        self.generic_arities.insert(ty, generic_arity);
        self.interfaces.insert(ty);
    }

    pub fn insert_generic_interface(
        &mut self,
        ty: TypeDefId,
        parameters: Vec<GenericParameterId>,
        bounds: Vec<InterfaceBound>,
    ) {
        self.insert_generic_type(ty, parameters, bounds);
        self.interfaces.insert(ty);
    }

    pub fn insert_implementation(&mut self, implementation: ImplementationScheme) {
        self.implementations.push(implementation);
    }

    pub fn insert_interface_method(&mut self, method: InterfaceMethodScheme) {
        self.interface_methods.push(method);
    }
}

#[derive(Debug)]
pub struct TypeCheckResult<'ast> {
    expression_types: HashMap<AstId<'ast>, Type>,
    value_types: HashMap<AstId<'ast>, Type>,
    member_types: HashMap<AstId<'ast>, Type>,
    type_info_types: HashMap<AstId<'ast>, Type>,
    member_resolutions: HashMap<AstId<'ast>, MemberResolution>,
    symbol_types: HashMap<SymbolId, TypeScheme>,
    field_types: HashMap<FieldId, Type>,
    field_lookup: HashMap<(TypeDefId, String), FieldId>,
    type_arities: HashMap<TypeDefId, usize>,
    type_parameters: HashMap<TypeDefId, Vec<GenericParameterId>>,
    type_bounds: HashMap<TypeDefId, Vec<InterfaceBound>>,
    interfaces: HashSet<TypeDefId>,
    implementations: Vec<ImplementationScheme>,
    interface_methods: Vec<InterfaceMethodScheme>,
    errors: Vec<TypeCheckError>,
}

impl<'ast> TypeCheckResult<'ast> {
    pub fn errors(&self) -> &[TypeCheckError] {
        &self.errors
    }

    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn expression_type<'input, 'allocator>(
        &self,
        expression: &'ast Expression<'input, 'allocator>,
    ) -> Option<&Type> {
        self.expression_types
            .get(&AstId::new(expression, AstKind::Expression))
    }

    pub fn value_type<'input, 'allocator>(
        &self,
        value: &'ast Value<'input, 'allocator>,
    ) -> Option<&Type> {
        self.value_types.get(&AstId::new(value, AstKind::Value))
    }

    pub fn member_type<'input, 'allocator>(
        &self,
        access: &'ast PrimaryAccess<'input, 'allocator>,
    ) -> Option<&Type> {
        self.member_types
            .get(&AstId::new(access, AstKind::PrimaryAccess))
    }

    pub fn type_info_type<'input, 'allocator>(
        &self,
        type_info: &'ast TypeInfo<'input, 'allocator>,
    ) -> Option<&Type> {
        self.type_info_types
            .get(&AstId::new(type_info, AstKind::TypeInfo))
    }

    pub fn member_resolution<'input, 'allocator>(
        &self,
        access: &'ast PrimaryAccess<'input, 'allocator>,
    ) -> Option<&MemberResolution> {
        self.member_resolutions
            .get(&AstId::new(access, AstKind::PrimaryAccess))
    }

    pub fn symbol_type(&self, symbol: SymbolId) -> Option<&TypeScheme> {
        self.symbol_types.get(&symbol)
    }

    pub fn field_type(&self, field: FieldId) -> Option<&Type> {
        self.field_types.get(&field)
    }

    pub fn named_field(&self, receiver: &Type, name: &str) -> Option<(FieldId, Type)> {
        let Type::Named(owner, arguments) = receiver else {
            return None;
        };
        let field = *self.field_lookup.get(&(*owner, name.to_owned()))?;
        let field_type = self.field_types.get(&field)?;
        let substitutions = self
            .type_parameters
            .get(owner)
            .into_iter()
            .flatten()
            .copied()
            .zip(arguments.iter().cloned())
            .collect();
        Some((field, substitute(field_type, &substitutions)))
    }

    pub fn type_parameters(&self, ty: TypeDefId) -> &[GenericParameterId] {
        self.type_parameters.get(&ty).map_or(&[], Vec::as_slice)
    }

    pub fn type_environment(&self) -> TypeEnvironment {
        TypeEnvironment {
            symbols: self.symbol_types.clone(),
            fields: self.field_types.clone(),
            field_lookup: self.field_lookup.clone(),
            generic_arities: self.type_arities.clone(),
            type_parameters: self.type_parameters.clone(),
            type_bounds: self.type_bounds.clone(),
            interfaces: self.interfaces.clone(),
            implementations: self.implementations.clone(),
            interface_methods: self.interface_methods.clone(),
        }
    }
}

pub fn check_module<'ast, 'input, 'allocator>(
    source: &'ast Source<'input, 'allocator>,
    resolution: &NameResolution<'ast>,
    environment: &TypeEnvironment,
) -> TypeCheckResult<'ast> {
    Checker::new(source, resolution, environment).check()
}

#[derive(Debug, Clone)]
pub struct ImplementationScheme {
    pub parameters: Vec<GenericParameterId>,
    pub interface: Type,
    pub target: Type,
    pub bounds: Vec<InterfaceBound>,
    pub methods: HashMap<String, SymbolId>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct InterfaceMethodScheme {
    pub owner: TypeDefId,
    pub symbol: SymbolId,
    pub name: String,
    pub interface: Type,
    pub receiver: Option<Type>,
    pub scheme: TypeScheme,
    pub explicit_parameters: Vec<GenericParameterId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberResolution {
    Field(FieldId),
    AttrSetKey,
    Builtin(BuiltinMethod),
    InterfaceMethod {
        interface: TypeDefId,
        declaration: SymbolId,
        implementation: Option<SymbolId>,
    },
}

#[derive(Debug, Clone)]
struct DispatchOrigin<'ast> {
    member: AstId<'ast>,
    interface: TypeDefId,
    declaration: SymbolId,
    method: String,
}

#[derive(Debug, Clone)]
struct Obligation<'ast> {
    bound: InterfaceBound,
    assumptions: Vec<InterfaceBound>,
    dispatch: Option<DispatchOrigin<'ast>>,
    span: Range<usize>,
}

#[derive(Debug, Clone)]
enum Capability {
    Numeric(Type, Range<usize>),
    Boolean(Type, Range<usize>),
}

#[derive(Debug)]
enum PrimaryState {
    Value(Type),
    Type(TypeDefId),
    Module,
    Error,
}

struct Checker<'ast, 'input, 'allocator, 'context, 'environment> {
    source: &'ast Source<'input, 'allocator>,
    resolution: &'context NameResolution<'ast>,
    environment: &'environment TypeEnvironment,
    unifier: Unifier,
    expression_types: HashMap<AstId<'ast>, Type>,
    value_types: HashMap<AstId<'ast>, Type>,
    member_types: HashMap<AstId<'ast>, Type>,
    type_info_types: HashMap<AstId<'ast>, Type>,
    member_resolutions: HashMap<AstId<'ast>, MemberResolution>,
    symbol_types: HashMap<SymbolId, TypeScheme>,
    field_types: HashMap<FieldId, Type>,
    field_lookup: HashMap<(TypeDefId, String), FieldId>,
    type_arities: HashMap<TypeDefId, usize>,
    type_parameters: HashMap<TypeDefId, Vec<GenericParameterId>>,
    type_bounds: HashMap<TypeDefId, Vec<InterfaceBound>>,
    interface_types: HashSet<TypeDefId>,
    implementations: Vec<ImplementationScheme>,
    interface_methods: Vec<InterfaceMethodScheme>,
    obligations: VecDeque<Obligation<'ast>>,
    capabilities: Vec<Capability>,
    assumptions: Vec<InterfaceBound>,
    current_return: Option<Type>,
    package_literals: HashSet<AstId<'ast>>,
    errors: Vec<TypeCheckError>,
}

impl<'ast, 'input, 'allocator, 'context, 'environment>
    Checker<'ast, 'input, 'allocator, 'context, 'environment>
{
    fn new(
        source: &'ast Source<'input, 'allocator>,
        resolution: &'context NameResolution<'ast>,
        environment: &'environment TypeEnvironment,
    ) -> Self {
        let mut symbol_types = environment.symbols.clone();
        let mut field_types = environment.fields.clone();
        let mut type_arities = environment.generic_arities.clone();
        let type_parameters = environment.type_parameters.clone();
        let type_bounds = environment.type_bounds.clone();
        let mut interface_types = environment.interfaces.clone();
        for symbol in resolution.symbols() {
            symbol_types.remove(&symbol.id);
        }
        for field in resolution.fields() {
            field_types.remove(&field.id);
        }
        for ty in resolution.types() {
            type_arities.entry(ty.id).or_insert(0);
            if ty.kind == TypeDefKind::Interface {
                interface_types.insert(ty.id);
            }
        }
        Self {
            source,
            resolution,
            environment,
            unifier: Unifier::default(),
            expression_types: HashMap::new(),
            value_types: HashMap::new(),
            member_types: HashMap::new(),
            type_info_types: HashMap::new(),
            member_resolutions: HashMap::new(),
            symbol_types,
            field_types,
            field_lookup: environment.field_lookup.clone(),
            type_arities,
            type_parameters,
            type_bounds,
            interface_types,
            implementations: environment.implementations.clone(),
            interface_methods: environment.interface_methods.clone(),
            obligations: VecDeque::new(),
            capabilities: Vec::new(),
            assumptions: Vec::new(),
            current_return: None,
            package_literals: HashSet::new(),
            errors: Vec::new(),
        }
    }

    fn check(mut self) -> TypeCheckResult<'ast> {
        self.collect_type_shapes(self.source);
        self.collect_interfaces(self.source);
        self.collect_implementations(self.source);
        self.predeclare_statements(self.source);
        self.check_statements(self.source);
        self.solve_obligations();
        self.check_capabilities();
        self.check_unresolved_types();
        self.finish()
    }

    fn finish(mut self) -> TypeCheckResult<'ast> {
        for ty in self.expression_types.values_mut() {
            *ty = self.unifier.resolve(ty);
        }
        for ty in self.value_types.values_mut() {
            *ty = self.unifier.resolve(ty);
        }
        for ty in self.member_types.values_mut() {
            *ty = self.unifier.resolve(ty);
        }
        for ty in self.type_info_types.values_mut() {
            *ty = self.unifier.resolve(ty);
        }
        for scheme in self.symbol_types.values_mut() {
            scheme.ty = self.unifier.resolve(&scheme.ty);
            for bound in &mut scheme.bounds {
                bound.subject = self.unifier.resolve(&bound.subject);
                bound.interface = self.unifier.resolve(&bound.interface);
            }
        }
        for ty in self.field_types.values_mut() {
            *ty = self.unifier.resolve(ty);
        }
        for implementation in &mut self.implementations {
            implementation.interface = self.unifier.resolve(&implementation.interface);
            implementation.target = self.unifier.resolve(&implementation.target);
            for bound in &mut implementation.bounds {
                bound.subject = self.unifier.resolve(&bound.subject);
                bound.interface = self.unifier.resolve(&bound.interface);
            }
        }
        for method in &mut self.interface_methods {
            method.interface = self.unifier.resolve(&method.interface);
            if let Some(receiver) = &mut method.receiver {
                *receiver = self.unifier.resolve(receiver);
            }
            method.scheme.ty = self.unifier.resolve(&method.scheme.ty);
            for bound in &mut method.scheme.bounds {
                bound.subject = self.unifier.resolve(&bound.subject);
                bound.interface = self.unifier.resolve(&bound.interface);
            }
        }
        TypeCheckResult {
            expression_types: self.expression_types,
            value_types: self.value_types,
            member_types: self.member_types,
            type_info_types: self.type_info_types,
            member_resolutions: self.member_resolutions,
            symbol_types: self.symbol_types,
            field_types: self.field_types,
            field_lookup: self.field_lookup,
            type_arities: self.type_arities,
            type_parameters: self.type_parameters,
            type_bounds: self.type_bounds,
            interfaces: self.interface_types,
            implementations: self.implementations,
            interface_methods: self.interface_methods,
            errors: self.errors,
        }
    }

    fn collect_type_shapes(&mut self, statements: &'ast Statements<'input, 'allocator>) {
        for statement in statements.statements {
            match statement {
                Statement::TypeDefine(node) => {
                    if let Some(id) = self.type_declaration(&node.name) {
                        let parameters = self.generic_parameter_ids(node.generic_parameters);
                        self.type_arities.insert(id, parameters.len());
                        self.type_parameters.insert(id, parameters);
                    }
                }
                Statement::InterfaceDefine(node) => {
                    if let Some(id) = self.type_declaration(&node.name) {
                        let parameters = self.generic_parameter_ids(node.generic_parameters);
                        self.type_arities.insert(id, parameters.len());
                        self.type_parameters.insert(id, parameters);
                        self.interface_types.insert(id);
                    }
                }
                _ => {}
            }
        }
        for statement in statements.statements {
            match statement {
                Statement::TypeDefine(node) => {
                    if let Some(id) = self.type_declaration(&node.name) {
                        let bounds =
                            self.collect_bounds(node.generic_parameters, node.where_clause);
                        self.type_bounds.insert(id, bounds);
                    }
                }
                Statement::InterfaceDefine(node) => {
                    if let Some(id) = self.type_declaration(&node.name) {
                        let bounds =
                            self.collect_bounds(node.generic_parameters, node.where_clause);
                        self.type_bounds.insert(id, bounds);
                    }
                }
                _ => {}
            }
        }
        for statement in statements.statements {
            if let Statement::TypeDefine(node) = statement {
                self.collect_record(node);
            }
        }
    }

    fn collect_record(&mut self, node: &'ast TypeDefine<'input, 'allocator>) {
        let Some(owner) = self.type_declaration(&node.name) else {
            return;
        };
        let parameters = self
            .type_parameters
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let old_assumptions = self.assumptions.clone();
        self.assumptions
            .extend(self.type_bounds.get(&owner).cloned().unwrap_or_default());
        self.collect_record_block(node.body, owner, &parameters);
        self.assumptions = old_assumptions;
    }

    fn collect_record_block(
        &mut self,
        block: &'ast TypedefBlock<'input, 'allocator>,
        owner: TypeDefId,
        parameters: &[GenericParameterId],
    ) {
        for field in block.fields {
            let Some(Declaration::Field(field_id)) =
                self.resolution.declaration_of_literal(&field.name)
            else {
                continue;
            };
            let ty = match field.value {
                TypedefValue::TypeInfo(type_info) => self.lower_type_info(type_info),
                TypedefValue::Block(nested) => {
                    let nested_id = self
                        .resolution
                        .fields()
                        .iter()
                        .find(|candidate| candidate.id == field_id)
                        .and_then(|field| field.nested_type);
                    match nested_id {
                        Some(nested_id) => {
                            self.type_arities.insert(nested_id, parameters.len());
                            self.type_parameters.insert(nested_id, parameters.to_vec());
                            self.type_bounds.insert(
                                nested_id,
                                self.type_bounds.get(&owner).cloned().unwrap_or_default(),
                            );
                            self.collect_record_block(nested, nested_id, parameters);
                            Type::Named(
                                nested_id,
                                parameters.iter().copied().map(Type::Parameter).collect(),
                            )
                        }
                        None => Type::Error,
                    }
                }
            };
            self.field_types.insert(field_id, ty);
            self.field_lookup
                .insert((owner, field.name.value.to_owned()), field_id);
        }
    }

    fn collect_interfaces(&mut self, statements: &'ast Statements<'input, 'allocator>) {
        for statement in statements.statements {
            let Statement::InterfaceDefine(interface) = statement else {
                continue;
            };
            self.collect_interface(interface);
        }
    }

    fn collect_interface(&mut self, node: &'ast InterfaceDefine<'input, 'allocator>) {
        let Some(owner) = self.type_declaration(&node.name) else {
            return;
        };
        let interface_parameters = self.generic_parameter_ids(node.generic_parameters);
        let interface = Type::Named(
            owner,
            interface_parameters
                .iter()
                .copied()
                .map(Type::Parameter)
                .collect(),
        );
        let this_parameter = self
            .resolution
            .generic_parameters()
            .iter()
            .find(|parameter| parameter.declaration == AstId::new(node, AstKind::InterfaceDefine))
            .map(|parameter| parameter.id);
        let outer_bounds = self.type_bounds.get(&owner).cloned().unwrap_or_default();
        let old_assumptions = self.assumptions.clone();
        self.assumptions.extend(outer_bounds.clone());
        for method in node.methods {
            let Some(symbol) = self.symbol_declaration(&method.name) else {
                continue;
            };
            let receiver = method
                .arguments
                .receiver
                .as_ref()
                .map(|_| this_parameter.map_or(Type::Error, Type::Parameter));
            let mut scheme = self.function_scheme(method, None);
            let explicit_parameters = explicit_parameter_order(&scheme);
            let mut all_parameters = scheme.parameters.clone();
            for parameter in &interface_parameters {
                if !all_parameters.contains(parameter) {
                    all_parameters.push(*parameter);
                }
            }
            if let Some(this_parameter) = this_parameter
                && !all_parameters.contains(&this_parameter)
            {
                all_parameters.push(this_parameter);
            }
            scheme.parameters = all_parameters;
            scheme.bounds.extend(outer_bounds.clone());
            self.interface_methods.push(InterfaceMethodScheme {
                owner,
                symbol,
                name: method.name.value.to_owned(),
                interface: interface.clone(),
                receiver,
                scheme,
                explicit_parameters,
            });
        }
        self.assumptions = old_assumptions;
    }

    fn collect_implementations(&mut self, statements: &'ast Statements<'input, 'allocator>) {
        for statement in statements.statements {
            let Statement::ImplementsDefine(implementation) = statement else {
                continue;
            };
            self.collect_implementation(implementation);
        }
        self.check_duplicate_implementations();
    }

    fn collect_implementation(&mut self, node: &'ast ImplementsDefine<'input, 'allocator>) {
        let parameters = self.generic_parameter_ids(node.generic_parameters);
        let bounds = self.collect_bounds(node.generic_parameters, node.where_clause);
        let old_assumptions = self.assumptions.clone();
        self.assumptions.extend(bounds.clone());
        let interface = self.lower_type_info(node.interface);
        let target = self.lower_type_info(node.target);
        if matches!(target, Type::Union(_)) {
            self.error(
                TypeCheckErrorKind::UnionImplementationTarget,
                node.target.span(),
            );
        }
        let methods = node
            .methods
            .iter()
            .filter_map(|method| {
                self.symbol_declaration(&method.name)
                    .map(|symbol| (method.name.value.to_owned(), symbol))
            })
            .collect();
        if !self.is_interface_type(&interface) && !interface.is_error() {
            self.error(
                TypeCheckErrorKind::ExpectedInterface {
                    found: interface.clone(),
                },
                node.interface.span(),
            );
        }
        self.implementations.push(ImplementationScheme {
            parameters,
            interface,
            target,
            bounds,
            methods,
            span: node.span.clone(),
        });
        self.check_implementation_methods(node);
        self.assumptions = old_assumptions;
    }

    fn check_duplicate_implementations(&mut self) {
        for right in 0..self.implementations.len() {
            for left in 0..right {
                let left_impl = self.implementations[left].clone();
                let right_impl = self.implementations[right].clone();
                let Some(interface) = named_type_id(&right_impl.interface) else {
                    continue;
                };
                if named_type_id(&left_impl.interface) != Some(interface) {
                    continue;
                }
                let mut trial = self.unifier.clone();
                let left_map = instantiate_parameter_map(&mut trial, &left_impl.parameters);
                let right_map = instantiate_parameter_map(&mut trial, &right_impl.parameters);
                let left_target = substitute(&left_impl.target, &left_map);
                let right_target = substitute(&right_impl.target, &right_map);
                let left_interface = substitute(&left_impl.interface, &left_map);
                let right_interface = substitute(&right_impl.interface, &right_map);
                if trial.unify(&left_target, &right_target).is_ok()
                    && trial.unify(&left_interface, &right_interface).is_ok()
                {
                    self.error(
                        TypeCheckErrorKind::DuplicateImplementation { interface },
                        right_impl.span,
                    );
                }
            }
        }
    }

    fn check_implementation_methods(&mut self, node: &'ast ImplementsDefine<'input, 'allocator>) {
        let interface = self.lower_type_info_raw(node.interface);
        let Some(owner) = named_type_id(&interface) else {
            return;
        };
        let expected: Vec<_> = self
            .interface_methods
            .iter()
            .filter(|method| method.owner == owner)
            .cloned()
            .collect();
        for method in &expected {
            if !node
                .methods
                .iter()
                .any(|candidate| candidate.name.value == method.name)
            {
                self.error(
                    TypeCheckErrorKind::MissingInterfaceMethod {
                        interface: owner,
                        method: method.name.clone(),
                    },
                    node.span.clone(),
                );
            }
        }
        for method in node.methods {
            let Some(expected_method) = expected
                .iter()
                .find(|candidate| candidate.name == method.name.value)
            else {
                self.error(
                    TypeCheckErrorKind::UnknownInterfaceMethod {
                        interface: owner,
                        method: method.name.value.to_owned(),
                    },
                    method.name.span.clone(),
                );
                continue;
            };
            let target = self.lower_type_info_raw(node.target);
            let actual_interface = self.lower_type_info_raw(node.interface);
            let actual = self.function_scheme(method, Some(&target));
            let mut trial = self.unifier.clone();
            let expected_map =
                instantiate_parameter_map(&mut trial, &expected_method.scheme.parameters);
            let actual_map = instantiate_parameter_map(&mut trial, &actual.parameters);
            let expected_type = substitute(&expected_method.scheme.ty, &expected_map);
            let actual_type = substitute(&actual.ty, &actual_map);
            let expected_interface = substitute(&expected_method.interface, &expected_map);
            let receiver_mismatch =
                method.arguments.receiver.is_some() != expected_method.receiver.is_some();
            let generic_mismatch = method
                .generic_parameters
                .map_or(0, |parameters| parameters.parameters.len())
                != expected_method.explicit_parameters.len();
            let mut mismatch = receiver_mismatch
                || generic_mismatch
                || trial
                    .unify(&expected_interface, &actual_interface)
                    .and_then(|_| trial.unify(&expected_type, &actual_type))
                    .is_err();
            if let Some(expected_receiver) = &expected_method.receiver {
                mismatch |= trial
                    .unify(&substitute(expected_receiver, &expected_map), &target)
                    .is_err();
            }
            for actual_bound in &actual.bounds {
                let actual_subject = substitute(&actual_bound.subject, &actual_map);
                let actual_interface = substitute(&actual_bound.interface, &actual_map);
                let implied = expected_method.scheme.bounds.iter().any(|expected_bound| {
                    let mut bound_trial = trial.clone();
                    bound_trial
                        .unify(
                            &actual_subject,
                            &substitute(&expected_bound.subject, &expected_map),
                        )
                        .and_then(|_| {
                            bound_trial.unify(
                                &actual_interface,
                                &substitute(&expected_bound.interface, &expected_map),
                            )
                        })
                        .is_ok()
                });
                mismatch |= !implied;
            }
            if mismatch {
                self.error(
                    TypeCheckErrorKind::InterfaceMethodTypeMismatch {
                        interface: owner,
                        method: method.name.value.to_owned(),
                    },
                    method.span.clone(),
                );
            }
        }
    }

    fn predeclare_statements(&mut self, statements: &'ast Statements<'input, 'allocator>) {
        for statement in statements.statements {
            match statement {
                Statement::LetStatement(node) => self.predeclare_let(node),
                Statement::FunctionDefine(node) => self.predeclare_function(node),
                _ => {}
            }
        }
    }

    fn predeclare_let(&mut self, node: &'ast LetStatement<'input, 'allocator>) {
        let Some(symbol) = self.symbol_declaration(&node.name) else {
            return;
        };
        let ty = node
            .type_info
            .map(|ty| self.lower_type_info(ty))
            .unwrap_or_else(|| self.unifier.fresh());
        self.symbol_types
            .insert(symbol, TypeScheme::monomorphic(ty));
    }

    fn predeclare_function(&mut self, node: &'ast FunctionDefine<'input, 'allocator>) {
        let Some(symbol) = self.symbol_declaration(&node.name) else {
            return;
        };
        let scheme = self.function_scheme(node, None);
        self.symbol_types.insert(symbol, scheme);
    }

    fn function_scheme(
        &mut self,
        node: &'ast FunctionDefine<'input, 'allocator>,
        this_replacement: Option<&Type>,
    ) -> TypeScheme {
        let parameters = self.generic_parameter_ids(node.generic_parameters);
        let mut bounds = self.collect_bounds(node.generic_parameters, node.where_clause);
        let old_assumptions = self.assumptions.clone();
        self.assumptions.extend(bounds.clone());
        let mut argument_types: Vec<_> = node
            .arguments
            .arguments
            .iter()
            .map(|argument| self.lower_type_info(argument.type_info))
            .collect();
        let mut return_type = node
            .return_type
            .map(|ty| self.lower_type_info(ty))
            .unwrap_or_else(|| self.unifier.fresh());
        self.assumptions = old_assumptions;
        if let Some(this_replacement) = this_replacement {
            let mut referenced_parameters: Vec<_> = argument_types
                .iter()
                .chain(std::iter::once(&return_type))
                .flat_map(type_parameters)
                .collect();
            for bound in &bounds {
                referenced_parameters.extend(type_parameters(&bound.subject));
                referenced_parameters.extend(type_parameters(&bound.interface));
            }
            let this_ids: HashSet<_> = referenced_parameters
                .into_iter()
                .filter(|id| self.resolution.name(self.generic_name(*id)) == "This")
                .collect();
            let replacements: HashMap<_, _> = this_ids
                .into_iter()
                .map(|id| (id, this_replacement.clone()))
                .collect();
            argument_types = argument_types
                .iter()
                .map(|ty| substitute(ty, &replacements))
                .collect();
            return_type = substitute(&return_type, &replacements);
            bounds = bounds
                .into_iter()
                .map(|bound| InterfaceBound {
                    subject: substitute(&bound.subject, &replacements),
                    interface: substitute(&bound.interface, &replacements),
                })
                .collect();
        }
        TypeScheme {
            parameters,
            ty: Type::function(argument_types, return_type),
            bounds,
        }
    }

    fn check_statements(&mut self, statements: &'ast Statements<'input, 'allocator>) -> Type {
        let mut result = Type::Unit;
        for statement in statements.statements {
            result = self.check_statement(statement);
        }
        result
    }

    fn check_statement(&mut self, statement: &'ast Statement<'input, 'allocator>) -> Type {
        match statement {
            Statement::ImportStatement(_) | Statement::UseDeclare(_) => Type::Unit,
            Statement::EnumDefine(node) => {
                self.check_enum(node);
                Type::Unit
            }
            Statement::TypeDefine(_) => Type::Unit,
            Statement::InterfaceDefine(node) => {
                self.check_interface_bodies(node);
                Type::Unit
            }
            Statement::ImplementsDefine(node) => {
                self.check_implements_bodies(node);
                Type::Unit
            }
            Statement::LetStatement(node) => {
                self.check_let(node);
                Type::Unit
            }
            Statement::AssertStatement(node) => {
                let condition = self.infer_expression(node.condition);
                self.require_boolean(&condition, node.condition.span());
                if let Some(message) = node.message {
                    let message_type = self.infer_expression(message);
                    self.unify_at(&Type::String, &message_type, message.span());
                }
                Type::Unit
            }
            Statement::AssignStatement(node) => {
                if !is_assignment_target(node.target) {
                    self.error(
                        TypeCheckErrorKind::InvalidAssignmentTarget,
                        node.target.span(),
                    );
                }
                let target = self.infer_expression(node.target);
                self.check_assign_value(&target, &node.value);
                Type::Unit
            }
            Statement::FunctionDefine(node) => {
                self.check_function(node, None);
                Type::Unit
            }
            Statement::Expression(expression) => self.infer_expression(expression),
        }
    }

    fn check_assign_value(
        &mut self,
        expected: &Type,
        value: &'ast AssignValue<'input, 'allocator>,
    ) {
        match value {
            AssignValue::Expression(expression) => {
                self.promote_expression_literals(expected, expression);
                let found = self.infer_expression(expression);
                self.assign_at(expected, &found, expression.span());
            }
            AssignValue::Nested(nested) => self.check_nested_assignment(expected, nested),
        }
    }

    /// Interprets string literals against the expected type of an assignment
    /// or annotated let binding, so that `"firefox"` in a `Package` position
    /// denotes a package reference rather than a string.  Only literal-shaped
    /// syntax is descended: promotion never applies to variables, accesses,
    /// calls, or the results of control flow.
    fn promote_expression_literals(
        &mut self,
        expected: &Type,
        expression: &'ast Expression<'input, 'allocator>,
    ) {
        let Expression::Primary(primary) = expression else {
            return;
        };
        if !primary.accesses.is_empty() {
            return;
        }
        self.promote_value_literals(expected, &primary.value);
    }

    fn promote_value_literals(&mut self, expected: &Type, value: &'ast Value<'input, 'allocator>) {
        let expected = self.unifier.resolve(expected);
        match (&expected, value) {
            (Type::Package, Value::String(_)) => {
                self.package_literals
                    .insert(AstId::new(value, AstKind::Value));
            }
            (Type::Set(element) | Type::List(element), Value::Array(array)) => {
                for element_value in array.values {
                    self.promote_value_literals(element, element_value);
                }
            }
            (Type::Optional(inner), Value::Some(some)) => {
                let inner = inner.as_ref().clone();
                if let Some(inner_expression) = some.value {
                    self.promote_expression_literals(&inner, inner_expression);
                }
            }
            (Type::Optional(inner), _) => {
                let inner = inner.as_ref().clone();
                self.promote_value_literals(&inner, value);
            }
            _ => {}
        }
    }

    fn check_nested_assignment(
        &mut self,
        expected: &Type,
        nested: &'ast NestedAssignment<'input, 'allocator>,
    ) {
        let receiver = self.unifier.resolve(expected);
        for field in nested.fields {
            let Some((field_id, field_type)) =
                self.resolve_named_field(&receiver, field.name.value)
            else {
                self.error(
                    TypeCheckErrorKind::UnknownMember {
                        receiver: receiver.clone(),
                        member: field.name.value.to_owned(),
                    },
                    field.name.span.clone(),
                );
                continue;
            };
            self.member_resolutions.insert(
                AstId::new(&field.name, AstKind::Literal),
                MemberResolution::Field(field_id),
            );
            self.check_assign_value(&field_type, &field.value);
        }
    }

    fn check_enum(&mut self, node: &'ast light_nix_parser::ast::EnumDefine<'input, 'allocator>) {
        let Some(representation) = node.representation_type else {
            return;
        };
        let expected = self.lower_type_info(representation);
        for variant in node.variants {
            if let Some(value) = variant.value {
                let found = self.infer_expression(value);
                self.assign_at(&expected, &found, value.span());
            }
        }
    }

    fn check_let(&mut self, node: &'ast LetStatement<'input, 'allocator>) {
        let Some(symbol) = self.symbol_declaration(&node.name) else {
            return;
        };
        let expected = self
            .symbol_types
            .get(&symbol)
            .map(|scheme| scheme.ty.clone())
            .unwrap_or(Type::Error);
        if let Some(value) = node.value {
            if node.type_info.is_some() {
                self.promote_expression_literals(&expected, value);
            }
            let found = self.infer_expression(value);
            self.assign_at(&expected, &found, value.span());
        } else if node.type_info.is_none() {
            self.error(TypeCheckErrorKind::MissingTypeAndValue, node.span.clone());
        }
    }

    fn check_interface_bodies(&mut self, node: &'ast InterfaceDefine<'input, 'allocator>) {
        let this = self
            .resolution
            .generic_parameters()
            .iter()
            .find(|parameter| parameter.declaration == AstId::new(node, AstKind::InterfaceDefine))
            .map_or(Type::Error, |parameter| Type::Parameter(parameter.id));
        let old_assumptions = self.assumptions.clone();
        if let Some(owner) = self.type_declaration(&node.name) {
            let arguments = self
                .generic_parameter_ids(node.generic_parameters)
                .into_iter()
                .map(Type::Parameter)
                .collect();
            self.assumptions.push(InterfaceBound {
                subject: this.clone(),
                interface: Type::Named(owner, arguments),
            });
        }
        let bounds = self.collect_bounds(node.generic_parameters, node.where_clause);
        self.assumptions.extend(bounds);
        for method in node.methods {
            self.check_function(method, Some(this.clone()));
        }
        self.assumptions = old_assumptions;
    }

    fn check_implements_bodies(&mut self, node: &'ast ImplementsDefine<'input, 'allocator>) {
        let old_assumptions = self.assumptions.clone();
        let bounds = self.collect_bounds(node.generic_parameters, node.where_clause);
        self.assumptions.extend(bounds);
        let target = self.lower_type_info(node.target);
        let interface = self.lower_type_info(node.interface);
        self.assumptions.push(InterfaceBound {
            subject: target.clone(),
            interface,
        });
        for method in node.methods {
            self.check_function(method, Some(target.clone()));
        }
        self.assumptions = old_assumptions;
    }

    fn check_function(
        &mut self,
        node: &'ast FunctionDefine<'input, 'allocator>,
        receiver_type: Option<Type>,
    ) {
        let symbol = self.symbol_declaration(&node.name);
        let scheme = symbol
            .and_then(|symbol| self.symbol_types.get(&symbol).cloned())
            .unwrap_or_else(|| self.function_scheme(node, receiver_type.as_ref()));
        let Type::Function(function_type) = scheme.ty.clone() else {
            return;
        };
        let old_return = self
            .current_return
            .replace((*function_type.return_type).clone());
        let old_assumptions = self.assumptions.clone();
        self.assumptions.extend(scheme.bounds.clone());

        if let (Some(receiver), Some(receiver_type)) =
            (&node.arguments.receiver, receiver_type.as_ref())
            && let Some(symbol) = self.symbol_declaration(receiver)
        {
            self.symbol_types
                .insert(symbol, TypeScheme::monomorphic(receiver_type.clone()));
        }
        for (argument, ty) in node
            .arguments
            .arguments
            .iter()
            .zip(&function_type.parameters)
        {
            if let Some(symbol) = self.symbol_declaration(&argument.name) {
                self.symbol_types
                    .insert(symbol, TypeScheme::monomorphic(ty.clone()));
            }
        }

        self.predeclare_statements(&node.body.statements);
        let body_type = self.check_statements(&node.body.statements);
        self.assign_at(&function_type.return_type, &body_type, node.body.span());

        self.assumptions = old_assumptions;
        self.current_return = old_return;
    }

    fn infer_expression(&mut self, expression: &'ast Expression<'input, 'allocator>) -> Type {
        let ty = match expression {
            Expression::If(node) => self.infer_if(node),
            Expression::Match(node) => self.infer_match(node),
            Expression::Return(node) => {
                let found = node
                    .value
                    .map(|value| self.infer_expression(value))
                    .unwrap_or(Type::Unit);
                if let Some(expected) = self.current_return.clone() {
                    self.assign_at(&expected, &found, node.span.clone());
                } else {
                    self.error(TypeCheckErrorKind::ReturnOutsideFunction, node.span.clone());
                }
                Type::Never
            }
            Expression::Throw(node) => {
                if let Some(message) = node.message {
                    let message_type = self.infer_expression(message);
                    self.unify_at(&Type::String, &message_type, message.span());
                }
                Type::Never
            }
            Expression::Closure(node) => self.infer_closure(node),
            Expression::Elvis(node) => {
                let optional = self.infer_expression(node.optional);
                let fallback = self.infer_expression(node.fallback);
                let inner = self.unifier.fresh();
                self.unify_at(
                    &Type::optional(inner.clone()),
                    &optional,
                    node.optional.span(),
                );
                self.unify_at(&inner, &fallback, node.fallback.span());
                inner
            }
            Expression::TypeOperation(node) => {
                let inferred_value = self.infer_expression(node.value);
                let value = self.unifier.resolve(&inferred_value);
                let lowered_target = self.lower_type_info(node.target);
                let target = self.unifier.resolve(&lowered_target);
                let invalid_target =
                    matches!(target, Type::Union(_) | Type::Optional(_) | Type::Never);
                if invalid_target {
                    self.error(
                        TypeCheckErrorKind::InvalidUnionCastTarget {
                            found: target.clone(),
                        },
                        node.target.span(),
                    );
                } else if !value.contains_union_alternative(&target) {
                    self.error(
                        if matches!(value, Type::Union(_)) {
                            TypeCheckErrorKind::InvalidUnionAlternative {
                                union: value,
                                target: target.clone(),
                            }
                        } else {
                            TypeCheckErrorKind::ExpectedUnion { found: value }
                        },
                        node.value.span(),
                    );
                }
                match node.operator.value {
                    TypeOperator::Is => Type::Bool,
                    TypeOperator::SafeCast => Type::optional(target),
                }
            }
            Expression::Binary(node) => {
                let left = self.infer_expression(node.left);
                let right = self.infer_expression(node.right);
                match node.operator.value {
                    BinaryOperator::Or | BinaryOperator::And => {
                        self.require_boolean(&left, node.left.span());
                        self.require_boolean(&right, node.right.span());
                        Type::Bool
                    }
                    BinaryOperator::Equal | BinaryOperator::NotEqual => {
                        self.unify_at(&left, &right, node.right.span());
                        Type::Bool
                    }
                    BinaryOperator::LessThan
                    | BinaryOperator::GreaterThan
                    | BinaryOperator::LessThanOrEqual
                    | BinaryOperator::GreaterThanOrEqual => {
                        self.unify_at(&left, &right, node.right.span());
                        self.require_numeric(&left, node.span.clone());
                        Type::Bool
                    }
                    BinaryOperator::Add => {
                        self.unify_at(&left, &right, node.right.span());
                        let resolved = self.unifier.resolve(&left);
                        if !matches!(resolved, Type::String) {
                            self.require_numeric(&resolved, node.span.clone());
                        }
                        left
                    }
                    BinaryOperator::Subtract
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide => {
                        self.unify_at(&left, &right, node.right.span());
                        self.require_numeric(&left, node.span.clone());
                        left
                    }
                }
            }
            Expression::Unary(node) => {
                let operand = self.infer_expression(node.operand);
                match node.operator.value {
                    UnaryOperator::Positive | UnaryOperator::Negate => {
                        self.require_numeric(&operand, node.span.clone());
                    }
                }
                operand
            }
            Expression::Primary(node) => self.infer_primary(node),
        };
        self.expression_types
            .insert(AstId::new(expression, AstKind::Expression), ty.clone());
        ty
    }

    fn infer_closure(&mut self, node: &'ast ClosureExpression<'input, 'allocator>) -> Type {
        let parameters = node
            .parameters
            .iter()
            .map(|parameter| {
                let ty = parameter
                    .type_info
                    .map(|type_info| self.lower_type_info(type_info))
                    .unwrap_or_else(|| self.unifier.fresh());
                if let Some(symbol) = self.symbol_declaration(&parameter.name) {
                    self.symbol_types
                        .insert(symbol, TypeScheme::monomorphic(ty.clone()));
                }
                ty
            })
            .collect::<Vec<_>>();
        let return_type = node
            .return_type
            .map(|type_info| self.lower_type_info(type_info))
            .unwrap_or_else(|| self.unifier.fresh());
        let old_return = self.current_return.replace(return_type.clone());
        let body_type = match node.body {
            ClosureBody::Expression(expression) => self.infer_expression(expression),
            ClosureBody::Block(block) => {
                self.predeclare_statements(&block.statements);
                self.check_statements(&block.statements)
            }
        };
        self.assign_at(&return_type, &body_type, node.body.span());
        self.current_return = old_return;
        Type::function(parameters, return_type)
    }

    fn infer_if(
        &mut self,
        node: &'ast light_nix_parser::ast::IfExpression<'input, 'allocator>,
    ) -> Type {
        let condition = self.infer_expression(node.branch.condition);
        self.require_boolean(&condition, node.branch.condition.span());
        let refinement = self.type_refinement(node.branch.condition);
        self.predeclare_statements(&node.branch.body.statements);
        let mut result = self.check_statements_with_refinement(
            &node.branch.body.statements,
            refinement
                .as_ref()
                .map(|(symbol, positive, _)| (*symbol, positive.clone())),
        );
        let mut residual = refinement.map(|(symbol, _, negative)| (symbol, negative));
        let mut has_else = false;
        for branch in node.else_branches {
            let branch_type = match branch.value {
                ElseBranchValue::If(if_branch) => {
                    let saved = self.apply_refinement(residual.clone());
                    let condition = self.infer_expression(if_branch.condition);
                    self.require_boolean(&condition, if_branch.condition.span());
                    let branch_refinement = self.type_refinement(if_branch.condition);
                    self.predeclare_statements(&if_branch.body.statements);
                    let branch_type = self.check_statements_with_refinement(
                        &if_branch.body.statements,
                        branch_refinement
                            .as_ref()
                            .map(|(symbol, positive, _)| (*symbol, positive.clone())),
                    );
                    if let Some((symbol, _, negative)) = branch_refinement {
                        residual = Some((symbol, negative));
                    }
                    self.restore_refinement(saved);
                    branch_type
                }
                ElseBranchValue::Block(block) => {
                    has_else = true;
                    self.predeclare_statements(&block.statements);
                    self.check_statements_with_refinement(&block.statements, residual.clone())
                }
            };
            result = self.join_at(&result, &branch_type);
        }
        if has_else {
            result
        } else {
            self.unify_at(&Type::Unit, &result, node.span.clone());
            Type::Unit
        }
    }

    fn infer_match(
        &mut self,
        node: &'ast light_nix_parser::ast::MatchExpression<'input, 'allocator>,
    ) -> Type {
        let matched = self.infer_expression(node.value);
        let mut result = None;
        for arm in node.arms {
            self.check_pattern(&arm.pattern, &matched);
            let arm_type = self.infer_expression(arm.value);
            result = Some(result.map_or(arm_type.clone(), |current| {
                self.join_at(&current, &arm_type)
            }));
        }
        result.unwrap_or(Type::Never)
    }

    fn type_refinement(
        &mut self,
        condition: &'ast Expression<'input, 'allocator>,
    ) -> Option<(SymbolId, Type, Type)> {
        let Expression::TypeOperation(operation) = condition else {
            return None;
        };
        if operation.operator.value != TypeOperator::Is {
            return None;
        }
        let Expression::Primary(primary) = operation.value else {
            return None;
        };
        if !primary.accesses.is_empty() {
            return None;
        }
        let Value::Literal(literal) = &primary.value else {
            return None;
        };
        if literal.call.is_some() || literal.type_arguments.is_some() {
            return None;
        }
        let Some(Res::Symbol(symbol)) = self.resolution.resolve_literal(&literal.literal) else {
            return None;
        };
        let original = self
            .symbol_types
            .get(&symbol)
            .map(|scheme| self.unifier.resolve(&scheme.ty))?;
        let target = self
            .type_info_types
            .get(&AstId::new(operation.target, AstKind::TypeInfo))
            .map(|ty| self.unifier.resolve(ty))?;
        Some((
            symbol,
            target.clone(),
            original.without_union_alternative(&target),
        ))
    }

    fn check_statements_with_refinement(
        &mut self,
        statements: &'ast Statements<'input, 'allocator>,
        refinement: Option<(SymbolId, Type)>,
    ) -> Type {
        let saved = self.apply_refinement(refinement);
        let result = self.check_statements(statements);
        self.restore_refinement(saved);
        result
    }

    fn apply_refinement(
        &mut self,
        refinement: Option<(SymbolId, Type)>,
    ) -> Option<(SymbolId, TypeScheme)> {
        let (symbol, ty) = refinement?;
        let previous = self
            .symbol_types
            .insert(symbol, TypeScheme::monomorphic(ty))?;
        Some((symbol, previous))
    }

    fn restore_refinement(&mut self, saved: Option<(SymbolId, TypeScheme)>) {
        if let Some((symbol, scheme)) = saved {
            self.symbol_types.insert(symbol, scheme);
        }
    }

    fn check_pattern(&mut self, pattern: &'ast Pattern<'input, 'allocator>, expected: &Type) {
        match pattern {
            Pattern::Some(pattern) => {
                let inner = self.unifier.fresh();
                self.unify_at(
                    &Type::optional(inner.clone()),
                    expected,
                    pattern.span.clone(),
                );
                self.check_pattern(pattern.pattern, &inner);
            }
            Pattern::Null(node) => {
                let inner = self.unifier.fresh();
                self.unify_at(&Type::optional(inner), expected, node.span());
            }
            Pattern::Wildcard(_) => {}
            Pattern::Binding(binding) => {
                if let Some(symbol) = self.symbol_declaration(binding) {
                    self.symbol_types
                        .insert(symbol, TypeScheme::monomorphic(expected.clone()));
                }
            }
            Pattern::EnumVariant(pattern) => {
                if let Some(Res::EnumVariant(variant)) =
                    self.resolution.resolve_literal(&pattern.variant)
                {
                    let owner = self
                        .resolution
                        .variants()
                        .iter()
                        .find(|candidate| candidate.id == variant)
                        .map(|variant| variant.owner);
                    if let Some(owner) = owner {
                        self.unify_at(
                            &Type::Named(owner, Vec::new()),
                            expected,
                            pattern.span.clone(),
                        );
                    }
                }
            }
        }
    }

    fn infer_primary(&mut self, primary: &'ast Primary<'input, 'allocator>) -> Type {
        let mut state = self.infer_primary_root(&primary.value);
        if let PrimaryState::Value(ty) = &state {
            self.value_types
                .insert(AstId::new(&primary.value, AstKind::Value), ty.clone());
        }
        for access in primary.accesses {
            state = self.infer_access(state, access);
            if let PrimaryState::Value(ty) = &state {
                self.member_types
                    .insert(AstId::new(access, AstKind::PrimaryAccess), ty.clone());
            }
        }
        match state {
            PrimaryState::Value(ty) => ty,
            PrimaryState::Type(_) | PrimaryState::Module | PrimaryState::Error => Type::Error,
        }
    }

    fn infer_primary_root(&mut self, value: &'ast Value<'input, 'allocator>) -> PrimaryState {
        match value {
            Value::Array(array) => PrimaryState::Value(self.infer_array(array)),
            Value::Literal(literal) => {
                let resolution = self.resolution.resolve_literal(&literal.literal);
                match resolution {
                    Some(Res::Symbol(symbol)) => {
                        let ty = self.instantiate_symbol(
                            symbol,
                            literal.type_arguments,
                            literal.literal.span.clone(),
                        );
                        PrimaryState::Value(match literal.call {
                            Some(call) => self.infer_call(ty, call),
                            None => ty,
                        })
                    }
                    Some(Res::Type(ty)) => PrimaryState::Type(ty),
                    Some(Res::Module(_)) => PrimaryState::Module,
                    Some(Res::EnumVariant(variant)) => {
                        let owner = self
                            .resolution
                            .variants()
                            .iter()
                            .find(|candidate| candidate.id == variant)
                            .map(|variant| variant.owner);
                        PrimaryState::Value(
                            owner.map_or(Type::Error, |owner| Type::Named(owner, Vec::new())),
                        )
                    }
                    _ => PrimaryState::Error,
                }
            }
            Value::Some(some) => {
                let inner = some
                    .value
                    .map(|value| self.infer_expression(value))
                    .unwrap_or_else(|| self.unifier.fresh());
                PrimaryState::Value(Type::optional(inner))
            }
            Value::Numeric(number) => {
                let float = number.value.contains(['.', 'e', 'E']);
                PrimaryState::Value(if float { Type::Float } else { Type::Int })
            }
            Value::String(_) => PrimaryState::Value(
                if self
                    .package_literals
                    .contains(&AstId::new(value, AstKind::Value))
                {
                    Type::Package
                } else {
                    Type::String
                },
            ),
            Value::Boolean(_) => PrimaryState::Value(Type::Bool),
            Value::Null(_) => {
                let inner = self.unifier.fresh();
                PrimaryState::Value(Type::optional(inner))
            }
        }
    }

    fn infer_access(
        &mut self,
        state: PrimaryState,
        access: &'ast light_nix_parser::ast::PrimaryAccess<'input, 'allocator>,
    ) -> PrimaryState {
        if access.key().is_some() {
            return match state {
                PrimaryState::Value(receiver) => {
                    let receiver = self.unifier.resolve(&receiver);
                    if let Type::Optional(_) = receiver {
                        self.error(
                            TypeCheckErrorKind::OptionalAccessRequiresSafeOperator {
                                found: receiver,
                            },
                            access.operator.span.clone(),
                        );
                        return PrimaryState::Error;
                    }
                    match receiver {
                        Type::AttrSet(element) => {
                            self.member_resolutions.insert(
                                AstId::new(access, AstKind::PrimaryAccess),
                                MemberResolution::AttrSetKey,
                            );
                            PrimaryState::Value(*element)
                        }
                        found => {
                            self.error(
                                TypeCheckErrorKind::InvalidAttrSetAccess { found },
                                access.member_span(),
                            );
                            PrimaryState::Error
                        }
                    }
                }
                PrimaryState::Type(_) | PrimaryState::Module => {
                    self.error(
                        TypeCheckErrorKind::InvalidAttrSetAccess { found: Type::Error },
                        access.member_span(),
                    );
                    PrimaryState::Error
                }
                PrimaryState::Error => PrimaryState::Error,
            };
        }
        let member = access
            .named_member()
            .expect("non-index primary access must have a named member");
        match state {
            PrimaryState::Type(owner) => {
                if access.operator.value != AccessOperator::DoubleColon {
                    return PrimaryState::Error;
                }
                match self.resolution.resolve_literal(member) {
                    Some(Res::EnumVariant(variant)) => {
                        let valid =
                            self.resolution.variants().iter().any(|candidate| {
                                candidate.id == variant && candidate.owner == owner
                            });
                        if valid {
                            PrimaryState::Value(Type::Named(owner, Vec::new()))
                        } else {
                            self.error(
                                TypeCheckErrorKind::InvalidStaticMember {
                                    owner,
                                    member: member.value.to_owned(),
                                },
                                member.span.clone(),
                            );
                            PrimaryState::Error
                        }
                    }
                    Some(Res::Symbol(symbol)) => {
                        let ty = self.instantiate_symbol(
                            symbol,
                            access.type_arguments,
                            member.span.clone(),
                        );
                        PrimaryState::Value(match access.call {
                            Some(call) => self.infer_call(ty, call),
                            None => ty,
                        })
                    }
                    _ => {
                        self.error(
                            TypeCheckErrorKind::InvalidStaticMember {
                                owner,
                                member: member.value.to_owned(),
                            },
                            member.span.clone(),
                        );
                        PrimaryState::Error
                    }
                }
            }
            PrimaryState::Module => match self.resolution.resolve_literal(member) {
                Some(Res::Symbol(symbol)) => {
                    let ty =
                        self.instantiate_symbol(symbol, access.type_arguments, member.span.clone());
                    PrimaryState::Value(match access.call {
                        Some(call) => self.infer_call(ty, call),
                        None => ty,
                    })
                }
                Some(Res::Type(ty)) => PrimaryState::Type(ty),
                _ => PrimaryState::Error,
            },
            PrimaryState::Value(receiver) => {
                let safe = access.operator.value == AccessOperator::SafeDot;
                let receiver = self.unifier.resolve(&receiver);
                let base = if safe {
                    match receiver {
                        Type::Optional(inner) => *inner,
                        other => other,
                    }
                } else if let Type::Optional(_) = receiver {
                    self.error(
                        TypeCheckErrorKind::OptionalAccessRequiresSafeOperator {
                            found: receiver.clone(),
                        },
                        access.operator.span.clone(),
                    );
                    Type::Error
                } else {
                    receiver
                };
                let member = self.infer_member(&base, access);
                PrimaryState::Value(if safe { Type::optional(member) } else { member })
            }
            PrimaryState::Error => PrimaryState::Error,
        }
    }

    fn infer_member(
        &mut self,
        receiver: &Type,
        access: &'ast light_nix_parser::ast::PrimaryAccess<'input, 'allocator>,
    ) -> Type {
        let member = access
            .named_member()
            .expect("member inference requires a named access");
        if access.call.is_none()
            && let Some((field_id, field_type)) = self.resolve_named_field(receiver, member.value)
        {
            self.member_resolutions.insert(
                AstId::new(access, AstKind::PrimaryAccess),
                MemberResolution::Field(field_id),
            );
            return field_type;
        }
        if let Some(ty) = self.infer_builtin_member(receiver, access) {
            return ty;
        }
        let mut methods: Vec<_> = self
            .interface_methods
            .iter()
            .filter(|method| method.name == member.value && method.receiver.is_some())
            .cloned()
            .collect();
        if methods.len() > 1 {
            methods.retain(|method| self.may_satisfy_interface(receiver, method.owner));
        }
        if methods.len() == 1 {
            let method = &methods[0];
            let (method_type, bounds, map) = self.instantiate_scheme_in_order(
                &method.scheme,
                access.type_arguments,
                member.span.clone(),
                &method.explicit_parameters,
            );
            let instantiated_receiver = substitute(
                method.receiver.as_ref().expect("instance method receiver"),
                &map,
            );
            self.unify_at(receiver, &instantiated_receiver, member.span.clone());
            for mut bound in bounds {
                if bound.subject == instantiated_receiver {
                    bound.subject = receiver.clone();
                }
                self.obligations.push_back(Obligation {
                    bound,
                    assumptions: self.assumptions.clone(),
                    dispatch: None,
                    span: member.span.clone(),
                });
            }
            let interface = substitute(&method.interface, &map);
            let member_id = AstId::new(access, AstKind::PrimaryAccess);
            self.member_resolutions.insert(
                member_id,
                MemberResolution::InterfaceMethod {
                    interface: method.owner,
                    declaration: method.symbol,
                    implementation: None,
                },
            );
            self.obligations.push_back(Obligation {
                bound: InterfaceBound {
                    subject: receiver.clone(),
                    interface,
                },
                assumptions: self.assumptions.clone(),
                dispatch: Some(DispatchOrigin {
                    member: member_id,
                    interface: method.owner,
                    declaration: method.symbol,
                    method: method.name.clone(),
                }),
                span: member.span.clone(),
            });
            return match access.call {
                Some(call) => self.infer_call(method_type, call),
                None => method_type,
            };
        }
        if methods.len() > 1 {
            self.error(
                TypeCheckErrorKind::AmbiguousMember {
                    receiver: receiver.clone(),
                    member: member.value.to_owned(),
                },
                member.span.clone(),
            );
            return Type::Error;
        }
        self.error(
            TypeCheckErrorKind::UnknownMember {
                receiver: receiver.clone(),
                member: member.value.to_owned(),
            },
            member.span.clone(),
        );
        Type::Error
    }

    fn resolve_named_field(&self, receiver: &Type, name: &str) -> Option<(FieldId, Type)> {
        let Type::Named(owner, arguments) = receiver else {
            return None;
        };
        let field_id = *self.field_lookup.get(&(*owner, name.to_owned()))?;
        let field_type = self
            .field_types
            .get(&field_id)
            .cloned()
            .unwrap_or(Type::Error);
        let substitutions = self
            .type_parameters
            .get(owner)
            .into_iter()
            .flatten()
            .copied()
            .zip(arguments.iter().cloned())
            .collect();
        Some((field_id, substitute(&field_type, &substitutions)))
    }

    fn infer_builtin_member(
        &mut self,
        receiver: &Type,
        access: &'ast PrimaryAccess<'input, 'allocator>,
    ) -> Option<Type> {
        let member = access.named_member()?;
        let method = find_builtin_method(receiver, member.value)?;
        self.member_resolutions.insert(
            AstId::new(access, AstKind::PrimaryAccess),
            MemberResolution::Builtin(method),
        );
        let function = match method {
            BuiltinMethod::Contains => {
                let element = collection_element(receiver)?;
                Type::function(vec![element], Type::Bool)
            }
            BuiltinMethod::Filter => {
                let element = collection_element(receiver)?;
                Type::function(
                    vec![Type::function(vec![element], Type::Bool)],
                    receiver.clone(),
                )
            }
            BuiltinMethod::Map => {
                let element = collection_element(receiver)?;
                let result = self.unifier.fresh();
                let result_collection = match receiver {
                    Type::Set(_) => Type::Set(Box::new(result.clone())),
                    Type::List(_) => Type::List(Box::new(result.clone())),
                    _ => return None,
                };
                Type::function(
                    vec![Type::function(vec![element], result)],
                    result_collection,
                )
            }
            BuiltinMethod::ToFloat => Type::function(Vec::new(), Type::Float),
            BuiltinMethod::TryToInt => Type::function(Vec::new(), Type::optional(Type::Int)),
            BuiltinMethod::ToString => Type::function(Vec::new(), Type::String),
        };
        Some(match access.call {
            Some(call) => self.infer_call(function, call),
            None => function,
        })
    }

    fn infer_array(&mut self, array: &'ast Array<'input, 'allocator>) -> Type {
        let mut element = None;
        for value in array.values {
            let found = match self.infer_primary_root(value) {
                PrimaryState::Value(ty) => ty,
                _ => Type::Error,
            };
            self.value_types
                .insert(AstId::new(value, AstKind::Value), found.clone());
            element = Some(element.map_or(found.clone(), |current| self.join_at(&current, &found)));
        }
        let element = Box::new(element.unwrap_or_else(|| self.unifier.fresh()));
        match array.kind {
            CollectionKind::List => Type::List(element),
            CollectionKind::Set => Type::Set(element),
        }
    }

    fn infer_call(&mut self, callee: Type, call: &'ast FunctionCall<'input, 'allocator>) -> Type {
        let callee = self.unifier.resolve(&callee);
        let Type::Function(function) = callee else {
            self.error(
                TypeCheckErrorKind::NotCallable { ty: callee },
                call.span.clone(),
            );
            for argument in call.arguments {
                self.infer_expression(argument);
            }
            return Type::Error;
        };
        if function.parameters.len() != call.arguments.len() {
            self.error(
                TypeCheckErrorKind::ArgumentCount {
                    expected: function.parameters.len(),
                    found: call.arguments.len(),
                },
                call.span.clone(),
            );
        }
        for (argument, expected) in call.arguments.iter().zip(&function.parameters) {
            let found = self.infer_expression(argument);
            self.assign_at(expected, &found, argument.span());
        }
        (*function.return_type).clone()
    }

    fn instantiate_symbol(
        &mut self,
        symbol: SymbolId,
        explicit: Option<&'ast ExplicitTypeArguments<'input, 'allocator>>,
        span: Range<usize>,
    ) -> Type {
        let scheme = self
            .symbol_types
            .get(&symbol)
            .cloned()
            .or_else(|| self.environment.symbols.get(&symbol).cloned());
        let Some(scheme) = scheme else {
            return Type::Error;
        };
        let (ty, bounds, _) = self.instantiate_scheme(&scheme, explicit, span.clone());
        for bound in bounds {
            self.obligations.push_back(Obligation {
                bound,
                assumptions: self.assumptions.clone(),
                dispatch: None,
                span: span.clone(),
            });
        }
        ty
    }

    fn instantiate_scheme(
        &mut self,
        scheme: &TypeScheme,
        explicit: Option<&'ast ExplicitTypeArguments<'input, 'allocator>>,
        span: Range<usize>,
    ) -> (Type, Vec<InterfaceBound>, HashMap<GenericParameterId, Type>) {
        let explicit_order = explicit_parameter_order(scheme);
        self.instantiate_scheme_in_order(scheme, explicit, span, &explicit_order)
    }

    fn instantiate_scheme_in_order(
        &mut self,
        scheme: &TypeScheme,
        explicit: Option<&'ast ExplicitTypeArguments<'input, 'allocator>>,
        span: Range<usize>,
        explicit_order: &[GenericParameterId],
    ) -> (Type, Vec<InterfaceBound>, HashMap<GenericParameterId, Type>) {
        if explicit.is_some() && scheme.parameters.is_empty() {
            self.error(
                TypeCheckErrorKind::TypeArgumentsOnMonomorphicValue,
                span.clone(),
            );
        }
        let explicit_arguments = explicit.map_or(&[][..], |arguments| arguments.arguments);
        if explicit_arguments.len() > explicit_order.len() {
            self.error(
                TypeCheckErrorKind::TypeArgumentCount {
                    expected: explicit_order.len(),
                    found: explicit_arguments.len(),
                },
                span,
            );
        }
        let mut substitutions: HashMap<_, _> = scheme
            .parameters
            .iter()
            .copied()
            .map(|parameter| (parameter, self.unifier.fresh()))
            .collect();
        for (index, parameter) in explicit_order.iter().copied().enumerate() {
            let ty = match explicit_arguments.get(index) {
                Some(ExplicitTypeArgument::Type(ty)) => self.lower_type_info(ty),
                Some(ExplicitTypeArgument::Infer(_)) => continue,
                None => break,
            };
            substitutions.insert(parameter, ty);
        }
        let ty = substitute(&scheme.ty, &substitutions);
        let bounds = scheme
            .bounds
            .iter()
            .map(|bound| InterfaceBound {
                subject: substitute(&bound.subject, &substitutions),
                interface: substitute(&bound.interface, &substitutions),
            })
            .collect();
        (ty, bounds, substitutions)
    }

    fn may_satisfy_interface(&self, receiver: &Type, interface: TypeDefId) -> bool {
        for assumption in &self.assumptions {
            if named_type_id(&assumption.interface) != Some(interface) {
                continue;
            }
            let mut trial = self.unifier.clone();
            if trial.unify(receiver, &assumption.subject).is_ok() {
                return true;
            }
        }
        for implementation in &self.implementations {
            if named_type_id(&implementation.interface) != Some(interface) {
                continue;
            }
            let mut trial = self.unifier.clone();
            let map = instantiate_parameter_map(&mut trial, &implementation.parameters);
            if trial
                .unify(receiver, &substitute(&implementation.target, &map))
                .is_ok()
            {
                return true;
            }
        }
        false
    }

    fn collect_bounds(
        &mut self,
        parameters: Option<&'ast GenericParameters<'input, 'allocator>>,
        where_clause: Option<&'ast WhereClause<'input, 'allocator>>,
    ) -> Vec<InterfaceBound> {
        let mut bounds = Vec::new();
        if let Some(parameters) = parameters {
            for parameter in parameters.parameters {
                let subject = self
                    .resolution
                    .declaration_of_literal(&parameter.name)
                    .and_then(|declaration| match declaration {
                        Declaration::GenericParameter(id) => Some(Type::Parameter(id)),
                        _ => None,
                    })
                    .unwrap_or(Type::Error);
                for bound in parameter.bounds {
                    if !bound.alternatives.is_empty() {
                        self.error(TypeCheckErrorKind::UnionInterfaceBound, bound.span());
                        continue;
                    }
                    bounds.push(InterfaceBound {
                        subject: subject.clone(),
                        interface: self.lower_type_info_raw(bound),
                    });
                }
            }
        }
        if let Some(where_clause) = where_clause {
            for predicate in where_clause.predicates {
                let subject = self.lower_type_info_raw(predicate.ty);
                for bound in predicate.bounds {
                    if !bound.alternatives.is_empty() {
                        self.error(TypeCheckErrorKind::UnionInterfaceBound, bound.span());
                        continue;
                    }
                    bounds.push(InterfaceBound {
                        subject: subject.clone(),
                        interface: self.lower_type_info_raw(bound),
                    });
                }
            }
        }
        bounds
    }

    fn solve_obligations(&mut self) {
        let mut remaining_steps = self.obligations.len().saturating_mul(16).max(16);
        let mut consecutively_deferred = 0;
        let mut stalled = false;
        while let Some(obligation) = self.obligations.pop_front() {
            if remaining_steps == 0 {
                self.obligations.push_front(obligation);
                break;
            }
            remaining_steps -= 1;
            let subject = self.unifier.resolve(&obligation.bound.subject);
            let interface = self.unifier.resolve(&obligation.bound.interface);
            if subject.is_error() || interface.is_error() {
                self.clear_dispatch(obligation.dispatch.as_ref());
                continue;
            }
            if self.try_assumptions(&subject, &interface, &obligation.assumptions) {
                if obligation.dispatch.is_some()
                    && !contains_unresolved_dispatch_type(&subject)
                    && !contains_unresolved_dispatch_type(&interface)
                {
                    let successes = self.matching_implementations(&subject, &interface);
                    if let [(_, _, implementation)] = successes.as_slice() {
                        self.record_dispatch(obligation.dispatch.as_ref(), Some(*implementation));
                    }
                }
                consecutively_deferred = 0;
                continue;
            }
            let mut successes = self.matching_implementations(&subject, &interface);
            let may_gain_information =
                contains_type_variable(&subject) || contains_type_variable(&interface);
            match successes.len() {
                0 if may_gain_information => {
                    self.obligations.push_back(obligation);
                    consecutively_deferred += 1;
                }
                0 => {
                    self.clear_dispatch(obligation.dispatch.as_ref());
                    self.error(
                        TypeCheckErrorKind::MissingImplementation { subject, interface },
                        obligation.span,
                    );
                }
                1 => {
                    consecutively_deferred = 0;
                    let (unifier, bounds, implementation) =
                        successes.pop().expect("one successful candidate");
                    self.unifier = unifier;
                    self.record_dispatch(obligation.dispatch.as_ref(), Some(implementation));
                    for bound in bounds {
                        self.obligations.push_back(Obligation {
                            bound,
                            assumptions: obligation.assumptions.clone(),
                            dispatch: None,
                            span: obligation.span.clone(),
                        });
                    }
                }
                _ if may_gain_information => {
                    self.obligations.push_back(obligation);
                    consecutively_deferred += 1;
                }
                _ => {
                    self.clear_dispatch(obligation.dispatch.as_ref());
                    self.error(
                        TypeCheckErrorKind::AmbiguousImplementation { subject, interface },
                        obligation.span,
                    );
                }
            }
            if !self.obligations.is_empty() && consecutively_deferred >= self.obligations.len() {
                stalled = true;
                break;
            }
        }
        if stalled {
            while let Some(obligation) = self.obligations.pop_front() {
                let subject = self.unifier.resolve(&obligation.bound.subject);
                let interface = self.unifier.resolve(&obligation.bound.interface);
                let successes = self.matching_implementations(&subject, &interface);
                self.clear_dispatch(obligation.dispatch.as_ref());
                let kind = if successes.is_empty() {
                    TypeCheckErrorKind::MissingImplementation { subject, interface }
                } else {
                    TypeCheckErrorKind::AmbiguousImplementation { subject, interface }
                };
                self.error(kind, obligation.span);
            }
            return;
        }
        if let Some(obligation) = self.obligations.pop_front() {
            let subject = self.unifier.resolve(&obligation.bound.subject);
            let interface = self.unifier.resolve(&obligation.bound.interface);
            self.clear_dispatch(obligation.dispatch.as_ref());
            self.error(
                TypeCheckErrorKind::OverflowEvaluatingBound { subject, interface },
                obligation.span,
            );
            self.obligations.clear();
        }
    }

    fn matching_implementations(
        &self,
        subject: &Type,
        interface: &Type,
    ) -> Vec<(Unifier, Vec<InterfaceBound>, usize)> {
        let interface_id = named_type_id(interface);
        let mut successes = Vec::new();
        for (index, implementation) in self.implementations.iter().enumerate() {
            if named_type_id(&implementation.interface) != interface_id {
                continue;
            }
            let mut trial = self.unifier.clone();
            let substitutions = instantiate_parameter_map(&mut trial, &implementation.parameters);
            let candidate_subject = substitute(&implementation.target, &substitutions);
            let candidate_interface = substitute(&implementation.interface, &substitutions);
            if trial.unify(subject, &candidate_subject).is_err()
                || trial.unify(interface, &candidate_interface).is_err()
            {
                continue;
            }
            let bounds = implementation
                .bounds
                .iter()
                .map(|bound| InterfaceBound {
                    subject: substitute(&bound.subject, &substitutions),
                    interface: substitute(&bound.interface, &substitutions),
                })
                .collect();
            successes.push((trial, bounds, index));
        }
        successes
    }

    fn record_dispatch(
        &mut self,
        origin: Option<&DispatchOrigin<'ast>>,
        implementation: Option<usize>,
    ) {
        let Some(origin) = origin else {
            return;
        };
        let implementation = implementation.and_then(|index| {
            self.implementations[index]
                .methods
                .get(&origin.method)
                .copied()
        });
        self.member_resolutions.insert(
            origin.member,
            MemberResolution::InterfaceMethod {
                interface: origin.interface,
                declaration: origin.declaration,
                implementation,
            },
        );
    }

    fn clear_dispatch(&mut self, origin: Option<&DispatchOrigin<'ast>>) {
        if let Some(origin) = origin {
            self.member_resolutions.remove(&origin.member);
        }
    }

    fn try_assumptions(
        &mut self,
        subject: &Type,
        interface: &Type,
        assumptions: &[InterfaceBound],
    ) -> bool {
        for assumption in assumptions {
            let mut trial = self.unifier.clone();
            if trial.unify(subject, &assumption.subject).is_ok()
                && trial.unify(interface, &assumption.interface).is_ok()
            {
                self.unifier = trial;
                return true;
            }
        }
        false
    }

    fn check_capabilities(&mut self) {
        for capability in std::mem::take(&mut self.capabilities) {
            match capability {
                Capability::Numeric(ty, span) => {
                    let ty = self.unifier.resolve(&ty);
                    if !matches!(
                        ty,
                        Type::Int | Type::Float | Type::Error | Type::Variable(_)
                    ) {
                        self.error(TypeCheckErrorKind::ExpectedNumeric { found: ty }, span);
                    }
                }
                Capability::Boolean(ty, span) => {
                    let ty = self.unifier.resolve(&ty);
                    if !matches!(ty, Type::Bool | Type::Error) {
                        self.error(TypeCheckErrorKind::ExpectedBoolean { found: ty }, span);
                    }
                }
            }
        }
    }

    fn check_unresolved_types(&mut self) {
        let local_symbols: Vec<_> = self
            .resolution
            .symbols()
            .iter()
            .filter(|symbol| matches!(symbol.kind, SymbolKind::Let | SymbolKind::Function))
            .map(|symbol| (symbol.id, symbol.span.clone()))
            .collect();
        for (symbol, span) in local_symbols {
            let Some(scheme) = self.symbol_types.get(&symbol) else {
                continue;
            };
            let ty = self.unifier.resolve(&scheme.ty);
            if contains_variable(&ty) {
                self.error(TypeCheckErrorKind::CannotInferType { ty }, span);
            }
        }
    }

    fn lower_type_info(&mut self, type_info: &'ast TypeInfo<'input, 'allocator>) -> Type {
        self.lower_type_info_with_bounds(type_info, true)
    }

    fn lower_type_info_raw(&mut self, type_info: &'ast TypeInfo<'input, 'allocator>) -> Type {
        self.lower_type_info_with_bounds(type_info, false)
    }

    fn lower_type_info_with_bounds(
        &mut self,
        type_info: &'ast TypeInfo<'input, 'allocator>,
        enforce_bounds: bool,
    ) -> Type {
        let parameters: Vec<_> = type_info
            .parameters
            .iter()
            .map(|parameter| self.lower_type_info_with_bounds(parameter, enforce_bounds))
            .collect();
        let ty = match self.resolution.resolve_literal(&type_info.name) {
            Some(Res::BuiltinType(builtin)) => self.lower_builtin(builtin, parameters, type_info),
            Some(Res::Type(id)) => {
                let expected = self.type_arities.get(&id).copied().unwrap_or(0);
                if expected != parameters.len() {
                    self.error(
                        TypeCheckErrorKind::InvalidGenericArity {
                            expected,
                            found: parameters.len(),
                        },
                        type_info.span.clone(),
                    );
                }
                if enforce_bounds {
                    self.enforce_type_bounds(id, &parameters, type_info.span.clone());
                }
                Type::Named(id, parameters)
            }
            Some(Res::GenericParameter(id)) => {
                if !parameters.is_empty() {
                    self.error(
                        TypeCheckErrorKind::InvalidGenericArity {
                            expected: 0,
                            found: parameters.len(),
                        },
                        type_info.span.clone(),
                    );
                }
                Type::Parameter(id)
            }
            _ => Type::Error,
        };
        let ty = if type_info.optional {
            Type::optional(ty)
        } else {
            ty
        };
        let ty =
            if type_info.alternatives.is_empty() {
                ty
            } else {
                Type::union(std::iter::once(ty).chain(type_info.alternatives.iter().map(
                    |alternative| self.lower_type_info_with_bounds(alternative, enforce_bounds),
                )))
            };
        self.type_info_types
            .insert(AstId::new(type_info, AstKind::TypeInfo), ty.clone());
        ty
    }

    fn enforce_type_bounds(&mut self, id: TypeDefId, arguments: &[Type], span: Range<usize>) {
        let Some(parameters) = self.type_parameters.get(&id).cloned() else {
            return;
        };
        let bounds = self.type_bounds.get(&id).cloned().unwrap_or_default();
        let substitutions: HashMap<_, _> = parameters
            .into_iter()
            .zip(arguments.iter().cloned())
            .collect();
        for bound in bounds {
            self.obligations.push_back(Obligation {
                bound: InterfaceBound {
                    subject: substitute(&bound.subject, &substitutions),
                    interface: substitute(&bound.interface, &substitutions),
                },
                assumptions: self.assumptions.clone(),
                dispatch: None,
                span: span.clone(),
            });
        }
    }

    fn lower_builtin(
        &mut self,
        builtin: BuiltinType,
        mut parameters: Vec<Type>,
        type_info: &TypeInfo<'_, '_>,
    ) -> Type {
        let expected = match builtin {
            BuiltinType::List | BuiltinType::Set | BuiltinType::AttrSet => 1,
            _ => 0,
        };
        if parameters.len() != expected {
            self.error(
                TypeCheckErrorKind::InvalidGenericArity {
                    expected,
                    found: parameters.len(),
                },
                type_info.span.clone(),
            );
            return Type::Error;
        }
        match builtin {
            BuiltinType::Bool => Type::Bool,
            BuiltinType::Int => Type::Int,
            BuiltinType::Float => Type::Float,
            BuiltinType::String => Type::String,
            BuiltinType::Package => Type::Package,
            BuiltinType::List => Type::List(Box::new(parameters.remove(0))),
            BuiltinType::Set => Type::Set(Box::new(parameters.remove(0))),
            BuiltinType::AttrSet => Type::AttrSet(Box::new(parameters.remove(0))),
        }
    }

    fn generic_parameter_ids(
        &self,
        parameters: Option<&GenericParameters<'input, 'allocator>>,
    ) -> Vec<GenericParameterId> {
        parameters
            .into_iter()
            .flat_map(|parameters| parameters.parameters)
            .filter_map(
                |parameter| match self.resolution.declaration_of_literal(&parameter.name) {
                    Some(Declaration::GenericParameter(id)) => Some(id),
                    _ => None,
                },
            )
            .collect()
    }

    fn is_interface_type(&self, ty: &Type) -> bool {
        let Some(id) = named_type_id(ty) else {
            return false;
        };
        self.interface_types.contains(&id)
    }

    fn symbol_declaration(
        &self,
        literal: &'ast light_nix_parser::ast::Literal<'input>,
    ) -> Option<SymbolId> {
        match self.resolution.declaration_of_literal(literal) {
            Some(Declaration::Symbol(symbol)) => Some(symbol),
            _ => None,
        }
    }

    fn type_declaration(
        &self,
        literal: &'ast light_nix_parser::ast::Literal<'input>,
    ) -> Option<TypeDefId> {
        match self.resolution.declaration_of_literal(literal) {
            Some(Declaration::Type(ty)) => Some(ty),
            _ => None,
        }
    }

    fn generic_name(&self, id: GenericParameterId) -> light_nix_name_resolver::NameId {
        self.resolution
            .generic_parameters()
            .iter()
            .find(|parameter| parameter.id == id)
            .map(|parameter| parameter.name)
            .expect("generic parameter must belong to this resolution")
    }

    fn require_numeric(&mut self, ty: &Type, span: Range<usize>) {
        self.capabilities
            .push(Capability::Numeric(ty.clone(), span));
    }

    fn require_boolean(&mut self, ty: &Type, span: Range<usize>) {
        self.unify_at(&Type::Bool, ty, span.clone());
        self.capabilities
            .push(Capability::Boolean(ty.clone(), span));
    }

    fn unify_at(&mut self, expected: &Type, found: &Type, span: Range<usize>) -> Type {
        match self.unifier.unify(expected, found) {
            Ok(ty) => ty,
            Err(kind) => {
                self.error(kind, span);
                Type::Error
            }
        }
    }

    fn assign_at(&mut self, expected: &Type, found: &Type, span: Range<usize>) -> Type {
        let expected = self.unifier.resolve(expected);
        let found = self.unifier.resolve(found);
        if expected.accepts(&found) {
            expected
        } else {
            self.unify_at(&expected, &found, span)
        }
    }

    fn join_at(&mut self, left: &Type, right: &Type) -> Type {
        let left = self.unifier.resolve(left);
        let right = self.unifier.resolve(right);
        if left.accepts(&right) {
            left
        } else if right.accepts(&left) {
            right
        } else {
            Type::union([left, right])
        }
    }

    fn error(&mut self, kind: TypeCheckErrorKind, span: Range<usize>) {
        self.errors.push(TypeCheckError { kind, span });
    }
}

fn named_type_id(ty: &Type) -> Option<TypeDefId> {
    match ty {
        Type::Named(id, _) => Some(*id),
        _ => None,
    }
}

fn contains_type_variable(ty: &Type) -> bool {
    match ty {
        Type::Variable(_) => true,
        Type::Set(element)
        | Type::List(element)
        | Type::AttrSet(element)
        | Type::Optional(element) => contains_type_variable(element),
        Type::Union(alternatives) => alternatives.iter().any(contains_type_variable),
        Type::Named(_, arguments) => arguments.iter().any(contains_type_variable),
        Type::Function(function) => {
            function.parameters.iter().any(contains_type_variable)
                || contains_type_variable(&function.return_type)
        }
        _ => false,
    }
}

fn contains_unresolved_dispatch_type(ty: &Type) -> bool {
    match ty {
        Type::Parameter(_) | Type::Variable(_) => true,
        Type::Set(element)
        | Type::List(element)
        | Type::AttrSet(element)
        | Type::Optional(element) => contains_unresolved_dispatch_type(element),
        Type::Union(alternatives) => alternatives.iter().any(contains_unresolved_dispatch_type),
        Type::Named(_, arguments) => arguments.iter().any(contains_unresolved_dispatch_type),
        Type::Function(function) => {
            function
                .parameters
                .iter()
                .any(contains_unresolved_dispatch_type)
                || contains_unresolved_dispatch_type(&function.return_type)
        }
        _ => false,
    }
}

fn collection_element(ty: &Type) -> Option<Type> {
    match ty {
        Type::Set(element) | Type::List(element) => Some((**element).clone()),
        _ => None,
    }
}

fn contains_variable(ty: &Type) -> bool {
    match ty {
        Type::Variable(_) => true,
        Type::Set(element)
        | Type::List(element)
        | Type::AttrSet(element)
        | Type::Optional(element) => contains_variable(element),
        Type::Union(alternatives) => alternatives.iter().any(contains_variable),
        Type::Named(_, parameters) => parameters.iter().any(contains_variable),
        Type::Function(function) => {
            function.parameters.iter().any(contains_variable)
                || contains_variable(&function.return_type)
        }
        _ => false,
    }
}

fn is_assignment_target(expression: &Expression<'_, '_>) -> bool {
    let Expression::Primary(primary) = expression else {
        return false;
    };
    if matches!(&primary.value, Value::Literal(literal) if literal.call.is_some()) {
        return false;
    }
    primary
        .accesses
        .last()
        .is_none_or(|access| access.call.is_none())
}

fn instantiate_parameter_map(
    unifier: &mut Unifier,
    parameters: &[GenericParameterId],
) -> HashMap<GenericParameterId, Type> {
    parameters
        .iter()
        .copied()
        .map(|parameter| (parameter, unifier.fresh()))
        .collect()
}

fn explicit_parameter_order(scheme: &TypeScheme) -> Vec<GenericParameterId> {
    let mut ordered = Vec::with_capacity(scheme.parameters.len());
    for bound in &scheme.bounds {
        if let Type::Parameter(parameter) = bound.subject
            && scheme.parameters.contains(&parameter)
            && !ordered.contains(&parameter)
        {
            ordered.push(parameter);
        }
    }
    for parameter in &scheme.parameters {
        if !ordered.contains(parameter) {
            ordered.push(*parameter);
        }
    }
    ordered
}

fn substitute(ty: &Type, substitutions: &HashMap<GenericParameterId, Type>) -> Type {
    match ty {
        Type::Parameter(parameter) => substitutions
            .get(parameter)
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        Type::Set(element) => Type::Set(Box::new(substitute(element, substitutions))),
        Type::List(element) => Type::List(Box::new(substitute(element, substitutions))),
        Type::AttrSet(element) => Type::AttrSet(Box::new(substitute(element, substitutions))),
        Type::Optional(inner) => Type::optional(substitute(inner, substitutions)),
        Type::Union(alternatives) => {
            Type::union(alternatives.iter().map(|ty| substitute(ty, substitutions)))
        }
        Type::Named(id, parameters) => Type::Named(
            *id,
            parameters
                .iter()
                .map(|ty| substitute(ty, substitutions))
                .collect(),
        ),
        Type::Function(function) => Type::function(
            function
                .parameters
                .iter()
                .map(|ty| substitute(ty, substitutions))
                .collect(),
            substitute(&function.return_type, substitutions),
        ),
        _ => ty.clone(),
    }
}

fn type_parameters(ty: &Type) -> Vec<GenericParameterId> {
    let mut parameters = Vec::new();
    collect_type_parameters(ty, &mut parameters);
    parameters
}

fn collect_type_parameters(ty: &Type, parameters: &mut Vec<GenericParameterId>) {
    match ty {
        Type::Parameter(parameter) => parameters.push(*parameter),
        Type::Set(element)
        | Type::List(element)
        | Type::AttrSet(element)
        | Type::Optional(element) => {
            collect_type_parameters(element, parameters);
        }
        Type::Union(alternatives) => {
            for alternative in alternatives {
                collect_type_parameters(alternative, parameters);
            }
        }
        Type::Named(_, arguments) => {
            for argument in arguments {
                collect_type_parameters(argument, parameters);
            }
        }
        Type::Function(function) => {
            for parameter in &function.parameters {
                collect_type_parameters(parameter, parameters);
            }
            collect_type_parameters(&function.return_type, parameters);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use light_nix_name_resolver::{ImportEnvironment, ModuleId, collect_module};
    use light_nix_parser::{
        ast::{AstArena, Literal, Statement},
        lexer::Lexer,
        parser::{ParseErrors, parse_source},
    };

    use super::*;

    fn check_source(
        source: &str,
        test: impl FnOnce(&Source<'_, '_>, &NameResolution<'_>, &TypeCheckResult<'_>),
    ) {
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:#?}");
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(
            resolution.errors().is_empty(),
            "name errors: {:#?}",
            resolution.errors()
        );
        let result = check_module(ast, &resolution, &TypeEnvironment::default());
        test(ast, &resolution, &result);
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

    #[test]
    fn infers_generic_functions_and_reports_type_mismatches() {
        let source = r#"
opaque function identity<T>(value: T) -> T {
    return value
}
let integer = identity:<Int>(1)
let text = identity("hello")
let invalid: Bool = 1
"#;
        check_source(source, |ast, resolution, result| {
            assert_eq!(result.errors().len(), 1, "{:#?}", result.errors());
            assert!(matches!(
                result.errors()[0].kind,
                TypeCheckErrorKind::TypeMismatch {
                    expected: Type::Bool,
                    found: Type::Int
                }
            ));
            let Statement::LetStatement(integer) = ast.statements[1] else {
                panic!("expected integer binding");
            };
            let Statement::LetStatement(text) = ast.statements[2] else {
                panic!("expected text binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &integer.name))
                    .unwrap()
                    .ty,
                Type::Int
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &text.name))
                    .unwrap()
                    .ty,
                Type::String
            );
        });
    }

    #[test]
    fn arrays_default_to_lists_and_set_literals_are_explicit() {
        let source = r#"
opaque function stringify(value: Int) -> String {
    return value.to_string()
}
let values = [1, 2]
let mapped = values.map(stringify)
let contained = values.contains(1)
let unique = @set [1, 1, 2]
"#;
        check_source(source, |ast, resolution, result| {
            assert!(result.errors().is_empty(), "{:#?}", result.errors());
            let Statement::LetStatement(values) = ast.statements[1] else {
                panic!("expected values binding");
            };
            let Statement::LetStatement(mapped) = ast.statements[2] else {
                panic!("expected mapped binding");
            };
            let Statement::LetStatement(contained) = ast.statements[3] else {
                panic!("expected contained binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &values.name))
                    .unwrap()
                    .ty,
                Type::List(Box::new(Type::Int))
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &mapped.name))
                    .unwrap()
                    .ty,
                Type::List(Box::new(Type::String))
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &contained.name))
                    .unwrap()
                    .ty,
                Type::Bool
            );
            let Expression::Primary(mapped_value) = mapped.value.unwrap() else {
                panic!("expected primary map expression");
            };
            assert_eq!(
                result.member_resolution(&mapped_value.accesses[0]),
                Some(&MemberResolution::Builtin(BuiltinMethod::Map))
            );
            let Statement::LetStatement(unique) = ast.statements[4] else {
                panic!("expected explicit set binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &unique.name))
                    .unwrap()
                    .ty,
                Type::Set(Box::new(Type::Int))
            );
        });
    }

    #[test]
    fn infers_inline_and_opaque_closure_parameters_from_builtin_methods() {
        let source = r#"
let values = [1, 2]
let threshold = 1
let filtered = values.filter(inline |value| => value > threshold)
let mapped = values.map(opaque |value: Int| -> String => {
    return value.to_string()
})
"#;
        check_source(source, |ast, resolution, result| {
            assert!(result.errors().is_empty(), "{:#?}", result.errors());
            let Statement::LetStatement(filtered) = ast.statements[2] else {
                panic!("expected filtered binding");
            };
            let Statement::LetStatement(mapped) = ast.statements[3] else {
                panic!("expected mapped binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &filtered.name))
                    .unwrap()
                    .ty,
                Type::List(Box::new(Type::Int))
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &mapped.name))
                    .unwrap()
                    .ty,
                Type::List(Box::new(Type::String))
            );

            let Expression::Primary(filtered_value) = filtered.value.unwrap() else {
                panic!("expected filter call");
            };
            let Expression::Closure(closure) =
                filtered_value.accesses[0].call.unwrap().arguments[0]
            else {
                panic!("expected closure");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &closure.parameters[0].name))
                    .unwrap()
                    .ty,
                Type::Int
            );
        });
    }

    #[test]
    fn safe_access_and_elvis_flatten_optional_types() {
        let source = r#"
type Config { enabled: Bool }
declare let config: Config?
let enabled = config?.enabled ?: false
let nested: Int? = some(null)
"#;
        check_source(source, |ast, resolution, result| {
            assert!(result.errors().is_empty(), "{:#?}", result.errors());
            let Statement::LetStatement(enabled) = ast.statements[2] else {
                panic!("expected enabled binding");
            };
            let Statement::LetStatement(nested) = ast.statements[3] else {
                panic!("expected nested binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &enabled.name))
                    .unwrap()
                    .ty,
                Type::Bool
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &nested.name))
                    .unwrap()
                    .ty,
                Type::optional(Type::Int)
            );
            let Expression::Elvis(enabled_value) = enabled.value.unwrap() else {
                panic!("expected elvis expression");
            };
            let Expression::Primary(optional) = enabled_value.optional else {
                panic!("expected safe member access");
            };
            assert_eq!(
                result.member_resolution(&optional.accesses[0]),
                Some(&MemberResolution::Field(resolution.fields()[0].id))
            );
        });
    }

    #[test]
    fn generic_record_fields_are_instantiated_and_bounds_are_checked() {
        let source = r#"
interface Marker {}
type Item {}
type Invalid {}
implements Marker for Item {}

type Test<T>
where T: Marker {
    value: T
    nested: { value: T }
}

declare let valid: Test<Item>
declare let invalid: Test<Invalid>
let direct = valid.value
let nested = valid.nested.value
"#;
        check_source(source, |ast, resolution, result| {
            assert_eq!(result.errors().len(), 1, "{:#?}", result.errors());
            assert!(matches!(
                result.errors()[0].kind,
                TypeCheckErrorKind::MissingImplementation { .. }
            ));
            let Statement::TypeDefine(item) = ast.statements[1] else {
                panic!("expected Item type");
            };
            let item_type = Type::Named(type_of(resolution, &item.name), Vec::new());
            let Statement::LetStatement(direct) = ast.statements[7] else {
                panic!("expected direct binding");
            };
            let Statement::LetStatement(nested) = ast.statements[8] else {
                panic!("expected nested binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &direct.name))
                    .unwrap()
                    .ty,
                item_type
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &nested.name))
                    .unwrap()
                    .ty,
                item_type
            );
        });
    }

    #[test]
    fn implementation_constraints_infer_interface_type_arguments() {
        let source = r#"
interface TestInterface<T> {
    inline function value(this) -> T { throw "abstract" }
}
type Test {}
implements TestInterface<Int> for Test {
    inline function value(this) -> Int { return 1 }
}
opaque function extract<U, T: TestInterface<U>>(value: T) -> U {
    return value.value()
}
declare let test: Test
let inferred = extract:<Test>(test)
"#;
        check_source(source, |ast, resolution, result| {
            assert!(result.errors().is_empty(), "{:#?}", result.errors());
            let Statement::LetStatement(inferred) = ast.statements[5] else {
                panic!("expected inferred binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &inferred.name))
                    .unwrap()
                    .ty,
                Type::Int
            );
            let Statement::InterfaceDefine(interface) = ast.statements[0] else {
                panic!("expected interface");
            };
            let Statement::FunctionDefine(extract) = ast.statements[3] else {
                panic!("expected extract function");
            };
            let Statement::Expression(Expression::Return(returned)) =
                &extract.body.statements.statements[0]
            else {
                panic!("expected return expression");
            };
            let Expression::Primary(value) = returned.value.unwrap() else {
                panic!("expected interface method access");
            };
            assert_eq!(
                result.member_resolution(&value.accesses[0]),
                Some(&MemberResolution::InterfaceMethod {
                    interface: type_of(resolution, &interface.name),
                    declaration: symbol_of(resolution, &interface.methods[0].name),
                    implementation: None,
                })
            );
        });
    }

    #[test]
    fn interface_constraints_can_compute_type_level_addition() {
        let source = r#"
interface Nat {}

type Zero {}
type One {}
type Two {}
type Three {}
type Four {}
type Five {}

implements Nat for Zero {}
implements Nat for One {}
implements Nat for Two {}
implements Nat for Three {}
implements Nat for Four {}
implements Nat for Five {}

interface Add<Right: Nat, Answer: Nat> {
    inline function add(this, right: Right) -> Answer { throw "abstract" }
}

implements Add<Two, Three> for One {
    inline function add(this, right: Two) -> Three { throw "type-level" }
}
implements Add<Two, Four> for Two {
    inline function add(this, right: Two) -> Four { throw "type-level" }
}
implements Add<Two, Five> for Three {
    inline function add(this, right: Two) -> Five { throw "type-level" }
}

declare let one: One
declare let two: Two
declare let three_value: Three

let three: Three = one.add(two)
let four: Four = two.add(two)
let incorrect: Four = three_value.add(two)
"#;
        check_source(source, |_, _, result| {
            assert_eq!(result.errors().len(), 1, "{:#?}", result.errors());
            assert!(matches!(
                result.errors()[0].kind,
                TypeCheckErrorKind::MissingImplementation { .. }
            ));
        });
    }

    #[test]
    fn recursive_implementations_compute_peano_addition() {
        let source = r#"
interface Nat {}

type Zero {}
implements Nat for Zero {}

type Succ<N: Nat> { n: N }
implements<N: Nat> Nat for Succ<N> {}

interface Add<Right: Nat, Answer: Nat> {
    inline function add(this, right: Right) -> Answer { throw "abstract" }
}

implements<N: Nat> Add<N, N> for Zero {
    inline function add(this, right: N) -> N { throw "type-level" }
}

implements<N: Nat, M: Nat, Sum: Nat> Add<N, Succ<Sum>> for Succ<M>
where N: Add<M, Sum> {
    inline function add(this, right: N) -> Succ<Sum> { throw "type-level" }
}

declare let zero: Zero
declare let one: Succ<Zero>
declare let two: Succ<Succ<Zero>>

let three = one.add(two)
let four = two.add(two)
let incorrect: Succ<Succ<Succ<Succ<Zero>>>> = three.add(two)
"#;
        check_source(source, |ast, resolution, result| {
            assert_eq!(result.errors().len(), 1, "{:#?}", result.errors());
            assert!(matches!(
                result.errors()[0].kind,
                TypeCheckErrorKind::MissingImplementation { .. }
            ));

            let Statement::TypeDefine(zero) = ast.statements[1] else {
                panic!("expected Zero type");
            };
            let Statement::TypeDefine(succ) = ast.statements[3] else {
                panic!("expected Succ type");
            };
            let Statement::LetStatement(three) = ast.statements[11] else {
                panic!("expected three binding");
            };
            let Statement::LetStatement(four) = ast.statements[12] else {
                panic!("expected four binding");
            };

            let zero = Type::Named(type_of(resolution, &zero.name), Vec::new());
            let succ = type_of(resolution, &succ.name);
            let one = Type::Named(succ, vec![zero]);
            let two = Type::Named(succ, vec![one]);
            let three_type = Type::Named(succ, vec![two]);
            let four_type = Type::Named(succ, vec![three_type.clone()]);

            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &three.name))
                    .unwrap()
                    .ty,
                three_type
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &four.name))
                    .unwrap()
                    .ty,
                four_type
            );
        });
    }

    #[test]
    fn implementation_methods_must_match_the_interface() {
        let source = r#"
interface Value<T> {
    inline function value(this) -> T { throw "abstract" }
}
type Test {}
implements Value<Int> for Test {
    inline function value(this) -> String { return "wrong" }
}
"#;
        check_source(source, |_, _, result| {
            assert!(result.errors().iter().any(|error| matches!(
                error.kind,
                TypeCheckErrorKind::InterfaceMethodTypeMismatch { .. }
            )));
        });
    }

    #[test]
    fn implementation_methods_cannot_add_generic_requirements() {
        let source = r#"
interface Marker {}
interface Mapper {
    inline function map<T>(this, value: T) -> T { return value }
}
type Test {}
implements Mapper for Test {
    inline function map<T: Marker>(this, value: T) -> T { return value }
}
"#;
        check_source(source, |_, _, result| {
            assert!(result.errors().iter().any(|error| matches!(
                error.kind,
                TypeCheckErrorKind::InterfaceMethodTypeMismatch { .. }
            )));
        });
    }

    #[test]
    fn same_named_interface_methods_are_selected_by_the_receiver() {
        let source = r#"
interface IntegerValue {
    inline function value(this) -> Int { throw "abstract" }
}
interface StringValue {
    inline function value(this) -> String { throw "abstract" }
}
type Test {}
implements IntegerValue for Test {
    inline function value(this) -> Int { return 1 }
}
declare let test: Test
let selected = test.value()
"#;
        check_source(source, |ast, resolution, result| {
            assert!(result.errors().is_empty(), "{:#?}", result.errors());
            let Statement::LetStatement(selected) = ast.statements[5] else {
                panic!("expected selected binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &selected.name))
                    .unwrap()
                    .ty,
                Type::Int
            );
            let Statement::InterfaceDefine(interface) = ast.statements[0] else {
                panic!("expected IntegerValue interface");
            };
            let Statement::ImplementsDefine(implementation) = ast.statements[3] else {
                panic!("expected IntegerValue implementation");
            };
            let Expression::Primary(selected_value) = selected.value.unwrap() else {
                panic!("expected selected method call");
            };
            assert_eq!(
                result.member_resolution(&selected_value.accesses[0]),
                Some(&MemberResolution::InterfaceMethod {
                    interface: type_of(resolution, &interface.name),
                    declaration: symbol_of(resolution, &interface.methods[0].name),
                    implementation: Some(symbol_of(resolution, &implementation.methods[0].name,)),
                })
            );
        });
    }

    #[test]
    fn implementations_may_differ_by_interface_type_arguments() {
        let source = r#"
interface Marker<T> {
    inline function get(this) -> T { throw "abstract" }
}
type Multi {}
implements Marker<Int> for Multi {
    inline function get(this) -> Int { return 1 }
}
implements Marker<String> for Multi {
    inline function get(this) -> String { return "value" }
}
"#;
        check_source(source, |_, _, result| {
            assert!(result.errors().is_empty(), "{:#?}", result.errors());
        });
    }

    #[test]
    fn numeric_conversion_is_explicit_and_unknown_types_are_diagnosed() {
        let source = r#"
let invalid = 1 + 2.0
let valid = 1.to_float() + 2.0
let empty = []
"#;
        check_source(source, |ast, resolution, result| {
            assert_eq!(result.errors().len(), 2, "{:#?}", result.errors());
            assert!(result.errors().iter().any(|error| matches!(
                error.kind,
                TypeCheckErrorKind::TypeMismatch {
                    expected: Type::Int,
                    found: Type::Float
                }
            )));
            assert!(
                result
                    .errors()
                    .iter()
                    .any(|error| matches!(error.kind, TypeCheckErrorKind::CannotInferType { .. }))
            );
            let Statement::LetStatement(valid) = ast.statements[1] else {
                panic!("expected valid binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &valid.name))
                    .unwrap()
                    .ty,
                Type::Float
            );
        });
    }

    #[test]
    fn checks_record_assignments_and_optional_match_arms() {
        let source = r#"
type Config { enabled: Bool }
declare let config: Config
declare let optional: Int?
config.enabled = true
config.enabled = 1
let unwrapped = match optional {
    some(value) => value
    null => 0
}
let invalid = match optional {
    some(value) => value
    null => "none"
}
"#;
        check_source(source, |ast, resolution, result| {
            assert_eq!(result.errors().len(), 1, "{:#?}", result.errors());
            assert_eq!(
                result
                    .errors()
                    .iter()
                    .filter(|error| matches!(error.kind, TypeCheckErrorKind::TypeMismatch { .. }))
                    .count(),
                1
            );
            let Statement::LetStatement(unwrapped) = ast.statements[5] else {
                panic!("expected unwrapped binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &unwrapped.name))
                    .unwrap()
                    .ty,
                Type::Int
            );
            let Statement::LetStatement(union) = ast.statements[6] else {
                panic!("expected union binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &union.name))
                    .unwrap()
                    .ty,
                Type::union([Type::Int, Type::String])
            );
        });
    }

    #[test]
    fn checks_nested_assignment_fields_and_leaf_values() {
        let source = r#"
type Settings { count: Int }
type Config {
    enabled: Bool
    settings: Settings
}
declare let config: Config
config = {
    enabled = true
    settings = {
        count = 10
    }
}
config = {
    enabled = 1
    settings = {
        missing = false
    }
}
"#;
        check_source(source, |_, _, result| {
            assert_eq!(result.errors().len(), 2, "{:#?}", result.errors());
            assert!(result.errors().iter().any(|error| matches!(
                error.kind,
                TypeCheckErrorKind::TypeMismatch {
                    expected: Type::Bool,
                    found: Type::Int,
                }
            )));
            assert!(result.errors().iter().any(|error| matches!(
                &error.kind,
                TypeCheckErrorKind::UnknownMember { member, .. } if member == "missing"
            )));
        });
    }

    #[test]
    fn external_type_environment_supports_imported_functions_and_fields() {
        let library_source = r#"
export type Config { enabled: Bool }
export type Boxed<T> { value: T }
export opaque function identity<T>(value: T) -> T { return value }
export interface Flag {
    inline function flag(this) -> Bool { throw "abstract" }
}
implements Flag for Config {
    inline function flag(this) -> Bool { return this.enabled }
}
"#;
        let library_arena = AstArena::new();
        let mut library_lexer = Lexer::new(library_source);
        let mut library_parse_errors = ParseErrors::new_in(&library_arena);
        let library_ast = parse_source(
            &mut library_lexer,
            &mut library_parse_errors,
            &library_arena,
        );
        assert!(library_parse_errors.is_empty());
        let library_collected = collect_module(library_ast, ModuleId(1));
        let library_interface = library_collected.interface().clone();
        let library_resolution = library_collected.resolve(&ImportEnvironment::default());
        assert!(library_resolution.errors().is_empty());
        let library_result = check_module(
            library_ast,
            &library_resolution,
            &TypeEnvironment::default(),
        );
        assert!(library_result.errors().is_empty());

        let type_environment = library_result.type_environment();

        let source = r#"
import { Config, Boxed, identity } from "./library.lnix"
declare let config: Config
declare let boxed: Boxed<String>
let enabled = config.enabled
let number = identity(1)
let flag = config.flag()
let boxed_value = boxed.value
"#;
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty());
        let mut imports = ImportEnvironment::default();
        imports.insert(r#""./library.lnix""#, library_interface);
        let resolution = collect_module(ast, ModuleId(2)).resolve(&imports);
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let result = check_module(ast, &resolution, &type_environment);
        assert!(result.errors().is_empty(), "{:#?}", result.errors());

        let Statement::LetStatement(enabled) = ast.statements[3] else {
            panic!("expected enabled binding");
        };
        let Statement::LetStatement(number) = ast.statements[4] else {
            panic!("expected number binding");
        };
        let Statement::LetStatement(flag) = ast.statements[5] else {
            panic!("expected flag binding");
        };
        let Statement::LetStatement(boxed_value) = ast.statements[6] else {
            panic!("expected boxed value binding");
        };
        assert_eq!(
            result
                .symbol_type(symbol_of(&resolution, &enabled.name))
                .unwrap()
                .ty,
            Type::Bool
        );
        assert_eq!(
            result
                .symbol_type(symbol_of(&resolution, &number.name))
                .unwrap()
                .ty,
            Type::Int
        );
        assert_eq!(
            result
                .symbol_type(symbol_of(&resolution, &flag.name))
                .unwrap()
                .ty,
            Type::Bool
        );
        assert_eq!(
            result
                .symbol_type(symbol_of(&resolution, &boxed_value.name))
                .unwrap()
                .ty,
            Type::String
        );
    }

    #[test]
    fn unions_support_type_refinement_safe_casts_and_elvis() {
        let source = r#"
inline function normalize(value: Int | String) -> Int {
    if value is Int {
        let number: Int = value
        return value
    } else {
        let text: String = value
        return 0
    }
}
let tunable choice: Int | String = 1
let casted = choice as? Int ?: 0
let normalized = normalize(choice)
"#;
        check_source(source, |ast, resolution, result| {
            assert!(result.errors().is_empty(), "{:#?}", result.errors());
            let Statement::LetStatement(choice) = ast.statements[1] else {
                panic!("expected choice binding");
            };
            let Statement::LetStatement(casted) = ast.statements[2] else {
                panic!("expected cast binding");
            };
            let Statement::LetStatement(normalized) = ast.statements[3] else {
                panic!("expected normalized binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &choice.name))
                    .unwrap()
                    .ty,
                Type::union([Type::Int, Type::String])
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &casted.name))
                    .unwrap()
                    .ty,
                Type::Int
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &normalized.name))
                    .unwrap()
                    .ty,
                Type::Int
            );
        });
    }

    #[test]
    fn union_interface_bounds_and_implementation_targets_are_rejected() {
        let source = r#"
interface First {}
interface Second {}
opaque function invalid<T: First | Second>(value: T) -> T {
    return value
}
implements First for Int | String {}
"#;
        check_source(source, |_, _, result| {
            assert!(
                result
                    .errors()
                    .iter()
                    .any(|error| error.kind == TypeCheckErrorKind::UnionInterfaceBound)
            );
            assert!(
                result
                    .errors()
                    .iter()
                    .any(|error| error.kind == TypeCheckErrorKind::UnionImplementationTarget)
            );
        });
    }

    #[test]
    fn package_expectation_promotes_literals_but_never_variables() {
        let source = r#"
type Environment {
    packages: Set<Package>
    single: Package
    fallback: Package?
}
let packages: Set<Package> = @set ["firefox"]
let names = @set ["firefox"]
declare let environment: Environment
environment.packages = packages
environment.single = "kitty"
environment.fallback = some("mozc")
let invalid: Set<Package> = names
"#;
        check_source(source, |ast, resolution, result| {
            assert_eq!(result.errors().len(), 1, "{:#?}", result.errors());
            assert!(
                matches!(
                    &result.errors()[0].kind,
                    TypeCheckErrorKind::TypeMismatch { expected, found }
                        if *expected == Type::Package && *found == Type::String
                ),
                "{:#?}",
                result.errors()
            );
            let Statement::LetStatement(packages) = ast.statements[1] else {
                panic!("expected packages binding");
            };
            let Statement::LetStatement(names) = ast.statements[2] else {
                panic!("expected names binding");
            };
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &packages.name))
                    .unwrap()
                    .ty,
                Type::Set(Box::new(Type::Package))
            );
            assert_eq!(
                result
                    .symbol_type(symbol_of(resolution, &names.name))
                    .unwrap()
                    .ty,
                Type::Set(Box::new(Type::String))
            );
        });
    }
}
