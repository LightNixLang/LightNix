mod builtin;
mod checker;
pub mod error;
mod types;
mod unify;

pub use builtin::{BUILTIN_METHODS, BuiltinMethod, BuiltinMethodDefinition, BuiltinReceiver};
pub use checker::{
    ImplementationScheme, InterfaceMethodScheme, TypeCheckResult, TypeEnvironment, check_module,
};
pub use error::{TypeCheckError, TypeCheckErrorKind};
pub use types::{FunctionType, InterfaceBound, Type, TypeScheme, TypeVariableId};
