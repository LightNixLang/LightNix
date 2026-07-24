use std::marker::PhantomData;

use light_nix_name_resolver::{Declaration, NameResolution, Res, SymbolKind};
use light_nix_parser::ast::{
    AST, Array, ClosureBody, ElseBranchValue, Expression, FunctionCall, FunctionDefine, Literal,
    MatchArm, Pattern, Primary, Source, Statement, Statements, Value,
};
use light_nix_type_checker::{MemberResolution, TypeCheckResult};

use crate::{
    DependencyEdgeKind, DependencyGraph, DependencyNodeId, DependencyNodeKind, StatementId,
};

pub fn build_dependency_graph<'ast, 'input, 'allocator>(
    source: &'ast Source<'input, 'allocator>,
    resolution: &NameResolution<'ast>,
) -> DependencyGraph {
    Builder {
        resolution,
        graph: DependencyGraph::default(),
        owner: None,
        next_statement: 0,
        collect_base: true,
        types: None,
        marker: PhantomData,
    }
    .build(source)
}

pub fn refine_dependency_graph<'ast, 'input, 'allocator>(
    source: &'ast Source<'input, 'allocator>,
    resolution: &NameResolution<'ast>,
    types: &TypeCheckResult<'ast>,
    graph: &mut DependencyGraph,
) {
    let owned = std::mem::take(graph);
    *graph = Builder {
        resolution,
        graph: owned,
        owner: None,
        next_statement: 0,
        collect_base: false,
        types: Some(types),
        marker: PhantomData,
    }
    .build(source);
}

struct Builder<'ast, 'input, 'allocator, 'context> {
    resolution: &'context NameResolution<'ast>,
    graph: DependencyGraph,
    owner: Option<DependencyNodeId>,
    next_statement: u32,
    collect_base: bool,
    types: Option<&'context TypeCheckResult<'ast>>,
    marker: PhantomData<(&'input (), &'allocator ())>,
}

impl<'ast, 'input, 'allocator, 'context> Builder<'ast, 'input, 'allocator, 'context> {
    fn build(mut self, source: &'ast Source<'input, 'allocator>) -> DependencyGraph {
        self.visit_statements(source, true);
        self.graph
    }

    fn visit_statements(
        &mut self,
        statements: &'ast Statements<'input, 'allocator>,
        module_scope: bool,
    ) {
        for statement in statements.statements {
            self.visit_statement(statement, module_scope);
        }
    }

    fn visit_statement(
        &mut self,
        statement: &'ast Statement<'input, 'allocator>,
        module_scope: bool,
    ) {
        match statement {
            Statement::ImportStatement(_) | Statement::TypeDefine(_) | Statement::UseDeclare(_) => {
            }
            Statement::EnumDefine(node) => {
                for variant in node.variants {
                    let Some(Declaration::EnumVariant(id)) =
                        self.resolution.declaration_of_literal(&variant.name)
                    else {
                        continue;
                    };
                    let id = DependencyNodeId::Variant(id);
                    self.graph.define_node(
                        id,
                        DependencyNodeKind::EnumVariant,
                        variant.span.clone(),
                    );
                    if let Some(value) = variant.value {
                        self.with_owner(id, |builder| builder.visit_expression(value));
                    }
                }
            }
            Statement::InterfaceDefine(node) => {
                for method in node.methods {
                    self.visit_function(method);
                }
            }
            Statement::ImplementsDefine(node) => {
                for method in node.methods {
                    self.visit_function(method);
                }
            }
            Statement::LetStatement(node) => {
                let Some(Declaration::Symbol(symbol)) =
                    self.resolution.declaration_of_literal(&node.name)
                else {
                    return;
                };
                let id = DependencyNodeId::Symbol(symbol);
                self.graph
                    .define_node(id, DependencyNodeKind::Let, node.span.clone());
                if let Some(value) = node.value {
                    self.with_owner(id, |builder| builder.visit_expression(value));
                }
            }
            Statement::AssertStatement(node) => {
                let owner = self.statement_owner(
                    module_scope,
                    DependencyNodeKind::Assert,
                    node.span.clone(),
                );
                self.with_optional_owner(owner, |builder| {
                    builder.visit_expression(node.condition);
                    if let Some(message) = node.message {
                        builder.visit_expression(message);
                    }
                });
            }
            Statement::AssignStatement(node) => {
                let owner = self.statement_owner(
                    module_scope,
                    DependencyNodeKind::Assignment,
                    node.span.clone(),
                );
                self.with_optional_owner(owner, |builder| {
                    builder.visit_expression(node.target);
                    builder.visit_expression(node.value);
                });
            }
            Statement::FunctionDefine(node) => self.visit_function(node),
            Statement::Expression(expression) => {
                let owner = self.statement_owner(
                    module_scope,
                    DependencyNodeKind::Expression,
                    expression.span(),
                );
                self.with_optional_owner(owner, |builder| builder.visit_expression(expression));
            }
        }
    }

    fn visit_function(&mut self, function: &'ast FunctionDefine<'input, 'allocator>) {
        let Some(Declaration::Symbol(symbol)) =
            self.resolution.declaration_of_literal(&function.name)
        else {
            return;
        };
        let id = DependencyNodeId::Symbol(symbol);
        self.graph
            .define_node(id, DependencyNodeKind::Function, function.span.clone());
        self.with_owner(id, |builder| {
            builder.visit_statements(&function.body.statements, false);
        });
    }

    fn visit_expression(&mut self, expression: &'ast Expression<'input, 'allocator>) {
        match expression {
            Expression::If(node) => {
                self.visit_expression(node.branch.condition);
                self.visit_statements(&node.branch.body.statements, false);
                for branch in node.else_branches {
                    match branch.value {
                        ElseBranchValue::If(branch) => {
                            self.visit_expression(branch.condition);
                            self.visit_statements(&branch.body.statements, false);
                        }
                        ElseBranchValue::Block(block) => {
                            self.visit_statements(&block.statements, false);
                        }
                    }
                }
            }
            Expression::Match(node) => {
                self.visit_expression(node.value);
                for arm in node.arms {
                    self.visit_match_arm(arm);
                }
            }
            Expression::Return(node) => {
                if let Some(value) = node.value {
                    self.visit_expression(value);
                }
            }
            Expression::Throw(node) => {
                if let Some(message) = node.message {
                    self.visit_expression(message);
                }
            }
            Expression::Closure(node) => match node.body {
                ClosureBody::Expression(expression) => self.visit_expression(expression),
                ClosureBody::Block(block) => {
                    self.visit_statements(&block.statements, false);
                }
            },
            Expression::Elvis(node) => {
                self.visit_expression(node.optional);
                self.visit_expression(node.fallback);
            }
            Expression::Binary(node) => {
                self.visit_expression(node.left);
                self.visit_expression(node.right);
            }
            Expression::Unary(node) => self.visit_expression(node.operand),
            Expression::Primary(primary) => self.visit_primary(primary),
        }
    }

    fn visit_match_arm(&mut self, arm: &'ast MatchArm<'input, 'allocator>) {
        self.visit_pattern(&arm.pattern);
        self.visit_expression(arm.value);
    }

    fn visit_pattern(&mut self, pattern: &'ast Pattern<'input, 'allocator>) {
        match pattern {
            Pattern::Some(pattern) => self.visit_pattern(pattern.pattern),
            Pattern::EnumVariant(pattern) => {
                self.record_literal(&pattern.variant, DependencyEdgeKind::Reference);
            }
            Pattern::Null(_) | Pattern::Wildcard(_) | Pattern::Binding(_) => {}
        }
    }

    fn visit_primary(&mut self, primary: &'ast Primary<'input, 'allocator>) {
        self.visit_value(&primary.value);
        for access in primary.accesses {
            let kind = if access.call.is_some() {
                DependencyEdgeKind::Call
            } else {
                DependencyEdgeKind::Reference
            };
            self.record_literal(&access.member, kind);
            self.record_typed_member(access);
            if let Some(call) = access.call {
                self.visit_call(call);
            }
        }
    }

    fn visit_value(&mut self, value: &'ast Value<'input, 'allocator>) {
        match value {
            Value::Array(array) => self.visit_array(array),
            Value::Literal(literal) => {
                let kind = if literal.call.is_some() {
                    DependencyEdgeKind::Call
                } else {
                    DependencyEdgeKind::Reference
                };
                self.record_literal(&literal.literal, kind);
                if let Some(call) = literal.call {
                    self.visit_call(call);
                }
            }
            Value::Some(some) => {
                if let Some(value) = some.value {
                    self.visit_expression(value);
                }
            }
            Value::Numeric(_) | Value::String(_) | Value::Boolean(_) | Value::Null(_) => {}
        }
    }

    fn visit_array(&mut self, array: &'ast Array<'input, 'allocator>) {
        for value in array.values {
            self.visit_value(value);
        }
    }

    fn visit_call(&mut self, call: &'ast FunctionCall<'input, 'allocator>) {
        for argument in call.arguments {
            self.visit_expression(argument);
        }
    }

    fn record_literal(&mut self, literal: &'ast Literal<'input>, kind: DependencyEdgeKind) {
        if !self.collect_base {
            return;
        }
        let Some(source) = self.owner else {
            return;
        };
        let target = match self.resolution.resolve_literal(literal) {
            Some(Res::Symbol(symbol)) if self.is_dependency_symbol(symbol) => {
                DependencyNodeId::Symbol(symbol)
            }
            Some(Res::EnumVariant(variant)) => DependencyNodeId::Variant(variant),
            _ => return,
        };
        self.graph
            .add_edge(source, target, kind, literal.span.clone());
    }

    fn record_typed_member(
        &mut self,
        access: &'ast light_nix_parser::ast::PrimaryAccess<'input, 'allocator>,
    ) {
        let (Some(types), Some(source)) = (self.types, self.owner) else {
            return;
        };
        let Some(MemberResolution::InterfaceMethod {
            declaration,
            implementation,
            ..
        }) = types.member_resolution(access)
        else {
            return;
        };
        let (target, kind) = match implementation {
            Some(implementation) => (
                *implementation,
                if access.call.is_some() {
                    DependencyEdgeKind::Call
                } else {
                    DependencyEdgeKind::Reference
                },
            ),
            None => (*declaration, DependencyEdgeKind::InterfaceDispatch),
        };
        self.graph.add_edge(
            source,
            DependencyNodeId::Symbol(target),
            kind,
            access.member.span.clone(),
        );
    }

    fn is_dependency_symbol(&self, symbol: light_nix_name_resolver::SymbolId) -> bool {
        self.resolution
            .symbols()
            .iter()
            .find(|candidate| candidate.id == symbol)
            .is_none_or(|symbol| matches!(symbol.kind, SymbolKind::Let | SymbolKind::Function))
    }

    fn statement_owner(
        &mut self,
        module_scope: bool,
        kind: DependencyNodeKind,
        span: std::ops::Range<usize>,
    ) -> Option<DependencyNodeId> {
        if !module_scope {
            return self.owner;
        }
        let id = DependencyNodeId::Statement(StatementId {
            module: self.resolution.module(),
            index: self.next_statement,
        });
        self.next_statement = self
            .next_statement
            .checked_add(1)
            .expect("dependency statement table exceeded u32::MAX entries");
        self.graph.define_node(id, kind, span);
        Some(id)
    }

    fn with_owner(&mut self, owner: DependencyNodeId, visit: impl FnOnce(&mut Self)) {
        let previous = self.owner.replace(owner);
        visit(self);
        self.owner = previous;
    }

    fn with_optional_owner(
        &mut self,
        owner: Option<DependencyNodeId>,
        visit: impl FnOnce(&mut Self),
    ) {
        let previous = self.owner;
        self.owner = owner;
        visit(self);
        self.owner = previous;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use light_nix_name_resolver::{
        Declaration, ImportEnvironment, ModuleId, NameResolution, collect_module,
    };
    use light_nix_parser::{
        ast::{AstArena, Literal, Source, Statement},
        lexer::Lexer,
        parser::{ParseErrors, parse_source},
    };
    use light_nix_type_checker::{TypeEnvironment, check_module};

    use super::*;
    use crate::DependencyGraphErrorKind;

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

    fn symbol_of(resolution: &NameResolution<'_>, literal: &Literal<'_>) -> DependencyNodeId {
        let Some(Declaration::Symbol(symbol)) = resolution.declaration_of_literal(literal) else {
            panic!("expected symbol declaration");
        };
        DependencyNodeId::Symbol(symbol)
    }

    #[test]
    fn builds_value_dependencies_and_evaluation_order() {
        let source = r#"
let base = 1
opaque function read() -> Int {
    return base
}
let selected = if base > 0 {
    return read()
} else {
    return base
}
assert selected > 0, "selected must be positive"
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let graph = build_dependency_graph(ast, &resolution);

        let Statement::LetStatement(base) = ast.statements[0] else {
            panic!("expected base binding");
        };
        let Statement::FunctionDefine(read) = ast.statements[1] else {
            panic!("expected read function");
        };
        let Statement::LetStatement(selected) = ast.statements[2] else {
            panic!("expected selected binding");
        };
        let base = symbol_of(&resolution, &base.name);
        let read = symbol_of(&resolution, &read.name);
        let selected = symbol_of(&resolution, &selected.name);
        let assertion = graph
            .nodes()
            .find(|node| node.kind == DependencyNodeKind::Assert)
            .expect("expected assert node")
            .id;

        assert_eq!(graph.dependencies(read).collect::<Vec<_>>(), vec![base]);
        assert_eq!(
            graph.dependencies(selected).collect::<HashSet<_>>(),
            HashSet::from([base, read])
        );
        assert_eq!(
            graph.dependencies(assertion).collect::<Vec<_>>(),
            vec![selected]
        );
        assert!(graph.cycles().is_empty());

        let positions = graph
            .evaluation_order()
            .expect("graph should be acyclic")
            .into_iter()
            .enumerate()
            .map(|(index, node)| (node, index))
            .collect::<HashMap<_, _>>();
        assert!(positions[&base] < positions[&read]);
        assert!(positions[&read] < positions[&selected]);
        assert!(positions[&selected] < positions[&assertion]);
    }

    #[test]
    fn reports_mutual_and_self_cycles_with_reference_spans() {
        let source = r#"
let first = second
let second = first
opaque function recursive() -> Int {
    return recursive()
}
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let graph = build_dependency_graph(ast, &resolution);

        let cycles = graph.cycles();
        assert_eq!(cycles.len(), 2, "{cycles:#?}");
        assert!(cycles.iter().any(|cycle| cycle.nodes.len() == 2));
        assert!(cycles.iter().any(|cycle| {
            cycle.nodes.len() == 1
                && cycle
                    .edges
                    .iter()
                    .any(|edge| edge.source == edge.target && edge.kind == DependencyEdgeKind::Call)
        }));
        assert!(graph.evaluation_order().is_err());
        assert!(graph.errors().iter().all(|error| {
            matches!(error.kind, DependencyGraphErrorKind::Cycle { .. })
                && !error.locations.is_empty()
                && error
                    .locations
                    .iter()
                    .all(|location| location.module == ModuleId(0))
        }));
    }

    #[test]
    fn parameters_are_not_dependencies_but_enum_values_are() {
        let source = r#"
let representation = "desktop"
enum Desktop: String {
    KDE = representation
}
opaque function identity(value: String) -> String {
    return value
}
let desktop = Desktop::KDE
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let graph = build_dependency_graph(ast, &resolution);

        let Statement::LetStatement(representation) = ast.statements[0] else {
            panic!("expected representation binding");
        };
        let Statement::EnumDefine(desktop_enum) = ast.statements[1] else {
            panic!("expected Desktop enum");
        };
        let Statement::FunctionDefine(identity) = ast.statements[2] else {
            panic!("expected identity function");
        };
        let Statement::LetStatement(desktop) = ast.statements[3] else {
            panic!("expected desktop binding");
        };
        let representation = symbol_of(&resolution, &representation.name);
        let identity = symbol_of(&resolution, &identity.name);
        let desktop = symbol_of(&resolution, &desktop.name);
        let Some(Declaration::EnumVariant(variant)) =
            resolution.declaration_of_literal(&desktop_enum.variants[0].name)
        else {
            panic!("expected enum variant declaration");
        };
        let variant = DependencyNodeId::Variant(variant);

        assert!(graph.dependencies(identity).next().is_none());
        assert_eq!(
            graph.dependencies(variant).collect::<Vec<_>>(),
            vec![representation]
        );
        assert_eq!(
            graph.dependencies(desktop).collect::<Vec<_>>(),
            vec![variant]
        );
    }

    #[test]
    fn closure_parameters_are_local_while_captured_values_are_dependencies() {
        let source = r#"
let values = [1, 2, 3]
let threshold = 1
let filtered = values.filter(inline |value| => value > threshold)
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let graph = build_dependency_graph(ast, &resolution);

        let Statement::LetStatement(values) = ast.statements[0] else {
            panic!("expected values binding");
        };
        let Statement::LetStatement(threshold) = ast.statements[1] else {
            panic!("expected threshold binding");
        };
        let Statement::LetStatement(filtered) = ast.statements[2] else {
            panic!("expected filtered binding");
        };
        let values = symbol_of(&resolution, &values.name);
        let threshold = symbol_of(&resolution, &threshold.name);
        let filtered = symbol_of(&resolution, &filtered.name);

        assert_eq!(
            graph.dependencies(filtered).collect::<HashSet<_>>(),
            HashSet::from([values, threshold])
        );
    }

    #[test]
    fn module_graphs_merge_external_imported_symbols() {
        let source_a = "export let base = 1";
        let arena_a = AstArena::new();
        let ast_a = parse(source_a, &arena_a);
        let collected_a = collect_module(ast_a, ModuleId(1));
        let interface_a = collected_a.interface().clone();
        let resolution_a = collected_a.resolve(&ImportEnvironment::default());

        let source_b = r#"
import * as common from "./a.lnix"
let derived = common.base
"#;
        let arena_b = AstArena::new();
        let ast_b = parse(source_b, &arena_b);
        let mut imports = ImportEnvironment::default();
        imports.insert("\"./a.lnix\"", interface_a);
        let resolution_b = collect_module(ast_b, ModuleId(2)).resolve(&imports);
        assert!(
            resolution_b.errors().is_empty(),
            "{:#?}",
            resolution_b.errors()
        );

        let Statement::LetStatement(base) = ast_a.statements[0] else {
            panic!("expected base binding");
        };
        let Statement::LetStatement(derived) = ast_b.statements[1] else {
            panic!("expected derived binding");
        };
        let base = symbol_of(&resolution_a, &base.name);
        let derived = symbol_of(&resolution_b, &derived.name);
        let graph_a = build_dependency_graph(ast_a, &resolution_a);
        let mut graph = build_dependency_graph(ast_b, &resolution_b);

        assert_eq!(graph.node(base).unwrap().kind, DependencyNodeKind::External);
        graph.extend(&graph_a);
        assert_eq!(graph.node(base).unwrap().kind, DependencyNodeKind::Let);
        assert_eq!(graph.dependencies(derived).collect::<Vec<_>>(), vec![base]);
        let order = graph.evaluation_order().expect("merged graph is acyclic");
        let base_position = order.iter().position(|node| *node == base).unwrap();
        let derived_position = order.iter().position(|node| *node == derived).unwrap();
        assert!(base_position < derived_position);
    }

    #[test]
    fn detects_cycles_across_modules() {
        let source_a = r#"
import { b } from "./b.lnix"
export let a = b
"#;
        let source_b = r#"
import { a } from "./a.lnix"
export let b = a
"#;
        let arena_a = AstArena::new();
        let arena_b = AstArena::new();
        let ast_a = parse(source_a, &arena_a);
        let ast_b = parse(source_b, &arena_b);
        let collected_a = collect_module(ast_a, ModuleId(1));
        let collected_b = collect_module(ast_b, ModuleId(2));
        let mut imports = ImportEnvironment::default();
        imports.insert("\"./a.lnix\"", collected_a.interface().clone());
        imports.insert("\"./b.lnix\"", collected_b.interface().clone());
        let resolution_a = collected_a.resolve(&imports);
        let resolution_b = collected_b.resolve(&imports);
        assert!(
            resolution_a.errors().is_empty(),
            "{:#?}",
            resolution_a.errors()
        );
        assert!(
            resolution_b.errors().is_empty(),
            "{:#?}",
            resolution_b.errors()
        );

        let mut graph = build_dependency_graph(ast_a, &resolution_a);
        graph.extend(&build_dependency_graph(ast_b, &resolution_b));
        let cycles = graph.cycles();
        assert_eq!(cycles.len(), 1, "{cycles:#?}");
        assert_eq!(cycles[0].nodes.len(), 2);
        assert_eq!(
            graph.errors()[0]
                .locations
                .iter()
                .map(|location| location.module)
                .collect::<HashSet<_>>(),
            HashSet::from([ModuleId(1), ModuleId(2)])
        );
    }

    #[test]
    fn typed_refinement_adds_concrete_dispatch_and_detects_method_recursion() {
        let source = r#"
interface Step {
    inline function step(this) -> Int { throw "abstract" }
}
type Counter {}
implements Step for Counter {
    inline function step(this) -> Int { return this.step() }
}
declare let counter: Counter
let value = counter.step()
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(types.errors().is_empty(), "{:#?}", types.errors());
        let mut graph = build_dependency_graph(ast, &resolution);
        assert!(graph.cycles().is_empty());

        refine_dependency_graph(ast, &resolution, &types, &mut graph);
        let refined_edge_count = graph.edges().len();
        refine_dependency_graph(ast, &resolution, &types, &mut graph);
        assert_eq!(graph.edges().len(), refined_edge_count);
        let Statement::ImplementsDefine(implementation) = ast.statements[2] else {
            panic!("expected Step implementation");
        };
        let implementation_method = symbol_of(&resolution, &implementation.methods[0].name);
        let cycles = graph.cycles();
        assert_eq!(cycles.len(), 1, "{cycles:#?}");
        assert_eq!(cycles[0].nodes, vec![implementation_method]);
        assert!(cycles[0].edges.iter().any(|edge| {
            edge.source == implementation_method
                && edge.target == implementation_method
                && edge.kind == DependencyEdgeKind::Call
        }));

        let Statement::LetStatement(value) = ast.statements[4] else {
            panic!("expected value binding");
        };
        let value = symbol_of(&resolution, &value.name);
        assert!(graph.edges_from(value).any(|edge| {
            edge.target == implementation_method && edge.kind == DependencyEdgeKind::Call
        }));
    }

    #[test]
    fn typed_refinement_preserves_symbolic_generic_dispatch() {
        let source = r#"
interface Value<T> {
    inline function value(this) -> T { throw "abstract" }
}
type Test {}
implements Value<Int> for Test {
    inline function value(this) -> Int { return 1 }
}
opaque function extract<U, T: Value<U>>(value: T) -> U {
    return value.value()
}
"#;
        let arena = AstArena::new();
        let ast = parse(source, &arena);
        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let types = check_module(ast, &resolution, &TypeEnvironment::default());
        assert!(types.errors().is_empty(), "{:#?}", types.errors());
        let mut graph = build_dependency_graph(ast, &resolution);
        refine_dependency_graph(ast, &resolution, &types, &mut graph);

        let Statement::InterfaceDefine(interface) = ast.statements[0] else {
            panic!("expected Value interface");
        };
        let Statement::FunctionDefine(extract) = ast.statements[3] else {
            panic!("expected extract function");
        };
        let declaration = symbol_of(&resolution, &interface.methods[0].name);
        let extract = symbol_of(&resolution, &extract.name);
        assert!(graph.edges_from(extract).any(|edge| {
            edge.target == declaration && edge.kind == DependencyEdgeKind::InterfaceDispatch
        }));
        assert!(graph.cycles().is_empty());
    }
}
