use ir::{BUILTIN, Ref, TypeIds};
use target::Os;

use super::{Ctx, ValueOrPlace};

pub fn call_intrinsic(ctx: &mut Ctx, intrinsic: &str, args: &[Ref]) -> super::Result<ValueOrPlace> {
    let crate::compiler::Dialects { arith, tuple, .. } = ctx.dialects;
    Ok(ValueOrPlace::Value(match intrinsic {
        "eq" => {
            let bool = ctx.builder.types.add(ir::Type::from(ir::Primitive::I1));
            ctx.builder.append(arith.Eq(args[0], args[1], bool))
        }
        "xor" => ctx
            .builder
            .append(arith.Xor(args[0], args[1], ctx.builder.get_ref_ty(args[0]))),
        "rotate_left" => {
            ctx.builder
                .append(arith.Rol(args[0], args[1], ctx.builder.get_ref_ty(args[0])))
        }
        "rotate_right" => {
            ctx.builder
                .append(arith.Ror(args[0], args[1], ctx.builder.get_ref_ty(args[0])))
        }
        "os" => {
            let os_ordinal = match ctx.compiler.target.os {
                Os::Unknown => 0,
                Os::Linux => 1,
                Os::Windows => 2,
                Os::Darwin => 3,
            };
            // TODO: ordinal-only enums should really just be simple int types
            let ordinal_ty = ctx.builder.types.add(ir::Type::from(ir::Primitive::U8));
            let tuple_ty = ctx
                .builder
                .types
                .add(ir::Type::Tuple(TypeIds::one(ordinal_ty)));
            let tuple_value = ctx.builder.append(BUILTIN.Undef(tuple_ty));
            let ordinal = ctx.builder.append(arith.Int(os_ordinal, ordinal_ty));
            ctx.builder
                .append(tuple.InsertMember(tuple_value, 0, ordinal, tuple_ty))
        }
        _ => panic!("called unknown intrinsic: {intrinsic}"),
    }))
}
