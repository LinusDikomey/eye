use crate::{
    Compiler, Type,
    compiler::{Instance, ResolvedTypeContent},
    types::{BuiltinType, TypeFull},
};

pub fn get(
    compiler: &Compiler,
    ir: &ir::Environment,
    ir_types: &mut ir::Types,
    ty: Type,
    instance: Instance,
) -> Option<ir::Type> {
    // PERF: could cache converted types so that no duplicates are created in the ir
    Some(match compiler.types.lookup(ty) {
        TypeFull::Instance(base, type_generics) => {
            // PERF: cloning generics
            let type_generics: Box<[Type]> = type_generics.into();
            let resolved = compiler.get_base_type_def(base);
            match &resolved.def {
                ResolvedTypeContent::Builtin(builtin) => match builtin {
                    BuiltinType::Invalid => return None,
                    BuiltinType::Unit => ir::Type::UNIT,
                    BuiltinType::I8 => ir::Type::Primitive(ir::Primitive::I8.into()),
                    BuiltinType::U8 => ir::Type::Primitive(ir::Primitive::U8.into()),
                    BuiltinType::I16 => ir::Type::Primitive(ir::Primitive::I16.into()),
                    BuiltinType::U16 => ir::Type::Primitive(ir::Primitive::U16.into()),
                    BuiltinType::I32 => ir::Type::Primitive(ir::Primitive::I32.into()),
                    BuiltinType::U32 => ir::Type::Primitive(ir::Primitive::U32.into()),
                    BuiltinType::I64 => ir::Type::Primitive(ir::Primitive::I64.into()),
                    BuiltinType::U64 => ir::Type::Primitive(ir::Primitive::U64.into()),
                    BuiltinType::I128 => ir::Type::Primitive(ir::Primitive::I128.into()),
                    BuiltinType::U128 => ir::Type::Primitive(ir::Primitive::U128.into()),
                    BuiltinType::F32 => ir::Type::Primitive(ir::Primitive::F32.into()),
                    BuiltinType::F64 => ir::Type::Primitive(ir::Primitive::F64.into()),
                    BuiltinType::Type => ir::Type::Primitive(ir::Primitive::U64.into()), // TODO: convert Type Type
                    BuiltinType::Tuple => {
                        let members: &[Type] = &type_generics;
                        let refs = get_multiple(
                            compiler,
                            ir,
                            ir_types,
                            members.iter().copied(),
                            instance,
                        )?;
                        Some(ir::Type::Tuple(refs))
                    }?,
                    BuiltinType::Pointer | BuiltinType::Function => {
                        ir::Type::Primitive(ir::Primitive::Ptr.into())
                    }
                    BuiltinType::Array => {
                        let n = type_generics[1];
                        let elem = get(compiler, ir, ir_types, type_generics[0], instance)?;
                        let TypeFull::Const(n) = compiler.types.lookup(n) else {
                            return None;
                        };
                        ir::Type::Array(
                            ir_types.add(elem),
                            n.try_into().expect("TODO: handle 64-bit arrays?"),
                        )
                    }
                },
                ResolvedTypeContent::Struct(def) => {
                    let elems =
                        ir_types.add_multiple((0..def.field_count()).map(|_| ir::Type::UNIT));
                    for ((_, field), r) in def.all_fields().zip(elems.iter()) {
                        let ty = get(
                            compiler,
                            ir,
                            ir_types,
                            field,
                            Instance {
                                types: &type_generics,
                                outer: Some(&instance),
                            },
                        )?;
                        ir_types[r] = ty;
                    }
                    ir::Type::Tuple(elems)
                }
                ResolvedTypeContent::Enum(def) => {
                    let mut accumulated_layout = ir::Layout::EMPTY;
                    // TODO: reduce number of variants if reduced by never types in variants
                    let ordinal_type = int_from_variant_count(def.variants.len() as u32);
                    let mut has_content = false;
                    'variants: for (_variant_name, _ordinal, args) in &*def.variants {
                        if !args.is_empty() {
                            has_content = true;
                        }
                        let elems =
                            ir_types.add_multiple((0..args.len() + 1).map(|_| ir::Type::UNIT));
                        let mut elem_iter = elems.iter();
                        ir_types[elem_iter.next().unwrap()] = ordinal_type;
                        for (&arg, r) in args.iter().zip(elem_iter) {
                            let Some(elem) = get(
                                compiler,
                                ir,
                                ir_types,
                                arg,
                                Instance {
                                    types: &type_generics,
                                    outer: Some(&instance),
                                },
                            ) else {
                                continue 'variants;
                            };
                            ir_types[r] = elem;
                        }
                        let variant_layout =
                            ir::type_layout(ir::Type::Tuple(elems), ir_types, ir.primitives());
                        accumulated_layout.accumulate_variant(variant_layout);
                    }
                    if has_content {
                        type_from_layout(ir_types, accumulated_layout)
                    } else {
                        ordinal_type
                    }
                }
            }
        }
        TypeFull::Generic(i) => get(compiler, ir, ir_types, instance[i], instance.outer())?,
        TypeFull::FunctionItem { .. } | TypeFull::Const(_) => ir::Type::UNIT,
    })
}

pub fn get_multiple(
    compiler: &Compiler,
    ir: &ir::Environment,
    ir_types: &mut ir::Types,
    types: impl ExactSizeIterator<Item = Type>,
    instance: Instance,
) -> Option<ir::TypeIds> {
    let refs = ir_types.add_multiple((0..types.len()).map(|_| ir::Type::UNIT));
    for (elem, ty) in types.into_iter().zip(refs.iter()) {
        let elem = get(compiler, ir, ir_types, elem, instance)?;
        ir_types[ty] = elem;
    }
    Some(refs)
}

pub fn int_from_variant_count(count: u32) -> ir::Type {
    match count {
        0 | 1 => ir::Type::UNIT,
        2 => ir::Primitive::I1.into(),
        3..256 => ir::Primitive::U8.into(),
        256..=0xFF_FF => ir::Primitive::U16.into(),
        _ => ir::Primitive::U32.into(),
    }
}

pub fn get_primitive(p: parser::ast::Primitive) -> ir::Primitive {
    use ir::Primitive as I;
    use parser::ast::Primitive as P;
    match p {
        P::I8 => I::I8,
        P::I16 => I::I16,
        P::I32 => I::I32,
        P::I64 => I::I64,
        P::I128 => I::I128,
        P::U8 => I::U8,
        P::U16 => I::U16,
        P::U32 => I::U32,
        P::U64 => I::U64,
        P::U128 => I::U128,
        P::F32 => I::F32,
        P::F64 => I::F64,
        P::Type => I::U64, // TODO
    }
}

pub fn type_from_layout(ir_types: &mut ir::Types, layout: ir::Layout) -> ir::Type {
    if layout.size == 0 {
        debug_assert_eq!(layout.align.get(), 1);
        return ir::Type::UNIT;
    }
    let base_type = match layout.align.get() {
        1 => ir::Primitive::U8.into(),
        2 => ir::Primitive::U16.into(),
        4 => ir::Primitive::U32.into(),
        8 => ir::Primitive::U64.into(),
        16 => ir::Primitive::U128.into(),
        _ => unreachable!(),
    };
    let base_type_count = layout.size / layout.align.get();
    let u8_count = layout.size % layout.align.get();
    debug_assert!(
        base_type_count > 0,
        "can't represent a type with larger align than size"
    );
    let final_layout = ir_types.add_multiple(
        std::iter::repeat_n(base_type, base_type_count as usize).chain(std::iter::repeat_n(
            ir::Primitive::U8.into(),
            u8_count as usize,
        )),
    );
    ir::Type::Tuple(final_layout)
}
