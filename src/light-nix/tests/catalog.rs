use light_nix::{
    AnalysisEnvironment, ModuleAnalysis, PlanOutcome, UnsatCause, analyze_module,
    catalog::{Axiom, Catalog, CatalogError, Fact, Intent, Mechanism, Recipe},
    evaluator::{EvaluationInputs, RuntimeValue},
    ir::Constant,
    name_resolver::ModuleId,
    parser::ast::AstArena,
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

#[test]
fn install_falls_back_to_the_packages_option() {
    let arena = AstArena::new();
    let analysis = analyze(
        r#"
type Environment {
    tunable(cost = 1) system_packages: Set<Package>
}
declare let environment: Environment
"#,
        &arena,
    );
    let catalog = Catalog::new();
    let expansion = catalog
        .expand(
            &analysis,
            &[Intent::Install {
                package: "kitty".to_owned(),
            }],
        )
        .unwrap();
    assert_eq!(
        expansion.mechanisms,
        vec![Mechanism::PackagesOption {
            option: "environment.system_packages".to_owned(),
        }]
    );
    assert!(expansion.applied_axioms.is_empty());

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &expansion.request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    let path = analysis
        .output_path("environment.system_packages")
        .expect("packages output");
    assert_eq!(plan.solution().cost, 1);
    assert_eq!(
        plan.after().get(&path).map(|entry| &entry.value),
        Some(&RuntimeValue::Set(vec![RuntimeValue::Package(
            "kitty".to_owned()
        )]))
    );
    assert!(!plan.requires_confirmation());
}

#[test]
fn enable_uses_the_schema_name_hint() {
    let arena = AstArena::new();
    let analysis = analyze(
        r#"
type Firefox {
    tunable(cost = 2) enable: Bool
}
type Programs {
    firefox: Firefox
}
declare let programs: Programs
"#,
        &arena,
    );
    let catalog = Catalog::new();
    let expansion = catalog
        .expand(
            &analysis,
            &[Intent::Enable {
                name: "firefox".to_owned(),
            }],
        )
        .unwrap();
    assert_eq!(
        expansion.mechanisms,
        vec![Mechanism::EnableHint {
            option: "programs.firefox.enable".to_owned(),
        }]
    );

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &expansion.request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    let path = analysis
        .output_path("programs.firefox.enable")
        .expect("enable output");
    assert_eq!(plan.solution().cost, 2);
    assert_eq!(
        plan.after().get(&path).map(|entry| &entry.value),
        Some(&RuntimeValue::Bool(true))
    );
    assert!(!plan.requires_confirmation());
}

fn audio_catalog() -> Catalog {
    let mut catalog = Catalog::new();
    catalog
        .insert_recipe(
            "pipewire",
            Recipe::new(vec![Fact::Equals {
                option: "audio.pipewire".to_owned(),
                value: Constant::Bool(true),
            }]),
        )
        .insert_recipe(
            "pulseaudio",
            Recipe::new(vec![Fact::Equals {
                option: "audio.pulseaudio".to_owned(),
                value: Constant::Bool(true),
            }]),
        )
        .add_axiom(Axiom::Conflicts {
            left: Fact::Equals {
                option: "audio.pipewire".to_owned(),
                value: Constant::Bool(true),
            },
            right: Fact::Equals {
                option: "audio.pulseaudio".to_owned(),
                value: Constant::Bool(true),
            },
        })
        // Dormant axiom about options this schema does not declare: it must
        // neither join the request nor fail expansion.
        .add_axiom(Axiom::Conflicts {
            left: Fact::Equals {
                option: "bogus.option".to_owned(),
                value: Constant::Bool(true),
            },
            right: Fact::Equals {
                option: "other.bogus".to_owned(),
                value: Constant::Bool(true),
            },
        });
    catalog
}

#[test]
fn recipe_with_conflict_axiom_disables_the_competitor_as_a_side_effect() {
    let arena = AstArena::new();
    let analysis = analyze(
        r#"
type Audio {
    tunable(cost = 1) pipewire: Bool
    tunable(cost = 1) pulseaudio: Bool
}
declare let audio: Audio
audio.pulseaudio = true
"#,
        &arena,
    );
    let catalog = audio_catalog();
    let expansion = catalog
        .expand(
            &analysis,
            &[Intent::Enable {
                name: "pipewire".to_owned(),
            }],
        )
        .unwrap();
    assert_eq!(
        expansion.mechanisms,
        vec![Mechanism::Recipe {
            name: "pipewire".to_owned(),
        }]
    );
    assert_eq!(expansion.applied_axioms.len(), 1);

    let PlanOutcome::Applicable(plan) = analysis
        .plan(&EvaluationInputs::default(), &expansion.request)
        .unwrap()
    else {
        panic!("expected an applicable plan");
    };
    let pipewire = analysis.output_path("audio.pipewire").expect("pipewire");
    let pulseaudio = analysis
        .output_path("audio.pulseaudio")
        .expect("pulseaudio");
    assert_eq!(plan.solution().cost, 2);
    assert_eq!(
        plan.after().get(&pipewire).map(|entry| &entry.value),
        Some(&RuntimeValue::Bool(true))
    );
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
fn conflicting_intents_report_the_axiom_in_the_core() {
    let arena = AstArena::new();
    let analysis = analyze(
        r#"
type Audio {
    tunable(cost = 1) pipewire: Bool
    tunable(cost = 1) pulseaudio: Bool
}
declare let audio: Audio
"#,
        &arena,
    );
    let catalog = audio_catalog();
    let expansion = catalog
        .expand(
            &analysis,
            &[
                Intent::Enable {
                    name: "pipewire".to_owned(),
                },
                Intent::Enable {
                    name: "pulseaudio".to_owned(),
                },
            ],
        )
        .unwrap();

    let PlanOutcome::Unsatisfiable {
        rejected_candidates,
        conflict,
    } = analysis
        .plan(&EvaluationInputs::default(), &expansion.request)
        .unwrap()
    else {
        panic!("expected an unsatisfiable outcome");
    };
    assert_eq!(rejected_candidates, 0);
    assert!(
        conflict
            .iter()
            .any(|cause| matches!(cause, UnsatCause::Constraint(_))),
        "the conflict should name the catalog axiom: {conflict:?}"
    );
    assert!(
        conflict
            .iter()
            .filter(|cause| matches!(cause, UnsatCause::Goal(_)))
            .count()
            >= 2,
        "the conflict should name both requested goals: {conflict:?}"
    );
}

#[test]
fn intents_without_any_mechanism_are_rejected() {
    let arena = AstArena::new();
    let analysis = analyze(
        r#"
type Programs {
    firefox: Bool
}
declare let programs: Programs
"#,
        &arena,
    );
    let catalog = Catalog::new();
    let error = catalog
        .expand(
            &analysis,
            &[Intent::Enable {
                name: "nonexistent".to_owned(),
            }],
        )
        .unwrap_err();
    assert!(matches!(error, CatalogError::NoMechanism { .. }));
}
