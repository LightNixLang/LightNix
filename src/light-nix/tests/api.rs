use light_nix::{
    AnalysisEnvironment, Goal, PlanOutcome, PlanningRequest, analyze_module,
    evaluator::{EvaluationInputs, RuntimeValue},
    ir::{Constant, MutationPolicy, OutputPathSegment, VariableSource},
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
let tunable(cost = 3) packages = @set []
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
fn planning_preserves_list_order_and_duplicate_occurrences() {
    let source = r#"
type Environment {
    arguments: List<String>
}
let tunable(cost = 2) value = "before"
declare let environment: Environment
environment.arguments = [value, value]
"#;
    let arena = AstArena::new();
    let analysis = analyze_module(source, &arena, ModuleId(0), &AnalysisEnvironment::default());
    assert!(analysis.is_success());
    let path = analysis
        .output_path("environment.arguments")
        .expect("arguments output")
        .clone();
    let mut request = PlanningRequest::new();
    request.require_output(
        path.clone(),
        Constant::List(vec![
            Constant::String("after".to_owned()),
            Constant::String("after".to_owned()),
        ]),
    );

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert_eq!(plan.solution().cost, 2);
    assert_eq!(plan.solution().variables.len(), 1);
    assert_eq!(
        plan.solution().variables[0].after,
        Constant::String("after".to_owned())
    );
    assert_eq!(
        plan.after().get(&path).map(|entry| &entry.value),
        Some(&RuntimeValue::List(vec![
            RuntimeValue::String("after".to_owned()),
            RuntimeValue::String("after".to_owned()),
        ]))
    );
}

#[test]
fn nested_assignment_is_lowered_to_leaf_outputs() {
    let source = r#"
type FirefoxSettings {
    count: Int
}
type Firefox {
    tunable(cost = 5) enable: Bool
    settings: FirefoxSettings
}
type Programs {
    firefox: Firefox
}
let tunable(cost = 2) firefoxEnabled = false
declare let programs: Programs
programs.firefox = {
    enable = firefoxEnabled,
    settings = {
        count = 10,
    },
}
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
    assert_eq!(analysis.model().paths().count(), 2);
    let enabled = analysis
        .output_path("programs.firefox.enable")
        .expect("enable output")
        .clone();
    let count = analysis
        .output_path("programs.firefox.settings.count")
        .expect("count output")
        .clone();
    assert_eq!(
        analysis.model().path(&enabled).unwrap().policy(),
        MutationPolicy::Tunable { cost: 5 }
    );
    assert_eq!(
        analysis.model().path(&count).unwrap().policy(),
        MutationPolicy::Readonly
    );
    let mut request = PlanningRequest::new();
    request.require_output(enabled.clone(), Constant::Bool(true));

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert_eq!(plan.solution().cost, 2);
    assert_eq!(
        plan.before().get(&enabled).map(|entry| &entry.value),
        Some(&RuntimeValue::Bool(false))
    );
    assert_eq!(
        plan.after().get(&enabled).map(|entry| &entry.value),
        Some(&RuntimeValue::Bool(true))
    );
    assert_eq!(
        plan.after().get(&count).map(|entry| &entry.value),
        Some(&RuntimeValue::Int(10))
    );
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
fn catalog_conflict_disables_the_other_output_as_a_side_effect() {
    let source = r#"
type Audio {
    pipewire: Bool
    pulseaudio: Bool
}
let tunable(cost = 1) pipewire = false
let tunable(cost = 1) pulseaudio = true
declare let audio: Audio
audio.pipewire = pipewire
audio.pulseaudio = pulseaudio
"#;
    let arena = AstArena::new();
    let analysis = analyze_module(source, &arena, ModuleId(0), &AnalysisEnvironment::default());
    assert!(analysis.is_success());
    let pipewire = analysis
        .output_path("audio.pipewire")
        .expect("pipewire output")
        .clone();
    let pulseaudio = analysis
        .output_path("audio.pulseaudio")
        .expect("pulseaudio output")
        .clone();
    let pipewire_enabled = Goal::Equals {
        path: pipewire.clone(),
        value: Constant::Bool(true),
    };
    let pulseaudio_enabled = Goal::Equals {
        path: pulseaudio.clone(),
        value: Constant::Bool(true),
    };
    let mut request = PlanningRequest::new();
    request
        .require(pipewire_enabled.clone())
        .conflict(pipewire_enabled, pulseaudio_enabled);

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert_eq!(plan.solution().cost, 2);
    assert_eq!(plan.solution().variables.len(), 2);
    let side_effects = plan.side_effects().collect::<Vec<_>>();
    assert_eq!(side_effects.len(), 1);
    assert_eq!(side_effects[0].path, pulseaudio);
    assert_eq!(
        side_effects[0].after.as_ref().map(|entry| &entry.value),
        Some(&RuntimeValue::Bool(false))
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

#[test]
fn attr_set_keys_are_typed_tracked_and_solved_as_finite_output_paths() {
    let source = r#"
type FileSystemSettings {
    device: String
    neededForBoot: Bool
}
let tunable(cost = 4) rootDevice = "/dev/disk/by-label/old"
declare let fileSystems: AttrSet<FileSystemSettings>
fileSystems["/"] = {
    device = rootDevice,
    neededForBoot = true,
}
let observedDevice = fileSystems["/"].device
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

    let device = analysis
        .output_path(r#"fileSystems["/"].device"#)
        .expect("root device output")
        .clone();
    assert_eq!(
        analysis.output_path_segments(["fileSystems", "/", "device"]),
        Some(&device)
    );
    assert!(matches!(
        device.segments()[0],
        OutputPathSegment::Key(ref key) if key == "/"
    ));
    let needed_for_boot = analysis
        .output_path_segments(["fileSystems", "/", "neededForBoot"])
        .expect("neededForBoot output")
        .clone();
    let observed = analysis
        .resolution()
        .symbols()
        .iter()
        .find(|symbol| analysis.resolution().name(symbol.name) == "observedDevice")
        .map(|symbol| symbol.id)
        .expect("observedDevice symbol");
    let evaluated = light_nix::evaluator::evaluate_module(
        analysis.source(),
        analysis.resolution(),
        analysis.types(),
        &EvaluationInputs::default(),
    );
    assert_eq!(
        evaluated.symbol_value(observed),
        Some(&RuntimeValue::String("/dev/disk/by-label/old".to_owned()))
    );

    let mut request = PlanningRequest::new();
    request.require_output(
        device.clone(),
        Constant::String("/dev/disk/by-label/nixos".to_owned()),
    );
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };

    assert_eq!(plan.solution().cost, 4);
    assert_eq!(
        plan.after().get(&device).map(|entry| &entry.value),
        Some(&RuntimeValue::String("/dev/disk/by-label/nixos".to_owned()))
    );
    assert_eq!(
        plan.after().get(&needed_for_boot).map(|entry| &entry.value),
        Some(&RuntimeValue::Bool(true))
    );
    assert!(plan.side_effects().next().is_none());
}

#[test]
fn package_literals_are_typed_by_expectation_and_planned_as_package_atoms() {
    let source = r#"
type Environment {
    packages: Set<Package>
}
let tunable(cost = 3) packages: Set<Package> = @set ["firefox"]
declare let environment: Environment
environment.packages = packages
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
    let path = analysis
        .output_path("environment.packages")
        .expect("packages output")
        .clone();

    let evaluated = light_nix::evaluator::evaluate_module(
        analysis.source(),
        analysis.resolution(),
        analysis.types(),
        &EvaluationInputs::default(),
    );
    assert_eq!(
        evaluated.snapshot().get(&path).map(|entry| &entry.value),
        Some(&RuntimeValue::Set(vec![RuntimeValue::Package(
            "firefox".to_owned()
        )]))
    );

    let mut request = PlanningRequest::new();
    request.require_member(path.clone(), Constant::Package("kitty".to_owned()));

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert_eq!(plan.solution().cost, 3);
    assert_eq!(plan.solution().variables.len(), 1);
    assert_eq!(
        plan.after().get(&path).map(|entry| &entry.value),
        Some(&RuntimeValue::Set(vec![
            RuntimeValue::Package("firefox".to_owned()),
            RuntimeValue::Package("kitty".to_owned()),
        ]))
    );
    assert!(plan.side_effects().next().is_none());
    assert!(!plan.requires_confirmation());
}

#[test]
fn string_collections_do_not_flow_into_package_positions() {
    let source = r#"
type Environment {
    packages: Set<Package>
}
let packages = @set ["firefox"]
declare let environment: Environment
environment.packages = packages
"#;
    let arena = AstArena::new();
    let analysis = analyze_module(source, &arena, ModuleId(0), &AnalysisEnvironment::default());
    assert!(!analysis.is_success());
    assert!(!analysis.type_errors().is_empty());
}

#[test]
fn union_safe_cast_and_elvis_are_tracked_through_z3() {
    let source = r#"
type Result {
    value: Int
    casted: Int
    isInt: Bool
    matched: Int | String
}
let tunable(cost = 6) choice: Int | String = "not-an-int"
declare let result: Result
if choice is Int {
    result.value = choice
} else {
    result.value = 0
}
result.casted = choice as? Int ?: 0
result.isInt = choice is Int
result.matched = match choice as? Int {
    some(value) => value
    null => "none"
}
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

    let path = analysis
        .output_path("result.value")
        .expect("result value output")
        .clone();
    let is_int = analysis
        .output_path("result.isInt")
        .expect("type-test output")
        .clone();
    let casted = analysis
        .output_path("result.casted")
        .expect("safe-cast output")
        .clone();
    let matched = analysis
        .output_path("result.matched")
        .expect("union match output")
        .clone();
    let mut request = PlanningRequest::new();
    request.require_output(path.clone(), Constant::Int(7));
    request.require_output(casted.clone(), Constant::Int(7));
    request.require_output(is_int.clone(), Constant::Bool(true));
    request.require_output(matched.clone(), Constant::Int(7));
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };

    assert_eq!(plan.solution().cost, 6);
    assert_eq!(plan.solution().variables.len(), 1);
    assert_eq!(plan.solution().variables[0].after, Constant::Int(7));
    assert_eq!(
        plan.after().get(&path).map(|entry| &entry.value),
        Some(&RuntimeValue::Int(7))
    );
    assert_eq!(
        plan.after().get(&casted).map(|entry| &entry.value),
        Some(&RuntimeValue::Int(7))
    );
    assert_eq!(
        plan.after().get(&is_int).map(|entry| &entry.value),
        Some(&RuntimeValue::Bool(true))
    );
    assert_eq!(
        plan.after().get(&matched).map(|entry| &entry.value),
        Some(&RuntimeValue::Int(7))
    );
}
