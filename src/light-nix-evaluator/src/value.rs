use std::collections::HashMap;

use light_nix_name_resolver::{FieldId, ModuleId, SymbolId, TypeDefId, VariantId};

#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Error,
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Set(Vec<RuntimeValue>),
    Optional(Option<Box<RuntimeValue>>),
    Record(RuntimeRecord),
    Enum(VariantId),
    Function(SymbolId),
    Type(TypeDefId),
    Module(ModuleId),
}

impl PartialEq for RuntimeValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Error, Self::Error) | (Self::Unit, Self::Unit) => true,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Set(left), Self::Set(right)) => {
                left.iter()
                    .all(|value| right.iter().any(|other| value == other))
                    && right
                        .iter()
                        .all(|value| left.iter().any(|other| value == other))
            }
            (Self::Optional(left), Self::Optional(right)) => left == right,
            (Self::Record(left), Self::Record(right)) => left == right,
            (Self::Enum(left), Self::Enum(right)) => left == right,
            (Self::Function(left), Self::Function(right)) => left == right,
            (Self::Type(left), Self::Type(right)) => left == right,
            (Self::Module(left), Self::Module(right)) => left == right,
            _ => false,
        }
    }
}

impl RuntimeValue {
    pub fn record(ty: TypeDefId) -> Self {
        Self::Record(RuntimeRecord {
            ty,
            fields: HashMap::new(),
        })
    }

    pub fn optional(value: Option<Self>) -> Self {
        Self::Optional(value.map(Box::new))
    }

    pub fn with_field(mut self, field: FieldId, value: Self) -> Self {
        if let Self::Record(record) = &mut self {
            record.fields.insert(field, value);
        }
        self
    }

    pub fn field(&self, field: FieldId) -> Option<&Self> {
        let Self::Record(record) = self else {
            return None;
        };
        record.fields.get(&field)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeRecord {
    pub ty: TypeDefId,
    pub fields: HashMap<FieldId, RuntimeValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_compare_without_observing_storage_order() {
        assert_eq!(
            RuntimeValue::Set(vec![RuntimeValue::Int(1), RuntimeValue::Int(2)]),
            RuntimeValue::Set(vec![RuntimeValue::Int(2), RuntimeValue::Int(1)])
        );
        assert_eq!(
            RuntimeValue::Set(vec![RuntimeValue::Int(1), RuntimeValue::Int(1)]),
            RuntimeValue::Set(vec![RuntimeValue::Int(1)])
        );
    }
}
