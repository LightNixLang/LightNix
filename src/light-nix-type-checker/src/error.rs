use std::ops::Range;

use light_nix_name_resolver::TypeDefId;

use crate::Type;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCheckError {
    pub kind: TypeCheckErrorKind,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeCheckErrorKind {
    MissingTypeAndValue,
    TypeMismatch {
        expected: Type,
        found: Type,
    },
    InfiniteType {
        variable: Type,
        contains: Type,
    },
    NotCallable {
        ty: Type,
    },
    ArgumentCount {
        expected: usize,
        found: usize,
    },
    TypeArgumentCount {
        expected: usize,
        found: usize,
    },
    TypeArgumentsOnMonomorphicValue,
    InvalidGenericArity {
        expected: usize,
        found: usize,
    },
    UnknownMember {
        receiver: Type,
        member: String,
    },
    AmbiguousMember {
        receiver: Type,
        member: String,
    },
    InvalidStaticMember {
        owner: TypeDefId,
        member: String,
    },
    OptionalAccessRequiresSafeOperator {
        found: Type,
    },
    InvalidAttrSetAccess {
        found: Type,
    },
    ExpectedNumeric {
        found: Type,
    },
    ExpectedBoolean {
        found: Type,
    },
    ReturnOutsideFunction,
    InvalidAssignmentTarget,
    MissingImplementation {
        subject: Type,
        interface: Type,
    },
    AmbiguousImplementation {
        subject: Type,
        interface: Type,
    },
    OverflowEvaluatingBound {
        subject: Type,
        interface: Type,
    },
    ExpectedInterface {
        found: Type,
    },
    MissingInterfaceMethod {
        interface: TypeDefId,
        method: String,
    },
    UnknownInterfaceMethod {
        interface: TypeDefId,
        method: String,
    },
    InterfaceMethodTypeMismatch {
        interface: TypeDefId,
        method: String,
    },
    DuplicateImplementation {
        interface: TypeDefId,
    },
    CannotInferType {
        ty: Type,
    },
}
