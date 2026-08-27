use std::convert::Infallible;

use dmap::DHashMap;
use ir::{
    BlockGraph, BlockId, Environment, MCReg, MCRegOffset, ModuleOf, Primitive, Ref, StackSlot,
    TypeId, Types,
    dialect::Mem,
    mc::Mc,
    modify::{Insert, IrModify},
    rewrite::{ReverseRewriteOrder, Rewrite},
    slots::Slots,
};

use crate::{
    IselCtx, Size,
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
        target: &target::Target,
        state: &mut crate::BackendState,
    ) -> ir::FunctionIr {
        let body = body.clone();
        let mut regs = Slots::with_default(&body, types, self.tuple, MCReg::from_virt(0));
        let block_graph = BlockGraph::calculate(&body, env);
        let mut ir = IrModify::new(body);
        for r in ir.refs() {
            if let Some(inst) = ir.get_inst(r).as_module(self.mem)
                && inst.op() == Mem::Decl
            {
                // legacy stack slots still need to be allocated here
                let decl_ty: TypeId = ir.typed_args(&inst);
                let layout = ir::type_layout(types[decl_ty], types, env.primitives());

                let slot = state.new_stack_slot(layout);
                let reg = regs.get_one_mut(r);
                *reg = ir.new_reg::<Reg>(RegClass::GP32);
                ir.replace(
                    env,
                    r,
                    self.mc.StackValue(*reg, slot.into_inner(), TypeId::PTR),
                );
                continue;
            }
            _ = regs.visit_primitive_slots_mut::<Infallible, _>(
                r,
                types[ir.get_ref_ty(r)],
                types,
                env.primitives(),
                |regs, p, _offset| {
                    use ir::Primitive as P;
                    match p {
                        P::I1 | P::I8 | P::U8 | P::I16 | P::U16 | P::I32 | P::U32 => {
                            regs[0] = ir.new_reg::<Reg>(RegClass::GP32)
                        }
                        P::I64 | P::U64 | P::Ptr => regs[0] = ir.new_reg::<Reg>(RegClass::GP64),
                        P::F32 | P::F64 | P::I128 | P::U128 => todo!(),
                    }
                    Ok(())
                },
            )
        }

        let args = ir.get_block_args(BlockId::ENTRY);
        abi.implement_params(args, &mut ir, env, self.mc, self.arm, types, &regs);

        // legacy stack slots not used by this isel
        let stack_slots = DHashMap::default();
        let mut ctx = IselCtx::new(
            main_module,
            env,
            &ir,
            regs,
            self.mc,
            abi,
            target,
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
            size @ (Size::S32 | Size::S64) => {
                let hws = if size == Size::S64 { 4 } else { 2 };
                let value = x;
                let mut first = true;
                for hw in 0..hws {
                    let imm = (value >> (16 * hw)) as u16;
                    if imm == 0 && value != 0 { continue }
                    match (first, size) {
                        (true, Size::S64) => ir.replace(env, r, arm.movz64(regs[0], hw, imm.into())),
                        (true, _) => ir.replace(env, r, arm.movz32(regs[0], hw, imm.into())),
                        (false, Size::S64) => {
                            ir.add_after(env, r, arm.movk64(regs[0], hw, imm.into()));
                        }
                        (false, _) => {
                            ir.add_after(env, r, arm.movk32(regs[0], hw, imm.into()));
                        }
                    }
                    if value == 0 {
                        break;
                    }
                    first = false;
                }
            }
            Size::S128 => todo!("128 bit ints")
        }
    };
    (%r = mem.Load ptr) => {
        ctx.load_store(ir, env, types, Insert::Before(r), true, arm, r, ptr);
        Rewrite::Rename(Ref::UNIT)
    };
    (%r = mem.Store ptr value) => {
        ctx.load_store(ir, env, types, Insert::Before(r), false, arm, value, ptr);
        Rewrite::Rename(Ref::UNIT)

    };
    (%r = cf.Ret value) => {
        ctx.abi.implement_return(value, ir, env, mc, arm, types, &ctx.regs, r);
    };
    (%r = _) => {
        if inst.module() == ctx.main_module {
            ctx.abi.implement_call(r, ir, env, mc, arm, types, &ctx.regs, false);
            let function_id = inst.function_id();
            ir.replace(env, r, arm.bl_function(function_id));
        } else if inst.module() != dialects.arm.id() {
            unreachable!("unhandled instruction at {r}: {}", env.get_inst_name(inst))
        }
    };
}

impl<'a> IselCtx<'a, Arm> {
    fn load_store(
        &mut self,
        ir: &mut IrModify,
        env: &Environment,
        types: &Types,
        position: Insert,
        load: bool,
        arm: ModuleOf<Arm>,
        value: Ref,
        ptr: Ref,
    ) {
        let ptr: MCReg = if let Some(inst) = ir.get_inst(ptr).as_module(self.mc)
            && inst.inst == Mc::StackValue
        {
            self.remove_use(ptr, ir, env);
            let (_, slot): (MCReg, u32) = ir.typed_args(&inst);
            // stack slot reg will be part of a load/store with an MCRegOffset
            StackSlot::new(slot).into()
        } else {
            self.regs.get_one(ptr)
        };
        load_store_reg(ir, env, types, position, load, &self.regs, arm, value, ptr);
    }
}

pub fn load_store_reg(
    ir: &mut IrModify,
    env: &Environment,
    types: &Types,
    position: Insert,
    load: bool,
    regs: &Slots<MCReg>,
    arm: ModuleOf<Arm>,
    value: Ref,
    ptr: MCReg,
) {
    regs.visit_primitive_slots::<Infallible, _>(
        value,
        types[ir.get_ref_ty(value)],
        types,
        env.primitives(),
        |regs, primitive, offset| {
            match primitive {
                Primitive::I1 | Primitive::I8 | Primitive::U8 => {
                    if offset >= (1 << 12) {
                        todo!("irregular/large offsets")
                    }
                    if load {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.ldrb32(regs[0], MCRegOffset(ptr, offset as _)),
                        );
                    } else {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.strb32(regs[0], MCRegOffset(ptr, offset as _)),
                        );
                    }
                }
                Primitive::I16 | Primitive::U16 => {
                    if offset % 2 != 0 || offset >= (1 << 13) {
                        todo!("irregular/large offsets")
                    }
                    if load {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.ldrh32(regs[0], MCRegOffset(ptr, offset as _)),
                        );
                    } else {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.strh32(regs[0], MCRegOffset(ptr, offset as _)),
                        );
                    }
                }
                Primitive::I32 | Primitive::U32 => {
                    if offset % 4 != 0 || offset >= (1 << 14) {
                        todo!("irregular/large offsets")
                    }
                    if load {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.ldr32(regs[0], MCRegOffset(ptr, offset as _)),
                        );
                    } else {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.str32(regs[0], MCRegOffset(ptr, offset as _)),
                        );
                    }
                }
                Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                    if offset % 8 != 0 || offset >= (1 << 15) {
                        todo!("irregular/large offsets")
                    }
                    if load {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.ldr64(regs[0], MCRegOffset(ptr, offset as _)),
                        );
                    } else {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.str64(regs[0], MCRegOffset(ptr, offset as _)),
                        );
                    }
                }
                Primitive::I128 | Primitive::U128 => {
                    if offset % 8 != 0 || offset >= (1 << 10) {
                        todo!("irregular/large offsets")
                    }
                    if load {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.ldp64(regs[0], regs[1], MCRegOffset(ptr, offset as u32)),
                        );
                    } else {
                        ir.add_before_or_after(
                            env,
                            position,
                            arm.stp64(regs[0], regs[1], MCRegOffset(ptr, offset as u32)),
                        );
                    }
                }
                Primitive::F32 | Primitive::F64 => todo!("floats"),
            }
            let size = primitive.byte_size();
            if size.get() == 16 {
            } else {
                debug_assert!(size.get() <= 8);
            }
            Ok(())
        },
    );
}
