use std::convert::Infallible;

use dmap::DHashMap;
use ir::{
    BlockGraph, BlockId, Environment, MCReg, ModuleOf, Ref, Type, TypeId, Types,
    dialect::Mem,
    mc::IselCtx,
    modify::{Insert, IrModify},
    rewrite::{ReverseRewriteOrder, Rewrite},
    slots::Slots,
};

use crate::{
    Size,
    arch::arm::isa::{Arm, Reg, RegClass},
    int_size_of_ref,
};

impl crate::InstructionSelector<Arm> for InstructionSelector {
    fn codegen(
        &self,
        env: &ir::Environment,
        body: &ir::FunctionIr,
        types: &ir::Types,
        main_module: ir::ModuleId,
        abi: &'static dyn ir::mc::Abi<Arm>,
        state: &mut ir::mc::BackendState,
    ) -> ir::FunctionIr {
        let mut body = body.clone();
        let mut regs = Slots::with_default(&body, types, self.tuple, MCReg::from_virt(0));
        let mut stack_slots = DHashMap::default();
        for r in body.refs() {
            if let Some(inst) = body.get_inst(r).as_module(self.mem)
                && inst.op() == Mem::Decl
            {
                let decl_ty: TypeId = body.typed_args(&inst);
                let layout = ir::type_layout(types[decl_ty], types, env.primitives());
                stack_slots.insert(r, state.alloc_stack(layout));
            }
            _ = regs.visit_primitive_slots_mut::<Infallible, _>(
                r,
                types[body.get_ref_ty(r)],
                types,
                env.primitives(),
                |regs, p, _offset| {
                    use ir::Primitive as P;
                    match p {
                        P::I1 | P::I8 | P::U8 | P::I16 | P::U16 | P::I32 | P::U32 => {
                            regs[0] = body.new_reg::<Reg>(RegClass::GP32)
                        }
                        P::I64 | P::U64 | P::Ptr => regs[0] = body.new_reg::<Reg>(RegClass::GP64),
                        P::F32 | P::F64 | P::I128 | P::U128 => todo!(),
                    }
                    Ok(())
                },
            )
        }
        let block_graph = BlockGraph::calculate(&body, env);

        let mut ir = IrModify::new(body);
        let args = ir.get_block_args(BlockId::ENTRY);
        abi.implement_params(args, &mut ir, env, self.mc, self.arm, types, &regs);
        let mut ctx = IselCtx::new(
            main_module,
            env,
            &ir,
            regs,
            self.mc,
            abi,
            state,
            &block_graph,
            &stack_slots,
        );

        ir::rewrite::rewrite_in_place(
            &mut ir,
            types,
            env,
            &mut ctx,
            self,
            ReverseRewriteOrder::new(&block_graph),
        );
        ir.finish_and_compress(env)
    }
}

pub fn load(
    ir: &mut IrModify,
    env: &Environment,
    types: &Types,
    position: Insert,
    regs: &Slots<MCReg>,
    arm: ModuleOf<Arm>,
    dst: Ref,
    ptr: MCReg,
    ty: Type,
) {
    regs.visit_primitive_slots::<Infallible, _>(
        dst,
        ty,
        types,
        env.primitives(),
        |regs, primitive, offset| {
            let size = primitive.byte_size();
            if size.get() == 16 {
                assert!(offset % 16 == 0, "unaligned load");
                let offset = offset / 8;
                if offset > (1 << 7) {
                    todo!("large offsets")
                }
                ir.add_before_or_after(
                    env,
                    position,
                    arm.ldp64(regs[0], regs[1], ptr, offset as u32),
                );
            } else {
                debug_assert!(size.get() <= 8);
                todo!()
            }
            Ok(())
        },
    );
}

ir::visitor! {
    InstructionSelector,
    Rewrite,
    ir, types, inst, block, env, dialects,
    ctx: IselCtx<'_, Arm>;

    use builtin: ir::Builtin;
    use arith: ir::dialect::Arith;
    use cf: ir::dialect::Cf;
    use mem: ir::dialect::Mem;
    use tuple: ir::dialect::Tuple;

    use arm: Arm;
    use mc: ir::mc::Mc;

    patterns:
    (builtin.Undef) => {
        Rewrite::Rename(Ref::UNIT)
    };
    (%r = arith.Int (#x)) => {
        let regs = ctx.regs.get(r);
        match int_size_of_ref(r, ir, types) {
            Size::S8 => {
                ir.replace(env, r, arm.movz32(regs[0], 0, x as i8 as u32));
            }
            Size::S16 => {
                ir.replace(env, r, arm.movz32(regs[0], 0, x as i16 as u32));
            }
            Size::S32 => {
                let value = x as i32 as u32;
                let lower = value as u16;
                let upper = value >> 16;
                ir.replace(env, r, arm.movz32(regs[0], 0, lower.into()));
                if upper != 0 {
                    ir.add_after(env, r, arm.movk32(regs[0], 1, upper));
                }
            }
            Size::S64 => {
                let mut value = x;
                let lower = value as u16;
                let mut hw = 0;
                ir.replace(env, r, arm.movz64(regs[0], hw, lower.into()));
                loop {
                    value >>= 16;
                    if value == 0 { break }
                    hw += 1;
                    ir.add_after(env, r, arm.movk64(regs[0], hw, u32::from(value as u16)));
                }
            }
            Size::S128 => todo!("128 bit ints")
        }
    };
    (%r = cf.Ret value) => {
        ctx.abi.implement_return(value, ir, env, mc, arm, types, &ctx.regs, r);
    };
}
