use dmap::DHashMap;
use error::Error;
use error::span::TSpan;
use parser::ast::{self, Ast, TraitId};
use parser::ast::{FunctionId, ModuleId};

use super::{LocalTypeIds, TypeInfoOrIdx, TypeTable};

use crate::types::{BaseType, TypeFull, Types};
use crate::{
    Compiler,
    compiler::Signature,
    compiler::{CheckedTrait, Generics},
};
use crate::{InvalidTypeError, Type};

pub fn trait_def(compiler: &Compiler, ast: &Ast, id: (ModuleId, TraitId)) -> CheckedTrait {
    let module = id.0;
    let def = &ast[id.1];
    // don't include self type in generics, it is handled seperately
    let generics = compiler.resolve_generics(&def.generics.types[1..], id.0, def.scope, ast);
    let mut functions_by_name = dmap::with_capacity(def.functions.len());
    let functions: Vec<Signature> = def
        .functions
        .iter()
        .zip(0..)
        .map(|(function, function_index)| {
            let name = ast[function.name].to_owned();
            let prev = functions_by_name.insert(name, function_index);
            if prev.is_some() {
                compiler
                    .errors
                    .emit(module, Error::DuplicateDefinition.at_span(function.name));
            }
            compiler.check_signature(&ast[function.function], module, ast)
        })
        .collect();
    let impls = def
        .impls
        .iter()
        .filter_map(|impl_| {
            let impl_ty = compiler.resolve_type(&impl_.implemented_type, module, impl_.base.scope);
            check_impl(
                compiler,
                &impl_.base,
                impl_ty,
                module,
                ast,
                generics.count(),
                &functions,
                &functions_by_name,
                &ast.src()[def.associated_name.range()],
            )
        })
        .collect();
    CheckedTrait {
        name: if def.associated_name == TSpan::MISSING {
            "<unnamed trait>".into()
        } else {
            ast[def.associated_name].into()
        },
        generics,
        functions,
        functions_by_name,
        impls,
    }
}

pub fn check_impl(
    compiler: &Compiler,
    impl_: &ast::BaseImpl,
    impl_ty: Type,
    module: ModuleId,
    ast: &Ast,
    trait_generic_count: u8,
    trait_functions: &[Signature],
    trait_functions_by_name: &DHashMap<String, u16>,
    trait_name: &str,
) -> Option<Impl> {
    let impl_generics = compiler.resolve_generics(&impl_.generics.types, module, impl_.scope, ast);

    let trait_generics: Vec<_> = impl_
        .trait_generics
        .iter()
        .map(|generic| compiler.resolve_type(generic, module, impl_.scope))
        .collect();

    if trait_generics.len() as u8 != trait_generic_count {
        compiler.errors.emit(
            module,
            Error::InvalidGenericCount {
                expected: trait_generic_count,
                found: trait_generics.len() as _,
            }
            .at_span(impl_.trait_generics_span),
        );
        return None;
    }

    let mut function_ids = vec![ast::FunctionId::from_inner(u32::MAX); trait_functions.len()];
    let base_generics: Box<[Type]> = std::iter::once(impl_ty)
        .chain(trait_generics.clone())
        .collect();
    let base_offset = base_generics.len() as u8;
    for method in &impl_.functions {
        let name = &ast[method.name];
        let Some(&function_idx) = trait_functions_by_name.get(name) else {
            compiler.errors.emit(
                module,
                Error::NotATraitMember {
                    trait_name: trait_name.to_owned(),
                    function: name.to_owned(),
                }
                .at_span(method.name),
            );
            continue;
        };
        if function_ids[function_idx as usize] != ast::FunctionId::from_inner(u32::MAX) {
            compiler
                .errors
                .emit(module, Error::DuplicateDefinition.at_span(method.name));
            continue;
        }

        let signature = compiler.get_signature(module, method.function);
        let trait_signature = &trait_functions[function_idx as usize];
        let compatible = signature.compatible_with(
            &compiler.types,
            trait_signature,
            impl_generics.count(),
            base_offset,
            &base_generics,
        );
        match compatible {
            Ok(None) | Err(InvalidTypeError) => {}
            Ok(Some(incompat)) => compiler.errors.emit(
                module,
                Error::TraitSignatureMismatch(incompat).at_span(method.name),
            ),
        }
        function_ids[function_idx as usize] = method.function;
    }
    let mut unimplemented = Vec::new();
    for (name, &i) in trait_functions_by_name {
        if function_ids[i as usize] == ast::FunctionId::from_inner(u32::MAX) {
            unimplemented.push(name.clone());
        }
    }
    if !unimplemented.is_empty() {
        compiler.errors.emit(
            module,
            Error::NotAllFunctionsImplemented { unimplemented }.at_span(ast[impl_.scope].span),
        );
    }
    Some(Impl {
        generics: impl_generics,
        trait_instance: trait_generics,
        impl_ty,
        impl_module: module,
        functions: function_ids,
    })
}

#[derive(Debug)]
pub enum Candidates<'a, Ty> {
    None,
    Invalid,
    Multiple,
    Unique(SelectedInstance<'a, Ty>),
}

#[derive(Debug)]
pub struct SelectedInstance<'a, Ty> {
    pub module: ModuleId,
    pub impl_generics: Box<[Ty]>,
    pub impl_ty: Type,
    pub trait_instance: &'a [Type],
    pub functions: &'a [FunctionId],
}

pub fn match_instance(
    implemented_ty: Type,
    ty: Type,
    types: &Types,
    instance: &mut [Option<Type>],
) -> bool {
    match types.lookup(implemented_ty) {
        TypeFull::Generic(i) => {
            let instance_ty = &mut instance[usize::from(i)];
            if instance_ty.is_some_and(|instance_ty| instance_ty != ty) {
                return false;
            }
            *instance_ty = Some(ty);
            true
        }
        TypeFull::Instance(implemented_base, implemented_generics) => match types.lookup(ty) {
            TypeFull::Instance(BaseType::Invalid, _) => true,
            TypeFull::Instance(base, generics) => {
                if base != implemented_base {
                    return false;
                }
                if generics.len() != implemented_generics.len() {
                    return false;
                }
                for (&generic, &implemented_generic) in
                    generics.iter().zip(implemented_generics.iter())
                {
                    if !match_instance(implemented_generic, generic, types, instance) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        },
        TypeFull::FunctionItem { .. } => {
            unreachable!("FunctionItems are un-nameable and shouldn't occur in impls")
        }
        TypeFull::Const(implemented_n) => {
            let TypeFull::Const(n) = types.lookup(ty) else {
                return false;
            };
            implemented_n == n
        }
    }
}

#[derive(Debug)]
pub struct Impl {
    pub generics: Generics,
    pub trait_instance: Vec<Type>,
    pub impl_ty: Type,
    pub impl_module: ModuleId,
    pub functions: Vec<FunctionId>,
}
impl Impl {
    /// returns the instantiated impl_ty and the impl_generics
    pub fn instantiate(
        &self,
        trait_generics: LocalTypeIds,
        function_generics: &Generics,
        types: &mut TypeTable,
        compiler: &Compiler,
        span: TSpan,
    ) -> TypeInfoOrIdx {
        let impl_generics = self.generics.instantiate(types, &compiler.types, span);
        debug_assert_eq!(trait_generics.count, self.trait_instance.len() as u32);
        for (idx, &ty) in trait_generics.iter().zip(&self.trait_instance) {
            types.specify_type_instance(
                idx,
                ty,
                impl_generics,
                function_generics,
                compiler,
                // span should never be needed since the requirements should have already been
                // checked so no errors should occur
                || unreachable!(),
            );
        }
        types.from_type_instance(&compiler.types, self.impl_ty, impl_generics)
    }
}
