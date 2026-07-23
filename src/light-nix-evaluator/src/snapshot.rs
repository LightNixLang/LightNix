use std::collections::{BTreeMap, BTreeSet};

use light_nix_name_resolver::{FieldId, ModuleId, SymbolId};

use crate::RuntimeValue;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputPath {
    pub root: SymbolId,
    pub fields: Vec<FieldId>,
}

impl OutputPath {
    pub fn root(root: SymbolId) -> Self {
        Self {
            root,
            fields: Vec::new(),
        }
    }

    pub fn field(mut self, field: FieldId) -> Self {
        self.fields.push(field);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOrigin {
    pub module: ModuleId,
    pub span: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputEntry {
    pub value: RuntimeValue,
    pub dependencies: BTreeSet<SymbolId>,
    pub origin: SourceOrigin,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EvaluationSnapshot {
    outputs: BTreeMap<OutputPath, OutputEntry>,
}

impl EvaluationSnapshot {
    pub fn get(&self, path: &OutputPath) -> Option<&OutputEntry> {
        self.outputs.get(path)
    }

    pub fn outputs(&self) -> impl ExactSizeIterator<Item = (&OutputPath, &OutputEntry)> {
        self.outputs.iter()
    }

    pub fn diff(&self, after: &Self) -> Vec<OutputChange> {
        let paths = self
            .outputs
            .keys()
            .chain(after.outputs.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        paths
            .into_iter()
            .filter_map(|path| {
                let before = self.outputs.get(&path);
                let after = after.outputs.get(&path);
                (before.map(|entry| &entry.value) != after.map(|entry| &entry.value)).then(|| {
                    let dependencies = before
                        .into_iter()
                        .chain(after)
                        .flat_map(|entry| entry.dependencies.iter().copied())
                        .collect();
                    OutputChange {
                        path,
                        before: before.cloned(),
                        after: after.cloned(),
                        dependencies,
                    }
                })
            })
            .collect()
    }

    pub(crate) fn insert(&mut self, path: OutputPath, entry: OutputEntry) -> Option<OutputEntry> {
        self.outputs.insert(path, entry)
    }

    pub(crate) fn contains(&self, path: &OutputPath) -> bool {
        self.outputs.contains_key(path)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputChange {
    pub path: OutputPath,
    pub before: Option<OutputEntry>,
    pub after: Option<OutputEntry>,
    pub dependencies: BTreeSet<SymbolId>,
}
