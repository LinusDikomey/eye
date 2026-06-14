pub mod builtins;
mod display;

use core::iter::Iterator;
use std::{
    cell::{OnceCell, RefCell},
    collections::{VecDeque, hash_map},
    ops::Index,
    path::{Path, PathBuf},
};

use dmap::DHashMap;
use error::{
    CompileError, Error, Errors, ImplIncompatibility,
    span::{IdentPath, TSpan},
};
use indexmap::IndexMap;
use ir::{Environment, ModuleOf};
use parser::{
    self,
    ast::{
        self, Ast, DefExprId, FunctionId, GenericDef, GlobalId, ModuleId, Primitive, ScopeId,
        TraitId, UnresolvedType, UseId,
    },
};
use segment_list::SegmentList;

use crate::{
    InvalidTypeError, ProjectId, Type,
    callconv::CallConv,
    check::{
        self, Hooks, ProjectErrors,
        traits::{self, Candidates, SelectedInstance},
    },
    eval::{self, ConstValue, ConstValueId},
    helpers::IteratorExt,
    hir::{HIRBuilder, Hir, Var},
    irgen,
    types::{BaseType, BuiltinType, TypeFull, Types},
    typing::{
        Bound, LocalOrGlobalInstance, LocalTypeId, LocalTypeIds, TypeInfo, TypeInfoOrIdx, TypeTable,
    },
};

use builtins::Builtins;

pub struct Compiler {
    projects: SegmentList<Project>,
    pub modules: SegmentList<Module>,
    pub const_values: SegmentList<(ConstValue, Type)>,
    pub types: Types,
    // pub ir: ir::Environment,
    // pub ir_module: ir::ModuleId,
    // pub dialects: Dialects,
    pub errors: ProjectErrors,
    pub builtins: Builtins,
}

pub struct ModuleSpan {
    pub module: ModuleId,
    pub span: TSpan,
}
impl ModuleSpan {
    pub const MISSING: Self = Self {
        module: ModuleId::MISSING,
        span: TSpan::MISSING,
    };
}

#[derive(Clone, Copy)]
pub struct Dialects {
    pub arith: ModuleOf<ir::dialect::Arith>,
    pub tuple: ModuleOf<ir::dialect::Tuple>,
    pub mem: ModuleOf<ir::dialect::Mem>,
    pub cf: ModuleOf<ir::dialect::Cf>,
    pub main: ir::ModuleId,
}
impl Compiler {
    pub fn new() -> Self {
        // let mut ir = ir::Environment::new(ir::dialect::Primitive::create_infos());
        // let dialects = Dialects {
        //     arith: ir.get_dialect_module(),
        //     tuple: ir.get_dialect_module(),
        //     mem: ir.get_dialect_module(),
        //     cf: ir.get_dialect_module(),
        // };
        // let ir_module = ir.create_module("main");
        Self {
            projects: SegmentList::new(),
            modules: SegmentList::new(),
            const_values: SegmentList::new(),
            types: Types::new(),
            // ir,
            // ir_module,
            // dialects,
            errors: ProjectErrors::new(),
            builtins: Builtins::default(),
        }
    }

    pub fn add_project(
        &mut self,
        name: String,
        root: PathBuf,
        dependencies: Vec<ProjectId>,
    ) -> Result<ProjectId, ProjectError> {
        let root = root.canonicalize().unwrap_or(root);
        for (project, i) in self.projects.iter().zip(0..) {
            if project.name == name && project.root.as_ref().is_some_and(|r| r == &root) {
                return Ok(ProjectId(i));
            }
        }
        let next_module_id = ModuleId::from_inner(self.modules.len());
        let project_id = ProjectId(self.projects.add(Project {
            name: name.clone(),
            root: Some(root.clone()),
            root_module: next_module_id,
            dependencies,
        }));

        let root_module = ModuleId::from_inner(self.modules.add(Module::at_path(
            name.into_boxed_str(),
            root.clone(),
            project_id,
            next_module_id,
            None,
        )));
        assert_eq!(
            next_module_id, root_module,
            "Other module added while adding new project"
        );

        Ok(project_id)
    }

    pub fn add_single_file_project_from_string(
        &self,
        name: String,
        content: Box<str>,
        dependencies: Vec<ProjectId>,
    ) -> ProjectId {
        let module_id = ModuleId::from_inner(self.modules.len());
        let project = ProjectId(self.projects.add(Project {
            name: name.clone(),
            root: None,
            root_module: module_id,
            dependencies,
        }));
        self.modules.add(Module {
            name: name.into_boxed_str(),
            storage: ModuleStorage::String(content),
            project,
            parsed: OnceCell::new(),
            root: module_id,
            parent: None,
        });
        project
    }

    pub fn add_type_def(
        &self,
        module: ModuleId,
        id: ast::TypeId,
        name: Box<str>,
        name_span: TSpan,
        generic_count: u8,
    ) -> BaseType {
        self.types
            .add_base(module, id, name, name_span, generic_count)
    }

    pub fn add_const_value(&self, value: ConstValue, ty: Type) -> ConstValueId {
        ConstValueId(self.const_values.add((value, ty)))
    }

    pub fn get_project(&self, id: ProjectId) -> &Project {
        &self.projects[id.idx()]
    }

    pub fn get_root_module(&self, module: ModuleId) -> ModuleId {
        self.modules[module.idx()].root
    }

    pub fn get_module_ast(&self, module_id: ModuleId) -> &Ast {
        &self.get_parsed_module(module_id).ast
    }

    pub fn module_pre_definitions(
        &self,
        module_id: ModuleId,
    ) -> (DHashMap<String, ast::Definition>, Vec<ModuleId>) {
        let module = &self.modules[module_id.idx()];
        // add dependencies to each module first
        let mut definitions: DHashMap<String, ast::Definition> = self.projects
            [module.project.idx()]
        .dependencies
        .iter()
        .map(|dependency| {
            let dependency = &self.projects[dependency.idx()];
            (
                dependency.name.clone(),
                ast::Definition::Module(dependency.root_module),
            )
        })
        .collect();

        let mut child_modules = Vec::new();
        match &module.storage {
            ModuleStorage::Path(path) => {
                let (_, child_module_paths) =
                    crate::modules::module_and_children(path, module_id == module.root);
                for (name, path) in child_module_paths {
                    let id = ModuleId::from_inner(self.modules.add(Module::at_path(
                        name.clone().into_boxed_str(),
                        path,
                        module.project,
                        module.root,
                        Some(module_id),
                    )));
                    child_modules.push(id);
                    definitions.insert(name, ast::Definition::Module(id));
                }
            }
            ModuleStorage::String(_) => {}
        };
        (definitions, child_modules)
    }

    pub fn get_parsed_module(&self, module_id: ModuleId) -> &ParsedModule {
        self.modules[module_id.idx()].parsed.get_or_init(|| {
            let module = &self.modules[module_id.idx()];

            let (definitions, child_modules) = self.module_pre_definitions(module_id);
            let contents = match &module.storage {
                ModuleStorage::Path(path) => {
                    let (file, _) =
                        crate::modules::module_and_children(path, module_id == module.root);
                    std::fs::read_to_string(file).map_or_else(
                        |err| {
                            panic!(
                                "compiler failed to open the file {}: {:?}",
                                path.display(),
                                err
                            )
                        },
                        String::into_boxed_str,
                    )
                }
                ModuleStorage::String(contents) => contents.clone(),
            };
            let mut errors = Errors::new();
            let ast = parser::parse(contents, &mut errors, definitions);
            tracing::debug!(target: "ast", module = self.module_path(module_id), "Finished parsing:\n{}", ast);
            self.errors.add_module(module_id, errors);
            let checked = ModuleSymbols::empty(&ast);
            ParsedModule {
                ast,
                version: None,
                child_modules,
                symbols: checked,
            }
        })
    }

    pub fn project_ids(&self) -> impl use<> + ExactSizeIterator<Item = ProjectId> {
        (0..self.projects.len()).map(ProjectId)
    }

    pub fn module_ids(&self) -> impl use<> + ExactSizeIterator<Item = ModuleId> {
        (0..self.modules.len()).map(ModuleId::from_inner)
    }

    pub fn resolve_in_module(&self, module: ModuleId, name: &str, name_span: ModuleSpan) -> Def {
        let scope = self.get_module_ast(module).top_level_scope_id();
        self.resolve_in_scope(module, scope, name, name_span)
    }

    pub fn resolve_in_scope(
        &self,
        module: ModuleId,
        mut scope: ScopeId,
        name: &str,
        name_span: ModuleSpan,
    ) -> Def {
        let def = loop {
            match self.get_scope_def(module, scope, name) {
                Some(def) => break Some(def),
                None => {
                    if let Some(parent) = self.get_module_ast(module)[scope].parent {
                        scope = parent;
                    } else {
                        break None;
                    }
                }
            }
        };
        def.unwrap_or_else(|| {
            let builtin_module = builtins::get_prelude(self);
            let builtin_scope = self.get_module_ast(builtin_module).top_level_scope_id();
            if let Some(def) = self.get_scope_def(builtin_module, builtin_scope, name) {
                return def;
            }
            self.errors.emit(
                name_span.module,
                Error::UnknownIdent(name.into()).at_span(name_span.span),
            );
            Def::Invalid
        })
    }

    pub fn get_scope_def(&self, module: ModuleId, scope: ScopeId, name: &str) -> Option<Def> {
        let parsed = self.get_parsed_module(module);
        let &def = parsed.ast[scope].definitions.get(name)?;
        let def = match def {
            // PERF: return reference here instead of cloning if possible
            ast::Definition::Expr { id, name_span, .. } => {
                *self.get_def_expr(module, scope, name, name_span, id)
            }
            ast::Definition::Use { path, id, .. } => {
                self.resolve_use(parsed, module, scope, id, path)
            }
            ast::Definition::Global(id) => Def::Global(module, id),
            ast::Definition::Module(id) => Def::Module(id),
            ast::Definition::Generic(i) => Def::Type(self.types.intern(TypeFull::Generic(i))),
        };
        Some(def)
    }

    fn resolve_use(
        &self,
        parsed: &ParsedModule,
        module: ModuleId,
        scope: ScopeId,
        id: UseId,
        path: IdentPath,
    ) -> Def {
        *parsed.symbols.uses[id.idx()].get_or_resolve_with(
            || {
                self.errors
                    .emit(module, Error::RecursiveDefinition.at_span(path.span()));
                Def::Invalid
            },
            || self.resolve_path(module, scope, path),
        )
    }

    pub fn get_def_expr(
        &self,
        module: ModuleId,
        scope: ScopeId,
        name: &str,
        name_span: TSpan,
        id: DefExprId,
    ) -> &Def {
        let parsed = self.get_parsed_module(module);
        parsed.symbols.def_exprs[id.idx()].get_or_resolve_with(
            || {
                let ast = self.get_module_ast(module);
                let (value, _) = ast[id];
                let span = ast[value].span(ast);
                self.errors
                    .emit(module, Error::RecursiveDefinition.at_span(span));
                Def::Invalid
            },
            || {
                let ast = &parsed.ast;
                let (value, ty) = &ast[id];
                let value = *value;
                eval::def_expr(self, module, scope, ast, value, name, name_span, ty)
            },
        )
    }

    pub fn resolve_path(&self, module: ModuleId, mut scope: ScopeId, path: IdentPath) -> Def {
        let ast = self.get_module_ast(module);
        let (root, segments, last) = path.segments(ast.src());
        let mut current_module = module;
        if root.is_some() {
            let module = &self.modules[current_module.idx()];
            current_module = module.root;
            scope = self.get_module_ast(current_module).top_level_scope_id();
        }
        for (name, name_span) in segments {
            match self.resolve_in_scope(
                current_module,
                scope,
                name,
                ModuleSpan {
                    module,
                    span: name_span,
                },
            ) {
                Def::Module(new_mod) => {
                    current_module = new_mod;
                    scope = self.get_module_ast(current_module).top_level_scope_id();
                }
                Def::Invalid => return Def::Invalid,
                _ => {
                    self.errors
                        .emit(module, Error::ModuleExpected.at_span(name_span));
                    return Def::Invalid;
                }
            }
        }
        let Some((name, name_span)) = last else {
            return Def::Module(current_module);
        };
        self.resolve_in_scope(
            current_module,
            scope,
            name,
            ModuleSpan {
                module,
                span: name_span,
            },
        )
    }

    pub fn resolve_type(&self, ty: &UnresolvedType, module: ModuleId, scope: ScopeId) -> Type {
        match ty {
            &UnresolvedType::Primitive { ty, .. } => ty.into(),
            UnresolvedType::Unresolved(path, generics) => {
                match self.resolve_path(module, scope, *path) {
                    Def::BaseType(base) => {
                        let expected = self.types.get_base(base).generic_count;
                        let found = generics.as_ref().map_or(0, |g| g.0.len() as u8);
                        if expected != found {
                            let span = generics.as_ref().map_or_else(|| path.span(), |g| g.1);
                            self.errors.emit(
                                module,
                                Error::InvalidGenericCount { expected, found }.at_span(span),
                            );
                            return Type::Invalid;
                        }
                        let generics: Box<[Type]> = generics
                            .as_ref()
                            .map_or::<&[UnresolvedType], _>(&[], |g| &*g.0)
                            .iter()
                            .map(|ty| self.resolve_type(ty, module, scope))
                            .collect();
                        self.types.intern(TypeFull::Instance(base, &generics))
                    }
                    Def::Type(ty) => {
                        if let &Some((_, span)) = generics {
                            self.errors
                                .emit(module, Error::UnexpectedGenerics.at_span(span));
                        }
                        ty
                    }
                    Def::Invalid => Type::Invalid,
                    _ => {
                        self.errors
                            .emit(module, Error::TypeExpected.at_span(ty.span()));
                        Type::Invalid
                    }
                }
            }
            UnresolvedType::Pointer(b) => {
                let (pointee, _) = &**b;
                let pointee = self.resolve_type(pointee, module, scope);
                self.types
                    .intern(TypeFull::Instance(BaseType::Pointer, &[pointee]))
            }
            UnresolvedType::Array(b) => {
                let (elem_ty, size, _) = &**b;
                let elem_ty = self.resolve_type(elem_ty, module, scope);
                let Some(size) = *size else {
                    panic!("inferred array size is not allowed here")
                };
                let size_const = self.types.intern(TypeFull::Const(size.into()));
                self.types
                    .intern(TypeFull::Instance(BaseType::Array, &[elem_ty, size_const]))
            }
            UnresolvedType::Tuple(elems, _) => {
                let members: Box<[_]> = elems
                    .iter()
                    .map(|elem| self.resolve_type(elem, module, scope))
                    .collect();
                self.types.intern(TypeFull::Tuple {
                    members: &members,
                    // TODO: named tuple members
                    named_members: &[],
                })
            }
            UnresolvedType::Function {
                span_and_return_type,
                params,
            } => {
                let return_and_params: Box<[Type]> = std::iter::once(&span_and_return_type.1)
                    .chain(params.iter())
                    .map(|ty| self.resolve_type(ty, module, scope))
                    .collect();
                self.types
                    .intern(TypeFull::Instance(BaseType::Function, &return_and_params))
            }
            &UnresolvedType::Infer(span) => {
                self.errors
                    .emit(module, Error::InferredTypeNotAllowedHere.at_span(span));
                Type::Invalid
            }
        }
    }

    pub fn annotation_matches_type(
        &self,
        annotation: &UnresolvedType,
        ty: Type,
        module: ModuleId,
        scope: ScopeId,
    ) -> Result<bool, InvalidTypeError> {
        match annotation {
            UnresolvedType::Infer(_) => Ok(true),
            _ => match self.resolve_type(annotation, module, scope) {
                Type::Invalid => Err(InvalidTypeError),
                resolved => Ok(resolved == ty),
            },
        }
    }

    pub fn get_signature(&self, module: ModuleId, id: ast::FunctionId) -> &Signature {
        let parsed = self.modules[module.idx()].parsed.get().unwrap();
        parsed.symbols.function_signatures[id.idx()].get_or_resolve_with(
            || {
                // TODO: error
                panic!("function signature depends on itself recursively")
            },
            || {
                let function = &parsed.ast[id];
                // TODO: don't always allow inferring function
                self.check_signature(function, module, &parsed.ast)
            },
        )
    }

    pub fn check_signature_with_type(
        &self,
        func_id: (ModuleId, ast::FunctionId),
        ty: &UnresolvedType,
        scope: ScopeId,
        ty_module: ModuleId,
    ) -> Result<(), SignatureError> {
        let signature = self.get_signature(func_id.0, func_id.1);
        let func_signature_span =
            |compiler: &Self| compiler.get_module_ast(func_id.0)[func_id.1].signature_span;
        match ty {
            UnresolvedType::Infer(_) => Ok(()),
            UnresolvedType::Function {
                span_and_return_type: _,
                params: _,
            } => todo!("check partial function type annotation"),
            UnresolvedType::Unresolved(path, generics) => {
                match self.resolve_path(ty_module, scope, *path) {
                    Def::Invalid => Err(SignatureError),
                    Def::Type(ty) => {
                        if let Some((generics, generics_span)) = generics
                            && !generics.is_empty()
                        {
                            self.errors
                                .emit(ty_module, Error::UnexpectedGenerics.at_span(*generics_span));
                            return Err(SignatureError);
                        }
                        let TypeFull::Instance(BaseType::Function, return_and_params) =
                            self.types.lookup(ty)
                        else {
                            let span = func_signature_span(self);
                            self.errors.emit(
                                func_id.0,
                                Error::MismatchedType {
                                    expected: self
                                        .types
                                        .display(ty, &signature.generics)
                                        .to_string(),
                                    found: "a function".to_owned(),
                                }
                                .at_span(span),
                            );
                            return Err(SignatureError);
                        };
                        match signature.fits_function_type(return_and_params) {
                            Ok(true) => Ok(()),
                            Ok(false) => {
                                let span = func_signature_span(self);
                                self.errors.emit(
                                    func_id.0,
                                    Error::MismatchedType {
                                        expected: self
                                            .types
                                            .display(ty, &signature.generics)
                                            .to_string(),
                                        found: "TODO: display function type".to_owned(),
                                    }
                                    .at_span(span),
                                );
                                Err(SignatureError)
                            }
                            Err(InvalidTypeError) => Err(SignatureError),
                        }
                    }
                    _ => {
                        self.errors
                            .emit(ty_module, Error::TypeExpected.at_span(path.span()));
                        Err(SignatureError)
                    }
                }
            }
            _ => {
                let mut expected = String::new();
                ty.to_string(&mut expected, self.get_module_ast(ty_module).src());
                let span = func_signature_span(self);
                self.errors.emit(
                    func_id.0,
                    Error::MismatchedType {
                        expected,
                        found: "a function".to_owned(),
                    }
                    .at_span(span),
                );
                Err(SignatureError)
            }
        }
    }

    pub fn resolve_generics(
        &self,
        generics: &[GenericDef<()>],
        module: ModuleId,
        scope: ScopeId,
        ast: &Ast,
    ) -> Generics {
        let generics = generics
            .iter()
            .map(|def| {
                let requirements = def
                    .bounds
                    .iter()
                    .map(|bound| match self.resolve_path(module, scope, bound.path) {
                        Def::Trait(trait_module, trait_id) => {
                            let trait_id = (trait_module, trait_id);
                            let generic_count = self.get_trait_generic_count(trait_id);
                            if generic_count as usize != bound.generics.len() {
                                self.errors.emit(
                                    module,
                                    Error::InvalidGenericCount {
                                        expected: generic_count,
                                        found: bound.generics.len() as _,
                                    }
                                    .at_span(bound.generics_span),
                                );
                                todo!("handle invalid bounds")
                            }
                            let generics = bound
                                .generics
                                .iter()
                                .map(|ty| self.resolve_type(ty, module, scope))
                                .collect();
                            TraitBound { trait_id, generics }
                        }
                        Def::Invalid => todo!("handle invalid bounds"),
                        _ => {
                            self.errors
                                .emit(module, Error::TraitExpected.at_span(bound.path.span()));
                            todo!("handle invalid bounds")
                        }
                    })
                    .collect();
                (def.name(ast.src()).to_owned(), requirements)
            })
            .collect();
        Generics { generics }
    }

    pub fn check_signature(
        &self,
        function: &ast::Function,
        module: ModuleId,
        ast: &Ast,
    ) -> Signature {
        let scope = function.scope;
        let generics = self.resolve_generics(&function.generics.types, module, scope, ast);

        let params: Box<[_]> = function
            .params
            .iter()
            .map(|(name_span, ty)| {
                let name = ast[*name_span].into();
                let ty = self.resolve_type(ty, module, scope);
                (name, ty)
            })
            .collect();

        let named_params: Box<[_]> = function
            .named_params
            .iter()
            .map(|param| {
                let name: Box<str> = ast[param.name].into();
                let function_name = &ast[function.associated_name];
                let value_name = self.module_path(module) + "." + function_name + ".param$" + &name;
                let (value, ty) = match eval::value_expr(
                    self,
                    module,
                    scope,
                    ast,
                    param.default_value,
                    &param.ty,
                    &value_name,
                ) {
                    Ok(val) => val,
                    Err(err) => {
                        self.errors.emit(
                            module,
                            Error::EvalFailed(err).at_span(ast[param.default_value].span(ast)),
                        );
                        (ConstValue::Undefined, Type::Invalid)
                    }
                };
                (name, ty, Some(self.add_const_value(value, ty)))
            })
            .collect();
        let return_type = if matches!(function.return_type, UnresolvedType::Infer(_)) {
            Type::Unit
        } else {
            self.resolve_type(&function.return_type, module, scope)
        };
        let callconv = self
            .get_and_check_function_callconv(ast, function, &params, &named_params)
            .expect("todo: handle invalid callconv");
        Signature {
            params,
            named_params,
            varargs: function.varargs,
            return_type,
            generics,
            span: function.signature_span,
            callconv,
        }
    }

    pub fn get_trait_name(&self, module: ModuleId, id: ast::TraitId) -> &str {
        let parsed = self.modules[module.idx()].parsed.get().unwrap();
        let trait_def = &parsed.ast[id];
        &parsed.ast.src()[trait_def.associated_name.range()]
    }

    pub fn get_trait_generic_count(&self, trait_id: (ModuleId, TraitId)) -> u8 {
        // subtract the self generic
        (self.get_parsed_module(trait_id.0).ast[trait_id.1]
            .generics
            .types
            .len()
            - 1)
        .try_into()
        .unwrap()
    }

    /// returns None when the trait can't be resolved, an error was already emitted in that case
    pub fn get_checked_trait(&self, module: ModuleId, id: ast::TraitId) -> Option<&CheckedTrait> {
        let parsed = self.get_parsed_module(module);
        parsed.symbols.traits[id.idx()]
            .get_or_resolve_with(
                || {
                    let span = parsed.ast[id].span(parsed.ast.scopes());
                    self.errors
                        .emit(module, Error::RecursiveDefinition.at_span(span));
                    None
                },
                || Some(check::trait_def(self, &parsed.ast, (module, id))),
            )
            .as_ref()
    }

    pub fn get_impl_candidates<T: Copy>(
        &self,
        trait_id: (ModuleId, ast::TraitId),
        ty: T,
        instance: Option<BaseType>,
        trait_instance: impl Clone + ExactSizeIterator<Item = T>,
        mut type_fits: impl FnMut(Type, T, &Types, &mut [Option<T>]) -> bool,
    ) -> Candidates<'_, T> {
        tracing::debug!(target: "traitsolve", "❓ Finding instance of {trait_id:?}");
        // TODO: this is definitely wrong in some edge cases
        let mut impl_generics = Vec::new();
        let Some(checked_trait) = self.get_checked_trait(trait_id.0, trait_id.1) else {
            return Candidates::Invalid;
        };
        let impls = instance
            .iter()
            .filter_map(|&base| {
                self.get_base_type_def(base)
                    .inherent_trait_impls
                    .get(&trait_id)
            })
            .flatten()
            .chain(&checked_trait.impls);
        let mut found = None;
        'impls: for impl_ in impls {
            impl_generics.clear();
            impl_generics.resize_with(impl_.generics.count().into(), || None);
            if !type_fits(impl_.impl_ty, ty, &self.types, &mut impl_generics) {
                tracing::debug!(target: "traitsolve", "❌ Instance candidate type is not a match");
                continue 'impls;
            }
            let trait_instance = trait_instance.clone();
            debug_assert_eq!(trait_instance.len(), impl_.trait_instance.len());
            for (&impl_ty, ty) in impl_.trait_instance.iter().zip(trait_instance) {
                if !type_fits(impl_ty, ty, &self.types, &mut impl_generics) {
                    tracing::debug!(target: "traitsolve", "❌ Instance generic type is not a match");
                    continue 'impls;
                }
            }
            let impl_generics = impl_generics
                .iter()
                .map(|ty| ty.expect("Impl generics were not properly instantiated"))
                .collect();
            tracing::debug!(target: "traitsolve", "✅ Matching instance found");
            if found.is_some() {
                tracing::debug!(target: "traitsolve", "⚠️ Multiple qualifying candidates");
                return Candidates::Multiple;
            }
            found = Some(SelectedInstance {
                module: impl_.impl_module,
                impl_generics,
                impl_ty: impl_.impl_ty,
                trait_instance: &impl_.trait_instance,
                functions: &impl_.functions,
            });
        }
        if let Some(instance) = found {
            Candidates::Unique(instance)
        } else {
            Candidates::None
        }
    }

    pub fn get_hir(&self, module: ModuleId, id: ast::FunctionId) -> &CheckedFunction {
        let checked = &self.modules[module.idx()].parsed.get().unwrap().symbols;
        checked.functions[id.idx()].get_or_resolve_with(
            || panic!("checked function depends on itself recursively"),
            || check::function(self, module, id, &mut ()),
        )
    }

    pub fn get_function_name(&self, module: ModuleId, function: ast::FunctionId) -> &str {
        let ast = &self.modules[module.idx()].parsed.get().unwrap().ast;
        &ast[ast[function].associated_name]
    }

    pub fn get_base_type_generic_count(&self, ty: BaseType) -> u8 {
        self.types.get_base(ty).generic_count
    }

    pub fn get_base_type_def(&self, ty: BaseType) -> &ResolvedTypeDef {
        self.types.get_base(ty).resolved.get_or_resolve_with(
            || todo!("handle recursive type definition"),
            || check::type_def(self, ty),
        )
    }

    pub fn is_uninhabited(&self, ty: Type, instance: &Instance) -> Result<bool, InvalidTypeError> {
        Ok(match self.types.lookup(ty) {
            TypeFull::Instance(BaseType::Invalid, _) => return Err(InvalidTypeError),
            TypeFull::Instance(BaseType::Array, g) => {
                let &[item, count] = g else { unreachable!() };
                let TypeFull::Const(n) = self.types.lookup(count) else {
                    unreachable!()
                };
                n != 0 && self.is_uninhabited(item, instance)?
            }
            TypeFull::Instance(base, instance_generics) => {
                let instance = Instance {
                    types: instance_generics,
                    outer: Some(instance),
                };
                let def = self.get_base_type_def(base);
                match &def.def {
                    ResolvedTypeContent::Builtin(_) => false,
                    ResolvedTypeContent::Struct(def) => def
                        .all_fields()
                        .try_any(|(_, ty)| self.is_uninhabited(ty, &instance))?,
                    ResolvedTypeContent::Enum(def) => {
                        def.variants.iter().try_all(|(_, _, args)| {
                            args.iter()
                                .try_any(|&ty| self.is_uninhabited(ty, &instance))
                        })?
                    }
                }
            }
            TypeFull::Tuple {
                members,
                named_members,
            } => members
                .iter()
                .copied()
                .chain(named_members.iter().map(|(_, ty)| *ty))
                .try_any(|ty| self.is_uninhabited(ty, instance))?,
            // if no instance is provided, the type is not known to be uninhabited
            TypeFull::Generic(i) => {
                if instance.is_empty() {
                    self.is_uninhabited(instance[i], &instance.outer())?
                } else {
                    false
                }
            }
            TypeFull::FunctionItem { .. } | TypeFull::Const(_) => false,
        })
    }

    pub fn get_checked_global(&self, module: ModuleId, id: GlobalId) -> &(ConstValue, Type) {
        let parsed = self.get_parsed_module(module);
        let ast = &parsed.ast;
        parsed.symbols.globals[id.idx()].get_or_resolve_with(
            || {
                let span = ast[id].span;
                self.errors
                    .emit(module, Error::RecursiveDefinition.at_span(span));
                (ConstValue::Undefined, Type::Invalid)
            },
            || {
                let global = &ast[id];
                let (const_value, ty) = match eval::def_expr(
                    self,
                    module,
                    global.scope,
                    ast,
                    global.val,
                    &global.name,
                    global.name_span,
                    &global.ty,
                ) {
                    Def::ConstValue(id) => self.const_values[id.idx()].clone(),
                    Def::Invalid => (ConstValue::Undefined, Type::Invalid),
                    def => {
                        if !matches!(def, Def::Invalid) {
                            let error = Error::ExpectedValue.at_span(ast[global.val].span(ast));
                            self.errors.emit(module, error);
                        }
                        (ConstValue::Undefined, Type::Invalid)
                    }
                };
                (const_value, ty)
            },
        )
    }

    pub fn check_complete_project(&self, project: ProjectId) {
        let root = self.projects[project.idx()].root_module;
        let mut modules_to_check = VecDeque::from([root]);
        while let Some(module) = modules_to_check.pop_front() {
            let parsed = self.get_parsed_module(module);
            for scope in parsed.ast.scope_ids() {
                for (name, def) in &parsed.ast[scope].definitions {
                    match *def {
                        ast::Definition::Use { path, id, .. } => {
                            self.resolve_use(parsed, module, scope, id, path);
                        }
                        ast::Definition::Expr { id, name_span, .. } => {
                            let def = self.get_def_expr(module, scope, name, name_span, id);
                            if let &Def::Function(module, id) = def {
                                self.get_hir(module, id);
                            }
                        }
                        ast::Definition::Module(id) => {
                            // when checking std, each module still gets a reference to std. To
                            // prevent an infinite loop when checking the std library, check that
                            // this isn't the case by comparing against the root module
                            if self.modules[id.idx()].project == project && id != root {
                                modules_to_check.push_back(id);
                            }
                        }
                        ast::Definition::Global(_) | ast::Definition::Generic(_) => {}
                    }
                }
            }

            for id in parsed.ast.type_ids() {
                let parsed = self.modules[module.idx()].parsed.get().unwrap();
                let id = *parsed.symbols.types[id.idx()].get_or_init(|| {
                    let generic_count = parsed.ast[id].generic_count();
                    self.add_type_def(
                        module,
                        id,
                        "<anonymous type>".into(),
                        TSpan::MISSING,
                        generic_count,
                    )
                });
                let resolved = self.get_base_type_def(id);
                for &method in resolved.methods.values() {
                    self.get_hir(module, method);
                }
                for impls in resolved.inherent_trait_impls.values() {
                    for impl_ in impls {
                        for &id in &impl_.functions {
                            self.get_hir(module, id);
                        }
                    }
                }
            }

            for id in parsed.ast.global_ids() {
                self.get_checked_global(module, id);
            }

            for id in parsed.ast.trait_ids() {
                if let Some(checked) = self.get_checked_trait(module, id) {
                    for impl_ in &checked.impls {
                        for &id in &impl_.functions {
                            self.get_hir(module, id);
                        }
                    }
                }
            }
        }
    }

    pub fn get_ir_function_id(
        &self,
        ir: &mut ir::Environment,
        dialects: &Dialects,
        instances: &mut Instances,
        to_generate: &mut Vec<FunctionToGenerate>,
        module: ModuleId,
        id: ast::FunctionId,
        generics: Box<[Type]>,
    ) -> Option<ir::FunctionId> {
        instances.get_or_create_function(module, id, generics, |generics| {
            let signature = self.get_signature(module, id);
            let checked = self.get_hir(module, id);
            let func =
                irgen::declare_function(self, ir, checked, signature, module, generics).ok()?;
            let ir_id = ir.add_function(dialects.main, func);
            to_generate.push(FunctionToGenerate {
                ir_id,
                module,
                ast_function_id: id,
                generics: generics.into(),
            });
            Some(ir_id)
        })
    }

    pub fn generate_ir_body(
        &self,
        ir: &mut ir::Environment,
        dialects: &Dialects,
        instances: &mut Instances,
        to_generate: &mut Vec<FunctionToGenerate>,
        f: FunctionToGenerate,
    ) {
        let _enter = tracing::span!(
            target: "irgen",
            tracing::Level::INFO,
            "generate_ir_body",
            function = ir[f.ir_id].name,
        )
        .entered();

        let checked = self.get_hir(f.module, f.ast_function_id);
        assert_eq!(
            checked.generic_count as usize,
            f.generics.len(),
            "a function instance queued for ir generation has an invalid generic count"
        );

        if let BodyOrTypes::Body(body) = &checked.body_or_types {
            let callconv = self.get_signature(f.module, f.ast_function_id).callconv;
            let return_type = ir[f.ir_id].return_type().unwrap();
            let (builder, params) = ir::builder::Builder::begin_function(&mut *ir, f.ir_id);
            let (body, types) = irgen::lower_hir(
                self,
                dialects,
                instances,
                builder,
                body,
                to_generate,
                &f.generics,
                params,
                return_type,
                callconv,
            );
            ir[f.ir_id].attach_body(body);
            ir[f.ir_id].overwrite_types(types);
        }
    }

    pub fn print_project_hir(&mut self, project: ProjectId) {
        let project = self.get_project(project);
        let mut module_queue = VecDeque::from([project.root_module]);
        while let Some(module) = module_queue.pop_front() {
            let ast = self.get_module_ast(module);
            let functions = ast.function_ids();
            let checked = &self.modules[module.idx()].parsed.get().unwrap().symbols;
            for function in functions {
                let Some(hir) = checked.functions[function.idx()].get() else {
                    continue;
                };
                let generics = &self.get_signature(module, function).generics;
                tracing::debug!(target: "hir", function = hir.name, "\n{}", hir.display(self, generics));
            }
            module_queue.extend(self.get_parsed_module(module).child_modules.iter().copied())
        }
    }

    /// Emit project ir starting from a root function (for example the main function) while
    /// generating all functions recursively that are called by that function
    pub fn emit_ir_from_root(
        &self,
        ir: &mut ir::Environment,
        dialects: &Dialects,
        instances: &mut Instances,
        root: (ModuleId, FunctionId),
    ) -> Option<ir::FunctionId> {
        let mut to_generate = Vec::new();
        let id = self.get_ir_function_id(
            ir,
            dialects,
            instances,
            &mut to_generate,
            root.0,
            root.1,
            Box::new([]),
        );
        while let Some(f) = to_generate.pop() {
            self.generate_ir_body(ir, dialects, instances, &mut to_generate, f);
        }
        id
    }

    /// Emit the ir for all non-generic top-level functions in a project and functions called by them.
    pub fn emit_whole_project_ir(
        &self,
        ir: &mut Environment,
        dialects: &Dialects,
        instances: &mut Instances,
        project: ProjectId,
    ) {
        let project = self.get_project(project);
        let mut module_queue = VecDeque::from([project.root_module]);
        let mut to_generate = Vec::new();
        while let Some(module) = module_queue.pop_front() {
            let ast = self.get_module_ast(module);
            let functions = ast.function_ids();
            for function in functions {
                if ast[function].generics.types.is_empty() {
                    self.get_ir_function_id(
                        ir,
                        dialects,
                        instances,
                        &mut to_generate,
                        module,
                        function,
                        Box::new([]),
                    );
                }
            }
        }
        while let Some(f) = to_generate.pop() {
            self.generate_ir_body(ir, dialects, instances, &mut to_generate, f);
        }
    }

    pub fn verify_main_and_add_entry_point(
        &self,
        ir: &mut ir::Environment,
        dialects: &Dialects,
        main: (ModuleId, FunctionId),
        main_ir: ir::FunctionId,
    ) -> Result<ir::FunctionId, Option<CompileError>> {
        let main_signature = self.get_signature(main.0, main.1);
        check::verify_main_signature(self, main_signature).map(|()| {
            let main_signature = self.get_signature(main.0, main.1);
            let entry_point = irgen::entry_point(main_ir, main_signature.return_type, ir, dialects);
            ir.add_function(dialects.main, entry_point)
        })
    }

    /// prints all errors, consuming them and returns true if any fatal errors were present
    pub fn print_errors(&mut self) -> bool {
        let working_directory = std::env::current_dir().ok();
        use color_format::cprintln;
        let errors = std::mem::replace(&mut self.errors, ProjectErrors::new());
        let print = |ast: &Ast, path: &Path, error: &CompileError| {
            if error.span == TSpan::MISSING {
                println!(
                    "[missing location]: {} {}",
                    error.err.conclusion(),
                    error.err.details().unwrap_or_default(),
                );
            } else {
                error::print(
                    &error.err,
                    TSpan::new(error.span.start, error.span.end),
                    ast.src(),
                    path,
                );
            }
        };
        let by_file = errors.by_file.into_inner();
        let err_count: usize = by_file.values().map(|errors| errors.error_count()).sum();
        let module_path = |module: ModuleId| match &self.modules[module.idx()].storage {
            ModuleStorage::Path(path) => {
                let relative = working_directory
                    .as_ref()
                    .and_then(|cwd| pathdiff::diff_paths(path, cwd));
                relative.unwrap_or(path.clone())
            }
            ModuleStorage::String(_) => {
                PathBuf::from(&self.projects[self.modules[module.idx()].project.idx()].name)
            }
        };
        if err_count != 0 {
            cprintln!(
                "#r<Finished with #u;r!<{}> error{}>",
                err_count,
                if err_count == 1 { "" } else { "s" }
            );
            for (module, errors) in by_file {
                if errors.error_count() == 0 {
                    continue;
                }
                let ast = self.get_module_ast(module);
                let path = module_path(module);

                for error in &errors.errors {
                    print(ast, &path, error);
                }
            }
            return true;
        }
        let warning_count: usize = by_file.values().map(|errors| errors.warning_count()).sum();
        if warning_count != 0 {
            cprintln!(
                "#r<Finished with #u;r!<{warning_count}> warning{}>",
                if warning_count == 1 { "" } else { "s" }
            );
            for (module, errors) in by_file {
                if errors.warning_count() == 0 {
                    continue;
                }
                let ast = self.get_module_ast(module);
                let path = module_path(module);
                for warning in &errors.warnings {
                    print(ast, &path, warning);
                }
            }
        }
        false
    }

    pub fn resolve_builtins(&mut self, std: ProjectId) {
        self.builtins.std = std;
        self.builtins.primitives = builtins::Primitives::resolve(self);
    }

    pub fn mangle_name(
        &self,
        checked: &CheckedFunction,
        module: ModuleId,
        generics: &[Type],
    ) -> String {
        if checked.is_extern() {
            return checked.name.clone();
        }
        let mut name = self.module_path(module) + "." + &checked.name;
        if !generics.is_empty() {
            // 2 characters per type because of commas and brackets is the minimum
            name.reserve(1 + 2 * generics.len());
            name.push('[');
            let mut first = true;
            for &ty in generics {
                if first {
                    first = false;
                } else {
                    name.push(',');
                }
                use std::fmt::Write;
                // don't print colors in mangled name
                color_format::config::set_override(false);
                // this type is instantiated so we can pass empty generics here
                write!(name, "{}", self.types.display(ty, &Generics::EMPTY)).unwrap();
                color_format::config::unset_override();
            }
            name.push(']');
        }
        name
    }

    pub fn module_path(&self, module: ModuleId) -> String {
        let module = &self.modules[module.idx()];
        module
            .parent
            .map_or_else(String::new, |parent| self.module_path(parent) + ".")
            + &module.name
    }
}

#[derive(Debug)]
pub enum ProjectError {
    CantAccessPath(std::io::Error, PathBuf),
    NonexistentPath(PathBuf),
}

pub struct Resolvable<T> {
    resolving: OnceCell<()>,
    resolved: OnceCell<T>,
}
impl<T> Resolvable<T> {
    pub fn new() -> Self {
        Self {
            resolving: OnceCell::new(),
            resolved: OnceCell::new(),
        }
    }

    pub fn resolved(value: T) -> Self {
        Self {
            resolving: OnceCell::new(),
            resolved: OnceCell::from(value),
        }
    }

    pub fn get_or_resolve_with(
        &self,
        already_resolving: impl FnOnce() -> T,
        resolve: impl FnOnce() -> T,
    ) -> &T {
        if let Some(value) = self.resolved.get() {
            return value;
        }
        if self.resolving.set(()).is_err() {
            _ = self.resolved.set(already_resolving());
        } else {
            // if setting failed here it was set by already_resolving in a recursive context
            // and emitted an error already
            _ = self.resolved.set(resolve());
        }
        self.resolved.get().unwrap()
    }

    pub fn start_resolving(&self) {
        debug_assert!(
            self.resolved.get().is_none(),
            "Started resolving but was already resolved"
        );
        self.resolving.set(()).expect("started resolving twice");
    }

    pub fn get(&self) -> Option<&T> {
        self.resolved.get()
    }

    pub fn resolving(&self) -> bool {
        self.resolving.get().is_some()
    }

    pub fn put(&self, value: T) -> &T {
        debug_assert!(
            self.resolving.get().is_some(),
            "Resolving was not set before putting resolved"
        );
        self.resolved
            .set(value)
            .unwrap_or_else(|_| panic!("Value was put multiple times"));
        self.resolved.get().unwrap()
    }

    pub fn put_immediate(&self, value: T) -> &T {
        debug_assert!(
            self.resolving.get().is_none(),
            "Resolving was set before putting resolved despite using put_immediate"
        );
        self.resolved
            .set(value)
            .unwrap_or_else(|_| panic!("Value was put multiple times"));
        self.resolved.get().unwrap()
    }
}

id::id!(VarId);
id::id!(CaptureId);

#[derive(Debug)]
pub enum LocalScopeParent<'a> {
    Some(&'a LocalScope<'a>),
    ClosedOver {
        scope: &'a LocalScope<'a>,
        /// mapping from outer vars to inner vars
        captures: &'a RefCell<IndexMap<VarId, VarId>>,
        outer_vars: &'a RefCell<Vec<Var>>,
    },
    None,
}

#[derive(Debug)]
pub struct LocalScope<'p> {
    pub parent: LocalScopeParent<'p>,
    pub variables: DHashMap<Box<str>, VarId>,
    pub module: ModuleId,
    /// should only be none if this scope has a parent
    pub static_scope: Option<ScopeId>,
}
#[derive(Debug)]
pub enum LocalItem {
    Var(VarId),
    Def(Def),
    Invalid,
}
impl<'p> LocalScope<'p> {
    pub fn resolve(
        &self,
        name: &str,
        name_span: TSpan,
        compiler: &Compiler,
        vars: &mut Vec<Var>,
    ) -> LocalItem {
        if let Some(var) = self.variables.get(name) {
            LocalItem::Var(*var)
        } else if let Some(def) = self
            .static_scope
            .and_then(|static_scope| compiler.get_scope_def(self.module, static_scope, name))
        {
            LocalItem::Def(def)
        } else {
            match &self.parent {
                LocalScopeParent::Some(parent) => parent.resolve(name, name_span, compiler, vars),
                LocalScopeParent::ClosedOver {
                    scope,
                    captures,
                    outer_vars,
                } => {
                    let mut outer_vars = outer_vars.borrow_mut();
                    let local = scope.resolve(name, name_span, compiler, &mut outer_vars);
                    match local {
                        LocalItem::Var(outer) => {
                            // hir.add_captured_var(outer, ty)
                            let mut captures = captures.borrow_mut();
                            let next_capture_idx = captures.len() as u32;
                            LocalItem::Var(*captures.entry(outer).or_insert_with(|| {
                                let ty = outer_vars[outer.idx()].ty();
                                let id = VarId(vars.len() as _);
                                vars.push(Var::Capture {
                                    outer,
                                    ty,
                                    capture_idx: next_capture_idx,
                                });
                                id
                            }))
                        }
                        LocalItem::Def(def) => LocalItem::Def(def),
                        LocalItem::Invalid => LocalItem::Invalid,
                    }
                }
                LocalScopeParent::None => {
                    if let Some(static_parent) = self.static_scope.and_then(|static_scope| {
                        compiler.get_module_ast(self.module)[static_scope].parent
                    }) {
                        LocalItem::Def(compiler.resolve_in_scope(
                            self.module,
                            static_parent,
                            name,
                            ModuleSpan {
                                module: self.module,
                                span: name_span,
                            },
                        ))
                    } else {
                        compiler.errors.emit(
                            self.module,
                            Error::UnknownIdent(name.into()).at_span(name_span),
                        );
                        LocalItem::Invalid
                    }
                }
            }
        }
    }

    pub fn get_innermost_static_scope(&self) -> ScopeId {
        let mut current_local = self;
        loop {
            if let Some(static_scope) = current_local.static_scope {
                return static_scope;
            }
            // a local scope has to have a parent scope if it doesn't have a static scope
            // associated with it
            current_local = match &current_local.parent {
                LocalScopeParent::Some(parent) => parent,
                LocalScopeParent::ClosedOver { scope, .. } => scope,
                LocalScopeParent::None => unreachable!(),
            };
        }
    }

    pub fn child(&self) -> LocalScope<'_> {
        LocalScope {
            parent: LocalScopeParent::Some(self),
            variables: DHashMap::default(),
            module: self.module,
            static_scope: None,
        }
    }

    pub fn exit<H: Hooks>(mut self, hooks: &mut H, hir: &mut HIRBuilder) {
        hooks.on_exit_scope(&mut self, hir);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Def {
    Invalid,
    Function(ModuleId, ast::FunctionId),
    BaseType(BaseType),
    Type(Type),
    Trait(ModuleId, ast::TraitId),
    ConstValue(ConstValueId),
    Module(ModuleId),
    Global(ModuleId, GlobalId),
}
impl Def {
    /// returns a potentially changed Def if the type matches and "found" string on type mismatch
    /// or Err(None) on an invalid type
    pub fn check_with_type(
        self,
        module: ModuleId,
        scope: ScopeId,
        compiler: &Compiler,
        ty: &UnresolvedType,
    ) -> Result<Def, &'static str> {
        match self {
            Def::Invalid => Ok(Def::Invalid),
            Def::Function(_, _) | Def::Module(_) | Def::BaseType(_) => match ty {
                UnresolvedType::Infer(_) => Ok(self),
                _ => Err("a function"),
            },
            Def::Type(_) => match compiler.resolve_type(ty, module, scope) {
                Type::Invalid => Ok(Def::Invalid),
                Type::Type => Ok(self),
                _ => Err("type"),
            },
            Def::Trait(_, _) => match ty {
                UnresolvedType::Infer(_) => Ok(self),
                _ => Err("a trait"),
            },
            Def::ConstValue(_const_val_id) => {
                todo!("check const value with type")
                /*
                // PERF: cloning ConstValue
                let (const_val, const_val_ty) = compiler.const_values[const_val_id.idx()].clone();
                let ty = compiler.resolve_type(ty, module, scope);
                match const_val.check_with_type(module, scope, compiler, ty)? {
                    None => Ok(self),
                    Some(new_val) => Ok(Def::ConstValue(compiler.add_const_value(new_val))),
                }
                */
            }
            Def::Global(_, _) => todo!("handle globals in constants"),
        }
    }

    pub fn dump(&self, compiler: &Compiler) {
        match self {
            Self::Invalid => print!("<invalid>"),
            Self::Function(module, id) => print!("Function({}, {})", module.idx(), id.idx()),
            Self::BaseType(id) => print!("GenericType({id:?})"),
            Self::Type(ty) => print!("Type({ty:?})"),
            Self::Trait(module, id) => print!("Trait({}, {})", module.idx(), id.idx()),
            Self::ConstValue(value) => {
                let (val, ty) = &compiler.const_values[value.idx()];
                val.dump();
                print!(": {}", compiler.types.display(*ty, &Generics::EMPTY));
            }
            Self::Module(id) => print!("Module({})", id.idx()),
            Self::Global(module, id) => print!("Global({}, {})", module.idx(), id.idx()),
        }
    }

    pub fn get_span(&self, compiler: &mut Compiler) -> Option<ModuleSpan> {
        match self {
            Self::Invalid => None,
            &Self::Function(module, id) => {
                let ast = compiler.get_module_ast(module);
                Some(ModuleSpan {
                    module,
                    span: ast[ast[id].scope].span,
                })
            }
            Self::BaseType(_) | Self::Type(_) => todo!("spans of types"),
            &Self::Trait(module, id) => {
                let ast = compiler.get_module_ast(module);
                Some(ModuleSpan {
                    module,
                    span: ast[ast[id].scope].span,
                })
            }
            Self::ConstValue(_id) => todo!("spans of const values"),
            // TODO: somehow show reference to module
            &Self::Module(_) => Some(ModuleSpan::MISSING),
            &Self::Global(module, id) => {
                let ast = compiler.get_module_ast(module);
                Some(ModuleSpan {
                    module,
                    span: ast[id].span,
                })
            }
        }
    }
}

pub struct Project {
    pub name: String,
    pub root: Option<PathBuf>,
    pub root_module: ModuleId,
    pub dependencies: Vec<ProjectId>,
}
pub struct Module {
    pub name: Box<str>,
    pub storage: ModuleStorage,
    pub project: ProjectId,
    pub parsed: OnceCell<ParsedModule>,
    pub root: ModuleId,
    pub parent: Option<ModuleId>,
}
impl Module {
    pub fn at_path(
        name: Box<str>,
        path: PathBuf,
        project: ProjectId,
        root: ModuleId,
        parent: Option<ModuleId>,
    ) -> Self {
        Self {
            name,
            storage: ModuleStorage::Path(path),
            project,
            parsed: OnceCell::new(),
            root,
            parent,
        }
    }
}

pub enum ModuleStorage {
    /// Must always be a canonical path
    Path(PathBuf),
    String(Box<str>),
}
impl ModuleStorage {
    pub fn path(&self) -> Option<&Path> {
        let ModuleStorage::Path(path) = self else {
            return None;
        };
        Some(path)
    }
}

pub struct ParsedModule {
    pub ast: Ast,
    /// used by lsp
    pub version: Option<i32>,
    pub child_modules: Vec<ModuleId>,
    pub symbols: ModuleSymbols,
}

type NamedParams = Box<[(Box<str>, Type, Option<ConstValueId>)]>;

pub struct Signature {
    pub params: Box<[(Box<str>, Type)]>,
    pub named_params: NamedParams,
    pub varargs: bool,
    pub return_type: Type,
    pub generics: Generics,
    pub span: TSpan,
    pub callconv: CallConv,
}
impl Signature {
    pub fn total_arg_count(&self) -> usize {
        self.params.len() + self.named_params.len()
    }

    pub fn all_params(&self) -> impl ExactSizeIterator<Item = (&str, Type)> {
        AllParamsIter {
            params: &self.params,
            named_params: &self.named_params,
        }
    }

    pub fn fits_function_type(&self, return_and_params: &[Type]) -> Result<bool, InvalidTypeError> {
        if self.callconv != CallConv::default() {
            return Ok(false);
        }
        if self.generics.count() != 0 {
            return Ok(false);
        }
        if self.varargs {
            return Ok(false);
        }
        if !self.named_params.is_empty() {
            return Ok(false);
        }
        if self.params.len() != return_and_params.len() - 1 {
            return Ok(false);
        }
        for ((_, arg), &ty_arg) in self.all_params().zip(&return_and_params[1..]) {
            if !arg.is_same_as(ty_arg)? {
                return Ok(false);
            }
        }
        if !self.return_type.is_same_as(return_and_params[0])? {
            return Ok(false);
        }
        Ok(true)
    }

    /// used for checking if a function in a trait impl is compatible with a base type.
    /// The signature is allowed to have looser trait bound requirements than the base.
    pub fn compatible_with(
        &self,
        types: &Types,
        base: &Signature,
        generics_offset: u8,
        base_generics_offset: u8,
        base_generics: &[Type],
    ) -> Result<Option<ImplIncompatibility>, InvalidTypeError> {
        if self.callconv != base.callconv {
            return Ok(Some(ImplIncompatibility::CallConv));
        }
        if !self.generics.compatible_with(
            types,
            &base.generics,
            generics_offset,
            base_generics_offset,
            base_generics,
        )? {
            return Ok(Some(ImplIncompatibility::Generics));
        }
        if self.varargs != base.varargs {
            return Ok(Some(if base.varargs {
                ImplIncompatibility::VarargsNeeded
            } else {
                ImplIncompatibility::NoVarargsNeeded
            }));
        }
        if self.params.len() != base.params.len() {
            return Ok(Some(ImplIncompatibility::ArgCount {
                base: base.params.len() as _,
                impl_: self.params.len() as _,
            }));
        }
        // TODO: should the default value also be compared?
        for (((_, arg), (_, base_arg)), i) in self.all_params().zip(base.all_params()).zip(0..) {
            if !arg.is_same_as(types.instantiate(base_arg, base_generics))? {
                return Ok(Some(ImplIncompatibility::Arg(i)));
            }
        }
        if !self
            .return_type
            .is_same_as(types.instantiate(base.return_type, base_generics))?
        {
            return Ok(Some(ImplIncompatibility::ReturnType));
        }
        Ok(None)
    }
}

struct AllParamsIter<'a> {
    params: &'a [(Box<str>, Type)],
    named_params: &'a [(Box<str>, Type, Option<ConstValueId>)],
}
impl<'a> Iterator for AllParamsIter<'a> {
    type Item = (&'a str, Type);
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(((name, ty), params)) = self.params.split_first() {
            self.params = params;
            return Some((name, *ty));
        }
        if let Some(((name, ty, _), named_params)) = self.named_params.split_first() {
            self.named_params = named_params;
            return Some((name, *ty));
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
    }
}
impl<'a> ExactSizeIterator for AllParamsIter<'a> {
    fn len(&self) -> usize {
        self.params.len() + self.named_params.len()
    }
}

#[derive(Debug, Clone)]
pub struct Generics {
    generics: Vec<(String, Vec<TraitBound>)>,
}
impl Generics {
    pub const EMPTY: Self = Self {
        generics: Vec::new(),
    };

    pub fn new(generics: Vec<(String, Vec<TraitBound>)>) -> Self {
        Self { generics }
    }

    pub fn count(&self) -> u8 {
        self.generics.len() as u8
    }

    pub fn check_bound_satisfied(
        &self,
        generic: u8,
        bound: Bound,
        types: &mut TypeTable,
        compiler: &Compiler,
    ) -> bool {
        let mut found = None;
        for generic_bound in self.generics[generic as usize].1.iter() {
            if bound.trait_id == generic_bound.trait_id {
                assert!(
                    found.is_none(),
                    "TODO: handle multiple potential generic bounds"
                );
                found = Some(generic_bound);
            }
        }
        found.is_some_and(|found| {
            debug_assert_eq!(found.generics.len() as u8, bound.generics.count as u8);
            for (id, &ty) in bound.generics.iter().zip(found.generics.iter()) {
                if types
                    .try_specify(id, TypeInfo::Known(ty), self, compiler)
                    .is_err()
                {
                    return false;
                }
            }
            true
        })
    }

    pub fn instantiate(&self, table: &mut TypeTable, types: &Types, span: TSpan) -> LocalTypeIds {
        let generics = table.add_multiple_unknown(self.generics.len() as _);
        for ((_, bounds), r) in self.generics.iter().zip(generics.iter()) {
            let bound_ids = table.add_missing_bounds(bounds.len() as _);
            for (bound, r) in bounds.iter().zip(bound_ids.iter()) {
                // TODO: generic trait bounds, assuming no generics for now
                let trait_generics = table.add_multiple_unknown(bound.generics.len() as _);
                for (&ty, r) in bound.generics.iter().zip(trait_generics.iter()) {
                    // instantiate generics on requirements of bounds
                    let ty =
                        table.from_type_instance(types, ty, LocalOrGlobalInstance::Local(generics));
                    table.replace(r, ty);
                }
                let bound = crate::typing::Bound {
                    trait_id: bound.trait_id,
                    generics: trait_generics,
                    span,
                };
                table.replace_bound(r, bound);
            }
            table.replace(r, TypeInfo::Unknown(bound_ids));
        }
        generics
    }

    pub fn compatible_with(
        &self,
        types: &Types,
        base: &Self,
        offset: u8,
        base_offset: u8,
        base_generics: &[Type],
    ) -> Result<bool, InvalidTypeError> {
        if self.generics.len() as u8 - offset != base.generics.len() as u8 - base_offset {
            return Ok(false);
        }
        for ((_, bounds), (_, base_bounds)) in self
            .generics
            .iter()
            .skip(offset.into())
            .zip(base.generics.iter().skip(base_offset.into()))
        {
            'bounds: for bound in bounds {
                'base_bounds: for base_bound in base_bounds {
                    if base_bound.trait_id != bound.trait_id {
                        continue;
                    }
                    for (&a, &b) in base_bound.generics.iter().zip(&bound.generics) {
                        if !a.is_same_as(types.instantiate(b, base_generics))? {
                            continue 'base_bounds;
                        }
                    }
                    // bound is satisfied by base_bound
                    continue 'bounds;
                }
                // no base bound satisfies the bound
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn get_name(&self, i: u8) -> &str {
        &self.generics[usize::from(i)].0
    }

    pub fn get_bounds(&self, i: u8) -> &[TraitBound] {
        &self.generics[usize::from(i)].1
    }
}

#[derive(Debug, Clone)]
pub struct TraitBound {
    pub trait_id: (ModuleId, TraitId),
    pub generics: Box<[Type]>,
}

#[derive(Debug)]
pub struct ResolvedTypeDef {
    pub def: ResolvedTypeContent,
    pub module: ModuleId,
    pub methods: DHashMap<Box<str>, FunctionId>,
    pub generics: Generics,
    pub inherent_trait_impls: DHashMap<(ModuleId, TraitId), Vec<check::traits::Impl>>,
}
impl Default for ResolvedTypeDef {
    fn default() -> Self {
        Self {
            def: ResolvedTypeContent::Builtin(BuiltinType::Invalid),
            module: ModuleId::from_inner(0),
            methods: DHashMap::default(),
            generics: Generics::EMPTY,
            inherent_trait_impls: DHashMap::default(),
        }
    }
}

#[derive(Debug)]
pub enum ResolvedTypeContent {
    Builtin(BuiltinType),
    Struct(ResolvedStructDef),
    Enum(ResolvedEnumDef),
}
impl ResolvedTypeContent {
    // TODO: cleanup
    // pub fn uninhabited(
    //     &self,
    //     compiler: &mut Compiler,
    //     generics: &[TypeOld],
    // ) -> Result<bool, InvalidTypeError> {
    //     Ok(match self {
    //         Self::Builtin(_) => false, // TODO: tuples with uninhabited fields may be uninhabited
    //         Self::Struct(struct_def) => {
    //             struct_def
    //                 .all_fields()
    //                 .try_fold(false, |s, (_, field_ty)| {
    //                     Ok(s || compiler.types.is_generic_uninhabited(field_ty, generics)?)
    //                 })?
    //         }
    //         Self::Enum(enum_def) => {
    //             enum_def
    //                 .variants
    //                 .iter()
    //                 .try_fold(true, |s, (_, variant_param_types)| {
    //                     Ok(s && variant_param_types.iter().try_fold(false, |s, ty| {
    //                         Ok(s || compiler.uninhabited(ty, generics)?)
    //                     })?)
    //                 })?
    //         }
    //     })
    // }
}

#[derive(Debug)]
pub struct ResolvedStructDef {
    pub fields: Box<[(Box<str>, Type)]>,
    pub named_fields: NamedParams,
}
impl ResolvedStructDef {
    pub fn all_fields(&self) -> impl Iterator<Item = (&str, Type)> {
        self.fields
            .iter()
            .map(|(name, ty)| (&**name, *ty))
            .chain(self.named_fields.iter().map(|(name, ty, _)| (&**name, *ty)))
    }

    pub fn field_count(&self) -> u32 {
        (self.fields.len() + self.named_fields.len()) as u32
    }

    /// get the index of a field while getting the element types of all fields
    pub fn get_indexed_field<'a, H: Hooks>(
        &self,
        ctx: &mut crate::check::Ctx<H>,
        generics: impl Into<LocalOrGlobalInstance<'a>>,
        name: &str,
    ) -> Option<(u32, TypeInfoOrIdx)> {
        let generics = generics.into();
        for ((field_name, ty), index) in self.all_fields().zip(0..) {
            let ty = ctx.from_type_instance(ty, generics);
            if field_name == name {
                return Some((index, ty));
            }
        }
        None
    }
}

pub enum ResolvedPrimitive {
    Primitive(Primitive),
    Infer,
    Invalid,
    Other,
}

#[derive(Debug, Clone)]
pub struct ResolvedEnumDef {
    // TODO: pull variant out into struct
    pub variants: Box<[(Box<str>, u64, Box<[Type]>)]>,
}
impl ResolvedEnumDef {
    pub fn get_by_name(&self, name: &str) -> Option<(&str, u64, &[Type])> {
        self.variants
            .iter()
            .find_map(|(variant_name, ordinal, args)| {
                (&**variant_name == name).then_some((&**variant_name, *ordinal, &**args))
            })
    }
}

pub struct ResolvableTypeDef {
    pub module: ModuleId,
    pub id: ast::TypeId,
    pub name: Box<str>,
    pub name_span: TSpan,
    pub generic_count: u8,
    pub resolved: Resolvable<ResolvedTypeDef>,
}
impl Default for ResolvableTypeDef {
    fn default() -> Self {
        Self {
            module: ModuleId::from_inner(0),
            id: ast::TypeId::from_inner(0),
            name: "".into(),
            name_span: TSpan::MISSING,
            generic_count: 0,
            resolved: Resolvable::new(),
        }
    }
}

pub struct ModuleSymbols {
    pub function_signatures: Box<[Resolvable<Signature>]>,
    pub functions: Box<[Resolvable<CheckedFunction>]>,
    pub types: Box<[OnceCell<BaseType>]>,
    pub globals: Box<[Resolvable<(ConstValue, Type)>]>,
    pub traits: Box<[Resolvable<Option<CheckedTrait>>]>,
    pub def_exprs: Box<[Resolvable<Def>]>,
    pub uses: Box<[Resolvable<Def>]>,
}
impl ModuleSymbols {
    pub fn empty(ast: &Ast) -> Self {
        Self {
            function_signatures: (0..ast.function_count())
                .map(|_| Resolvable::new())
                .collect(),
            functions: (0..ast.function_count())
                .map(|_| Resolvable::new())
                .collect(),
            types: vec![OnceCell::new(); ast.type_count()].into_boxed_slice(),
            globals: (0..ast.global_count()).map(|_| Resolvable::new()).collect(),
            traits: (0..ast.trait_count()).map(|_| Resolvable::new()).collect(),
            def_exprs: (0..ast.def_expr_count())
                .map(|_| Resolvable::new())
                .collect(),
            uses: (0..ast.use_count()).map(|_| Resolvable::new()).collect(),
        }
    }
}

pub struct CheckedFunction {
    pub name: String,
    pub params: LocalTypeIds,
    pub varargs: bool,
    pub return_type: LocalTypeId,
    pub generic_count: u8,
    pub body_or_types: BodyOrTypes,
}
impl CheckedFunction {
    pub fn display<'a>(
        &'a self,
        compiler: &'a Compiler,
        generics: &'a Generics,
    ) -> display::CheckedFunctionDisplay<'a> {
        display::CheckedFunctionDisplay {
            function: self,
            compiler,
            generics,
        }
    }

    pub fn types(&self) -> &[Type] {
        match &self.body_or_types {
            BodyOrTypes::Body(hir) => hir.types(),
            BodyOrTypes::Types(types) => types,
        }
    }

    pub fn is_extern(&self) -> bool {
        matches!(self.body_or_types, BodyOrTypes::Types(_))
    }
}
pub enum BodyOrTypes {
    Body(Hir),
    Types(Box<[Type]>),
}
impl Index<LocalTypeId> for CheckedFunction {
    type Output = Type;
    fn index(&self, index: LocalTypeId) -> &Self::Output {
        &self.types()[index.idx()]
    }
}
impl Index<LocalTypeIds> for CheckedFunction {
    type Output = [Type];
    fn index(&self, index: LocalTypeIds) -> &Self::Output {
        &self.types()[index.idx as usize..index.idx as usize + index.count as usize]
    }
}

pub struct CheckedTrait {
    pub name: Box<str>,
    pub generics: Generics,
    pub functions: Vec<Signature>,
    pub functions_by_name: DHashMap<String, u16>,
    pub impls: Vec<traits::Impl>,
}

#[derive(Default)]
pub struct Instances {
    functions: DHashMap<(ast::ModuleId, ast::FunctionId, Box<[Type]>), ir::FunctionId>,
    globals: DHashMap<(ast::ModuleId, ast::GlobalId), Option<ir::GlobalId>>,
}
impl Instances {
    pub fn new() -> Self {
        Self::default()
    }

    /// Gets exising instance or creates a new one, returning None if any of the types are invalid
    pub fn get_or_create_function(
        &mut self,
        module: ast::ModuleId,
        function: ast::FunctionId,
        generics: Box<[Type]>,
        create: impl FnOnce(&[Type]) -> Option<ir::FunctionId>,
    ) -> Option<ir::FunctionId> {
        Some(match self.functions.entry((module, function, generics)) {
            hash_map::Entry::Occupied(occupied_entry) => *occupied_entry.get(),
            hash_map::Entry::Vacant(vacant_entry) => {
                let func = create(&vacant_entry.key().2)?;
                *vacant_entry.insert(func)
            }
        })
    }

    pub fn get_or_create_global(
        &mut self,
        module: ast::ModuleId,
        global: ast::GlobalId,
        create: impl FnOnce() -> Option<ir::GlobalId>,
    ) -> Option<ir::GlobalId> {
        *self.globals.entry((module, global)).or_insert_with(create)
    }
}

pub struct FunctionToGenerate {
    pub ir_id: ir::FunctionId,
    pub module: ModuleId,
    pub ast_function_id: ast::FunctionId,
    pub generics: Box<[Type]>,
}

pub fn function_name(
    ast: &Ast,
    function: &ast::Function,
    module: ModuleId,
    id: FunctionId,
) -> String {
    if function.associated_name != TSpan::EMPTY {
        ast.src()[function.associated_name.range()].to_owned()
    } else {
        // If no name is associated, assign a name based on unique ids.
        // This will be the case for lambdas right now, maybe a better name is possible
        format!("function_{}_{}", module.idx(), id.idx())
    }
}

pub struct SignatureError;

#[derive(Default, Clone, Copy)]
pub struct Instance<'a> {
    pub types: &'a [Type],
    pub outer: Option<&'a Instance<'a>>,
}
impl<'a> Instance<'a> {
    pub const EMPTY: Self = Self {
        types: &[],
        outer: None,
    };

    pub fn outer(&self) -> Instance<'_> {
        self.outer.copied().unwrap_or(Self::EMPTY)
    }

    pub fn new(types: &'a [Type]) -> Self {
        Self { types, outer: None }
    }

    fn is_empty(&self) -> bool {
        self.types.is_empty() && self.outer.is_none()
    }
}
impl<'a> Index<u8> for Instance<'a> {
    type Output = Type;
    fn index(&self, index: u8) -> &Self::Output {
        &self.types[index as usize]
    }
}
