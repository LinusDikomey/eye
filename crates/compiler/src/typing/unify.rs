use crate::{
    Compiler, InvalidTypeError, Type,
    check::expr::type_from_variant_count,
    compiler::{Generics, ResolvedTypeContent},
    types::BaseType,
    typing::{Bounds, LocalOrGlobalInstance, LocalTypeIds, TypeInfoOrIdx},
};

use super::{LocalTypeId, TypeInfo, TypeTable};

impl TypeTable {
    pub fn unify_infos(
        &mut self,
        a: TypeInfo,
        b: TypeInfo,
        function_generics: &Generics,
        compiler: &Compiler,
    ) -> Option<TypeInfoOrIdx> {
        use TypeInfo::*;
        Some(TypeInfoOrIdx::TypeInfo(match (a, b) {
            (Instance(BaseType::Invalid, _), _) | (_, Instance(BaseType::Invalid, _)) => {
                unreachable!("Invalid type should always be represented as Known(Type::Invalid)")
            }
            (Known(Type::Invalid), _) | (_, Known(Type::Invalid)) => TypeInfo::INVALID,
            (Unknown(a), Unknown(b)) => {
                if a.is_empty() && b.is_empty() {
                    Unknown(Bounds::EMPTY)
                } else {
                    // TODO: this might not work and it might be much better to unify duplicate traits
                    // PERF: avoid the vec and allocate into new bounds instead
                    let mut bounds = self.get_bounds(a).to_vec();
                    bounds.extend_from_slice(self.get_bounds(b));
                    Unknown(self.add_bounds(bounds))
                }
            }
            (info, Known(ty)) | (Known(ty), info) => {
                match self.specify_generic_type(info, ty, compiler, function_generics) {
                    Ok(true) => Known(ty),
                    Ok(false) => return None,
                    Err(InvalidTypeError) => Known(Type::Invalid),
                }
            }
            (t, Unknown(bounds)) | (Unknown(bounds), t) => {
                let mut chosen_ty = TypeInfoOrIdx::TypeInfo(t);
                for bound in bounds.iter() {
                    let bound = *self.get_bound(bound);
                    match self.unify_bound_with_info(
                        compiler,
                        function_generics,
                        self.get_info_or_idx(chosen_ty),
                        bound,
                    ) {
                        Ok(Some(new)) => chosen_ty = new,
                        // TODO: attach error context here when it's possible in the future
                        Ok(None) => return None,
                        Err(InvalidTypeError) => return Some(TypeInfo::INVALID.into()),
                    }
                }
                return Some(chosen_ty);
            }
            (Integer, Integer) => Integer,
            (Float, Float) => Float,
            (Instance(t, _), Integer) | (Integer, Instance(t, _)) if t.is_int() => {
                Instance(t, LocalTypeIds::EMPTY)
            }
            (Instance(t, _), Float) | (Float, Instance(t, _)) if t.is_float() => {
                Instance(t, LocalTypeIds::EMPTY)
            }
            (Instance(id_a, generics_a), Instance(id_b, generics_b)) if id_a == id_b => {
                if generics_a.count != generics_b.count {
                    return None;
                }
                for (a, b) in generics_a.iter().zip(generics_b.iter()) {
                    if !self.try_unify(a, b, function_generics, compiler) {
                        return None;
                    }
                }
                a
            }
            (Instance(id, generics), Enum(enum_id)) | (Enum(enum_id), Instance(id, generics)) => {
                return local_enum_with_instance(
                    compiler,
                    self,
                    function_generics,
                    enum_id,
                    id,
                    generics,
                )
                .then_some(TypeInfo::Instance(id, generics).into());
            }
            (Enum(a), Enum(b)) => {
                // always merge into a_variants which becomes the longer variant list to try to avoid
                // having to create new variants if one list is a subset of the other
                let (a, b) = if self.get_enum_variants(a).len() >= self.get_enum_variants(b).len() {
                    (a, b)
                } else {
                    (b, a)
                };
                let Some(&first_a) = self.get_enum_variants(a).first() else {
                    // if a is empty, both enums are empty and just returning one is fine
                    return Some(TypeInfo::Enum(a).into());
                };
                let ordinal_type_idx = self[first_a].args.idx;
                let b_variant_count = self.get_enum_variants(b).len();
                for i in 0..b_variant_count {
                    let b_variants = self.get_enum_variants(b);
                    debug_assert_eq!(
                        b_variants.len(),
                        b_variant_count,
                        "b_variant_count shouldn't change"
                    );
                    let b_id = b_variants[i];
                    let a_variants = self.get_enum_variants(a);
                    let variant = &self[b_id];
                    let a_variant = a_variants
                        .iter()
                        .copied()
                        .find(|&id| self[id].name == variant.name);
                    if let Some(a_variant) = a_variant {
                        let a_variant = &self[a_variant];
                        let ordinal = a_variant.ordinal;
                        // TODO: better errors
                        if a_variant.args.count != variant.args.count {
                            return None;
                        }
                        let a_args = a_variant.args;
                        let b_args = variant.args;
                        if !a_args
                            .iter()
                            .zip(b_args.iter())
                            .skip(1) // skip the ordinal argument
                            .all(|(a, b)| self.try_unify(a, b, function_generics, compiler))
                        {
                            return None;
                        }
                        self[b_id].ordinal = ordinal;
                        self.types[b_args.idx as usize] =
                            TypeInfoOrIdx::Idx(LocalTypeId(ordinal_type_idx));
                    } else {
                        let a_id = self.append_enum_variant(a, variant.name.clone(), variant.args);
                        let ordinal = self[a_id].ordinal;
                        self[b_id].ordinal = ordinal;
                    }
                }
                TypeInfo::Enum(a)
            }
            (
                Tuple {
                    members: a_members,
                    named_members: a_named,
                },
                Tuple {
                    members: b_members,
                    named_members: b_named,
                },
            ) if a_members.count == b_members.count && a_named.count == b_named.count => {
                return (a_members
                    .iter()
                    .zip(b_members.iter())
                    .all(|(a, b)| self.try_unify(a, b, function_generics, compiler))
                    && a_named.iter().zip(b_named.iter()).all(|(a, b)| {
                        a == b
                            || (self[a].name == self[b].name
                                && self.try_unify(
                                    self[a].ty,
                                    self[b].ty,
                                    function_generics,
                                    compiler,
                                ))
                    }))
                .then_some(a.into());
            }
            (BaseTypeItem(a_ty), BaseTypeItem(b_ty)) if a_ty == b_ty => a,
            (TypeItem(a_ty), TypeItem(b_ty)) => {
                // TODO: this could technically cause TypeItem to be converted to type unnecessarily
                // when both the types are the same. Maybe in the future, TypeItem should just hold
                // a Type, not a Type var
                if a_ty != b_ty {
                    return Some(TypeInfo::Known(Type::Type).into());
                }
                a
            }
            (
                MethodItem {
                    module: a_m,
                    function: a_f,
                    generics: a_g,
                },
                MethodItem {
                    module: b_m,
                    function: b_f,
                    generics: b_g,
                },
            ) if a_m == b_m && a_f == b_f => {
                for (a, b) in a_g.iter().zip(b_g.iter()) {
                    self.try_unify(a, b, function_generics, compiler);
                }
                a
            }
            _ => return None,
        }))
    }
}

pub fn local_enum_with_instance<'a>(
    compiler: &Compiler,
    types: &mut TypeTable,
    function_generics: &Generics,
    enum_id: super::InferredEnumId,
    id: BaseType,
    instance: impl Into<LocalOrGlobalInstance<'a>>,
) -> bool {
    let instance = instance.into();
    let resolved = compiler.get_base_type_def(id);
    let ResolvedTypeContent::Enum(def) = &resolved.def else {
        return false;
    };
    let variants = &types.enums[enum_id.idx()];
    if let Some(&first_variant) = variants.variants.first() {
        let ordinal_type = types[first_variant].args.iter().next().unwrap();
        debug_assert!(matches!(
            types.types[ordinal_type.idx()],
            TypeInfoOrIdx::TypeInfo(_)
        ));
        types.types[ordinal_type.idx()] =
            TypeInfo::Known(type_from_variant_count(def.variants.len() as _)).into();
    }
    // iterate by index because we need to borrow types mutably during the loop
    for variant_index in 0..variants.variants.len() {
        let variant = types.enums[enum_id.idx()].variants[variant_index];
        let variant = &mut types.variants[variant.idx()];
        // TODO: make it possible to return specific errors here so it's more clear when an
        // enum doesn't match a definition
        let Some((_, declared_ordinal, declared_args)) = def.get_by_name(&variant.name) else {
            return false;
        };
        variant.ordinal = declared_ordinal;
        // add one because the inferred enum args contain the ordinal type
        if variant.args.count != declared_args.len() as u32 + 1 {
            return false;
        }
        for (arg, &declared_arg) in variant.args.iter().skip(1).zip(declared_args) {
            if types
                .try_specify_type_instance(arg, declared_arg, instance, function_generics, compiler)
                .is_err()
            {
                return false;
            }
        }
    }
    true
}
