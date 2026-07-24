use light_nix::{
    AnalysisEnvironment, PlanOutcome, PlanningRequest, analyze_module,
    evaluator::{EvaluationInputs, RuntimeValue},
    ir::{Constant, VariableSource},
    name_resolver::ModuleId,
    parser::ast::AstArena,
};

#[test]
fn imported_schema_defines_the_available_root_and_fields() {
    let schema_source = r#"
export type Firefox {
    enable: Bool
}
export type Programs {
    firefox: Firefox
}
export declare let programs: Programs
"#;
    let schema_arena = AstArena::new();
    let schema = analyze_module(
        schema_source,
        &schema_arena,
        ModuleId(1),
        &AnalysisEnvironment::default(),
    );
    assert!(schema.is_success());

    let mut environment = AnalysisEnvironment::default();
    environment.register_module("@lnix/nixos", &schema);
    let source = r#"
import { programs } from "@lnix/nixos"
programs.firefox.enable = true
"#;
    let arena = AstArena::new();
    let analysis = analyze_module(source, &arena, ModuleId(2), &environment);

    assert!(
        analysis.is_success(),
        "parse={:?}\nname={:?}\ntype={:?}\ndependency={:?}\nlower={:?}",
        analysis.parse_errors(),
        analysis.name_errors(),
        analysis.type_errors(),
        analysis.dependency_errors(),
        analysis.lower_errors(),
    );
    let path = analysis
        .output_path("programs.firefox.enable")
        .expect("imported output path");
    let mut request = PlanningRequest::new();
    request.require_output(path.clone(), Constant::Bool(true));
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert_eq!(plan.solution().cost, 0);
    assert!(plan.solution().variables.is_empty());
    assert!(plan.output_changes().is_empty());
}

#[test]
fn membership_goal_is_a_low_level_api_for_command_catalogs() {
    let source = r#"
type Environment {
    packages: Set<String>
}
let tunable(cost = 3) packages = []
declare let environment: Environment
environment.packages = packages
"#;
    let arena = AstArena::new();
    let analysis = analyze_module(source, &arena, ModuleId(0), &AnalysisEnvironment::default());
    assert!(analysis.is_success());
    let path = analysis
        .output_path("environment.packages")
        .expect("packages output")
        .clone();
    let mut request = PlanningRequest::new();
    request.require_member(path, Constant::String("kitty".to_owned()));

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert_eq!(plan.solution().cost, 3);
    assert_eq!(plan.solution().variables.len(), 1);
    assert_eq!(
        plan.solution().variables[0].after,
        Constant::Set(vec![Constant::String("kitty".to_owned())])
    );
    assert!(plan.side_effects().next().is_none());
    assert!(!plan.requires_confirmation());
}

#[test]
fn plan_exposes_changes_outside_the_requested_path() {
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
    let analysis = analyze_module(source, &arena, ModuleId(0), &AnalysisEnvironment::default());
    assert!(analysis.is_success());
    let firefox = analysis
        .output_path("programs.firefox.enable")
        .expect("firefox output")
        .clone();
    let hyprland = analysis
        .output_path("programs.hyprland.enable")
        .expect("hyprland output")
        .clone();
    let mut request = PlanningRequest::new();
    request.require_output(firefox, Constant::Bool(true));

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert_eq!(plan.solution().cost, 7);
    assert_eq!(plan.output_changes().len(), 2);
    let side_effects = plan.side_effects().collect::<Vec<_>>();
    assert_eq!(side_effects.len(), 1);
    assert_eq!(side_effects[0].path, hyprland);
    assert_eq!(
        side_effects[0].after.as_ref().map(|entry| &entry.value),
        Some(&RuntimeValue::Bool(true))
    );
    assert!(plan.requires_confirmation());
}

#[test]
fn concrete_evaluation_rejects_a_false_opaque_candidate_and_retries() {
    let source = r#"
type Programs {
    enabled: Bool
}
let tunable(cost = 1) packages = []
let tunable(cost = 2) bypass = false
declare let programs: Programs
programs.enabled = bypass or packages.filter(opaque |package: String| -> Bool => {
        return package != "firefox"
    }).contains("firefox")
"#;
    let arena = AstArena::new();
    let analysis = analyze_module(source, &arena, ModuleId(0), &AnalysisEnvironment::default());
    assert!(
        analysis.is_success(),
        "parse={:?}\nname={:?}\ntype={:?}\ndependency={:?}\nlower={:?}",
        analysis.parse_errors(),
        analysis.name_errors(),
        analysis.type_errors(),
        analysis.dependency_errors(),
        analysis.lower_errors(),
    );
    let enabled = analysis
        .output_path("programs.enabled")
        .expect("enabled output")
        .clone();
    let bypass = analysis
        .resolution()
        .symbols()
        .iter()
        .find(|symbol| analysis.resolution().name(symbol.name) == "bypass")
        .map(|symbol| symbol.id)
        .expect("bypass symbol");
    let mut request = PlanningRequest::new();
    request.require_output(enabled, Constant::Bool(true));

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert!(plan.rejected_candidates() >= 1);
    assert_eq!(plan.solution().cost, 2);
    assert_eq!(plan.solution().variables.len(), 1);
    assert_eq!(
        plan.solution().variables[0].source,
        VariableSource::Symbol(bypass)
    );
    assert_eq!(plan.solution().variables[0].after, Constant::Bool(true));
}
