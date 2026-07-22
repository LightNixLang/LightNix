use std::marker::PhantomData;

/// Ephemeral identity of an arena-allocated parser AST node.
///
/// The lifetime prevents a side table keyed by this value from outliving the
/// AST arena. It is intentionally not a stable identifier across reparses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AstId<'ast> {
    address: usize,
    kind: AstKind,
    marker: PhantomData<&'ast ()>,
}

impl<'ast> AstId<'ast> {
    pub fn new<T>(value: &'ast T, kind: AstKind) -> Self {
        Self {
            address: value as *const T as usize,
            kind,
            marker: PhantomData,
        }
    }

    pub fn kind(self) -> AstKind {
        self.kind
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AstKind {
    Literal,
    Import,
    EnumDefine,
    EnumVariant,
    TypeDefine,
    InterfaceDefine,
    ImplementsDefine,
    TypeBlock,
    Field,
    LetStatement,
    FunctionDefine,
    FunctionArgument,
    GenericParameter,
    Pattern,
    Expression,
    TypeInfo,
}
