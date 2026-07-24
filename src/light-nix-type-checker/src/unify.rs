use crate::{Type, TypeCheckErrorKind, TypeVariableId};

#[derive(Debug, Clone)]
struct VariableNode {
    parent: TypeVariableId,
    rank: u8,
    binding: Option<Type>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Unifier {
    variables: Vec<VariableNode>,
}

impl Unifier {
    pub fn fresh(&mut self) -> Type {
        let id = TypeVariableId(
            u32::try_from(self.variables.len()).expect("type variable table exceeded u32::MAX"),
        );
        self.variables.push(VariableNode {
            parent: id,
            rank: 0,
            binding: None,
        });
        Type::Variable(id)
    }

    pub fn resolve(&mut self, ty: &Type) -> Type {
        let resolved = match ty {
            Type::Variable(variable) => {
                let root = self.find(*variable);
                match self.variables[root.0 as usize].binding.clone() {
                    Some(binding) => self.resolve(&binding),
                    None => Type::Variable(root),
                }
            }
            Type::Set(element) => Type::Set(Box::new(self.resolve(element))),
            Type::List(element) => Type::List(Box::new(self.resolve(element))),
            Type::AttrSet(element) => Type::AttrSet(Box::new(self.resolve(element))),
            Type::Optional(inner) => Type::optional(self.resolve(inner)),
            Type::Named(id, parameters) => {
                Type::Named(*id, parameters.iter().map(|ty| self.resolve(ty)).collect())
            }
            Type::Function(function) => Type::function(
                function
                    .parameters
                    .iter()
                    .map(|ty| self.resolve(ty))
                    .collect(),
                self.resolve(&function.return_type),
            ),
            _ => ty.clone(),
        };
        if let Type::Variable(variable) = ty {
            let root = self.find(*variable);
            if !matches!(resolved, Type::Variable(id) if id == root) {
                self.variables[root.0 as usize].binding = Some(resolved.clone());
            }
        }
        resolved
    }

    pub fn unify(&mut self, left: &Type, right: &Type) -> Result<Type, TypeCheckErrorKind> {
        let left = self.resolve(left);
        let right = self.resolve(right);
        if left == right {
            return Ok(left);
        }
        if left.is_error() || right.is_error() {
            return Ok(Type::Error);
        }
        if matches!(left, Type::Never) {
            return Ok(right);
        }
        if matches!(right, Type::Never) {
            return Ok(left);
        }
        match (&left, &right) {
            (Type::Variable(variable), _) => self.bind(*variable, &right),
            (_, Type::Variable(variable)) => self.bind(*variable, &left),
            (Type::Set(left), Type::Set(right)) => {
                Ok(Type::Set(Box::new(self.unify(left, right)?)))
            }
            (Type::List(left), Type::List(right)) => {
                Ok(Type::List(Box::new(self.unify(left, right)?)))
            }
            (Type::AttrSet(left), Type::AttrSet(right)) => {
                Ok(Type::AttrSet(Box::new(self.unify(left, right)?)))
            }
            (Type::Optional(left), Type::Optional(right)) => {
                Ok(Type::optional(self.unify(left, right)?))
            }
            (Type::Named(left_id, left_args), Type::Named(right_id, right_args))
                if left_id == right_id && left_args.len() == right_args.len() =>
            {
                let parameters = left_args
                    .iter()
                    .zip(right_args)
                    .map(|(left, right)| self.unify(left, right))
                    .collect::<Result<_, _>>()?;
                Ok(Type::Named(*left_id, parameters))
            }
            (Type::Function(left), Type::Function(right))
                if left.parameters.len() == right.parameters.len() =>
            {
                let parameters = left
                    .parameters
                    .iter()
                    .zip(&right.parameters)
                    .map(|(left, right)| self.unify(left, right))
                    .collect::<Result<_, _>>()?;
                let return_type = self.unify(&left.return_type, &right.return_type)?;
                Ok(Type::function(parameters, return_type))
            }
            _ => Err(TypeCheckErrorKind::TypeMismatch {
                expected: left,
                found: right,
            }),
        }
    }

    fn bind(&mut self, variable: TypeVariableId, ty: &Type) -> Result<Type, TypeCheckErrorKind> {
        let root = self.find(variable);
        let ty = self.resolve(ty);
        if ty == Type::Variable(root) {
            return Ok(ty);
        }
        if self.occurs(root, &ty) {
            return Err(TypeCheckErrorKind::InfiniteType {
                variable: Type::Variable(root),
                contains: ty,
            });
        }
        if let Type::Variable(other) = ty {
            let other = self.find(other);
            if root == other {
                return Ok(Type::Variable(root));
            }
            let root_rank = self.variables[root.0 as usize].rank;
            let other_rank = self.variables[other.0 as usize].rank;
            if root_rank < other_rank {
                self.variables[root.0 as usize].parent = other;
                return Ok(Type::Variable(other));
            }
            self.variables[other.0 as usize].parent = root;
            if root_rank == other_rank {
                self.variables[root.0 as usize].rank += 1;
            }
            return Ok(Type::Variable(root));
        }
        self.variables[root.0 as usize].binding = Some(ty.clone());
        Ok(ty)
    }

    fn occurs(&mut self, needle: TypeVariableId, ty: &Type) -> bool {
        match self.resolve(ty) {
            Type::Variable(variable) => self.find(variable) == needle,
            Type::Set(element)
            | Type::List(element)
            | Type::AttrSet(element)
            | Type::Optional(element) => self.occurs(needle, &element),
            Type::Named(_, parameters) => parameters.iter().any(|ty| self.occurs(needle, ty)),
            Type::Function(function) => {
                function.parameters.iter().any(|ty| self.occurs(needle, ty))
                    || self.occurs(needle, &function.return_type)
            }
            _ => false,
        }
    }

    fn find(&mut self, variable: TypeVariableId) -> TypeVariableId {
        let parent = self.variables[variable.0 as usize].parent;
        if parent == variable {
            return variable;
        }
        let root = self.find(parent);
        self.variables[variable.0 as usize].parent = root;
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurs_check_rejects_an_infinite_type() {
        let mut unifier = Unifier::default();
        let variable = unifier.fresh();
        let recursive = Type::Set(Box::new(variable.clone()));

        assert!(matches!(
            unifier.unify(&variable, &recursive),
            Err(TypeCheckErrorKind::InfiniteType { .. })
        ));
    }

    #[test]
    fn optional_constructor_flattens_nested_options() {
        assert_eq!(
            Type::optional(Type::optional(Type::Int)),
            Type::optional(Type::Int)
        );
    }
}
