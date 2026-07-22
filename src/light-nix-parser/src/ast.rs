use std::{fmt::Debug, ops::Range};

use extension_fn::extension_fn;

use crate::lexer::Token;

/// Arena used to allocate recursive AST nodes and variable-length AST slices.
pub type AstArena = bumpalo::Bump;

/// A node that occupies a byte range in the original source.
pub trait AST: Debug {
    fn span(&self) -> Range<usize>;
}

macro_rules! impl_ast {
    (impl<$($lifetime:lifetime),+> for $ty:ty) => {
        impl<$($lifetime),+> AST for $ty {
            fn span(&self) -> Range<usize> {
                self.span.clone()
            }
        }
    };
}

/// A value together with its byte range in the original source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Range<usize>,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Range<usize>) -> Self {
        Self { value, span }
    }

    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }

    pub fn map<N>(self, f: impl FnOnce(T) -> N) -> Spanned<N> {
        Spanned::new(f(self.value), self.span)
    }

    pub fn as_ref(&self) -> Spanned<&T> {
        Spanned::new(&self.value, self.span.clone())
    }
}

impl<T: Debug> AST for Spanned<T> {
    fn span(&self) -> Range<usize> {
        self.span.clone()
    }
}

/// A zero-copy string slice into the original source.
pub type SpannedStr<'input> = Spanned<&'input str>;
pub type Literal<'input> = SpannedStr<'input>;
pub type NumericLiteral<'input> = SpannedStr<'input>;
pub type StringLiteral<'input> = SpannedStr<'input>;

#[extension_fn(<'input> Token<'input>)]
pub fn into_literal(self) -> Literal<'input> {
    Literal::new(self.text, self.span)
}

/// The root node of a source file.
pub type Source<'input, 'allocator> = Statements<'input, 'allocator>;

#[derive(Debug)]
pub struct Statements<'input, 'allocator> {
    pub statements: &'allocator [Statement<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for Statements<'input, 'allocator>);

#[derive(Debug)]
pub enum Statement<'input, 'allocator> {
    ImportStatement(&'allocator ImportStatement<'input, 'allocator>),
    EnumDefine(&'allocator EnumDefine<'input, 'allocator>),
    TypeDefine(&'allocator TypeDefine<'input, 'allocator>),
    InterfaceDefine(&'allocator InterfaceDefine<'input, 'allocator>),
    ImplementsDefine(&'allocator ImplementsDefine<'input, 'allocator>),
    UseDeclare(&'allocator UseDeclare<'input, 'allocator>),
    LetStatement(&'allocator LetStatement<'input, 'allocator>),
    AssertStatement(&'allocator AssertStatement<'input, 'allocator>),
    AssignStatement(&'allocator AssignStatement<'input, 'allocator>),
    FunctionDefine(&'allocator FunctionDefine<'input, 'allocator>),
    Expression(&'allocator Expression<'input, 'allocator>),
}

impl AST for Statement<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::ImportStatement(node) => node.span(),
            Self::EnumDefine(node) => node.span(),
            Self::TypeDefine(node) => node.span(),
            Self::InterfaceDefine(node) => node.span(),
            Self::ImplementsDefine(node) => node.span(),
            Self::UseDeclare(node) => node.span(),
            Self::LetStatement(node) => node.span(),
            Self::AssertStatement(node) => node.span(),
            Self::AssignStatement(node) => node.span(),
            Self::FunctionDefine(node) => node.span(),
            Self::Expression(node) => node.span(),
        }
    }
}

#[derive(Debug)]
pub struct ImportStatement<'input, 'allocator> {
    pub kind: ImportKind<'input, 'allocator>,
    pub path: StringLiteral<'input>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for ImportStatement<'input, 'allocator>);

#[derive(Debug)]
pub enum ImportKind<'input, 'allocator> {
    SideEffect,
    Named(&'allocator [ImportElement<'input>]),
    Namespace { alias: Literal<'input> },
}

#[derive(Debug)]
pub struct ImportElement<'input> {
    pub name: Literal<'input>,
    pub alias: Option<Literal<'input>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input> for ImportElement<'input>);

#[derive(Debug)]
pub struct EnumDefine<'input, 'allocator> {
    pub exported: bool,
    pub name: Literal<'input>,
    pub representation_type: Option<&'allocator TypeInfo<'input, 'allocator>>,
    pub variants: &'allocator [EnumVariant<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for EnumDefine<'input, 'allocator>);

#[derive(Debug)]
pub struct EnumVariant<'input, 'allocator> {
    pub name: Literal<'input>,
    pub value: Option<&'allocator Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for EnumVariant<'input, 'allocator>);

#[derive(Debug)]
pub struct TypeDefine<'input, 'allocator> {
    pub exported: bool,
    pub name: Literal<'input>,
    pub body: &'allocator TypedefBlock<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for TypeDefine<'input, 'allocator>);

#[derive(Debug)]
pub struct InterfaceDefine<'input, 'allocator> {
    pub exported: bool,
    pub name: Literal<'input>,
    pub generic_parameters: Option<&'allocator GenericParameters<'input, 'allocator>>,
    pub where_clause: Option<&'allocator WhereClause<'input, 'allocator>>,
    pub methods: &'allocator [&'allocator FunctionDefine<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for InterfaceDefine<'input, 'allocator>);

#[derive(Debug)]
pub struct ImplementsDefine<'input, 'allocator> {
    pub generic_parameters: Option<&'allocator GenericParameters<'input, 'allocator>>,
    pub interface: &'allocator TypeInfo<'input, 'allocator>,
    pub target: &'allocator TypeInfo<'input, 'allocator>,
    pub where_clause: Option<&'allocator WhereClause<'input, 'allocator>>,
    pub methods: &'allocator [&'allocator FunctionDefine<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for ImplementsDefine<'input, 'allocator>);

#[derive(Debug)]
pub struct TypedefBlock<'input, 'allocator> {
    pub fields: &'allocator [Typedef<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for TypedefBlock<'input, 'allocator>);

#[derive(Debug)]
pub struct Typedef<'input, 'allocator> {
    pub policy: Option<MutationPolicy>,
    pub name: Literal<'input>,
    pub value: TypedefValue<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for Typedef<'input, 'allocator>);

#[derive(Debug)]
pub enum TypedefValue<'input, 'allocator> {
    Block(&'allocator TypedefBlock<'input, 'allocator>),
    TypeInfo(&'allocator TypeInfo<'input, 'allocator>),
}

impl AST for TypedefValue<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::Block(node) => node.span(),
            Self::TypeInfo(node) => node.span(),
        }
    }
}

#[derive(Debug)]
pub struct UseDeclare<'input, 'allocator> {
    pub names: &'allocator [Literal<'input>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for UseDeclare<'input, 'allocator>);

#[derive(Debug)]
pub struct LetStatement<'input, 'allocator> {
    pub exported: bool,
    pub declare: bool,
    pub policy: Option<MutationPolicy>,
    pub name: Literal<'input>,
    pub type_info: Option<&'allocator TypeInfo<'input, 'allocator>>,
    pub value: Option<&'allocator Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for LetStatement<'input, 'allocator>);

#[derive(Debug)]
pub struct AssertStatement<'input, 'allocator> {
    pub condition: &'allocator Expression<'input, 'allocator>,
    pub message: Option<&'allocator Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for AssertStatement<'input, 'allocator>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MutationPolicy {
    pub kind: MutationPolicyKind,
    pub span: Range<usize>,
}

impl AST for MutationPolicy {
    fn span(&self) -> Range<usize> {
        self.span.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MutationPolicyKind {
    Readonly,
    Tunable { cost: Option<Spanned<u64>> },
}

#[derive(Debug)]
pub struct AssignStatement<'input, 'allocator> {
    pub target: &'allocator Expression<'input, 'allocator>,
    pub value: &'allocator Expression<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for AssignStatement<'input, 'allocator>);

#[derive(Debug)]
pub struct FunctionDefine<'input, 'allocator> {
    pub exported: bool,
    pub attribute: Spanned<FunctionAttribute>,
    pub name: Literal<'input>,
    pub generic_parameters: Option<&'allocator GenericParameters<'input, 'allocator>>,
    pub arguments: FunctionArguments<'input, 'allocator>,
    pub return_type: Option<&'allocator TypeInfo<'input, 'allocator>>,
    pub where_clause: Option<&'allocator WhereClause<'input, 'allocator>>,
    pub body: &'allocator Block<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for FunctionDefine<'input, 'allocator>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionAttribute {
    Inline,
    Opaque,
}

#[derive(Debug)]
pub struct FunctionArguments<'input, 'allocator> {
    pub receiver: Option<Literal<'input>>,
    pub arguments: &'allocator [FunctionArgument<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for FunctionArguments<'input, 'allocator>);

#[derive(Debug)]
pub struct FunctionArgument<'input, 'allocator> {
    pub name: Literal<'input>,
    pub type_info: &'allocator TypeInfo<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for FunctionArgument<'input, 'allocator>);

#[derive(Debug)]
pub struct GenericParameters<'input, 'allocator> {
    pub parameters: &'allocator [GenericParameter<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for GenericParameters<'input, 'allocator>);

#[derive(Debug)]
pub struct GenericParameter<'input, 'allocator> {
    pub name: Literal<'input>,
    pub bounds: &'allocator [&'allocator TypeInfo<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for GenericParameter<'input, 'allocator>);

#[derive(Debug)]
pub struct WhereClause<'input, 'allocator> {
    pub predicates: &'allocator [WherePredicate<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for WhereClause<'input, 'allocator>);

#[derive(Debug)]
pub struct WherePredicate<'input, 'allocator> {
    pub ty: &'allocator TypeInfo<'input, 'allocator>,
    pub bounds: &'allocator [&'allocator TypeInfo<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for WherePredicate<'input, 'allocator>);

#[derive(Debug)]
pub struct Block<'input, 'allocator> {
    pub statements: Statements<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for Block<'input, 'allocator>);

/// Expression precedence is resolved by the parser and normalized into this tree.
#[derive(Debug, Clone, Copy)]
pub enum Expression<'input, 'allocator> {
    If(&'allocator IfExpression<'input, 'allocator>),
    Match(&'allocator MatchExpression<'input, 'allocator>),
    Return(&'allocator ReturnExpression<'input, 'allocator>),
    Throw(&'allocator ThrowExpression<'input, 'allocator>),
    Elvis(&'allocator ElvisExpression<'input, 'allocator>),
    Binary(&'allocator BinaryExpression<'input, 'allocator>),
    Unary(&'allocator UnaryExpression<'input, 'allocator>),
    Primary(&'allocator Primary<'input, 'allocator>),
}

impl AST for Expression<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::If(node) => node.span(),
            Self::Match(node) => node.span(),
            Self::Return(node) => node.span(),
            Self::Throw(node) => node.span(),
            Self::Elvis(node) => node.span(),
            Self::Binary(node) => node.span(),
            Self::Unary(node) => node.span(),
            Self::Primary(node) => node.span(),
        }
    }
}

#[derive(Debug)]
pub struct IfExpression<'input, 'allocator> {
    pub branch: &'allocator IfBranch<'input, 'allocator>,
    pub else_branches: &'allocator [ElseBranch<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for IfExpression<'input, 'allocator>);

#[derive(Debug)]
pub struct IfBranch<'input, 'allocator> {
    pub condition: &'allocator Expression<'input, 'allocator>,
    pub body: &'allocator Block<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for IfBranch<'input, 'allocator>);

/// One `else if ...` or `else { ... }` part, including the `else` in its span.
#[derive(Debug)]
pub struct ElseBranch<'input, 'allocator> {
    pub value: ElseBranchValue<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for ElseBranch<'input, 'allocator>);

#[derive(Debug)]
pub enum ElseBranchValue<'input, 'allocator> {
    If(&'allocator IfBranch<'input, 'allocator>),
    Block(&'allocator Block<'input, 'allocator>),
}

impl AST for ElseBranchValue<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::If(node) => node.span(),
            Self::Block(node) => node.span(),
        }
    }
}

#[derive(Debug)]
pub struct MatchExpression<'input, 'allocator> {
    pub value: &'allocator Expression<'input, 'allocator>,
    pub arms: &'allocator [MatchArm<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for MatchExpression<'input, 'allocator>);

#[derive(Debug)]
pub struct MatchArm<'input, 'allocator> {
    pub pattern: Pattern<'input, 'allocator>,
    pub value: &'allocator Expression<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for MatchArm<'input, 'allocator>);

#[derive(Debug)]
pub enum Pattern<'input, 'allocator> {
    Some(&'allocator SomePattern<'input, 'allocator>),
    Null(Spanned<()>),
    Wildcard(Spanned<()>),
    Binding(Literal<'input>),
    EnumVariant(&'allocator EnumVariantPattern<'input>),
}

impl AST for Pattern<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::Some(node) => node.span(),
            Self::Null(node) => node.span(),
            Self::Wildcard(node) => node.span(),
            Self::Binding(node) => node.span(),
            Self::EnumVariant(node) => node.span(),
        }
    }
}

#[derive(Debug)]
pub struct SomePattern<'input, 'allocator> {
    pub pattern: &'allocator Pattern<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for SomePattern<'input, 'allocator>);

#[derive(Debug)]
pub struct EnumVariantPattern<'input> {
    pub enum_name: Literal<'input>,
    pub variant: Literal<'input>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input> for EnumVariantPattern<'input>);

#[derive(Debug)]
pub struct ReturnExpression<'input, 'allocator> {
    pub value: Option<&'allocator Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for ReturnExpression<'input, 'allocator>);

#[derive(Debug)]
pub struct ThrowExpression<'input, 'allocator> {
    pub message: Option<&'allocator Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for ThrowExpression<'input, 'allocator>);

#[derive(Debug)]
pub struct ElvisExpression<'input, 'allocator> {
    pub optional: &'allocator Expression<'input, 'allocator>,
    pub fallback: &'allocator Expression<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for ElvisExpression<'input, 'allocator>);

#[derive(Debug)]
pub struct BinaryExpression<'input, 'allocator> {
    pub left: &'allocator Expression<'input, 'allocator>,
    pub operator: Spanned<BinaryOperator>,
    pub right: &'allocator Expression<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for BinaryExpression<'input, 'allocator>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOperator {
    Or,
    And,
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LessThanOrEqual,
    GreaterThanOrEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Debug)]
pub struct UnaryExpression<'input, 'allocator> {
    pub operator: Spanned<UnaryOperator>,
    pub operand: &'allocator Expression<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for UnaryExpression<'input, 'allocator>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOperator {
    Positive,
    Negate,
}

#[derive(Debug)]
pub struct Primary<'input, 'allocator> {
    pub value: Value<'input, 'allocator>,
    pub accesses: &'allocator [PrimaryAccess<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for Primary<'input, 'allocator>);

#[derive(Debug)]
pub struct PrimaryAccess<'input, 'allocator> {
    pub operator: Spanned<AccessOperator>,
    pub member: Literal<'input>,
    pub type_arguments: Option<&'allocator ExplicitTypeArguments<'input, 'allocator>>,
    pub call: Option<&'allocator FunctionCall<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for PrimaryAccess<'input, 'allocator>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessOperator {
    Dot,
    SafeDot,
    DoubleColon,
}

#[derive(Debug)]
pub enum Value<'input, 'allocator> {
    Array(&'allocator Array<'input, 'allocator>),
    Literal(LiteralValue<'input, 'allocator>),
    Some(&'allocator SomeValue<'input, 'allocator>),
    Numeric(NumericLiteral<'input>),
    String(StringLiteral<'input>),
    Boolean(Spanned<bool>),
    Null(Spanned<()>),
}

impl AST for Value<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::Array(node) => node.span(),
            Self::Literal(node) => node.span(),
            Self::Some(node) => node.span(),
            Self::Numeric(node) => node.span(),
            Self::String(node) => node.span(),
            Self::Boolean(node) => node.span(),
            Self::Null(node) => node.span(),
        }
    }
}

#[derive(Debug)]
pub struct LiteralValue<'input, 'allocator> {
    pub literal: Literal<'input>,
    pub type_arguments: Option<&'allocator ExplicitTypeArguments<'input, 'allocator>>,
    pub call: Option<&'allocator FunctionCall<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for LiteralValue<'input, 'allocator>);

#[derive(Debug)]
pub struct SomeValue<'input, 'allocator> {
    pub value: Option<&'allocator Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for SomeValue<'input, 'allocator>);

#[derive(Debug)]
pub struct Array<'input, 'allocator> {
    pub values: &'allocator [Value<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for Array<'input, 'allocator>);

#[derive(Debug)]
pub struct FunctionCall<'input, 'allocator> {
    pub arguments: &'allocator [Expression<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for FunctionCall<'input, 'allocator>);

#[derive(Debug)]
pub struct ExplicitTypeArguments<'input, 'allocator> {
    pub arguments: &'allocator [ExplicitTypeArgument<'input, 'allocator>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for ExplicitTypeArguments<'input, 'allocator>);

#[derive(Debug)]
pub enum ExplicitTypeArgument<'input, 'allocator> {
    Type(&'allocator TypeInfo<'input, 'allocator>),
    Infer(Spanned<()>),
}

impl AST for ExplicitTypeArgument<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::Type(node) => node.span(),
            Self::Infer(node) => node.span(),
        }
    }
}

#[derive(Debug)]
pub struct TypeInfo<'input, 'allocator> {
    pub name: Literal<'input>,
    pub parameters: &'allocator [&'allocator TypeInfo<'input, 'allocator>],
    pub optional: bool,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for TypeInfo<'input, 'allocator>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_allocated_tree_keeps_source_slices_and_spans() {
        let source = r#"import "./common.lnix""#;
        let arena = AstArena::new();

        let path_start = source.find('"').unwrap();
        let import = arena.alloc(ImportStatement {
            kind: ImportKind::SideEffect,
            path: StringLiteral::new(&source[path_start..], path_start..source.len()),
            span: 0..source.len(),
        });
        let statements = arena.alloc_slice_fill_iter([Statement::ImportStatement(import)]);
        let root = Statements {
            statements,
            span: 0..source.len(),
        };

        assert_eq!(root.span(), 0..source.len());
        assert_eq!(root.statements[0].span(), 0..source.len());
        let Statement::ImportStatement(import) = &root.statements[0] else {
            panic!("expected import statement");
        };
        assert_eq!(import.path.value, "\"./common.lnix\"");
        assert_eq!(&source[import.path.span()], import.path.value);
    }

    #[test]
    fn expression_variants_delegate_their_spans() {
        let arena = AstArena::new();
        let primary = arena.alloc(Primary {
            value: Value::Boolean(Spanned::new(true, 0..4)),
            accesses: &[],
            span: 0..4,
        });
        let expression = Expression::Primary(primary);

        assert_eq!(expression.span(), 0..4);
        assert_eq!(primary.value.span(), 0..4);
    }
}
