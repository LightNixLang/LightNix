use std::fmt;

use light_nix_name_resolver::{GenericParameterId, TypeDefId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeVariableId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Error,
    Never,
    Unit,
    Bool,
    Int,
    Float,
    String,
    Set(Box<Type>),
    List(Box<Type>),
    AttrSet(Box<Type>),
    Optional(Box<Type>),
    Named(TypeDefId, Vec<Type>),
    Function(FunctionType),
    Parameter(GenericParameterId),
    Variable(TypeVariableId),
}

impl Type {
    pub fn optional(inner: Type) -> Self {
        match inner {
            Self::Optional(_) => inner,
            _ => Self::Optional(Box::new(inner)),
        }
    }

    pub fn function(parameters: Vec<Type>, return_type: Type) -> Self {
        Self::Function(FunctionType {
            parameters,
            return_type: Box::new(return_type),
        })
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionType {
    pub parameters: Vec<Type>,
    pub return_type: Box<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceBound {
    pub subject: Type,
    pub interface: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeScheme {
    pub parameters: Vec<GenericParameterId>,
    pub ty: Type,
    pub bounds: Vec<InterfaceBound>,
}

impl TypeScheme {
    pub fn monomorphic(ty: Type) -> Self {
        Self {
            parameters: Vec::new(),
            ty,
            bounds: Vec::new(),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => formatter.write_str("<error>"),
            Self::Never => formatter.write_str("Never"),
            Self::Unit => formatter.write_str("Unit"),
            Self::Bool => formatter.write_str("Bool"),
            Self::Int => formatter.write_str("Int"),
            Self::Float => formatter.write_str("Float"),
            Self::String => formatter.write_str("String"),
            Self::Set(element) => write!(formatter, "Set<{element}>"),
            Self::List(element) => write!(formatter, "List<{element}>"),
            Self::AttrSet(element) => write!(formatter, "AttrSet<{element}>"),
            Self::Optional(inner) => write!(formatter, "{inner}?"),
            Self::Named(id, parameters) => {
                write!(formatter, "type#{}:{}", id.module.0, id.index)?;
                format_parameters(formatter, parameters)
            }
            Self::Function(function) => {
                formatter.write_str("function(")?;
                for (index, parameter) in function.parameters.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{parameter}")?;
                }
                write!(formatter, ") -> {}", function.return_type)
            }
            Self::Parameter(id) => write!(formatter, "parameter#{}:{}", id.module.0, id.index),
            Self::Variable(id) => write!(formatter, "?{}", id.0),
        }
    }
}

fn format_parameters(formatter: &mut fmt::Formatter<'_>, parameters: &[Type]) -> fmt::Result {
    if parameters.is_empty() {
        return Ok(());
    }
    formatter.write_str("<")?;
    for (index, parameter) in parameters.iter().enumerate() {
        if index != 0 {
            formatter.write_str(", ")?;
        }
        write!(formatter, "{parameter}")?;
    }
    formatter.write_str(">")
}
