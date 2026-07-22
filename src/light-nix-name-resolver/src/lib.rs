mod ast_id;
pub mod error;
mod resolver;

pub use ast_id::{AstId, AstKind};
pub use resolver::{
    BuiltinType, CollectedModule, Declaration, ExportedBinding, Field, FieldId, GenericParameter,
    GenericParameterId, ImportEnvironment, ModuleId, ModuleInterface, NameId, NameResolution,
    Namespace, Res, Scope, ScopeId, ScopeKind, Symbol, SymbolId, SymbolKind, TypeDef, TypeDefId,
    TypeDefKind, Variant, VariantId, collect_module,
};
