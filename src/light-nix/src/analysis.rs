use light_nix_dependency_graph::{
    DependencyGraph, DependencyGraphError, build_dependency_graph, refine_dependency_graph,
};
use light_nix_ir::{ConstraintModel, LowerError, LowerResult, OutputPath, lower_module};
use light_nix_name_resolver::{
    ImportEnvironment, ModuleId, ModuleInterface, NameResolution, Namespace, Res, collect_module,
};
use light_nix_parser::{
    ast::{AstArena, Source},
    error::ParseError,
    lexer::Lexer,
    parser::{ParseErrors, parse_source},
};
use light_nix_type_checker::{
    Type, TypeCheckError, TypeCheckResult, TypeEnvironment, check_module,
};

/// External module declarations and types available while analysing a source.
#[derive(Debug, Default)]
pub struct AnalysisEnvironment {
    imports: ImportEnvironment,
    types: TypeEnvironment,
}

impl AnalysisEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an import path and the interface and types it exposes.
    pub fn register_import(
        &mut self,
        path: impl AsRef<str>,
        interface: ModuleInterface,
        types: &TypeEnvironment,
    ) -> &mut Self {
        self.imports
            .insert(import_source_literal(path.as_ref()), interface);
        self.types.extend(types);
        self
    }

    /// Makes the exports and inferred types of another analysed module importable.
    pub fn register_module(
        &mut self,
        path: impl AsRef<str>,
        module: &ModuleAnalysis<'_, '_>,
    ) -> &mut Self {
        self.register_import(
            path,
            module.interface().clone(),
            &module.types().type_environment(),
        )
    }

    pub fn imports(&self) -> &ImportEnvironment {
        &self.imports
    }

    pub fn types(&self) -> &TypeEnvironment {
        &self.types
    }

    pub fn imports_mut(&mut self) -> &mut ImportEnvironment {
        &mut self.imports
    }

    pub fn types_mut(&mut self) -> &mut TypeEnvironment {
        &mut self.types
    }
}

/// All compiler products for one source module.
pub struct ModuleAnalysis<'input, 'allocator> {
    source: &'allocator Source<'input, 'allocator>,
    parse_errors: ParseErrors<'input, 'allocator>,
    resolution: NameResolution<'allocator>,
    types: TypeCheckResult<'allocator>,
    dependencies: DependencyGraph,
    lowered: LowerResult,
}

impl<'input, 'allocator> ModuleAnalysis<'input, 'allocator> {
    pub fn source(&self) -> &'allocator Source<'input, 'allocator> {
        self.source
    }

    pub fn parse_errors(&self) -> &[ParseError<'input, 'allocator>] {
        &self.parse_errors
    }

    pub fn resolution(&self) -> &NameResolution<'allocator> {
        &self.resolution
    }

    pub fn interface(&self) -> &ModuleInterface {
        self.resolution.interface()
    }

    pub fn types(&self) -> &TypeCheckResult<'allocator> {
        &self.types
    }

    pub fn dependencies(&self) -> &DependencyGraph {
        &self.dependencies
    }

    pub fn dependency_errors(&self) -> Vec<DependencyGraphError> {
        self.dependencies.errors()
    }

    pub fn model(&self) -> &ConstraintModel {
        self.lowered.model()
    }

    /// Resolves a catalog-style dotted output name such as
    /// `programs.firefox.enable` to the stable IDs used by the IR.
    pub fn output_path(&self, path: &str) -> Option<&OutputPath> {
        let segments = parse_output_path(path)?;
        self.output_path_segments(segments.iter().map(String::as_str))
    }

    pub fn output_path_segments<'name>(
        &self,
        segments: impl IntoIterator<Item = &'name str>,
    ) -> Option<&OutputPath> {
        let mut segments = segments.into_iter();
        let root_name = segments.next()?;
        let Res::Symbol(root) = self
            .resolution
            .resolve_root_name(Namespace::Value, root_name)?
        else {
            return None;
        };
        let mut receiver = self.types.symbol_type(root)?.ty.clone();
        let mut output = OutputPath::root(root);
        for segment in segments {
            match receiver {
                Type::AttrSet(element) => {
                    output = output.key(segment);
                    receiver = *element;
                }
                _ => {
                    if segment.is_empty() {
                        return None;
                    }
                    let (field, field_type) = self.types.named_field(&receiver, segment)?;
                    output = output.field(field);
                    receiver = field_type;
                }
            }
        }
        self.lowered
            .model()
            .path(&output)
            .map(|declaration| declaration.path())
    }

    pub fn name_errors(&self) -> &[light_nix_name_resolver::error::NameResolveError] {
        self.resolution.errors()
    }

    pub fn type_errors(&self) -> &[TypeCheckError] {
        self.types.errors()
    }

    pub fn lower_errors(&self) -> &[LowerError] {
        self.lowered.errors()
    }

    pub fn is_success(&self) -> bool {
        self.parse_errors.is_empty()
            && self.resolution.errors().is_empty()
            && self.types.errors().is_empty()
            && self.dependencies.cycles().is_empty()
            && self.lowered.errors().is_empty()
    }
}

fn parse_output_path(path: &str) -> Option<Vec<String>> {
    let mut chars = path.chars().peekable();
    let mut segments = Vec::new();
    let mut root = String::new();
    while let Some(character) = chars.peek().copied() {
        if matches!(character, '.' | '[') {
            break;
        }
        root.push(character);
        chars.next();
    }
    if root.is_empty() {
        return None;
    }
    segments.push(root);

    while let Some(character) = chars.next() {
        match character {
            '.' => {
                let mut field = String::new();
                while let Some(character) = chars.peek().copied() {
                    if matches!(character, '.' | '[') {
                        break;
                    }
                    field.push(character);
                    chars.next();
                }
                if field.is_empty() {
                    return None;
                }
                segments.push(field);
            }
            '[' => {
                let quote = chars.next().filter(|quote| matches!(quote, '\'' | '"'))?;
                let mut key = String::new();
                let mut escaped = false;
                let mut closed = false;
                for character in chars.by_ref() {
                    if escaped {
                        key.push(match character {
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
                        closed = true;
                        break;
                    } else {
                        key.push(character);
                    }
                }
                if !closed || escaped || chars.next() != Some(']') {
                    return None;
                }
                segments.push(key);
            }
            _ => return None,
        }
    }
    Some(segments)
}

fn import_source_literal(path: &str) -> String {
    if path.starts_with('"') && path.ends_with('"') {
        return path.to_owned();
    }
    let mut literal = String::with_capacity(path.len() + 2);
    literal.push('"');
    for character in path.chars() {
        match character {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            character => literal.push(character),
        }
    }
    literal.push('"');
    literal
}

pub fn analyze_module<'input, 'allocator>(
    input: &'input str,
    arena: &'allocator AstArena,
    module: ModuleId,
    environment: &AnalysisEnvironment,
) -> ModuleAnalysis<'input, 'allocator> {
    let mut lexer = Lexer::new(input);
    let mut parse_errors = ParseErrors::new_in(arena);
    let source = parse_source(&mut lexer, &mut parse_errors, arena);
    let resolution = collect_module(source, module).resolve(environment.imports());
    let types = check_module(source, &resolution, environment.types());
    let mut dependencies = build_dependency_graph(source, &resolution);
    refine_dependency_graph(source, &resolution, &types, &mut dependencies);
    let lowered = lower_module(source, &resolution, &types);
    ModuleAnalysis {
        source,
        parse_errors,
        resolution,
        types,
        dependencies,
        lowered,
    }
}
