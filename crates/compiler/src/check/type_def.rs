use dmap::DHashMap;
use error::Error;
use parser::ast::{self, FunctionId, ModuleId, TraitId};

use crate::{
    Compiler, Def, Type,
    compiler::{ResolvedEnumDef, ResolvedStructDef, ResolvedTypeContent, ResolvedTypeDef},
    types::{BaseType, TypeFull},
};

use super::traits;

pub fn type_def(
    compiler: &Compiler,
    module: ModuleId,
    ast_id: ast::TypeId,
    base: BaseType,
) -> ResolvedTypeDef {
    let ast = compiler.get_module_ast(module);
    let def = &ast[ast_id];
    let generics = compiler.resolve_generics(&def.generics.types, module, def.scope, ast);
    let resolved_def = match &def.content {
        ast::TypeContent::Struct { members } => {
            let named_fields = members
                .iter()
                .map(|member| {
                    (
                        ast[member.name].into(),
                        compiler.resolve_type(&member.ty, module, def.scope),
                        None,
                    )
                })
                .collect();
            ResolvedTypeContent::Struct(ResolvedStructDef {
                fields: Box::new([]),
                named_fields,
            })
        }
        ast::TypeContent::Enum { variants } => {
            let variants = variants
                .iter()
                .zip(0..)
                .map(|(variant, ordinal)| {
                    let variant_name = ast[variant.name_span].into();
                    let args = variant
                        .args
                        .iter()
                        .map(|ty| compiler.resolve_type(ty, module, def.scope))
                        .collect();
                    (variant_name, ordinal, args)
                })
                .collect();

            ResolvedTypeContent::Enum(ResolvedEnumDef { variants })
        }
    };

    let mut methods: DHashMap<Box<str>, FunctionId> = def
        .methods
        .iter()
        .map(|(name, method)| (name.clone().into_boxed_str(), method.function))
        .collect();

    let mut inherent_trait_impls: DHashMap<(ModuleId, TraitId), Vec<traits::Impl>> = dmap::new();

    let implemented_generics: Box<[Type]> = (0..generics.count())
        .map(|i| compiler.types.intern(TypeFull::Generic(i)))
        .collect();
    let implemented_ty = compiler
        .types
        .intern(TypeFull::Instance(base, &implemented_generics));
    for trait_impl in &def.impls {
        let trait_def = compiler.resolve_path(module, def.scope, trait_impl.implemented_trait);
        let trait_id = match trait_def {
            Def::Trait(module, id) => (module, id),
            Def::Invalid => continue,
            _ => {
                compiler.errors.emit(
                    module,
                    Error::TraitExpected.at_span(trait_impl.implemented_trait.span()),
                );
                continue;
            }
        };
        let Some(checked_trait) = compiler.get_checked_trait(trait_id.0, trait_id.1) else {
            // can't check trait impl because the trait was invalid
            continue;
        };
        let checked_impl = traits::check_impl(
            compiler,
            &trait_impl.base,
            implemented_ty,
            module,
            ast,
            checked_trait.generics.count(),
            &checked_trait.functions,
            &checked_trait.functions_by_name,
            &checked_trait.name,
        );
        if let Some(checked) = checked_impl {
            // TODO: replace this with proper attribute system with resolval
            let add_to_methods = !trait_impl
                .attributes
                .iter()
                .any(|attr| &ast[attr.path.span()] == "hidden");
            debug_assert_eq!(
                checked_trait.functions_by_name.len(),
                checked.functions.len()
            );
            if add_to_methods {
                for (name, &idx) in &checked_trait.functions_by_name {
                    let id = checked.functions[idx as usize];
                    let prev = methods.insert(name.clone().into_boxed_str(), id);
                    if prev.is_some() {
                        // duplicate function, find the correct function in the impl functions for the span
                        let span = trait_impl
                            .base
                            .functions
                            .iter()
                            .find_map(|m| (m.function == id).then_some(m.name))
                            .unwrap();
                        compiler
                            .errors
                            .emit(module, Error::DuplicateDefinition.at_span(span));
                    }
                }
            }
            inherent_trait_impls
                .entry(trait_id)
                .or_default()
                .push(checked);
        }
    }
    ResolvedTypeDef {
        def: resolved_def,
        module,
        methods,
        generics,
        inherent_trait_impls,
    }
}
