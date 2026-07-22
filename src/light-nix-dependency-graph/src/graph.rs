use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashMap, HashSet},
    ops::Range,
};

use light_nix_name_resolver::{ModuleId, SymbolId, VariantId};

use crate::{DependencyGraphError, EvaluationOrderError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StatementId {
    pub module: ModuleId,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DependencyNodeId {
    Symbol(SymbolId),
    Variant(VariantId),
    Statement(StatementId),
}

impl DependencyNodeId {
    pub fn module(self) -> ModuleId {
        match self {
            Self::Symbol(id) => id.module,
            Self::Variant(id) => id.module,
            Self::Statement(id) => id.module,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyNodeKind {
    Let,
    Function,
    EnumVariant,
    Assert,
    Assignment,
    Expression,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyNode {
    pub id: DependencyNodeId,
    pub kind: DependencyNodeKind,
    pub span: Option<Range<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyEdgeKind {
    Reference,
    Call,
    InterfaceDispatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEdge {
    pub source: DependencyNodeId,
    pub target: DependencyNodeId,
    pub kind: DependencyEdgeKind,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyCycle {
    pub nodes: Vec<DependencyNodeId>,
    pub edges: Vec<DependencyEdge>,
}

#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    nodes: Vec<DependencyNode>,
    indices: HashMap<DependencyNodeId, usize>,
    edges: Vec<DependencyEdge>,
    outgoing: Vec<Vec<usize>>,
    incoming: Vec<Vec<usize>>,
}

impl DependencyGraph {
    pub fn node(&self, id: DependencyNodeId) -> Option<&DependencyNode> {
        self.indices.get(&id).map(|index| &self.nodes[*index])
    }

    pub fn nodes(&self) -> impl ExactSizeIterator<Item = &DependencyNode> {
        self.nodes.iter()
    }

    pub fn edges(&self) -> impl ExactSizeIterator<Item = &DependencyEdge> {
        self.edges.iter()
    }

    pub fn edges_from(&self, id: DependencyNodeId) -> impl Iterator<Item = &DependencyEdge> {
        self.indices
            .get(&id)
            .into_iter()
            .flat_map(|index| self.outgoing[*index].iter())
            .map(|edge| &self.edges[*edge])
    }

    pub fn edges_to(&self, id: DependencyNodeId) -> impl Iterator<Item = &DependencyEdge> {
        self.indices
            .get(&id)
            .into_iter()
            .flat_map(|index| self.incoming[*index].iter())
            .map(|edge| &self.edges[*edge])
    }

    pub fn dependencies(
        &self,
        id: DependencyNodeId,
    ) -> impl Iterator<Item = DependencyNodeId> + '_ {
        let mut seen = HashSet::new();
        self.edges_from(id)
            .map(|edge| edge.target)
            .filter(move |target| seen.insert(*target))
    }

    pub fn dependents(&self, id: DependencyNodeId) -> impl Iterator<Item = DependencyNodeId> + '_ {
        let mut seen = HashSet::new();
        self.edges_to(id)
            .map(|edge| edge.source)
            .filter(move |source| seen.insert(*source))
    }

    pub fn extend(&mut self, other: &Self) {
        for node in &other.nodes {
            self.insert_node(node.clone());
        }
        for edge in &other.edges {
            self.insert_edge(edge.clone());
        }
    }

    pub fn strongly_connected_components(&self) -> Vec<Vec<DependencyNodeId>> {
        let adjacency = self.adjacency();
        let mut tarjan = Tarjan::new(&adjacency);
        for node in 0..self.nodes.len() {
            if tarjan.indices[node].is_none() {
                tarjan.visit(node);
            }
        }
        tarjan
            .components
            .into_iter()
            .map(|component| {
                let mut nodes = component
                    .into_iter()
                    .map(|index| self.nodes[index].id)
                    .collect::<Vec<_>>();
                nodes.sort_unstable();
                nodes
            })
            .collect()
    }

    pub fn cycles(&self) -> Vec<DependencyCycle> {
        let mut cycles = self
            .strongly_connected_components()
            .into_iter()
            .filter_map(|nodes| {
                let members: HashSet<_> = nodes.iter().copied().collect();
                let edges = self
                    .edges
                    .iter()
                    .filter(|edge| members.contains(&edge.source) && members.contains(&edge.target))
                    .cloned()
                    .collect::<Vec<_>>();
                (nodes.len() > 1 || edges.iter().any(|edge| edge.source == edge.target))
                    .then_some(DependencyCycle { nodes, edges })
            })
            .collect::<Vec<_>>();
        cycles.sort_by_key(|cycle| cycle.nodes.first().copied());
        cycles
    }

    pub fn errors(&self) -> Vec<DependencyGraphError> {
        self.cycles()
            .iter()
            .map(DependencyGraphError::from)
            .collect()
    }

    /// Returns dependencies before the nodes that use them.
    pub fn evaluation_order(&self) -> Result<Vec<DependencyNodeId>, EvaluationOrderError> {
        let cycles = self.cycles();
        if !cycles.is_empty() {
            return Err(EvaluationOrderError { cycles });
        }

        let adjacency = self.adjacency();
        let mut reverse = vec![Vec::new(); self.nodes.len()];
        let mut remaining_dependencies = adjacency.iter().map(Vec::len).collect::<Vec<_>>();
        for (source, dependencies) in adjacency.iter().enumerate() {
            for dependency in dependencies {
                reverse[*dependency].push(source);
            }
        }

        let mut ready = BTreeSet::new();
        for (index, remaining) in remaining_dependencies.iter().enumerate() {
            if *remaining == 0 {
                ready.insert(Reverse((self.nodes[index].id, index)));
            }
        }

        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(Reverse((id, index))) = ready.pop_last() {
            order.push(id);
            for dependent in &reverse[index] {
                remaining_dependencies[*dependent] -= 1;
                if remaining_dependencies[*dependent] == 0 {
                    ready.insert(Reverse((self.nodes[*dependent].id, *dependent)));
                }
            }
        }
        debug_assert_eq!(order.len(), self.nodes.len());
        Ok(order)
    }

    pub(crate) fn define_node(
        &mut self,
        id: DependencyNodeId,
        kind: DependencyNodeKind,
        span: Range<usize>,
    ) {
        self.insert_node(DependencyNode {
            id,
            kind,
            span: Some(span),
        });
    }

    pub(crate) fn add_edge(
        &mut self,
        source: DependencyNodeId,
        target: DependencyNodeId,
        kind: DependencyEdgeKind,
        span: Range<usize>,
    ) {
        self.ensure_external(target);
        self.insert_edge(DependencyEdge {
            source,
            target,
            kind,
            span,
        });
    }

    fn ensure_external(&mut self, id: DependencyNodeId) {
        if !self.indices.contains_key(&id) {
            self.insert_node(DependencyNode {
                id,
                kind: DependencyNodeKind::External,
                span: None,
            });
        }
    }

    fn insert_node(&mut self, node: DependencyNode) {
        if let Some(index) = self.indices.get(&node.id).copied() {
            if self.nodes[index].kind == DependencyNodeKind::External
                && node.kind != DependencyNodeKind::External
            {
                self.nodes[index] = node;
            }
            return;
        }
        let index = self.nodes.len();
        self.indices.insert(node.id, index);
        self.nodes.push(node);
        self.outgoing.push(Vec::new());
        self.incoming.push(Vec::new());
    }

    fn insert_edge(&mut self, edge: DependencyEdge) {
        self.ensure_external(edge.source);
        self.ensure_external(edge.target);
        let source = self.indices[&edge.source];
        let target = self.indices[&edge.target];
        if self.outgoing[source]
            .iter()
            .any(|existing| self.edges[*existing] == edge)
        {
            return;
        }
        let edge_index = self.edges.len();
        self.edges.push(edge);
        self.outgoing[source].push(edge_index);
        self.incoming[target].push(edge_index);
    }

    fn adjacency(&self) -> Vec<Vec<usize>> {
        self.outgoing
            .iter()
            .map(|edges| {
                let mut dependencies = edges
                    .iter()
                    .map(|edge| self.indices[&self.edges[*edge].target])
                    .collect::<Vec<_>>();
                dependencies.sort_unstable();
                dependencies.dedup();
                dependencies
            })
            .collect()
    }
}

struct Tarjan<'graph> {
    adjacency: &'graph [Vec<usize>],
    next_index: usize,
    indices: Vec<Option<usize>>,
    low_links: Vec<usize>,
    stack: Vec<usize>,
    on_stack: Vec<bool>,
    components: Vec<Vec<usize>>,
}

impl<'graph> Tarjan<'graph> {
    fn new(adjacency: &'graph [Vec<usize>]) -> Self {
        Self {
            adjacency,
            next_index: 0,
            indices: vec![None; adjacency.len()],
            low_links: vec![0; adjacency.len()],
            stack: Vec::new(),
            on_stack: vec![false; adjacency.len()],
            components: Vec::new(),
        }
    }

    fn visit(&mut self, node: usize) {
        let index = self.next_index;
        self.next_index += 1;
        self.indices[node] = Some(index);
        self.low_links[node] = index;
        self.stack.push(node);
        self.on_stack[node] = true;

        for dependency in &self.adjacency[node] {
            if self.indices[*dependency].is_none() {
                self.visit(*dependency);
                self.low_links[node] = self.low_links[node].min(self.low_links[*dependency]);
            } else if self.on_stack[*dependency] {
                self.low_links[node] = self.low_links[node]
                    .min(self.indices[*dependency].expect("visited node has an index"));
            }
        }

        if self.low_links[node] != index {
            return;
        }
        let mut component = Vec::new();
        loop {
            let member = self.stack.pop().expect("SCC root must be on the stack");
            self.on_stack[member] = false;
            component.push(member);
            if member == node {
                break;
            }
        }
        self.components.push(component);
    }
}
