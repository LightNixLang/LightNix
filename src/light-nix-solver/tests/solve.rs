use light_nix_ir::{
    BinaryOperation, CallTarget, Constant, ModelBuilder, MutationPolicy, ObjectiveKind, OutputPath,
    SourceOrigin, VariableKind, VariableSource, WeightedVariable, lower_module,
};
use light_nix_name_resolver::{
    Declaration, FieldId, ImportEnvironment, ModuleId, NameResolution, SymbolId, TypeDefId,
    VariantId, collect_module,
};
use light_nix_parser::{
    ast::{AstArena, Literal, Source, Statement},
    lexer::Lexer,
    parser::{ParseErrors, parse_source},
};
use light_nix_solver::{OutputGoal, SolveOutcome, SolveRequest, solve};
use light_nix_type_checker::{BuiltinMethod, Type, TypeEnvironment, check_module};

#[test]
fn chooses_the_lowest_cost_tunable_variable() {
    let module = ModuleId(0);
    let mut builder = ModelBuilder::new(module);
    let zero = builder.constant(Type::Int, Constant::Int(0), None).unwrap();
    let hundred = builder
        .constant(Type::Int, Constant::Int(100), None)
        .unwrap();
    let enabled = builder
        .constant(Type::Bool, Constant::Bool(true), None)
        .unwrap();
    let cheap = builder
        .add_variable(
            VariableSource::Synthetic("cheap".to_owned()),
            Type::Int,
            VariableKind::Tunable { cost: 7 },
            Some(zero),
            None,
        )
        .unwrap();
    let expensive = builder
        .add_variable(
            VariableSource::Synthetic("expensive".to_owned()),
            Type::Int,
            VariableKind::Tunable { cost: 20 },
            Some(zero),
            None,
        )
        .unwrap();
    let cheap_ref = builder.variable_reference(cheap, None).unwrap();
    let expensive_ref = builder.variable_reference(expensive, None).unwrap();
    let cheap_guard = builder
        .binary(BinaryOperation::Equal, cheap_ref, hundred, Type::Bool, None)
        .unwrap();
    let expensive_guard = builder
        .binary(
            BinaryOperation::Equal,
            expensive_ref,
            hundred,
            Type::Bool,
            None,
        )
        .unwrap();
    let path = test_path(0);
    builder
        .add_output_case(
            path.clone(),
            Type::Bool,
            MutationPolicy::Readonly,
            cheap_guard,
            enabled,
            origin(),
        )
        .unwrap();
    builder
        .add_output_case(
            path.clone(),
            Type::Bool,
            MutationPolicy::Readonly,
            expensive_guard,
            enabled,
            origin(),
        )
        .unwrap();
    builder
        .add_objective(
            ObjectiveKind::MinimizeChanges(vec![
                WeightedVariable::new(cheap, 7),
                WeightedVariable::new(expensive, 20),
            ]),
            None,
        )
        .unwrap();

    let model = builder.finish();
    let mut request = SolveRequest::new();
    request.require_output(path, Constant::Bool(true));
    let outcome = solve(&model, &request).unwrap();

    let SolveOutcome::Sat(solution) = outcome else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 7);
    assert_eq!(solution.variables.len(), 1);
    assert_eq!(solution.variables[0].variable, cheap);
    assert_eq!(solution.variables[0].after, Constant::Int(100));
    assert!(solution.outputs.is_empty());
}

#[test]
fn can_choose_a_cheaper_direct_output_edit() {
    let module = ModuleId(0);
    let mut builder = ModelBuilder::new(module);
    let zero = builder.constant(Type::Int, Constant::Int(0), None).unwrap();
    let hundred = builder
        .constant(Type::Int, Constant::Int(100), None)
        .unwrap();
    let enabled = builder
        .constant(Type::Bool, Constant::Bool(true), None)
        .unwrap();
    let variable = builder
        .add_variable(
            VariableSource::Synthetic("n".to_owned()),
            Type::Int,
            VariableKind::Tunable { cost: 7 },
            Some(zero),
            None,
        )
        .unwrap();
    let variable_ref = builder.variable_reference(variable, None).unwrap();
    let guard = builder
        .binary(
            BinaryOperation::Equal,
            variable_ref,
            hundred,
            Type::Bool,
            None,
        )
        .unwrap();
    let path = test_path(0);
    builder
        .add_output_case(
            path.clone(),
            Type::Bool,
            MutationPolicy::Tunable { cost: 2 },
            guard,
            enabled,
            origin(),
        )
        .unwrap();

    let model = builder.finish();
    let mut request = SolveRequest::new();
    request.require_output(path.clone(), Constant::Bool(true));
    let outcome = solve(&model, &request).unwrap();

    let SolveOutcome::Sat(solution) = outcome else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 2);
    assert!(solution.variables.is_empty());
    assert_eq!(solution.outputs.len(), 1);
    assert_eq!(solution.outputs[0].path, path);
    assert_eq!(solution.outputs[0].before, None);
    assert_eq!(solution.outputs[0].after, Some(Constant::Bool(true)));
}

#[test]
fn derived_output_changes_are_not_reported_as_direct_edits() {
    let module = ModuleId(0);
    let mut builder = ModelBuilder::new(module);
    let zero = builder.constant(Type::Int, Constant::Int(0), None).unwrap();
    let hundred = builder
        .constant(Type::Int, Constant::Int(100), None)
        .unwrap();
    let enabled = builder
        .constant(Type::Bool, Constant::Bool(true), None)
        .unwrap();
    let variable = builder
        .add_variable(
            VariableSource::Synthetic("n".to_owned()),
            Type::Int,
            VariableKind::Tunable { cost: 7 },
            Some(zero),
            None,
        )
        .unwrap();
    let variable_ref = builder.variable_reference(variable, None).unwrap();
    let guard = builder
        .binary(
            BinaryOperation::Equal,
            variable_ref,
            hundred,
            Type::Bool,
            None,
        )
        .unwrap();
    let firefox = test_path(0);
    let hyprland = test_path(1);
    builder
        .add_output_case(
            firefox.clone(),
            Type::Bool,
            MutationPolicy::Readonly,
            guard,
            enabled,
            origin(),
        )
        .unwrap();
    builder
        .add_output_case(
            hyprland,
            Type::Bool,
            MutationPolicy::Tunable { cost: 3 },
            guard,
            enabled,
            origin(),
        )
        .unwrap();

    let model = builder.finish();
    let mut request = SolveRequest::new();
    request.require_output(firefox, Constant::Bool(true));
    let outcome = solve(&model, &request).unwrap();

    let SolveOutcome::Sat(solution) = outcome else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 7);
    assert_eq!(solution.variables.len(), 1);
    assert!(solution.outputs.is_empty());
}

#[test]
fn adds_a_relevant_member_to_a_finite_set() {
    let module = ModuleId(0);
    let mut builder = ModelBuilder::new(module);
    let string_set = Type::Set(Box::new(Type::String));
    let initial = builder
        .constant(
            string_set.clone(),
            Constant::Set(vec![Constant::String("firefox".to_owned())]),
            None,
        )
        .unwrap();
    let kitty = builder
        .constant(Type::String, Constant::String("kitty".to_owned()), None)
        .unwrap();
    let enabled = builder
        .constant(Type::Bool, Constant::Bool(true), None)
        .unwrap();
    let packages = builder
        .add_variable(
            VariableSource::Synthetic("packages".to_owned()),
            string_set,
            VariableKind::Tunable { cost: 1 },
            Some(initial),
            None,
        )
        .unwrap();
    let packages_ref = builder.variable_reference(packages, None).unwrap();
    let contains = builder
        .call(
            CallTarget::Builtin(BuiltinMethod::Contains),
            Some(packages_ref),
            vec![kitty],
            Type::Bool,
            None,
        )
        .unwrap();
    let path = test_path(0);
    builder
        .add_output_case(
            path.clone(),
            Type::Bool,
            MutationPolicy::Readonly,
            contains,
            enabled,
            origin(),
        )
        .unwrap();

    let model = builder.finish();
    let mut request = SolveRequest::new();
    request.require_output(path, Constant::Bool(true));
    let outcome = solve(&model, &request).unwrap();

    let SolveOutcome::Sat(solution) = outcome else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 1);
    assert_eq!(
        solution.variables[0].after,
        Constant::Set(vec![
            Constant::String("firefox".to_owned()),
            Constant::String("kitty".to_owned()),
        ])
    );
}

#[test]
fn reports_unsat_for_an_impossible_readonly_goal() {
    let mut builder = ModelBuilder::new(ModuleId(0));
    let always = builder
        .constant(Type::Bool, Constant::Bool(true), None)
        .unwrap();
    let disabled = builder
        .constant(Type::Bool, Constant::Bool(false), None)
        .unwrap();
    let path = test_path(0);
    builder
        .add_output_case(
            path.clone(),
            Type::Bool,
            MutationPolicy::Readonly,
            always,
            disabled,
            origin(),
        )
        .unwrap();

    let model = builder.finish();
    let request = SolveRequest::new();
    let mut request = request;
    request.require_output(path.clone(), Constant::Bool(true));
    assert_eq!(
        request.goals(),
        &[OutputGoal::Equals {
            path,
            value: Constant::Bool(true),
        }]
    );
    assert_eq!(solve(&model, &request).unwrap(), SolveOutcome::Unsat);
}

#[test]
fn keeps_enum_candidates_inside_the_known_variant_domain() {
    let module = ModuleId(0);
    let enum_type = Type::Named(TypeDefId { module, index: 0 }, Vec::new());
    let first = VariantId { module, index: 0 };
    let second = VariantId { module, index: 1 };
    let mut builder = ModelBuilder::new(module);
    let initial = builder
        .constant(enum_type.clone(), Constant::Enum(first), None)
        .unwrap();
    let target = builder
        .constant(enum_type.clone(), Constant::Enum(second), None)
        .unwrap();
    let enabled = builder
        .constant(Type::Bool, Constant::Bool(true), None)
        .unwrap();
    let selected = builder
        .add_variable(
            VariableSource::Synthetic("desktop".to_owned()),
            enum_type,
            VariableKind::Tunable { cost: 4 },
            Some(initial),
            None,
        )
        .unwrap();
    let selected_ref = builder.variable_reference(selected, None).unwrap();
    let guard = builder
        .binary(
            BinaryOperation::Equal,
            selected_ref,
            target,
            Type::Bool,
            None,
        )
        .unwrap();
    let path = test_path(0);
    builder
        .add_output_case(
            path.clone(),
            Type::Bool,
            MutationPolicy::Readonly,
            guard,
            enabled,
            origin(),
        )
        .unwrap();

    let model = builder.finish();
    let mut request = SolveRequest::new();
    request.require_output(path, Constant::Bool(true));
    let SolveOutcome::Sat(solution) = solve(&model, &request).unwrap() else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 4);
    assert_eq!(solution.variables[0].after, Constant::Enum(second));
}

#[test]
fn solves_a_model_lowered_from_light_nix_source() {
    let source = r#"
type Firefox {
    enable: Bool
}
type Hyprland {
    enable: Bool
}
type Programs {
    readonly firefox: Firefox
    readonly hyprland: Hyprland
}
let tunable(cost = 7) n = 0
declare let tunable programs: Programs

if n == 100 {
    programs.firefox.enable = true
    programs.hyprland.enable = true
}
"#;
    let arena = AstArena::new();
    let ast = parse(source, &arena);
    let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
    assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
    let types = check_module(ast, &resolution, &TypeEnvironment::default());
    assert!(types.errors().is_empty(), "{:#?}", types.errors());
    let lowered = lower_module(ast, &resolution, &types);
    assert!(lowered.errors().is_empty(), "{:#?}", lowered.errors());

    let Statement::TypeDefine(firefox_type) = ast.statements[0] else {
        panic!("expected Firefox type");
    };
    let Statement::TypeDefine(programs_type) = ast.statements[2] else {
        panic!("expected Programs type");
    };
    let Statement::LetStatement(n_binding) = ast.statements[3] else {
        panic!("expected n");
    };
    let Statement::LetStatement(programs_binding) = ast.statements[4] else {
        panic!("expected programs");
    };
    let n = symbol_of(&resolution, &n_binding.name);
    let programs = symbol_of(&resolution, &programs_binding.name);
    let firefox = field_named(
        &resolution,
        type_of(&resolution, &programs_type.name),
        "firefox",
    );
    let enable = field_named(
        &resolution,
        type_of(&resolution, &firefox_type.name),
        "enable",
    );
    let path = OutputPath::root(programs).field(firefox).field(enable);

    let mut request = SolveRequest::new();
    request.require_output(path, Constant::Bool(true));
    let outcome = solve(lowered.model(), &request).unwrap();

    let SolveOutcome::Sat(solution) = outcome else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 7);
    assert_eq!(solution.variables.len(), 1);
    assert_eq!(solution.variables[0].source, VariableSource::Symbol(n));
    assert_eq!(solution.variables[0].after, Constant::Int(100));
    assert!(solution.outputs.is_empty());
}

#[test]
fn inline_filter_is_expanded_into_z3_constraints() {
    let source = r#"
type Programs {
    enabled: Bool
}
let tunable(cost = 4) values = []
declare let programs: Programs
programs.enabled = values.filter(inline |value: Int| => value > 0).contains(1)
"#;
    let arena = AstArena::new();
    let ast = parse(source, &arena);
    let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
    assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
    let types = check_module(ast, &resolution, &TypeEnvironment::default());
    assert!(types.errors().is_empty(), "{:#?}", types.errors());
    let lowered = lower_module(ast, &resolution, &types);
    assert!(lowered.errors().is_empty(), "{:#?}", lowered.errors());

    let Statement::TypeDefine(programs_type) = ast.statements[0] else {
        panic!("expected Programs type");
    };
    let Statement::LetStatement(values_binding) = ast.statements[1] else {
        panic!("expected values binding");
    };
    let Statement::LetStatement(programs_binding) = ast.statements[2] else {
        panic!("expected programs binding");
    };
    let values = symbol_of(&resolution, &values_binding.name);
    let programs = symbol_of(&resolution, &programs_binding.name);
    let enabled = field_named(
        &resolution,
        type_of(&resolution, &programs_type.name),
        "enabled",
    );
    let mut request = SolveRequest::new();
    request.require_output(
        OutputPath::root(programs).field(enabled),
        Constant::Bool(true),
    );

    let SolveOutcome::Sat(solution) = solve(lowered.model(), &request).unwrap() else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 4);
    assert_eq!(solution.variables.len(), 1);
    assert_eq!(solution.variables[0].source, VariableSource::Symbol(values));
    assert_eq!(
        solution.variables[0].after,
        Constant::List(vec![Constant::Int(1)])
    );
    assert!(solution.opaque_impacts.is_empty());
}

#[test]
fn tunable_lists_can_grow_to_cover_distinct_observable_values() {
    let source = r#"
type Programs {
    enabled: Bool
}
let tunable(cost = 4) values = []
declare let programs: Programs
programs.enabled = values.contains(1) and values.contains(2)
"#;
    let arena = AstArena::new();
    let ast = parse(source, &arena);
    let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
    assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
    let types = check_module(ast, &resolution, &TypeEnvironment::default());
    assert!(types.errors().is_empty(), "{:#?}", types.errors());
    let lowered = lower_module(ast, &resolution, &types);
    assert!(lowered.errors().is_empty(), "{:#?}", lowered.errors());

    let Statement::TypeDefine(programs_type) = ast.statements[0] else {
        panic!("expected Programs type");
    };
    let Statement::LetStatement(values_binding) = ast.statements[1] else {
        panic!("expected values binding");
    };
    let Statement::LetStatement(programs_binding) = ast.statements[2] else {
        panic!("expected programs binding");
    };
    let values = symbol_of(&resolution, &values_binding.name);
    let programs = symbol_of(&resolution, &programs_binding.name);
    let enabled = field_named(
        &resolution,
        type_of(&resolution, &programs_type.name),
        "enabled",
    );
    let mut request = SolveRequest::new();
    request.require_output(
        OutputPath::root(programs).field(enabled),
        Constant::Bool(true),
    );

    let SolveOutcome::Sat(solution) = solve(lowered.model(), &request).unwrap() else {
        panic!("expected a solution");
    };
    let change = solution
        .variables
        .iter()
        .find(|change| change.source == VariableSource::Symbol(values))
        .expect("list variable change");
    let Constant::List(after) = &change.after else {
        panic!("expected list value");
    };
    assert_eq!(after.len(), 2);
    assert!(after.contains(&Constant::Int(1)));
    assert!(after.contains(&Constant::Int(2)));
}

#[test]
fn inline_map_tracks_captured_tunable_values_symbolically() {
    let source = r#"
type Programs {
    enabled: Bool
}
let tunable(cost = 5) offset = 0
let values = [1]
declare let programs: Programs
programs.enabled = values.map(inline |value| => value + offset).contains(3)
"#;
    let arena = AstArena::new();
    let ast = parse(source, &arena);
    let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
    assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
    let types = check_module(ast, &resolution, &TypeEnvironment::default());
    assert!(types.errors().is_empty(), "{:#?}", types.errors());
    let lowered = lower_module(ast, &resolution, &types);
    assert!(lowered.errors().is_empty(), "{:#?}", lowered.errors());

    let Statement::TypeDefine(programs_type) = ast.statements[0] else {
        panic!("expected Programs type");
    };
    let Statement::LetStatement(offset_binding) = ast.statements[1] else {
        panic!("expected offset binding");
    };
    let Statement::LetStatement(programs_binding) = ast.statements[3] else {
        panic!("expected programs binding");
    };
    let offset = symbol_of(&resolution, &offset_binding.name);
    let programs = symbol_of(&resolution, &programs_binding.name);
    let enabled = field_named(
        &resolution,
        type_of(&resolution, &programs_type.name),
        "enabled",
    );
    let mut request = SolveRequest::new();
    request.require_output(
        OutputPath::root(programs).field(enabled),
        Constant::Bool(true),
    );

    let SolveOutcome::Sat(solution) = solve(lowered.model(), &request).unwrap() else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 5);
    assert_eq!(solution.variables.len(), 1);
    assert_eq!(solution.variables[0].source, VariableSource::Symbol(offset));
    assert_eq!(solution.variables[0].after, Constant::Int(2));
    assert!(solution.opaque_impacts.is_empty());
}

#[test]
fn list_constraints_preserve_order_and_duplicate_occurrences() {
    let source = r#"
type Programs {
    values: List<Int>
}
let tunable(cost = 5) offset = 0
let values = [1, 1, 2]
declare let programs: Programs
programs.values = values.map(inline |value| => value + offset)
"#;
    let arena = AstArena::new();
    let ast = parse(source, &arena);
    let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
    assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
    let types = check_module(ast, &resolution, &TypeEnvironment::default());
    assert!(types.errors().is_empty(), "{:#?}", types.errors());
    let lowered = lower_module(ast, &resolution, &types);
    assert!(lowered.errors().is_empty(), "{:#?}", lowered.errors());

    let Statement::TypeDefine(programs_type) = ast.statements[0] else {
        panic!("expected Programs type");
    };
    let Statement::LetStatement(offset_binding) = ast.statements[1] else {
        panic!("expected offset binding");
    };
    let Statement::LetStatement(programs_binding) = ast.statements[3] else {
        panic!("expected programs binding");
    };
    let offset = symbol_of(&resolution, &offset_binding.name);
    let programs = symbol_of(&resolution, &programs_binding.name);
    let values = field_named(
        &resolution,
        type_of(&resolution, &programs_type.name),
        "values",
    );
    let mut request = SolveRequest::new();
    request.require_output(
        OutputPath::root(programs).field(values),
        Constant::List(vec![Constant::Int(2), Constant::Int(2), Constant::Int(3)]),
    );

    let SolveOutcome::Sat(solution) = solve(lowered.model(), &request).unwrap() else {
        panic!("expected a solution");
    };
    assert_eq!(solution.cost, 5);
    assert_eq!(solution.variables.len(), 1);
    assert_eq!(solution.variables[0].source, VariableSource::Symbol(offset));
    assert_eq!(solution.variables[0].after, Constant::Int(1));
}

#[test]
fn opaque_filter_reports_the_changed_variables_crossing_its_boundary() {
    let source = r#"
type Programs {
    enabled: Bool
}
let tunable(cost = 4) values = []
declare let programs: Programs
programs.enabled = values.filter(opaque |value: Int| -> Bool => {
    return value > 0
}).contains(1)
"#;
    let arena = AstArena::new();
    let ast = parse(source, &arena);
    let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
    assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
    let types = check_module(ast, &resolution, &TypeEnvironment::default());
    assert!(types.errors().is_empty(), "{:#?}", types.errors());
    let lowered = lower_module(ast, &resolution, &types);
    assert!(lowered.errors().is_empty(), "{:#?}", lowered.errors());

    let Statement::TypeDefine(programs_type) = ast.statements[0] else {
        panic!("expected Programs type");
    };
    let Statement::LetStatement(values_binding) = ast.statements[1] else {
        panic!("expected values binding");
    };
    let Statement::LetStatement(programs_binding) = ast.statements[2] else {
        panic!("expected programs binding");
    };
    let values = symbol_of(&resolution, &values_binding.name);
    let programs = symbol_of(&resolution, &programs_binding.name);
    let enabled = field_named(
        &resolution,
        type_of(&resolution, &programs_type.name),
        "enabled",
    );
    let mut request = SolveRequest::new();
    request.require_output(
        OutputPath::root(programs).field(enabled),
        Constant::Bool(true),
    );

    let SolveOutcome::Sat(solution) = solve(lowered.model(), &request).unwrap() else {
        panic!("expected a solution");
    };
    assert_eq!(solution.variables.len(), 1);
    assert_eq!(solution.variables[0].source, VariableSource::Symbol(values));
    assert_eq!(solution.opaque_impacts.len(), 1);
    assert_eq!(
        solution.opaque_impacts[0].changed_variables,
        vec![solution.variables[0].variable]
    );
    assert!(solution.opaque_impacts[0].origin.is_some());
}

fn test_path(field: u32) -> OutputPath {
    OutputPath::root(SymbolId {
        module: ModuleId(0),
        index: 0,
    })
    .field(FieldId {
        module: ModuleId(0),
        index: field,
    })
}

fn origin() -> SourceOrigin {
    SourceOrigin::new(ModuleId(0), 0..0)
}

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
