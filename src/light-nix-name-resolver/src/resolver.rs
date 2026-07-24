use std::{collections::HashMap, ops::Range};

use light_nix_parser::ast::{
    AccessOperator, Array, Block, ClosureBody, ClosureExpression, ElseBranchValue, EnumDefine,
    ExplicitTypeArgument, Expression, FunctionCall, FunctionDefine, GenericParameters,
    ImplementsDefine, ImportKind, InterfaceDefine, LetStatement, Literal, MatchArm, Pattern,
    Primary, Source, Statement, Statements, TypeDefine, TypeInfo, TypedefBlock, TypedefValue,
    Value, WhereClause,
};

use crate::{
    AstId, AstKind,
    error::{NameResolveError, NameResolveErrorKind},
};

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
    };
}

define_id!(ModuleId);
define_id!(NameId);
define_id!(ScopeId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId {
    pub module: ModuleId,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeDefId {
    pub module: ModuleId,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldId {
    pub module: ModuleId,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VariantId {
    pub module: ModuleId,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GenericParameterId {
    pub module: ModuleId,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    Value,
    Type,
    Module,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinType {
    Bool,
    Int,
    Float,
    String,
    List,
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Res {
    Symbol(SymbolId),
    Type(TypeDefId),
    Field(FieldId),
    EnumVariant(VariantId),
    Module(ModuleId),
    BuiltinType(BuiltinType),
    GenericParameter(GenericParameterId),
    /// A field, method, or compiler-provided static member resolved after typing.
    Member(NameId),
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Declaration {
    Symbol(SymbolId),
    Type(TypeDefId),
    Field(FieldId),
    EnumVariant(VariantId),
    GenericParameter(GenericParameterId),
    Module(ModuleId),
    Import {
        module: ModuleId,
        value: Option<SymbolId>,
        ty: Option<TypeDefId>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Let,
    Function,
    Parameter,
    PatternBinding,
}

#[derive(Debug)]
pub struct Symbol<'ast> {
    pub id: SymbolId,
    pub name: NameId,
    pub kind: SymbolKind,
    pub declaration: AstId<'ast>,
    pub span: Range<usize>,
    pub exported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeDefKind {
    Record,
    Enum,
    Interface,
}

#[derive(Debug)]
pub struct TypeDef<'ast> {
    pub id: TypeDefId,
    pub name: Option<NameId>,
    pub kind: TypeDefKind,
    pub declaration: AstId<'ast>,
    pub span: Range<usize>,
    pub fields: Vec<FieldId>,
    pub variants: Vec<VariantId>,
    pub exported: bool,
}

#[derive(Debug)]
pub struct Field<'ast> {
    pub id: FieldId,
    pub owner: TypeDefId,
    pub name: NameId,
    pub declaration: AstId<'ast>,
    pub span: Range<usize>,
    pub nested_type: Option<TypeDefId>,
}

#[derive(Debug)]
pub struct Variant<'ast> {
    pub id: VariantId,
    pub owner: TypeDefId,
    pub name: NameId,
    pub declaration: AstId<'ast>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct GenericParameter<'ast> {
    pub id: GenericParameterId,
    pub name: NameId,
    pub declaration: AstId<'ast>,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeKind {
    Module,
    Block,
    TypeDefinition,
    Function,
    Closure,
    MatchArm,
    Interface,
    Implements,
}

#[derive(Debug, Clone)]
struct Binding {
    resolution: Res,
    span: Option<Range<usize>>,
}

#[derive(Debug)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    values: HashMap<NameId, Binding>,
    types: HashMap<NameId, Binding>,
    modules: HashMap<NameId, Binding>,
}

impl Scope {
    pub fn get(&self, namespace: Namespace, name: NameId) -> Option<Res> {
        self.bindings(namespace)
            .get(&name)
            .map(|binding| binding.resolution)
    }

    fn bindings(&self, namespace: Namespace) -> &HashMap<NameId, Binding> {
        match namespace {
            Namespace::Value => &self.values,
            Namespace::Type => &self.types,
            Namespace::Module => &self.modules,
        }
    }

    fn bindings_mut(&mut self, namespace: Namespace) -> &mut HashMap<NameId, Binding> {
        match namespace {
            Namespace::Value => &mut self.values,
            Namespace::Type => &mut self.types,
            Namespace::Module => &mut self.modules,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportedBinding {
    pub value: Option<SymbolId>,
    pub ty: Option<TypeDefId>,
}

#[derive(Debug, Clone)]
pub struct ModuleInterface {
    pub module: ModuleId,
    exports: HashMap<String, ExportedBinding>,
}

impl ModuleInterface {
    pub fn get(&self, name: &str) -> Option<ExportedBinding> {
        self.exports.get(name).copied()
    }

    pub fn exports(&self) -> impl Iterator<Item = (&str, ExportedBinding)> {
        self.exports
            .iter()
            .map(|(name, binding)| (name.as_str(), *binding))
    }
}

#[derive(Debug, Default)]
pub struct ImportEnvironment {
    by_path: HashMap<String, ModuleInterface>,
    by_id: HashMap<ModuleId, ModuleInterface>,
}

impl ImportEnvironment {
    /// `path` is the source spelling of the string literal, including quotes.
    pub fn insert(&mut self, path: impl Into<String>, interface: ModuleInterface) {
        self.by_id.insert(interface.module, interface.clone());
        self.by_path.insert(path.into(), interface);
    }

    pub fn get_by_path(&self, path: &str) -> Option<&ModuleInterface> {
        self.by_path.get(path)
    }

    pub fn get_by_id(&self, module: ModuleId) -> Option<&ModuleInterface> {
        self.by_id.get(&module)
    }
}

#[derive(Debug, Default)]
struct NameInterner {
    ids: HashMap<String, NameId>,
    names: Vec<String>,
}

impl NameInterner {
    fn intern(&mut self, name: &str) -> NameId {
        if let Some(id) = self.ids.get(name) {
            return *id;
        }

        let id = NameId(index_u32(self.names.len()));
        let name = name.to_owned();
        self.ids.insert(name.clone(), id);
        self.names.push(name);
        id
    }

    fn get(&self, id: NameId) -> &str {
        &self.names[id.0 as usize]
    }
}

pub struct CollectedModule<'ast, 'input, 'allocator> {
    source: &'ast Source<'input, 'allocator>,
    module: ModuleId,
    names: NameInterner,
    symbols: Vec<Symbol<'ast>>,
    types: Vec<TypeDef<'ast>>,
    fields: Vec<Field<'ast>>,
    variants: Vec<Variant<'ast>>,
    generic_parameters: Vec<GenericParameter<'ast>>,
    declarations: HashMap<AstId<'ast>, Declaration>,
    references: HashMap<AstId<'ast>, Res>,
    scopes: Vec<Scope>,
    root_scope: ScopeId,
    errors: Vec<NameResolveError>,
    interface: ModuleInterface,
}

impl<'ast, 'input, 'allocator> CollectedModule<'ast, 'input, 'allocator> {
    pub fn module(&self) -> ModuleId {
        self.module
    }

    pub fn interface(&self) -> &ModuleInterface {
        &self.interface
    }

    pub fn errors(&self) -> &[NameResolveError] {
        &self.errors
    }

    pub fn resolve(mut self, imports: &ImportEnvironment) -> NameResolution<'ast> {
        self.resolve_imports(imports);
        self.resolve_statements(self.source, self.root_scope, true, imports);

        NameResolution {
            module: self.module,
            names: self.names,
            symbols: self.symbols,
            types: self.types,
            fields: self.fields,
            variants: self.variants,
            generic_parameters: self.generic_parameters,
            declarations: self.declarations,
            references: self.references,
            scopes: self.scopes,
            root_scope: self.root_scope,
            errors: self.errors,
            interface: self.interface,
        }
    }
}

pub struct NameResolution<'ast> {
    module: ModuleId,
    names: NameInterner,
    symbols: Vec<Symbol<'ast>>,
    types: Vec<TypeDef<'ast>>,
    fields: Vec<Field<'ast>>,
    variants: Vec<Variant<'ast>>,
    generic_parameters: Vec<GenericParameter<'ast>>,
    declarations: HashMap<AstId<'ast>, Declaration>,
    references: HashMap<AstId<'ast>, Res>,
    scopes: Vec<Scope>,
    root_scope: ScopeId,
    errors: Vec<NameResolveError>,
    interface: ModuleInterface,
}

impl<'ast> NameResolution<'ast> {
    pub fn module(&self) -> ModuleId {
        self.module
    }

    pub fn name(&self, id: NameId) -> &str {
        self.names.get(id)
    }

    pub fn resolve_root_name(&self, namespace: Namespace, name: &str) -> Option<Res> {
        let name = self.names.ids.get(name)?;
        self.scopes[self.root_scope.0 as usize].get(namespace, *name)
    }

    pub fn symbols(&self) -> &[Symbol<'ast>] {
        &self.symbols
    }

    pub fn types(&self) -> &[TypeDef<'ast>] {
        &self.types
    }

    pub fn fields(&self) -> &[Field<'ast>] {
        &self.fields
    }

    pub fn variants(&self) -> &[Variant<'ast>] {
        &self.variants
    }

    pub fn generic_parameters(&self) -> &[GenericParameter<'ast>] {
        &self.generic_parameters
    }

    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    pub fn root_scope(&self) -> ScopeId {
        self.root_scope
    }

    pub fn errors(&self) -> &[NameResolveError] {
        &self.errors
    }

    pub fn interface(&self) -> &ModuleInterface {
        &self.interface
    }

    pub fn resolve_literal<'input>(&self, literal: &'ast Literal<'input>) -> Option<Res> {
        self.references
            .get(&AstId::new(literal, AstKind::Literal))
            .copied()
    }

    pub fn declaration_of_literal<'input>(
        &self,
        literal: &'ast Literal<'input>,
    ) -> Option<Declaration> {
        self.declarations
            .get(&AstId::new(literal, AstKind::Literal))
            .copied()
    }
}

pub fn collect_module<'ast, 'input, 'allocator>(
    source: &'ast Source<'input, 'allocator>,
    module: ModuleId,
) -> CollectedModule<'ast, 'input, 'allocator> {
    let root_scope = ScopeId(0);
    let mut collected = CollectedModule {
        source,
        module,
        names: NameInterner::default(),
        symbols: Vec::new(),
        types: Vec::new(),
        fields: Vec::new(),
        variants: Vec::new(),
        generic_parameters: Vec::new(),
        declarations: HashMap::new(),
        references: HashMap::new(),
        scopes: vec![Scope {
            id: root_scope,
            parent: None,
            kind: ScopeKind::Module,
            values: HashMap::new(),
            types: HashMap::new(),
            modules: HashMap::new(),
        }],
        root_scope,
        errors: Vec::new(),
        interface: ModuleInterface {
            module,
            exports: HashMap::new(),
        },
    };

    collected.define_builtin_types();
    collected.collect_module_headers(source);
    collected.build_interface();
    collected
}

fn index_u32(index: usize) -> u32 {
    u32::try_from(index).expect("name resolver table exceeded u32::MAX entries")
}

impl<'ast, 'input, 'allocator> CollectedModule<'ast, 'input, 'allocator> {
    fn define_builtin_types(&mut self) {
        for (name, builtin) in [
            ("Bool", BuiltinType::Bool),
            ("bool", BuiltinType::Bool),
            ("Int", BuiltinType::Int),
            ("int", BuiltinType::Int),
            ("Float", BuiltinType::Float),
            ("float", BuiltinType::Float),
            ("String", BuiltinType::String),
            ("string", BuiltinType::String),
            ("List", BuiltinType::List),
            ("Set", BuiltinType::Set),
        ] {
            let name = self.names.intern(name);
            self.define(
                self.root_scope,
                Namespace::Type,
                name,
                Res::BuiltinType(builtin),
                None,
                0..0,
            );
        }
    }

    fn collect_module_headers(&mut self, statements: &'ast Statements<'input, 'allocator>) {
        for statement in statements.statements {
            match statement {
                Statement::EnumDefine(node) => {
                    self.collect_enum(node, self.root_scope);
                }
                Statement::TypeDefine(node) => {
                    self.collect_named_record(node, self.root_scope);
                }
                Statement::InterfaceDefine(node) => {
                    self.collect_interface(node, self.root_scope);
                }
                Statement::LetStatement(node) => {
                    self.collect_let(node, self.root_scope);
                }
                Statement::FunctionDefine(node) => {
                    self.collect_function(node, self.root_scope);
                }
                _ => {}
            }
        }
    }

    fn collect_block_headers(
        &mut self,
        statements: &'ast Statements<'input, 'allocator>,
        scope: ScopeId,
    ) {
        for statement in statements.statements {
            match statement {
                Statement::EnumDefine(node) => {
                    self.collect_enum(node, scope);
                }
                Statement::TypeDefine(node) => {
                    self.collect_named_record(node, scope);
                }
                Statement::InterfaceDefine(node) => {
                    self.collect_interface(node, scope);
                }
                Statement::FunctionDefine(node) => {
                    self.collect_function(node, scope);
                }
                _ => {}
            }
        }
    }

    fn collect_let(&mut self, node: &'ast LetStatement<'input, 'allocator>, scope: ScopeId) {
        let name = self.names.intern(node.name.value);
        let id = self.allocate_symbol(
            name,
            SymbolKind::Let,
            &node.name,
            node.span.clone(),
            node.exported,
        );
        self.define(
            scope,
            Namespace::Value,
            name,
            Res::Symbol(id),
            Some(node.name.span.clone()),
            node.name.span.clone(),
        );
    }

    fn collect_function(&mut self, node: &'ast FunctionDefine<'input, 'allocator>, scope: ScopeId) {
        let name = self.names.intern(node.name.value);
        let id = self.allocate_symbol(
            name,
            SymbolKind::Function,
            &node.name,
            node.span.clone(),
            node.exported,
        );
        self.define(
            scope,
            Namespace::Value,
            name,
            Res::Symbol(id),
            Some(node.name.span.clone()),
            node.name.span.clone(),
        );
    }

    fn collect_enum(&mut self, node: &'ast EnumDefine<'input, 'allocator>, scope: ScopeId) {
        let name = self.names.intern(node.name.value);
        let id = self.allocate_type(
            Some(name),
            TypeDefKind::Enum,
            AstId::new(&node.name, AstKind::Literal),
            node.span.clone(),
            node.exported,
        );
        self.define(
            scope,
            Namespace::Type,
            name,
            Res::Type(id),
            Some(node.name.span.clone()),
            node.name.span.clone(),
        );

        let mut known = HashMap::<NameId, Range<usize>>::new();
        let mut variants = Vec::with_capacity(node.variants.len());
        for variant in node.variants {
            let variant_name = self.names.intern(variant.name.value);
            if let Some(first) = known.get(&variant_name) {
                self.errors.push(NameResolveError {
                    kind: NameResolveErrorKind::DuplicateVariant {
                        name: variant_name,
                        first: first.clone(),
                    },
                    span: variant.name.span.clone(),
                });
            } else {
                known.insert(variant_name, variant.name.span.clone());
            }

            let variant_id = VariantId {
                module: self.module,
                index: index_u32(self.variants.len()),
            };
            self.declarations.insert(
                AstId::new(&variant.name, AstKind::Literal),
                Declaration::EnumVariant(variant_id),
            );
            self.variants.push(Variant {
                id: variant_id,
                owner: id,
                name: variant_name,
                declaration: AstId::new(&variant.name, AstKind::Literal),
                span: variant.span.clone(),
            });
            variants.push(variant_id);
        }
        self.type_mut(id).variants = variants;
    }

    fn collect_named_record(&mut self, node: &'ast TypeDefine<'input, 'allocator>, scope: ScopeId) {
        let name = self.names.intern(node.name.value);
        let id = self.allocate_type(
            Some(name),
            TypeDefKind::Record,
            AstId::new(&node.name, AstKind::Literal),
            node.span.clone(),
            node.exported,
        );
        self.define(
            scope,
            Namespace::Type,
            name,
            Res::Type(id),
            Some(node.name.span.clone()),
            node.name.span.clone(),
        );
        self.collect_record_fields(node.body, id);
    }

    fn collect_interface(
        &mut self,
        node: &'ast InterfaceDefine<'input, 'allocator>,
        scope: ScopeId,
    ) {
        let name = self.names.intern(node.name.value);
        let id = self.allocate_type(
            Some(name),
            TypeDefKind::Interface,
            AstId::new(&node.name, AstKind::Literal),
            node.span.clone(),
            node.exported,
        );
        self.define(
            scope,
            Namespace::Type,
            name,
            Res::Type(id),
            Some(node.name.span.clone()),
            node.name.span.clone(),
        );
    }

    fn collect_anonymous_record(
        &mut self,
        block: &'ast TypedefBlock<'input, 'allocator>,
    ) -> TypeDefId {
        let id = self.allocate_type(
            None,
            TypeDefKind::Record,
            AstId::new(block, AstKind::TypeBlock),
            block.span.clone(),
            false,
        );
        self.collect_record_fields(block, id);
        id
    }

    fn collect_record_fields(
        &mut self,
        block: &'ast TypedefBlock<'input, 'allocator>,
        owner: TypeDefId,
    ) {
        let mut known = HashMap::<NameId, Range<usize>>::new();
        let mut fields = Vec::with_capacity(block.fields.len());
        for field in block.fields {
            let name = self.names.intern(field.name.value);
            if let Some(first) = known.get(&name) {
                self.errors.push(NameResolveError {
                    kind: NameResolveErrorKind::DuplicateField {
                        name,
                        first: first.clone(),
                    },
                    span: field.name.span.clone(),
                });
            } else {
                known.insert(name, field.name.span.clone());
            }

            let nested_type = match field.value {
                TypedefValue::Block(nested) => Some(self.collect_anonymous_record(nested)),
                TypedefValue::TypeInfo(_) => None,
            };
            let id = FieldId {
                module: self.module,
                index: index_u32(self.fields.len()),
            };
            self.declarations.insert(
                AstId::new(&field.name, AstKind::Literal),
                Declaration::Field(id),
            );
            self.fields.push(Field {
                id,
                owner,
                name,
                declaration: AstId::new(&field.name, AstKind::Literal),
                span: field.span.clone(),
                nested_type,
            });
            fields.push(id);
        }
        self.type_mut(owner).fields = fields;
    }

    fn allocate_symbol(
        &mut self,
        name: NameId,
        kind: SymbolKind,
        declaration: &'ast Literal<'input>,
        span: Range<usize>,
        exported: bool,
    ) -> SymbolId {
        let id = SymbolId {
            module: self.module,
            index: index_u32(self.symbols.len()),
        };
        let declaration_id = AstId::new(declaration, AstKind::Literal);
        self.declarations
            .insert(declaration_id, Declaration::Symbol(id));
        self.symbols.push(Symbol {
            id,
            name,
            kind,
            declaration: declaration_id,
            span,
            exported,
        });
        id
    }

    fn allocate_type(
        &mut self,
        name: Option<NameId>,
        kind: TypeDefKind,
        declaration: AstId<'ast>,
        span: Range<usize>,
        exported: bool,
    ) -> TypeDefId {
        let id = TypeDefId {
            module: self.module,
            index: index_u32(self.types.len()),
        };
        self.declarations.insert(declaration, Declaration::Type(id));
        self.types.push(TypeDef {
            id,
            name,
            kind,
            declaration,
            span,
            fields: Vec::new(),
            variants: Vec::new(),
            exported,
        });
        id
    }

    fn allocate_generic_parameter(
        &mut self,
        name: NameId,
        declaration: AstId<'ast>,
        span: Range<usize>,
    ) -> GenericParameterId {
        let id = GenericParameterId {
            module: self.module,
            index: index_u32(self.generic_parameters.len()),
        };
        self.declarations
            .insert(declaration, Declaration::GenericParameter(id));
        self.generic_parameters.push(GenericParameter {
            id,
            name,
            declaration,
            span,
        });
        id
    }

    fn type_mut(&mut self, id: TypeDefId) -> &mut TypeDef<'ast> {
        debug_assert_eq!(id.module, self.module);
        &mut self.types[id.index as usize]
    }

    fn define(
        &mut self,
        scope: ScopeId,
        namespace: Namespace,
        name: NameId,
        resolution: Res,
        span: Option<Range<usize>>,
        error_span: Range<usize>,
    ) {
        let bindings = self.scopes[scope.0 as usize].bindings_mut(namespace);
        if let Some(first) = bindings.get(&name).cloned() {
            self.errors.push(NameResolveError {
                kind: NameResolveErrorKind::DuplicateBinding {
                    name,
                    namespace,
                    first: first.span.clone(),
                },
                span: error_span,
            });
            bindings.insert(
                name,
                Binding {
                    resolution: Res::Error,
                    span: first.span,
                },
            );
        } else {
            bindings.insert(name, Binding { resolution, span });
        }
    }

    fn build_interface(&mut self) {
        let mut exports = HashMap::<String, ExportedBinding>::new();
        for symbol in &self.symbols {
            if !symbol.exported
                || self.scopes[self.root_scope.0 as usize].get(Namespace::Value, symbol.name)
                    != Some(Res::Symbol(symbol.id))
            {
                continue;
            }
            let export = exports
                .entry(self.names.get(symbol.name).to_owned())
                .or_insert(ExportedBinding {
                    value: None,
                    ty: None,
                });
            export.value = Some(symbol.id);
        }
        for ty in &self.types {
            let Some(name) = ty.name else {
                continue;
            };
            if !ty.exported
                || self.scopes[self.root_scope.0 as usize].get(Namespace::Type, name)
                    != Some(Res::Type(ty.id))
            {
                continue;
            }
            let export =
                exports
                    .entry(self.names.get(name).to_owned())
                    .or_insert(ExportedBinding {
                        value: None,
                        ty: None,
                    });
            export.ty = Some(ty.id);
        }
        self.interface.exports = exports;
    }

    fn resolve_imports(&mut self, imports: &ImportEnvironment) {
        for statement in self.source.statements {
            let Statement::ImportStatement(import) = statement else {
                continue;
            };
            let Some(interface) = imports.get_by_path(import.path.value).cloned() else {
                self.errors.push(NameResolveError {
                    kind: NameResolveErrorKind::UnknownModule,
                    span: import.path.span.clone(),
                });
                self.poison_unknown_import(import);
                continue;
            };

            match import.kind {
                ImportKind::SideEffect => {}
                ImportKind::Namespace { ref alias } => {
                    let name = self.names.intern(alias.value);
                    self.define(
                        self.root_scope,
                        Namespace::Module,
                        name,
                        Res::Module(interface.module),
                        Some(alias.span.clone()),
                        alias.span.clone(),
                    );
                    self.declarations.insert(
                        AstId::new(alias, AstKind::Literal),
                        Declaration::Module(interface.module),
                    );
                }
                ImportKind::Named(elements) => {
                    for element in elements {
                        let local = element.alias.as_ref().unwrap_or(&element.name);
                        let local_name = self.names.intern(local.value);
                        let imported_name = self.names.intern(element.name.value);
                        let Some(exported) = interface.get(element.name.value) else {
                            self.errors.push(NameResolveError {
                                kind: NameResolveErrorKind::UnknownExport {
                                    module: interface.module,
                                    name: imported_name,
                                },
                                span: element.name.span.clone(),
                            });
                            self.define_import_error(local_name, local.span.clone());
                            continue;
                        };

                        if let Some(symbol) = exported.value {
                            self.define(
                                self.root_scope,
                                Namespace::Value,
                                local_name,
                                Res::Symbol(symbol),
                                Some(local.span.clone()),
                                local.span.clone(),
                            );
                        }
                        if let Some(ty) = exported.ty {
                            self.define(
                                self.root_scope,
                                Namespace::Type,
                                local_name,
                                Res::Type(ty),
                                Some(local.span.clone()),
                                local.span.clone(),
                            );
                        }
                        self.declarations.insert(
                            AstId::new(local, AstKind::Literal),
                            Declaration::Import {
                                module: interface.module,
                                value: exported.value,
                                ty: exported.ty,
                            },
                        );
                    }
                }
            }
        }
    }

    fn poison_unknown_import(
        &mut self,
        import: &'ast light_nix_parser::ast::ImportStatement<'input, 'allocator>,
    ) {
        match import.kind {
            ImportKind::SideEffect => {}
            ImportKind::Namespace { ref alias } => {
                let name = self.names.intern(alias.value);
                self.define(
                    self.root_scope,
                    Namespace::Module,
                    name,
                    Res::Error,
                    Some(alias.span.clone()),
                    alias.span.clone(),
                );
            }
            ImportKind::Named(elements) => {
                for element in elements {
                    let local = element.alias.as_ref().unwrap_or(&element.name);
                    let name = self.names.intern(local.value);
                    self.define_import_error(name, local.span.clone());
                }
            }
        }
    }

    fn define_import_error(&mut self, name: NameId, span: Range<usize>) {
        self.define(
            self.root_scope,
            Namespace::Value,
            name,
            Res::Error,
            Some(span.clone()),
            span.clone(),
        );
        self.define(
            self.root_scope,
            Namespace::Type,
            name,
            Res::Error,
            Some(span.clone()),
            span,
        );
    }

    fn resolve_statements(
        &mut self,
        statements: &'ast Statements<'input, 'allocator>,
        scope: ScopeId,
        module_scope: bool,
        imports: &ImportEnvironment,
    ) {
        if !module_scope {
            self.collect_block_headers(statements, scope);
        }

        for statement in statements.statements {
            self.resolve_statement(statement, scope, module_scope, imports);
        }
    }

    fn resolve_statement(
        &mut self,
        statement: &'ast Statement<'input, 'allocator>,
        scope: ScopeId,
        module_scope: bool,
        imports: &ImportEnvironment,
    ) {
        match statement {
            Statement::ImportStatement(import) => {
                if !module_scope {
                    self.errors.push(NameResolveError {
                        kind: NameResolveErrorKind::ImportNotAtModuleScope,
                        span: import.span.clone(),
                    });
                }
            }
            Statement::EnumDefine(node) => {
                self.check_nested_export(node.exported, module_scope, node.span.clone());
                if let Some(representation) = node.representation_type {
                    self.resolve_type_info(representation, scope);
                }
                for variant in node.variants {
                    if let Some(value) = variant.value {
                        self.resolve_expression(value, scope, imports);
                    }
                }
            }
            Statement::TypeDefine(node) => {
                self.check_nested_export(node.exported, module_scope, node.span.clone());
                self.resolve_type_define(node, scope);
            }
            Statement::InterfaceDefine(node) => {
                self.check_nested_export(node.exported, module_scope, node.span.clone());
                self.resolve_interface(node, scope, imports);
            }
            Statement::ImplementsDefine(node) => {
                self.resolve_implements(node, scope, imports);
            }
            Statement::UseDeclare(_) => {}
            Statement::LetStatement(node) => {
                self.check_nested_export(node.exported, module_scope, node.span.clone());
                if let Some(type_info) = node.type_info {
                    self.resolve_type_info(type_info, scope);
                }
                if let Some(value) = node.value {
                    self.resolve_expression(value, scope, imports);
                }
                if !module_scope {
                    self.collect_let(node, scope);
                }
            }
            Statement::AssertStatement(node) => {
                self.resolve_expression(node.condition, scope, imports);
                if let Some(message) = node.message {
                    self.resolve_expression(message, scope, imports);
                }
            }
            Statement::AssignStatement(node) => {
                self.resolve_expression(node.target, scope, imports);
                self.resolve_expression(node.value, scope, imports);
            }
            Statement::FunctionDefine(node) => {
                self.check_nested_export(node.exported, module_scope, node.span.clone());
                self.resolve_function(node, scope, imports);
            }
            Statement::Expression(expression) => {
                self.resolve_expression(expression, scope, imports);
            }
        }
    }

    fn check_nested_export(&mut self, exported: bool, module_scope: bool, span: Range<usize>) {
        if exported && !module_scope {
            self.errors.push(NameResolveError {
                kind: NameResolveErrorKind::ExportNotAtModuleScope,
                span,
            });
        }
    }

    fn resolve_interface(
        &mut self,
        interface: &'ast InterfaceDefine<'input, 'allocator>,
        parent: ScopeId,
        imports: &ImportEnvironment,
    ) {
        let scope = self.new_scope(Some(parent), ScopeKind::Interface);
        self.define_this_parameter(
            scope,
            AstId::new(interface, AstKind::InterfaceDefine),
            interface.name.span.clone(),
        );
        self.resolve_generic_parameters(interface.generic_parameters, scope);
        if let Some(where_clause) = interface.where_clause {
            self.resolve_where_clause(where_clause, scope);
        }
        for method in interface.methods {
            self.collect_function(method, scope);
        }
        for method in interface.methods {
            self.resolve_function(method, scope, imports);
        }
    }

    fn resolve_implements(
        &mut self,
        implements: &'ast ImplementsDefine<'input, 'allocator>,
        parent: ScopeId,
        imports: &ImportEnvironment,
    ) {
        let scope = self.new_scope(Some(parent), ScopeKind::Implements);
        self.define_this_parameter(
            scope,
            AstId::new(implements, AstKind::ImplementsDefine),
            implements.target.span.clone(),
        );
        self.resolve_generic_parameters(implements.generic_parameters, scope);
        self.resolve_type_info(implements.interface, scope);
        self.resolve_type_info(implements.target, scope);
        if let Some(where_clause) = implements.where_clause {
            self.resolve_where_clause(where_clause, scope);
        }
        for method in implements.methods {
            self.collect_function(method, scope);
        }
        for method in implements.methods {
            self.resolve_function(method, scope, imports);
        }
    }

    fn resolve_generic_parameters(
        &mut self,
        parameters: Option<&'ast GenericParameters<'input, 'allocator>>,
        scope: ScopeId,
    ) {
        let Some(parameters) = parameters else {
            return;
        };

        for parameter in parameters.parameters {
            let name = self.names.intern(parameter.name.value);
            let id = self.allocate_generic_parameter(
                name,
                AstId::new(&parameter.name, AstKind::Literal),
                parameter.span.clone(),
            );
            self.define(
                scope,
                Namespace::Type,
                name,
                Res::GenericParameter(id),
                Some(parameter.name.span.clone()),
                parameter.name.span.clone(),
            );
        }
        for parameter in parameters.parameters {
            for bound in parameter.bounds {
                self.resolve_type_info(bound, scope);
            }
        }
    }

    fn resolve_where_clause(
        &mut self,
        where_clause: &'ast WhereClause<'input, 'allocator>,
        scope: ScopeId,
    ) {
        for predicate in where_clause.predicates {
            self.resolve_type_info(predicate.ty, scope);
            for bound in predicate.bounds {
                self.resolve_type_info(bound, scope);
            }
        }
    }

    fn define_this_parameter(
        &mut self,
        scope: ScopeId,
        declaration: AstId<'ast>,
        span: Range<usize>,
    ) {
        let name = self.names.intern("This");
        let id = self.allocate_generic_parameter(name, declaration, span.clone());
        self.define(
            scope,
            Namespace::Type,
            name,
            Res::GenericParameter(id),
            None,
            span,
        );
    }

    fn resolve_function(
        &mut self,
        function: &'ast FunctionDefine<'input, 'allocator>,
        parent: ScopeId,
        imports: &ImportEnvironment,
    ) {
        let scope = self.new_scope(Some(parent), ScopeKind::Function);
        self.resolve_generic_parameters(function.generic_parameters, scope);
        for argument in function.arguments.arguments {
            self.resolve_type_info(argument.type_info, scope);
        }
        if let Some(receiver) = &function.arguments.receiver {
            let name = self.names.intern(receiver.value);
            let id = self.allocate_symbol(
                name,
                SymbolKind::Parameter,
                receiver,
                receiver.span.clone(),
                false,
            );
            self.define(
                scope,
                Namespace::Value,
                name,
                Res::Symbol(id),
                Some(receiver.span.clone()),
                receiver.span.clone(),
            );
        }
        for argument in function.arguments.arguments {
            let name = self.names.intern(argument.name.value);
            let id = self.allocate_symbol(
                name,
                SymbolKind::Parameter,
                &argument.name,
                argument.span.clone(),
                false,
            );
            self.define(
                scope,
                Namespace::Value,
                name,
                Res::Symbol(id),
                Some(argument.name.span.clone()),
                argument.name.span.clone(),
            );
        }
        if let Some(return_type) = function.return_type {
            self.resolve_type_info(return_type, scope);
        }
        if let Some(where_clause) = function.where_clause {
            self.resolve_where_clause(where_clause, scope);
        }
        self.resolve_block(function.body, scope, imports);
    }

    fn resolve_block(
        &mut self,
        block: &'ast Block<'input, 'allocator>,
        parent: ScopeId,
        imports: &ImportEnvironment,
    ) {
        let scope = self.new_scope(Some(parent), ScopeKind::Block);
        self.resolve_statements(&block.statements, scope, false, imports);
    }

    fn resolve_typedef_block(
        &mut self,
        block: &'ast TypedefBlock<'input, 'allocator>,
        scope: ScopeId,
    ) {
        for field in block.fields {
            match field.value {
                TypedefValue::Block(nested) => self.resolve_typedef_block(nested, scope),
                TypedefValue::TypeInfo(type_info) => self.resolve_type_info(type_info, scope),
            }
        }
    }

    fn resolve_type_define(&mut self, node: &'ast TypeDefine<'input, 'allocator>, parent: ScopeId) {
        let scope = self.new_scope(Some(parent), ScopeKind::TypeDefinition);
        self.resolve_generic_parameters(node.generic_parameters, scope);
        if let Some(where_clause) = node.where_clause {
            self.resolve_where_clause(where_clause, scope);
        }
        self.resolve_typedef_block(node.body, scope);
    }

    fn resolve_type_info(&mut self, type_info: &'ast TypeInfo<'input, 'allocator>, scope: ScopeId) {
        let name = self.names.intern(type_info.name.value);
        let resolution =
            self.resolve_name(scope, name, Namespace::Type, type_info.name.span.clone());
        self.record_reference(&type_info.name, resolution);
        for parameter in type_info.parameters {
            self.resolve_type_info(parameter, scope);
        }
    }

    fn resolve_expression(
        &mut self,
        expression: &'ast Expression<'input, 'allocator>,
        scope: ScopeId,
        imports: &ImportEnvironment,
    ) {
        match expression {
            Expression::If(node) => {
                self.resolve_expression(node.branch.condition, scope, imports);
                self.resolve_block(node.branch.body, scope, imports);
                for branch in node.else_branches {
                    match branch.value {
                        ElseBranchValue::If(if_branch) => {
                            self.resolve_expression(if_branch.condition, scope, imports);
                            self.resolve_block(if_branch.body, scope, imports);
                        }
                        ElseBranchValue::Block(block) => {
                            self.resolve_block(block, scope, imports);
                        }
                    }
                }
            }
            Expression::Match(node) => {
                self.resolve_expression(node.value, scope, imports);
                for arm in node.arms {
                    self.resolve_match_arm(arm, scope, imports);
                }
            }
            Expression::Return(node) => {
                if let Some(value) = node.value {
                    self.resolve_expression(value, scope, imports);
                }
            }
            Expression::Throw(node) => {
                if let Some(message) = node.message {
                    self.resolve_expression(message, scope, imports);
                }
            }
            Expression::Closure(node) => self.resolve_closure(node, scope, imports),
            Expression::Elvis(node) => {
                self.resolve_expression(node.optional, scope, imports);
                self.resolve_expression(node.fallback, scope, imports);
            }
            Expression::Binary(node) => {
                self.resolve_expression(node.left, scope, imports);
                self.resolve_expression(node.right, scope, imports);
            }
            Expression::Unary(node) => {
                self.resolve_expression(node.operand, scope, imports);
            }
            Expression::Primary(node) => self.resolve_primary(node, scope, imports),
        }
    }

    fn resolve_closure(
        &mut self,
        closure: &'ast ClosureExpression<'input, 'allocator>,
        parent: ScopeId,
        imports: &ImportEnvironment,
    ) {
        let scope = self.new_scope(Some(parent), ScopeKind::Closure);
        for parameter in closure.parameters {
            if let Some(type_info) = parameter.type_info {
                self.resolve_type_info(type_info, scope);
            }
            let name = self.names.intern(parameter.name.value);
            let id = self.allocate_symbol(
                name,
                SymbolKind::Parameter,
                &parameter.name,
                parameter.span.clone(),
                false,
            );
            self.define(
                scope,
                Namespace::Value,
                name,
                Res::Symbol(id),
                Some(parameter.name.span.clone()),
                parameter.name.span.clone(),
            );
        }
        if let Some(return_type) = closure.return_type {
            self.resolve_type_info(return_type, scope);
        }
        match closure.body {
            ClosureBody::Expression(expression) => {
                self.resolve_expression(expression, scope, imports);
            }
            ClosureBody::Block(block) => self.resolve_block(block, scope, imports),
        }
    }

    fn resolve_match_arm(
        &mut self,
        arm: &'ast MatchArm<'input, 'allocator>,
        parent: ScopeId,
        imports: &ImportEnvironment,
    ) {
        let scope = self.new_scope(Some(parent), ScopeKind::MatchArm);
        self.resolve_pattern(&arm.pattern, scope);
        self.resolve_expression(arm.value, scope, imports);
    }

    fn resolve_pattern(&mut self, pattern: &'ast Pattern<'input, 'allocator>, scope: ScopeId) {
        match pattern {
            Pattern::Some(node) => self.resolve_pattern(node.pattern, scope),
            Pattern::Null(_) | Pattern::Wildcard(_) => {}
            Pattern::Binding(binding) => {
                let name = self.names.intern(binding.value);
                let id = self.allocate_symbol(
                    name,
                    SymbolKind::PatternBinding,
                    binding,
                    binding.span.clone(),
                    false,
                );
                self.define(
                    scope,
                    Namespace::Value,
                    name,
                    Res::Symbol(id),
                    Some(binding.span.clone()),
                    binding.span.clone(),
                );
            }
            Pattern::EnumVariant(variant) => {
                let name = self.names.intern(variant.enum_name.value);
                let owner =
                    self.resolve_name(scope, name, Namespace::Type, variant.enum_name.span.clone());
                self.record_reference(&variant.enum_name, owner);
                let variant_name = self.names.intern(variant.variant.value);
                let resolution = match owner {
                    Res::Type(owner) => self
                        .find_variant(owner, variant_name)
                        .map(Res::EnumVariant)
                        .unwrap_or(Res::Member(variant_name)),
                    Res::Error => Res::Error,
                    _ => Res::Member(variant_name),
                };
                self.record_reference(&variant.variant, resolution);
            }
        }
    }

    fn resolve_primary(
        &mut self,
        primary: &'ast Primary<'input, 'allocator>,
        scope: ScopeId,
        imports: &ImportEnvironment,
    ) {
        let mut current = match &primary.value {
            Value::Literal(literal) => {
                let has_static_access = primary
                    .accesses
                    .first()
                    .is_some_and(|access| access.operator.value == AccessOperator::DoubleColon);
                let name = self.names.intern(literal.literal.value);
                let resolution = if has_static_access {
                    self.resolve_static_root(scope, name, literal.literal.span.clone())
                } else {
                    self.resolve_value_root(scope, name, literal.literal.span.clone())
                };
                self.record_reference(&literal.literal, resolution);
                self.resolve_explicit_type_arguments(literal.type_arguments, scope);
                if let Some(call) = literal.call {
                    self.resolve_call(call, scope, imports);
                }
                resolution
            }
            value => {
                self.resolve_value(value, scope, imports);
                Res::Error
            }
        };

        for (index, access) in primary.accesses.iter().enumerate() {
            let name = self.names.intern(access.member.value);
            let next_is_static = primary
                .accesses
                .get(index + 1)
                .is_some_and(|next| next.operator.value == AccessOperator::DoubleColon);
            current = match current {
                Res::Module(module) => self.resolve_module_member(
                    module,
                    name,
                    next_is_static,
                    access.call.is_some(),
                    imports,
                    access.member.span.clone(),
                ),
                Res::Type(owner) if access.operator.value == AccessOperator::DoubleColon => self
                    .find_variant(owner, name)
                    .map(Res::EnumVariant)
                    .unwrap_or(Res::Member(name)),
                Res::Error => Res::Error,
                _ => Res::Member(name),
            };
            self.record_reference(&access.member, current);
            self.resolve_explicit_type_arguments(access.type_arguments, scope);
            if let Some(call) = access.call {
                self.resolve_call(call, scope, imports);
            }
        }
    }

    fn resolve_value(
        &mut self,
        value: &'ast Value<'input, 'allocator>,
        scope: ScopeId,
        imports: &ImportEnvironment,
    ) {
        match value {
            Value::Array(array) => self.resolve_array(array, scope, imports),
            Value::Literal(literal) => {
                let name = self.names.intern(literal.literal.value);
                let resolution = self.resolve_value_root(scope, name, literal.literal.span.clone());
                self.record_reference(&literal.literal, resolution);
                self.resolve_explicit_type_arguments(literal.type_arguments, scope);
                if let Some(call) = literal.call {
                    self.resolve_call(call, scope, imports);
                }
            }
            Value::Some(node) => {
                if let Some(value) = node.value {
                    self.resolve_expression(value, scope, imports);
                }
            }
            Value::Numeric(_) | Value::String(_) | Value::Boolean(_) | Value::Null(_) => {}
        }
    }

    fn resolve_array(
        &mut self,
        array: &'ast Array<'input, 'allocator>,
        scope: ScopeId,
        imports: &ImportEnvironment,
    ) {
        for value in array.values {
            self.resolve_value(value, scope, imports);
        }
    }

    fn resolve_call(
        &mut self,
        call: &'ast FunctionCall<'input, 'allocator>,
        scope: ScopeId,
        imports: &ImportEnvironment,
    ) {
        for argument in call.arguments {
            self.resolve_expression(argument, scope, imports);
        }
    }

    fn resolve_explicit_type_arguments(
        &mut self,
        arguments: Option<&'ast light_nix_parser::ast::ExplicitTypeArguments<'input, 'allocator>>,
        scope: ScopeId,
    ) {
        let Some(arguments) = arguments else {
            return;
        };
        for argument in arguments.arguments {
            if let ExplicitTypeArgument::Type(ty) = argument {
                self.resolve_type_info(ty, scope);
            }
        }
    }

    fn resolve_module_member(
        &mut self,
        module: ModuleId,
        name: NameId,
        prefer_type: bool,
        called: bool,
        imports: &ImportEnvironment,
        span: Range<usize>,
    ) -> Res {
        let Some(interface) = imports.get_by_id(module) else {
            return Res::Member(name);
        };
        let Some(exported) = interface.get(self.names.get(name)) else {
            self.errors.push(NameResolveError {
                kind: NameResolveErrorKind::UnknownExport { module, name },
                span,
            });
            return Res::Error;
        };

        if prefer_type {
            exported
                .ty
                .map(Res::Type)
                .or_else(|| exported.value.map(Res::Symbol))
                .unwrap_or(Res::Error)
        } else if called {
            exported
                .value
                .map(Res::Symbol)
                .or_else(|| exported.ty.map(Res::Type))
                .unwrap_or(Res::Error)
        } else {
            exported
                .value
                .map(Res::Symbol)
                .or_else(|| exported.ty.map(Res::Type))
                .unwrap_or(Res::Error)
        }
    }

    fn resolve_static_root(&mut self, scope: ScopeId, name: NameId, span: Range<usize>) -> Res {
        if let Some(resolution) = self.lookup(scope, Namespace::Type, name) {
            return resolution;
        }
        if let Some(resolution) = self.lookup(scope, Namespace::Module, name) {
            return resolution;
        }
        if let Some(resolution) = self.lookup(scope, Namespace::Value, name) {
            return resolution;
        }
        self.unresolved(name, Namespace::Type, span)
    }

    fn resolve_value_root(&mut self, scope: ScopeId, name: NameId, span: Range<usize>) -> Res {
        if let Some(resolution) = self.lookup(scope, Namespace::Value, name) {
            return resolution;
        }
        if let Some(resolution) = self.lookup(scope, Namespace::Module, name) {
            return resolution;
        }
        self.unresolved(name, Namespace::Value, span)
    }

    fn resolve_name(
        &mut self,
        scope: ScopeId,
        name: NameId,
        namespace: Namespace,
        span: Range<usize>,
    ) -> Res {
        self.lookup(scope, namespace, name)
            .unwrap_or_else(|| self.unresolved(name, namespace, span))
    }

    fn unresolved(&mut self, name: NameId, namespace: Namespace, span: Range<usize>) -> Res {
        self.errors.push(NameResolveError {
            kind: NameResolveErrorKind::UnresolvedName { name, namespace },
            span,
        });
        Res::Error
    }

    fn lookup(&self, mut scope: ScopeId, namespace: Namespace, name: NameId) -> Option<Res> {
        loop {
            let current = &self.scopes[scope.0 as usize];
            if let Some(resolution) = current.get(namespace, name) {
                return Some(resolution);
            }
            scope = current.parent?;
        }
    }

    fn find_variant(&self, owner: TypeDefId, name: NameId) -> Option<VariantId> {
        if owner.module != self.module {
            return None;
        }
        self.types[owner.index as usize]
            .variants
            .iter()
            .copied()
            .find(|id| self.variants[id.index as usize].name == name)
    }

    fn record_reference(&mut self, literal: &'ast Literal<'input>, resolution: Res) {
        self.references
            .insert(AstId::new(literal, AstKind::Literal), resolution);
    }

    fn new_scope(&mut self, parent: Option<ScopeId>, kind: ScopeKind) -> ScopeId {
        let id = ScopeId(index_u32(self.scopes.len()));
        self.scopes.push(Scope {
            id,
            parent,
            kind,
            values: HashMap::new(),
            types: HashMap::new(),
            modules: HashMap::new(),
        });
        id
    }
}

#[cfg(test)]
mod tests {
    use light_nix_parser::{
        ast::{AstArena, Expression, Pattern, Statement, Value},
        lexer::Lexer,
        parser::{ParseErrors, parse_source},
    };

    use super::*;

    fn literal_from_expression<'ast, 'input, 'allocator>(
        expression: &'ast Expression<'input, 'allocator>,
    ) -> &'ast Literal<'input> {
        let Expression::Primary(primary) = expression else {
            panic!("expected primary expression");
        };
        let Value::Literal(literal) = &primary.value else {
            panic!("expected literal value");
        };
        &literal.literal
    }

    #[test]
    fn resolves_forward_module_values_and_separate_type_namespace() {
        let source = r#"
type Config { enabled: Bool }
let Config = true
let first = later
let later: Config = false
"#;
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:#?}");

        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());

        let Statement::LetStatement(first) = ast.statements[2] else {
            panic!("expected first binding");
        };
        let later_reference = literal_from_expression(first.value.unwrap());
        let Statement::LetStatement(later) = ast.statements[3] else {
            panic!("expected later binding");
        };
        let Declaration::Symbol(later_id) = resolution
            .declaration_of_literal(&later.name)
            .expect("later declaration")
        else {
            panic!("expected symbol declaration");
        };
        assert_eq!(
            resolution.resolve_literal(later_reference),
            Some(Res::Symbol(later_id))
        );

        let type_reference = &later.type_info.unwrap().name;
        let Statement::TypeDefine(config_type) = ast.statements[0] else {
            panic!("expected Config type");
        };
        let Declaration::Type(config_type_id) = resolution
            .declaration_of_literal(&config_type.name)
            .expect("Config declaration")
        else {
            panic!("expected type declaration");
        };
        assert_eq!(
            resolution.resolve_literal(type_reference),
            Some(Res::Type(config_type_id))
        );
    }

    #[test]
    fn resolves_named_and_namespace_imports_from_collected_interface() {
        let exported_source = r#"
export type Programs { enabled: Bool }
export let helper = true
export enum Desktop { KDE }
"#;
        let exported_arena = AstArena::new();
        let mut exported_lexer = Lexer::new(exported_source);
        let mut exported_errors = ParseErrors::new_in(&exported_arena);
        let exported_ast = parse_source(&mut exported_lexer, &mut exported_errors, &exported_arena);
        assert!(exported_errors.is_empty());
        let exported = collect_module(exported_ast, ModuleId(1));
        let interface = exported.interface().clone();

        let source = r#"
import { Programs, helper as run, Desktop } from "./module.lnix"
import * as module from "./module.lnix"
let config: Programs
let direct = run
let variant = Desktop::KDE
let namespaced = module.helper
"#;
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:#?}");

        let mut imports = ImportEnvironment::default();
        imports.insert(r#""./module.lnix""#, interface.clone());
        let resolution = collect_module(ast, ModuleId(2)).resolve(&imports);
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());

        let programs = interface.get("Programs").unwrap().ty.unwrap();
        let helper = interface.get("helper").unwrap().value.unwrap();

        let Statement::LetStatement(config) = ast.statements[2] else {
            panic!("expected config");
        };
        assert_eq!(
            resolution.resolve_literal(&config.type_info.unwrap().name),
            Some(Res::Type(programs))
        );

        let Statement::LetStatement(direct) = ast.statements[3] else {
            panic!("expected direct import use");
        };
        assert_eq!(
            resolution.resolve_literal(literal_from_expression(direct.value.unwrap())),
            Some(Res::Symbol(helper))
        );

        let Statement::LetStatement(namespaced) = ast.statements[5] else {
            panic!("expected namespace import use");
        };
        let Expression::Primary(primary) = namespaced.value.unwrap() else {
            panic!("expected namespace primary");
        };
        let Value::Literal(root) = &primary.value else {
            panic!("expected namespace root");
        };
        assert_eq!(
            resolution.resolve_literal(&root.literal),
            Some(Res::Module(ModuleId(1)))
        );
        assert_eq!(
            resolution.resolve_literal(&primary.accesses[0].member),
            Some(Res::Symbol(helper))
        );
    }

    #[test]
    fn isolates_blocks_and_match_arms() {
        let source = r#"
let condition = true
if condition {
    let local = condition
}
let escaped = local
let selected = match condition {
    some(value) => value
    null => value
}
"#;
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:#?}");

        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        let unresolved = resolution
            .errors()
            .iter()
            .filter(|error| matches!(error.kind, NameResolveErrorKind::UnresolvedName { .. }))
            .count();
        assert_eq!(unresolved, 2, "{:#?}", resolution.errors());

        let Statement::LetStatement(selected) = ast.statements[3] else {
            panic!("expected selected");
        };
        let Expression::Match(match_expression) = selected.value.unwrap() else {
            panic!("expected match");
        };
        let Pattern::Some(pattern) = &match_expression.arms[0].pattern else {
            panic!("expected some pattern");
        };
        let Pattern::Binding(binding) = pattern.pattern else {
            panic!("expected pattern binding");
        };
        let Declaration::Symbol(binding_id) = resolution
            .declaration_of_literal(binding)
            .expect("pattern declaration")
        else {
            panic!("expected symbol declaration");
        };
        let first_arm_value = literal_from_expression(match_expression.arms[0].value);
        assert_eq!(
            resolution.resolve_literal(first_arm_value),
            Some(Res::Symbol(binding_id))
        );
        let second_arm_value = literal_from_expression(match_expression.arms[1].value);
        assert_eq!(
            resolution.resolve_literal(second_arm_value),
            Some(Res::Error)
        );
    }

    #[test]
    fn reports_duplicate_bindings_fields_and_variants_without_stopping() {
        let source = r#"
type Config {
    field: Bool,
    field: String
}
enum Desktop { KDE, KDE }
let duplicate = true
let duplicate = false
let after = true
"#;
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:#?}");

        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(
            resolution
                .errors()
                .iter()
                .any(|error| matches!(error.kind, NameResolveErrorKind::DuplicateField { .. }))
        );
        assert!(
            resolution
                .errors()
                .iter()
                .any(|error| matches!(error.kind, NameResolveErrorKind::DuplicateVariant { .. }))
        );
        assert!(resolution.errors().iter().any(|error| matches!(
            error.kind,
            NameResolveErrorKind::DuplicateBinding {
                namespace: Namespace::Value,
                ..
            }
        )));
        assert!(matches!(ast.statements[4], Statement::LetStatement(_)));
    }

    #[test]
    fn unknown_import_poisoning_prevents_cascading_unresolved_errors() {
        let source = r#"
import { Missing } from "./missing.lnix"
let value = Missing
"#;
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty());

        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert_eq!(
            resolution
                .errors()
                .iter()
                .filter(|error| matches!(error.kind, NameResolveErrorKind::UnknownModule))
                .count(),
            1
        );
        assert!(
            !resolution
                .errors()
                .iter()
                .any(|error| matches!(error.kind, NameResolveErrorKind::UnresolvedName { .. }))
        );

        let Statement::LetStatement(value) = ast.statements[1] else {
            panic!("expected value");
        };
        assert_eq!(
            resolution.resolve_literal(literal_from_expression(value.value.unwrap())),
            Some(Res::Error)
        );
    }

    #[test]
    fn resolves_generic_interface_scopes_and_explicit_type_arguments() {
        let source = r#"
interface Comparable {}
interface TestInterface<U> {}
type Test {}

implements TestInterface<Int> for Test {}

inline function test<T, U>() -> U
where T: TestInterface<U> {
    let value: U
    return value
}

let result = test:<Test>()
"#;
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:#?}");

        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        assert_eq!(
            resolution
                .types()
                .iter()
                .filter(|ty| ty.kind == TypeDefKind::Interface)
                .count(),
            2
        );

        let Statement::FunctionDefine(function) = ast.statements[4] else {
            panic!("expected generic function");
        };
        assert!(matches!(
            resolution.resolve_literal(&function.return_type.unwrap().name),
            Some(Res::GenericParameter(_))
        ));

        let Statement::LetStatement(result) = ast.statements[5] else {
            panic!("expected result");
        };
        let Expression::Primary(call) = result.value.unwrap() else {
            panic!("expected generic call");
        };
        let Value::Literal(call) = &call.value else {
            panic!("expected literal call");
        };
        let ExplicitTypeArgument::Type(test_type) = &call.type_arguments.unwrap().arguments[0]
        else {
            panic!("expected explicit Test type");
        };
        assert!(matches!(
            resolution.resolve_literal(&test_type.name),
            Some(Res::Type(_))
        ));
    }

    #[test]
    fn resolves_generic_parameters_inside_type_fields_and_where_clauses() {
        let source = r#"
interface Marker {}
type Boxed<T: Marker>
where T: Marker {
    value: T
    nested: { value: Set<T> }
}
declare let boxed: Boxed<String>
"#;
        let arena = AstArena::new();
        let mut lexer = Lexer::new(source);
        let mut parse_errors = ParseErrors::new_in(&arena);
        let ast = parse_source(&mut lexer, &mut parse_errors, &arena);
        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:#?}");

        let resolution = collect_module(ast, ModuleId(0)).resolve(&ImportEnvironment::default());
        assert!(resolution.errors().is_empty(), "{:#?}", resolution.errors());
        let Statement::TypeDefine(boxed) = ast.statements[1] else {
            panic!("expected Boxed type");
        };
        let parameter = &boxed.generic_parameters.unwrap().parameters[0];
        let Some(Declaration::GenericParameter(parameter_id)) =
            resolution.declaration_of_literal(&parameter.name)
        else {
            panic!("expected generic parameter declaration");
        };
        let TypedefValue::TypeInfo(value_type) = boxed.body.fields[0].value else {
            panic!("expected value type");
        };
        assert_eq!(
            resolution.resolve_literal(&value_type.name),
            Some(Res::GenericParameter(parameter_id))
        );
        let nested = match boxed.body.fields[1].value {
            TypedefValue::Block(nested) => nested,
            _ => panic!("expected nested type"),
        };
        let TypedefValue::TypeInfo(nested_value) = nested.fields[0].value else {
            panic!("expected nested value type");
        };
        assert_eq!(
            resolution.resolve_literal(&nested_value.parameters[0].name),
            Some(Res::GenericParameter(parameter_id))
        );
    }
}
