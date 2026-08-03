use std::fmt;

use light_nix_name_resolver::{GenericParameterId, TypeDefId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeVariableId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Error,
    Never,
    Unit,
    Bool,
    Int,
    Float,
    String,
    Package,
    Set(Box<Type>),
    List(Box<Type>),
    AttrSet(Box<Type>),
    Optional(Box<Type>),
    Union(Vec<Type>),
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

    pub fn union(types: impl IntoIterator<Item = Type>) -> Self {
        let mut alternatives = Vec::new();
        for ty in types {
            match ty {
                Self::Union(nested) => {
                    for ty in nested {
                        if !alternatives.contains(&ty) {
                            alternatives.push(ty);
                        }
                    }
                }
                ty if !alternatives.contains(&ty) => alternatives.push(ty),
                _ => {}
            }
        }
        alternatives.sort();
        match alternatives.len() {
            0 => Self::Never,
            1 => alternatives.pop().unwrap(),
            _ => Self::Union(alternatives),
        }
    }

    /// Whether a value of `found` can be used where `self` is expected.
    pub fn accepts(&self, found: &Type) -> bool {
        if self == found || matches!(found, Self::Never) || self.is_error() || found.is_error() {
            return true;
        }
        if let Self::Union(found_alternatives) = found {
            return found_alternatives
                .iter()
                .all(|alternative| self.accepts(alternative));
        }
        match self {
            Self::Union(alternatives) => alternatives
                .iter()
                .any(|alternative| alternative.accepts(found)),
            _ => false,
        }
    }

    pub fn contains_union_alternative(&self, target: &Type) -> bool {
        match self {
            Self::Union(alternatives) => alternatives.iter().any(|ty| ty == target),
            ty => ty == target,
        }
    }

    pub fn without_union_alternative(&self, target: &Type) -> Self {
        match self {
            Self::Union(alternatives) => {
                Self::union(alternatives.iter().filter(|ty| *ty != target).cloned())
            }
            ty if ty == target => Self::Never,
            ty => ty.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
            Self::Package => formatter.write_str("Package"),
            Self::Set(element) => write!(formatter, "Set<{element}>"),
            Self::List(element) => write!(formatter, "List<{element}>"),
            Self::AttrSet(element) => write!(formatter, "AttrSet<{element}>"),
            Self::Optional(inner) => {
                if matches!(inner.as_ref(), Self::Union(_)) {
                    write!(formatter, "({inner})?")
                } else {
                    write!(formatter, "{inner}?")
                }
            }
            Self::Union(alternatives) => {
                for (index, alternative) in alternatives.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(" | ")?;
                    }
                    write!(formatter, "{alternative}")?;
                }
                Ok(())
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unions_are_flattened_deduplicated_and_order_independent() {
        let left = Type::union([Type::String, Type::union([Type::Int, Type::String])]);
        let right = Type::union([Type::Int, Type::String]);

        assert_eq!(left, right);
        assert!(Type::union([Type::Bool, Type::Int, Type::String]).accepts(&right));
        assert_eq!(right.without_union_alternative(&Type::Int), Type::String);
    }
}
