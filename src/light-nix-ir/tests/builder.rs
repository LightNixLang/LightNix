use light_nix_ir::{BinaryOperation, BuildErrorKind, Constant, ConstraintKind, ModelBuilder};
use light_nix_name_resolver::ModuleId;
use light_nix_type_checker::Type;

#[test]
fn builder_rejects_ill_typed_operations_and_constraints() {
    let mut builder = ModelBuilder::new(ModuleId(0));
    let boolean = builder
        .constant(Type::Bool, Constant::Bool(true), None)
        .unwrap();
    let integer = builder.constant(Type::Int, Constant::Int(1), None).unwrap();

    assert_eq!(
        builder
            .binary(BinaryOperation::Add, boolean, integer, Type::Int, None,)
            .unwrap_err()
            .kind,
        BuildErrorKind::InvalidOperation
    );
    assert_eq!(
        builder
            .add_constraint(integer, ConstraintKind::Target, None)
            .unwrap_err()
            .kind,
        BuildErrorKind::ExpectedBoolean { found: Type::Int }
    );
}
