use crate::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinReceiver {
    Set,
    List,
    Int,
    Float,
    Bool,
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinMethod {
    Contains,
    Filter,
    Map,
    ToFloat,
    TryToInt,
    ToString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BuiltinMethodDefinition {
    pub receiver: BuiltinReceiver,
    pub name: &'static str,
    pub method: BuiltinMethod,
}

pub const BUILTIN_METHODS: &[BuiltinMethodDefinition] = &[
    method(BuiltinReceiver::Set, "contains", BuiltinMethod::Contains),
    method(BuiltinReceiver::Set, "filter", BuiltinMethod::Filter),
    method(BuiltinReceiver::Set, "map", BuiltinMethod::Map),
    method(BuiltinReceiver::List, "contains", BuiltinMethod::Contains),
    method(BuiltinReceiver::List, "filter", BuiltinMethod::Filter),
    method(BuiltinReceiver::List, "map", BuiltinMethod::Map),
    method(BuiltinReceiver::Int, "to_float", BuiltinMethod::ToFloat),
    method(BuiltinReceiver::Int, "to_string", BuiltinMethod::ToString),
    method(
        BuiltinReceiver::Float,
        "try_to_int",
        BuiltinMethod::TryToInt,
    ),
    method(BuiltinReceiver::Float, "to_string", BuiltinMethod::ToString),
    method(BuiltinReceiver::Bool, "to_string", BuiltinMethod::ToString),
    method(
        BuiltinReceiver::String,
        "to_string",
        BuiltinMethod::ToString,
    ),
];

const fn method(
    receiver: BuiltinReceiver,
    name: &'static str,
    method: BuiltinMethod,
) -> BuiltinMethodDefinition {
    BuiltinMethodDefinition {
        receiver,
        name,
        method,
    }
}

pub(crate) fn find_builtin_method(receiver: &Type, name: &str) -> Option<BuiltinMethod> {
    let receiver = match receiver {
        Type::Set(_) => BuiltinReceiver::Set,
        Type::List(_) => BuiltinReceiver::List,
        Type::Int => BuiltinReceiver::Int,
        Type::Float => BuiltinReceiver::Float,
        Type::Bool => BuiltinReceiver::Bool,
        Type::String => BuiltinReceiver::String,
        _ => return None,
    };
    BUILTIN_METHODS
        .iter()
        .find(|definition| definition.receiver == receiver && definition.name == name)
        .map(|definition| definition.method)
}
