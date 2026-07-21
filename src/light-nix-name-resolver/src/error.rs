use std::ops::Range;

use crate::{ModuleId, NameId, Namespace};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameResolveError {
    pub kind: NameResolveErrorKind,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameResolveErrorKind {
    UnresolvedName {
        name: NameId,
        namespace: Namespace,
    },
    DuplicateBinding {
        name: NameId,
        namespace: Namespace,
        first: Option<Range<usize>>,
    },
    UnknownModule,
    UnknownExport {
        module: ModuleId,
        name: NameId,
    },
    ImportNotAtModuleScope,
    ExportNotAtModuleScope,
    DuplicateField {
        name: NameId,
        first: Range<usize>,
    },
    DuplicateVariant {
        name: NameId,
        first: Range<usize>,
    },
}
