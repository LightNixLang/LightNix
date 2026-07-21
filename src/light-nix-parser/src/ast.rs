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
    Inputs(&'allocator Inputs<'input, 'allocator>),
    ImportStatement(&'allocator ImportStatement<'input>),
    EnumDefine(&'allocator EnumDefine<'input, 'allocator>),
    TypeDefine(&'allocator TypeDefine<'input, 'allocator>),
    UseDeclare(&'allocator UseDeclare<'input, 'allocator>),
    HostDefine(&'allocator HostDefine<'input, 'allocator>),
    LetStatement(&'allocator LetStatement<'input, 'allocator>),
    AssignStatement(&'allocator AssignStatement<'input, 'allocator>),
    FunctionDefine(&'allocator FunctionDefine<'input, 'allocator>),
    Expression(&'allocator Expression<'input, 'allocator>),
}

impl AST for Statement<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::Inputs(node) => node.span(),
            Self::ImportStatement(node) => node.span(),
            Self::EnumDefine(node) => node.span(),
            Self::TypeDefine(node) => node.span(),
            Self::UseDeclare(node) => node.span(),
            Self::HostDefine(node) => node.span(),
            Self::LetStatement(node) => node.span(),
            Self::AssignStatement(node) => node.span(),
            Self::FunctionDefine(node) => node.span(),
            Self::Expression(node) => node.span(),
        }
    }
}

#[derive(Debug)]
pub struct Inputs<'input, 'allocator> {
    pub elements: &'allocator [InputsElement<'input>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for Inputs<'input, 'allocator>);

#[derive(Debug)]
pub struct InputsElement<'input> {
    pub key: Literal<'input>,
    pub value: StringLiteral<'input>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input> for InputsElement<'input>);

#[derive(Debug)]
pub struct ImportStatement<'input> {
    pub path: StringLiteral<'input>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input> for ImportStatement<'input>);

#[derive(Debug)]
pub struct EnumDefine<'input, 'allocator> {
    pub name: Literal<'input>,
    pub variants: &'allocator [Literal<'input>],
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for EnumDefine<'input, 'allocator>);

#[derive(Debug)]
pub struct TypeDefine<'input, 'allocator> {
    pub name: Literal<'input>,
    pub body: &'allocator TypedefBlock<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for TypeDefine<'input, 'allocator>);

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
pub struct HostDefine<'input, 'allocator> {
    pub host: StringLiteral<'input>,
    pub body: &'allocator Block<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for HostDefine<'input, 'allocator>);

#[derive(Debug)]
pub struct LetStatement<'input, 'allocator> {
    pub declare: bool,
    pub policy: Option<MutationPolicy>,
    pub name: Literal<'input>,
    pub type_info: Option<&'allocator TypeInfo<'input, 'allocator>>,
    pub value: Option<&'allocator Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for LetStatement<'input, 'allocator>);

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
    pub attribute: Spanned<FunctionAttribute>,
    pub name: Literal<'input>,
    pub arguments: FunctionArguments<'input, 'allocator>,
    pub return_type: Option<&'allocator TypeInfo<'input, 'allocator>>,
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
pub struct Block<'input, 'allocator> {
    pub statements: Statements<'input, 'allocator>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for Block<'input, 'allocator>);

/// Expression precedence is resolved by the parser and normalized into this tree.
#[derive(Debug, Clone, Copy)]
pub enum Expression<'input, 'allocator> {
    If(&'allocator IfExpression<'input, 'allocator>),
    Return(&'allocator ReturnExpression<'input, 'allocator>),
    Binary(&'allocator BinaryExpression<'input, 'allocator>),
    Unary(&'allocator UnaryExpression<'input, 'allocator>),
    Primary(&'allocator Primary<'input, 'allocator>),
}

impl AST for Expression<'_, '_> {
    fn span(&self) -> Range<usize> {
        match self {
            Self::If(node) => node.span(),
            Self::Return(node) => node.span(),
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
pub struct ReturnExpression<'input, 'allocator> {
    pub value: Option<&'allocator Expression<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for ReturnExpression<'input, 'allocator>);

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
    pub call: Option<&'allocator FunctionCall<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for PrimaryAccess<'input, 'allocator>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessOperator {
    Dot,
    DoubleColon,
}

#[derive(Debug)]
pub enum Value<'input, 'allocator> {
    Array(&'allocator Array<'input, 'allocator>),
    Literal(LiteralValue<'input, 'allocator>),
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
    pub call: Option<&'allocator FunctionCall<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for LiteralValue<'input, 'allocator>);

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
pub struct TypeInfo<'input, 'allocator> {
    pub name: Literal<'input>,
    pub parameter: Option<&'allocator TypeInfo<'input, 'allocator>>,
    pub span: Range<usize>,
}

impl_ast!(impl<'input, 'allocator> for TypeInfo<'input, 'allocator>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_allocated_tree_keeps_source_slices_and_spans() {
        let source = "inputs { nixpkgs = \"github:NixOS/nixpkgs\" }";
        let arena = AstArena::new();

        let elements = arena.alloc_slice_fill_iter([InputsElement {
            key: Literal::new(&source[9..16], 9..16),
            value: StringLiteral::new(&source[19..41], 19..41),
            span: 9..41,
        }]);
        let inputs = arena.alloc(Inputs {
            elements,
            span: 0..source.len(),
        });
        let statements = arena.alloc_slice_fill_iter([Statement::Inputs(inputs)]);
        let root = Statements {
            statements,
            span: 0..source.len(),
        };

        assert_eq!(root.span(), 0..source.len());
        assert_eq!(root.statements[0].span(), 0..source.len());
        let Statement::Inputs(inputs) = &root.statements[0] else {
            panic!("expected inputs statement");
        };
        assert_eq!(inputs.elements[0].key.value, "nixpkgs");
        assert_eq!(inputs.elements[0].value.value, "\"github:NixOS/nixpkgs\"");
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
