use light_nix::{
    AnalysisEnvironment, EditError, Goal, ModuleAnalysis, PlanOutcome, PlanningRequest,
    analyze_module, apply_plan,
    catalog::{Catalog, Intent},
    evaluator::{EvaluationInputs, RuntimeValue, evaluate_module},
    ir::Constant,
    name_resolver::ModuleId,
    parser::ast::AstArena,
    plan_edits,
};

fn analyze<'input, 'allocator>(
    source: &'input str,
    arena: &'allocator AstArena,
) -> ModuleAnalysis<'input, 'allocator> {
    let analysis = analyze_module(source, arena, ModuleId(0), &AnalysisEnvironment::default());
    assert!(
        analysis.is_success(),
        "parse={:?}\nname={:?}\ntype={:?}\ndependency={:?}\nlower={:?}",
        analysis.parse_errors(),
        analysis.name_errors(),
        analysis.type_errors(),
        analysis.dependency_errors(),
        analysis.lower_errors(),
    );
    analysis
}

/// PutGet: re-analyses the written-back source and asserts the option now
/// evaluates to the value the plan promised.
fn assert_output_value(source: &str, option: &str, expected: Option<RuntimeValue>) {
    let arena = AstArena::new();
    let analysis = analyze(source, &arena);
    let path = analysis.output_path(option).expect("option path");
    let evaluated = evaluate_module(
        analysis.source(),
        analysis.resolution(),
        analysis.types(),
        &EvaluationInputs::default(),
    );
    assert!(evaluated.is_success(), "{:?}", evaluated.errors());
    assert_eq!(
        evaluated
            .snapshot()
            .get(&path)
            .map(|entry| entry.value.clone()),
        expected,
        "in written-back source:\n{source}"
    );
}

const FIREFOX_SCHEMA: &str = r#"type Firefox {
    tunable(cost = 2) enable: Bool
}
type Programs {
    firefox: Firefox
    zsh: Firefox
}
declare let programs: Programs
"#;

#[test]
fn an_inserted_claim_lands_in_canonical_position_and_evaluates() {
    let arena = AstArena::new();
    let source = format!("{FIREFOX_SCHEMA}programs.zsh.enable = true\n");
    let analysis = analyze(&source, &arena);
    let catalog = Catalog::new();
    let expansion = catalog
        .expand(
            &analysis,
            &[Intent::Enable {
                name: "firefox".to_owned(),
            }],
        )
        .unwrap();
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &expansion.request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };

    let written = apply_plan(&analysis, &plan, &source).unwrap();
    // The formatter sorts claims by path, so the inserted claim precedes the
    // existing zsh claim even though it was appended at the end.
    let firefox_at = written.find("programs.firefox.enable = true").unwrap();
    let zsh_at = written.find("programs.zsh.enable = true").unwrap();
    assert!(firefox_at < zsh_at, "{written}");
    assert_output_value(
        &written,
        "programs.firefox.enable",
        Some(RuntimeValue::Bool(true)),
    );
}

#[test]
fn a_tunable_binding_edit_rewrites_only_its_initial_literal() {
    let source = r#"type Environment {
    system_packages: Set<Package>
}
declare let environment: Environment

let tunable(cost = 1) packages: Set<Package> = @set ["firefox"]
environment.system_packages = packages
"#;
    let arena = AstArena::new();
    let analysis = analyze(source, &arena);
    let catalog = Catalog::new();
    let expansion = catalog
        .expand(
            &analysis,
            &[Intent::Install {
                package: "kitty".to_owned(),
            }],
        )
        .unwrap();
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &expansion.request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    assert_eq!(plan.solution().variables.len(), 1);

    let written = apply_plan(&analysis, &plan, source).unwrap();
    assert!(written.contains("\"firefox\""), "{written}");
    assert!(written.contains("\"kitty\""), "{written}");
    let arena = AstArena::new();
    let analysis = analyze(&written, &arena);
    let path = analysis
        .output_path("environment.system_packages")
        .expect("packages path");
    let evaluated = evaluate_module(
        analysis.source(),
        analysis.resolution(),
        analysis.types(),
        &EvaluationInputs::default(),
    );
    let Some(RuntimeValue::Set(values)) = evaluated
        .snapshot()
        .get(&path)
        .map(|entry| entry.value.clone())
    else {
        panic!("expected a package set in:\n{written}");
    };
    assert!(values.contains(&RuntimeValue::Package("firefox".to_owned())));
    assert!(values.contains(&RuntimeValue::Package("kitty".to_owned())));
}

#[test]
fn a_claim_row_edit_replaces_the_value_literal_in_place() {
    let source = r#"type Audio {
    tunable(cost = 1) pipewire: Bool
    tunable(cost = 1) pulseaudio: Bool
}
declare let audio: Audio

audio.pulseaudio = true
"#;
    let arena = AstArena::new();
    let analysis = analyze(source, &arena);
    let pulseaudio = analysis.output_path("audio.pulseaudio").expect("path");
    let mut request = PlanningRequest::new();
    request.require_output(pulseaudio.clone(), Constant::Bool(false));
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };

    let written = apply_plan(&analysis, &plan, source).unwrap();
    assert!(written.contains("audio.pulseaudio = false"), "{written}");
    assert!(!written.contains("audio.pulseaudio = true"), "{written}");
    assert_output_value(&written, "audio.pulseaudio", Some(RuntimeValue::Bool(false)));
}

#[test]
fn requiring_absence_deletes_the_claim_row() {
    let source = r#"type Audio {
    tunable(cost = 1) pipewire: Bool
    tunable(cost = 1) pulseaudio: Bool
}
declare let audio: Audio

audio.pipewire = true
audio.pulseaudio = true
"#;
    let arena = AstArena::new();
    let analysis = analyze(source, &arena);
    let pulseaudio = analysis.output_path("audio.pulseaudio").expect("path");
    let mut request = PlanningRequest::new();
    request.require_output_absent(pulseaudio.clone());
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };

    let written = apply_plan(&analysis, &plan, source).unwrap();
    assert!(!written.contains("audio.pulseaudio"), "{written}");
    assert!(written.contains("audio.pipewire = true"), "{written}");
    assert_output_value(&written, "audio.pulseaudio", None);
}

#[test]
fn deleting_a_claim_takes_its_attached_comments_along() {
    let source = r#"type Audio {
    tunable(cost = 1) pipewire: Bool
    tunable(cost = 1) pulseaudio: Bool
}
declare let audio: Audio

audio.pipewire = true

# temporary until the migration finishes
audio.pulseaudio = true # remove me
"#;
    let arena = AstArena::new();
    let analysis = analyze(source, &arena);
    let pulseaudio = analysis.output_path("audio.pulseaudio").expect("path");
    let mut request = PlanningRequest::new();
    request.require_output_absent(pulseaudio);
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };

    let written = apply_plan(&analysis, &plan, source).unwrap();
    assert!(!written.contains("audio.pulseaudio"), "{written}");
    assert!(!written.contains("temporary until"), "{written}");
    assert!(!written.contains("remove me"), "{written}");
    assert!(written.contains("audio.pipewire = true"), "{written}");
}

#[test]
fn a_plan_that_changes_nothing_produces_no_edits() {
    let arena = AstArena::new();
    let analysis = analyze(FIREFOX_SCHEMA, &arena);
    let path = analysis
        .output_path("programs.firefox.enable")
        .expect("path");
    let mut request = PlanningRequest::new();
    request.require_output_absent(path);
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };

    assert_eq!(plan_edits(&analysis, &plan, FIREFOX_SCHEMA).unwrap(), []);
}

#[test]
fn conditional_claims_are_refused_instead_of_guessed_at() {
    let source = r#"type Audio {
    tunable(cost = 1) pipewire: Bool
}
declare let audio: Audio

let tunable(cost = 100) toggle = false
if toggle {
    audio.pipewire = false
}
"#;
    let arena = AstArena::new();
    let analysis = analyze(source, &arena);
    let pipewire = analysis.output_path("audio.pipewire").expect("path");
    let mut request = PlanningRequest::new();
    request.require(Goal::Equals {
        path: pipewire.clone(),
        value: Constant::Bool(true),
    });
    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };

    // The solver proposes editing the conditional claim's output directly;
    // the write-back layer must refuse rather than fabricate a row edit.
    if !plan.solution().outputs.is_empty() {
        let error = apply_plan(&analysis, &plan, source).unwrap_err();
        assert!(matches!(error, EditError::Unrepresentable { .. }));
    }
}
