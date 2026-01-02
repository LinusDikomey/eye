use crate::{
    Compiler, Type,
    check::expr::{int_primitive_from_variant_count, type_from_variant_count},
    compiler::{Instance, ResolvedTypeContent},
    eval::{self, ConstValue},
    layout::Layout,
    types::{BaseType, TypeFull},
};

#[derive(Debug, Clone, Copy)]
pub struct UndefinedValue;

pub fn translate(
    value: &eval::ConstValue,
    ty: Type,
    compiler: &Compiler,
    b: &mut [u8],
) -> Result<(), UndefinedValue> {
    if ty == Type::Invalid {
        return Err(UndefinedValue);
    }
    // TODO: currently assumes little-endian target arch
    match value {
        eval::ConstValue::Undefined => return Err(UndefinedValue),
        eval::ConstValue::Unit => debug_assert!(b.is_empty()),
        &eval::ConstValue::Int(val) => match ty {
            Type::I8 | Type::U8 => b.copy_from_slice(&[val as u8]),
            Type::I16 | Type::U16 => {
                b.copy_from_slice(&(val as u16).to_le_bytes());
            }
            Type::I32 | Type::U32 => {
                b.copy_from_slice(&(val as u32).to_le_bytes());
            }
            Type::I64 | Type::U64 => {
                b.copy_from_slice(&u64::to_le_bytes(val));
            }
            Type::I128 | Type::U128 => {
                b.copy_from_slice(&(val as u128).to_le_bytes());
            }
            _ if ty == compiler.builtins.primitives.bool => {
                debug_assert!(val < 2);
                b.copy_from_slice(&[val as u8]);
            }
            ty => unreachable!("unexpected type for integer ConstValue: {ty:?}"),
        },
        &eval::ConstValue::Float(val) => match ty {
            Type::F32 => b.copy_from_slice(&(val as f32).to_le_bytes()),
            Type::F64 => b.copy_from_slice(&val.to_le_bytes()),
            _ => unreachable!(),
        },
        eval::ConstValue::Aggregate(elems) => match compiler.types.lookup(ty) {
            TypeFull::Instance(BaseType::Invalid, _) => return Err(UndefinedValue),
            TypeFull::Instance(BaseType::Array, _) => todo!(),
            TypeFull::Instance(base, generics) => {
                let def = compiler.get_base_type_def(base);
                match &def.def {
                    ResolvedTypeContent::Builtin(_) => todo!(),
                    ResolvedTypeContent::Struct(struct_def) => {
                        let mut layout = Layout::EMPTY;
                        debug_assert_eq!(struct_def.field_count() as usize, elems.len());
                        for (val, (_, ty)) in elems.iter().zip(struct_def.all_fields()) {
                            let field_layout = compiler
                                .resolved_layout(
                                    ty,
                                    Instance {
                                        types: generics,
                                        outer: None,
                                    },
                                )
                                .map_err(|_| UndefinedValue)?;
                            let offset = layout.accumulate(field_layout) as usize;
                            translate(
                                val,
                                ty,
                                compiler,
                                &mut b[offset..offset + field_layout.size as usize],
                            )?;
                        }
                    }
                    ResolvedTypeContent::Enum(enum_def) => {
                        let ConstValue::Int(active_variant) = elems[0] else {
                            unreachable!()
                        };
                        let variant_layout =
                            int_primitive_from_variant_count(enum_def.variants.len() as _)
                                .map_or(Layout::EMPTY, Layout::primitive);
                        let ty = type_from_variant_count(enum_def.variants.len() as _);
                        translate(
                            &elems[0],
                            ty,
                            compiler,
                            &mut b[..variant_layout.size as usize],
                        )?;
                        let arg_types = &enum_def.variants[active_variant as usize].2;
                        debug_assert_eq!(elems[1..].len(), arg_types.len());
                        let mut layout = variant_layout;
                        for (arg, &arg_ty) in elems[1..].iter().zip(arg_types) {
                            let arg_layout = compiler
                                .resolved_layout(
                                    arg_ty,
                                    Instance {
                                        types: generics,
                                        outer: None,
                                    },
                                )
                                .map_err(|_| UndefinedValue)?;
                            let offset = layout.accumulate(arg_layout) as usize;
                            translate(
                                arg,
                                arg_ty,
                                compiler,
                                &mut b[offset..offset + arg_layout.size as usize],
                            )?;
                        }
                    }
                }
            }
            TypeFull::Generic(_) | TypeFull::FunctionItem { .. } | TypeFull::Const(_) => {
                unreachable!()
            }
        },
    }
    Ok(())
}
