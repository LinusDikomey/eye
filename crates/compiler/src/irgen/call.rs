use ir::{Ref, Refs};

use crate::{
    callconv::CallConv,
    compiler::{Dialects, builtins},
    crash_point,
    hir::{Node, NodeId, NodeIds, Var},
    irgen::{self, Ctx, NoReturn, Result, ValueOrPlace, lower},
    types::TypeFull,
    typing::{LocalTypeId, LocalTypeIds},
};

pub fn gen_call(
    ctx: &mut Ctx,
    function: NodeId,
    args: NodeIds,
    arg_types: LocalTypeIds,
    return_ty: LocalTypeId,
    noreturn: bool,
) -> Result<ValueOrPlace> {
    let Dialects { mem, cf, .. } = ctx.dialects;

    let return_ty = ctx.get_hir_type(return_ty)?;
    let res = if let Node::FunctionItem {
        function: (module, id),
        generics: call_generics,
        ty: _,
    } = ctx.hir[function]
    {
        let signature = ctx.compiler.get_signature(module, id);
        let callconv = signature.callconv;
        if (module, id) == builtins::get_intrinsic(ctx.compiler) {
            let arg_refs = args
                .iter()
                .skip(1)
                .map(|arg| lower(ctx, arg))
                .collect::<Result<Vec<_>>>()?;
            let Node::StringLiteral(intrinsic) = &ctx.hir[args.iter().next().unwrap()] else {
                panic!("expected string literal passed to intrinsic call");
            };
            return irgen::intrinsics::call_intrinsic(ctx, intrinsic, &arg_refs);
        }
        // PERF: make it possible to write refs directly into the ir to avoid collecting here
        let mut arg_refs = Vec::new();
        gen_args(ctx, &mut arg_refs, args, arg_types, callconv)?;
        let call_generics = call_generics
            .iter()
            .map(|generic| {
                ctx.compiler
                    .types
                    .instantiate(ctx.hir[generic], ctx.generics)
            })
            .collect();
        let Some(func) = ctx.get_ir_id(module, id, call_generics) else {
            crash_point!(ctx)
        };
        let return_ty = ctx.builder.types.add(return_ty);
        ctx.builder.append((func, arg_refs, return_ty))
    } else {
        // fn pointers currently can't have different calling conventions
        let callconv = CallConv::Eye;
        debug_assert_eq!(args.count, arg_types.count);
        let func = lower(ctx, function)?;
        let call_ptr = ir::FunctionId {
            module: mem.id(),
            function: ir::dialect::Mem::CallPtr.id(),
        };
        // PERF: reuse allocation in the future?
        let mut call_ptr_args = Vec::with_capacity(args.count as usize + 1);
        call_ptr_args.push(func);
        gen_args(ctx, &mut call_ptr_args, args, arg_types, callconv)?;
        let return_ty = ctx.builder.types.add(return_ty);
        ctx.builder.append((call_ptr, call_ptr_args, return_ty))
    };
    if noreturn {
        let ret = ctx.builder.append_undef(ctx.return_ty);
        ctx.builder.append(cf.Ret(ret));
        Err(NoReturn)
    } else {
        Ok(ValueOrPlace::Value(res))
    }
}

pub fn gen_args(
    ctx: &mut Ctx,
    ir_args: &mut Vec<Ref>,
    args: NodeIds,
    arg_types: LocalTypeIds,
    callconv: CallConv,
) -> Result<()> {
    match callconv {
        CallConv::Eye => {
            ir_args.reserve(args.count as usize);
            for arg in args.iter() {
                ir_args.push(lower(ctx, arg)?);
            }
        }
        CallConv::FnTrait => {
            debug_assert_eq!(args.count, 2);
            let first = lower(ctx, args.nth(0).unwrap())?;
            // unit self argument is always skipped in fn_trait callconv
            if first != Ref::UNIT {
                ir_args.push(first);
            }
            let TypeFull::Tuple {
                members: args_tuple_types,
                named_members: &[],
            } = ctx
                .compiler
                .types
                .lookup(ctx.hir[arg_types.nth(1).unwrap()])
            else {
                unreachable!()
            };
            let args_tuple = lower(ctx, args.nth(1).unwrap())?;
            ir_args.reserve(args_tuple_types.len());
            let args_tuple_types = ctx.get_multiple_types(args_tuple_types.iter().copied())?;
            for (ty, i) in args_tuple_types.iter().zip(0..) {
                let arg = ctx
                    .builder
                    .append(ctx.dialects.tuple.MemberValue(args_tuple, i, ty));
                ir_args.push(arg);
            }
        }
    }
    Ok(())
}

pub fn assign_args_to_vars(ctx: &mut Ctx, params: Refs, callconv: CallConv) -> Result<()> {
    match callconv {
        CallConv::Eye => {
            debug_assert_eq!(params.count(), ctx.hir.params.len() as _);
            for (param, &var) in params.iter().zip(&ctx.hir.params) {
                if matches!(ctx.hir.vars[var.idx()], Var::CapturesParam(_)) {
                    continue;
                }
                ctx.builder
                    .append(ctx.dialects.mem.Store(ctx.vars[var.idx()].0, param));
            }
        }
        CallConv::FnTrait => {
            debug_assert_eq!(ctx.hir.params.len(), 2);
            ctx.builder
                .append(ctx.dialects.mem.Store(ctx.vars[0].0, params.nth(0)));
            let args_tuple_ty = ctx.hir[ctx.hir.vars[ctx.hir.params[1].idx()].ty()];
            let TypeFull::Tuple {
                members,
                named_members: &[],
            } = ctx.compiler.types.lookup(args_tuple_ty)
            else {
                unreachable!()
            };
            let ir_members = ctx.get_multiple_types(members.iter().copied())?;
            assert_eq!(ir_members.count() + 1, params.count());
            let args_tuple_ir_ty = ctx.builder.types.add(ir::Type::Tuple(ir_members));
            let mut args_tuple = ctx.builder.append_undef(args_tuple_ir_ty);
            for (param, i) in params.iter().skip(1).zip(0..) {
                args_tuple = ctx.builder.append(ctx.dialects.tuple.InsertMember(
                    args_tuple,
                    i,
                    param,
                    args_tuple_ir_ty,
                ));
            }
            ctx.builder
                .append(ctx.dialects.mem.Store(ctx.vars[1].0, args_tuple));
        }
    }
    Ok(())
}
