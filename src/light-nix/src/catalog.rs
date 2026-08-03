use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    fmt,
};

use light_nix_ir::{Constant, OutputPath};
use light_nix_type_checker::Type;

use crate::{ExternalConstraint, Goal, ModuleAnalysis, PlanningRequest};

/// A user-facing command, before the catalog translates it into facts about
/// option values.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Intent {
    Install { package: String },
    Remove { package: String },
    Enable { name: String },
    Disable { name: String },
    Set { option: String, value: Constant },
    Unset { option: String },
}

/// A catalog-level statement about an option, naming the option by its
/// dotted path.  Facts are pure data: they are resolved against a concrete
/// schema only at expansion time, and their constants are adapted to the
/// option's declared type (so `"firefox"` written in a catalog entry becomes
/// a package atom when the option holds packages).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Fact {
    Equals { option: String, value: Constant },
    Absent { option: String },
    Contains { option: String, value: Constant },
    NotContains { option: String, value: Constant },
}

impl Fact {
    pub fn option(&self) -> &str {
        match self {
            Self::Equals { option, .. }
            | Self::Absent { option }
            | Self::Contains { option, .. }
            | Self::NotContains { option, .. } => option,
        }
    }
}

/// A curated invariant that holds for every request, independent of which
/// intent is being expanded.  Axioms are selected needle-style: one joins a
/// request only when it mentions an option the request already touches (and
/// then options it adds can pull in further axioms, to a fixpoint).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Axiom {
    Requires(Fact),
    Implies { condition: Fact, consequence: Fact },
    Conflicts { left: Fact, right: Fact },
}

impl Axiom {
    fn facts(&self) -> impl Iterator<Item = &Fact> {
        let (first, second) = match self {
            Self::Requires(fact) => (fact, None),
            Self::Implies {
                condition,
                consequence,
            } => (condition, Some(consequence)),
            Self::Conflicts { left, right } => (left, Some(right)),
        };
        std::iter::once(first).chain(second)
    }
}

/// A curated (layer-2) expansion of one intent name into facts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recipe {
    pub goals: Vec<Fact>,
}

impl Recipe {
    pub fn new(goals: Vec<Fact>) -> Self {
        Self { goals }
    }
}

/// Which layer of the catalog answered an intent.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Mechanism {
    /// Layer 2: a curated recipe was found under this name.
    Recipe { name: String },
    /// Layer 1: the schema itself suggested `<prefix>.<name>.enable`.
    EnableHint { option: String },
    /// Layer 0: the terminal fallback onto the packages option.
    PackagesOption { option: String },
    /// A direct `set`/`unset` of a named option; no catalog knowledge used.
    Direct { option: String },
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CatalogError {
    /// An option name did not resolve against the schema.  `context` names
    /// the catalog entry or intent that referenced it.
    UnknownOption { option: String, context: String },
    /// No recipe and no schema hint answers this intent.
    NoMechanism { intent: Intent },
    /// The intent needs a collection option but the schema declares another
    /// shape.
    NotACollection { option: String },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownOption { option, context } => {
                write!(formatter, "unknown option `{option}` (via {context})")
            }
            Self::NoMechanism { intent } => {
                write!(formatter, "no catalog mechanism answers {intent:?}")
            }
            Self::NotACollection { option } => {
                write!(formatter, "option `{option}` is not a collection")
            }
        }
    }
}

impl Error for CatalogError {}

/// The result of expanding intents: a solver-ready request plus the
/// provenance a UI needs to explain what will happen and why.
#[derive(Debug)]
pub struct Expansion {
    pub request: PlanningRequest,
    /// Which mechanism answered each intent, in intent order.
    pub mechanisms: Vec<Mechanism>,
    /// The axioms that joined this request via needle selection.
    pub applied_axioms: Vec<Axiom>,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    packages_option: String,
    enable_hints: Vec<String>,
    recipes: HashMap<String, Recipe>,
    axioms: Vec<Axiom>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            packages_option: "environment.system_packages".to_owned(),
            enable_hints: vec!["programs".to_owned(), "services".to_owned()],
            recipes: HashMap::new(),
            axioms: Vec::new(),
        }
    }
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The `Set` option that receives layer-0 package installs.
    pub fn set_packages_option(&mut self, option: impl Into<String>) -> &mut Self {
        self.packages_option = option.into();
        self
    }

    /// The root prefixes tried for the layer-1 `<prefix>.<name>.enable` hint,
    /// in priority order.
    pub fn set_enable_hints(&mut self, hints: Vec<String>) -> &mut Self {
        self.enable_hints = hints;
        self
    }

    pub fn insert_recipe(&mut self, name: impl Into<String>, recipe: Recipe) -> &mut Self {
        self.recipes.insert(name.into(), recipe);
        self
    }

    pub fn add_axiom(&mut self, axiom: Axiom) -> &mut Self {
        self.axioms.push(axiom);
        self
    }

    /// Translates intents into a [`PlanningRequest`]: recipes first, then the
    /// schema name hint, then the terminal packages fallback, plus every
    /// axiom the touched options pull in.
    pub fn expand(
        &self,
        analysis: &ModuleAnalysis<'_, '_>,
        intents: &[Intent],
    ) -> Result<Expansion, CatalogError> {
        let mut request = PlanningRequest::new();
        let mut mechanisms = Vec::new();
        let mut mentioned = BTreeSet::new();

        for intent in intents {
            let mechanism = self.expand_intent(analysis, intent, &mut request, &mut mentioned)?;
            mechanisms.push(mechanism);
        }
        let applied_axioms = self.apply_axioms(analysis, &mut request, &mut mentioned)?;

        Ok(Expansion {
            request,
            mechanisms,
            applied_axioms,
        })
    }

    fn expand_intent(
        &self,
        analysis: &ModuleAnalysis<'_, '_>,
        intent: &Intent,
        request: &mut PlanningRequest,
        mentioned: &mut BTreeSet<OutputPath>,
    ) -> Result<Mechanism, CatalogError> {
        match intent {
            Intent::Install { package } | Intent::Remove { package } => {
                let install = matches!(intent, Intent::Install { .. });
                if let Some(recipe) = self.recipes.get(package) {
                    if install {
                        return self.apply_recipe(analysis, package, recipe, request, mentioned);
                    }
                    // Removal by recipe inversion is future work; fall back to
                    // the packages option below.
                }
                let context = format!("the packages option for {intent:?}");
                let (path, ty) =
                    resolve_option(analysis, &self.packages_option, &context)?;
                let Some(element) = collection_element(&ty) else {
                    return Err(CatalogError::NotACollection {
                        option: self.packages_option.clone(),
                    });
                };
                let value = adapt_constant(element, &Constant::String(package.clone()));
                mentioned.insert(path.clone());
                if install {
                    request.require_member(path, value);
                } else {
                    request.forbid_member(path, value);
                }
                Ok(Mechanism::PackagesOption {
                    option: self.packages_option.clone(),
                })
            }
            Intent::Enable { name } | Intent::Disable { name } => {
                let enable = matches!(intent, Intent::Enable { .. });
                if enable && let Some(recipe) = self.recipes.get(name) {
                    return self.apply_recipe(analysis, name, recipe, request, mentioned);
                }
                for prefix in &self.enable_hints {
                    let Some(path) =
                        analysis.output_path_segments([prefix.as_str(), name, "enable"])
                    else {
                        continue;
                    };
                    let Some((Type::Bool, _)) = analysis.output_declaration(&path) else {
                        continue;
                    };
                    mentioned.insert(path.clone());
                    request.require_output(path, Constant::Bool(enable));
                    return Ok(Mechanism::EnableHint {
                        option: format!("{prefix}.{name}.enable"),
                    });
                }
                Err(CatalogError::NoMechanism {
                    intent: intent.clone(),
                })
            }
            Intent::Set { option, value } => {
                let context = format!("{intent:?}");
                let (path, ty) = resolve_option(analysis, option, &context)?;
                mentioned.insert(path.clone());
                request.require_output(path, adapt_constant(&ty, value));
                Ok(Mechanism::Direct {
                    option: option.clone(),
                })
            }
            Intent::Unset { option } => {
                let context = format!("{intent:?}");
                let (path, _) = resolve_option(analysis, option, &context)?;
                mentioned.insert(path.clone());
                request.require_output_absent(path);
                Ok(Mechanism::Direct {
                    option: option.clone(),
                })
            }
        }
    }

    fn apply_recipe(
        &self,
        analysis: &ModuleAnalysis<'_, '_>,
        name: &str,
        recipe: &Recipe,
        request: &mut PlanningRequest,
        mentioned: &mut BTreeSet<OutputPath>,
    ) -> Result<Mechanism, CatalogError> {
        let context = format!("recipe `{name}`");
        for fact in &recipe.goals {
            let goal = resolve_fact(analysis, fact, &context)?;
            mentioned.insert(goal.path().clone());
            request.require(goal);
        }
        Ok(Mechanism::Recipe {
            name: name.to_owned(),
        })
    }

    /// Needle-driven axiom selection: an axiom joins once any of its options
    /// is already mentioned, and the options it adds can pull in further
    /// axioms until nothing changes.  Axioms about options this schema does
    /// not declare stay dormant unless selected, in which case the dangling
    /// option is a hard error.
    fn apply_axioms(
        &self,
        analysis: &ModuleAnalysis<'_, '_>,
        request: &mut PlanningRequest,
        mentioned: &mut BTreeSet<OutputPath>,
    ) -> Result<Vec<Axiom>, CatalogError> {
        let mut selected = vec![false; self.axioms.len()];
        let mut applied = Vec::new();
        loop {
            let mut changed = false;
            for (index, axiom) in self.axioms.iter().enumerate() {
                if selected[index] {
                    continue;
                }
                let intersects = axiom.facts().any(|fact| {
                    analysis
                        .output_path(fact.option())
                        .is_some_and(|path| mentioned.contains(&path))
                });
                if !intersects {
                    continue;
                }
                selected[index] = true;
                changed = true;
                let context = format!("axiom {axiom:?}");
                let constraint = match axiom {
                    Axiom::Requires(fact) => {
                        ExternalConstraint::Required(resolve_fact(analysis, fact, &context)?)
                    }
                    Axiom::Implies {
                        condition,
                        consequence,
                    } => ExternalConstraint::Implies {
                        condition: resolve_fact(analysis, condition, &context)?,
                        consequence: resolve_fact(analysis, consequence, &context)?,
                    },
                    Axiom::Conflicts { left, right } => ExternalConstraint::Conflicts {
                        left: resolve_fact(analysis, left, &context)?,
                        right: resolve_fact(analysis, right, &context)?,
                    },
                };
                for fact in axiom.facts() {
                    if let Some(path) = analysis.output_path(fact.option()) {
                        mentioned.insert(path);
                    }
                }
                request.constrain(constraint);
                applied.push(axiom.clone());
            }
            if !changed {
                return Ok(applied);
            }
        }
    }
}

fn resolve_option(
    analysis: &ModuleAnalysis<'_, '_>,
    option: &str,
    context: &str,
) -> Result<(OutputPath, Type), CatalogError> {
    let unknown = || CatalogError::UnknownOption {
        option: option.to_owned(),
        context: context.to_owned(),
    };
    let path = analysis.output_path(option).ok_or_else(unknown)?;
    let (ty, _) = analysis.output_declaration(&path).ok_or_else(unknown)?;
    Ok((path, ty))
}

fn resolve_fact(
    analysis: &ModuleAnalysis<'_, '_>,
    fact: &Fact,
    context: &str,
) -> Result<Goal, CatalogError> {
    let (path, ty) = resolve_option(analysis, fact.option(), context)?;
    Ok(match fact {
        Fact::Equals { value, .. } => Goal::Equals {
            path,
            value: adapt_constant(&ty, value),
        },
        Fact::Absent { .. } => Goal::Absent { path },
        Fact::Contains { value, .. } | Fact::NotContains { value, .. } => {
            let Some(element) = collection_element(&ty) else {
                return Err(CatalogError::NotACollection {
                    option: fact.option().to_owned(),
                });
            };
            let value = adapt_constant(element, value);
            match fact {
                Fact::Contains { .. } => Goal::Contains { path, value },
                _ => Goal::NotContains { path, value },
            }
        }
    })
}

fn collection_element(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Set(element) | Type::List(element) => Some(element),
        _ => None,
    }
}

/// Reinterprets catalog constants against the option's declared type, the
/// same way string literals are elaborated in the language: `"firefox"`
/// aimed at a `Package` position becomes a package atom.  Anything that does
/// not fit is left untouched for the solver's type checking to reject.
fn adapt_constant(ty: &Type, value: &Constant) -> Constant {
    match (ty, value) {
        (Type::Package, Constant::String(name)) => Constant::Package(name.clone()),
        (Type::Set(element), Constant::Set(values)) => Constant::Set(
            values
                .iter()
                .map(|value| adapt_constant(element, value))
                .collect(),
        ),
        (Type::List(element), Constant::List(values)) => Constant::List(
            values
                .iter()
                .map(|value| adapt_constant(element, value))
                .collect(),
        ),
        (Type::Optional(inner), Constant::Optional(Some(value))) => {
            Constant::Optional(Some(Box::new(adapt_constant(inner, value))))
        }
        _ => value.clone(),
    }
}
