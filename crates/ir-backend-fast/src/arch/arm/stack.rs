use core::fmt;

use ir::{
    ArgumentMut, BlockId, MCReg, MCRegOffset, ModuleOf, mc::Abi, modify::IrModify,
    pipeline::FunctionPass,
};

use crate::{
    BackendState,
    arch::arm::{
        abi,
        isa::{Arm, Reg},
    },
};

pub struct StackFrameHandling {
    pub arm: ModuleOf<Arm>,
    pub abi: &'static dyn Abi<Arm>,
}
impl fmt::Debug for StackFrameHandling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "arm::StackFrameHandling")
    }
}
impl FunctionPass<BackendState> for StackFrameHandling {
    fn run(
        &self,
        env: &ir::Environment,
        _types: &ir::Types,
        ir: ir::FunctionIr,
        _name: &str,
        state: &mut BackendState,
    ) -> (ir::FunctionIr, Option<ir::Types>) {
        let mut ir = IrModify::new(ir);
        let mut stack_size: u64 = 0;
        let mut max_align = 1;
        debug_assert_eq!(
            state.stack_size, 0,
            "Legacy stack size shouldn't be used in arm backend"
        );
        let stack_offsets: Box<[_]> = state
            .stack_slots
            .iter()
            .map(|alloc| {
                max_align = max_align.max(alloc.layout.align.get());
                stack_size = stack_size.next_multiple_of(alloc.layout.size);
                let start = stack_size;
                stack_size += alloc.layout.size;
                start
            })
            .collect();
        let mut leaf = true;
        for r in ir.refs() {
            if let Some(inst) = ir.get_inst(r).as_module(self.arm)
                && inst.op().is_call()
            {
                leaf = false;
            }
            for arg in ir.args_mut(r, env) {
                let ArgumentMut::MCRegOffset(_usage, imm, reg, offset) = arg else {
                    continue;
                };
                let Some(slot) = reg.stack_slot() else {
                    continue;
                };
                let slot_offset = stack_offsets[slot.idx()];

                let new_offset = u64::from(*offset).saturating_add(slot_offset);
                let Some(new_fitting) = u32::try_from(new_offset)
                    .ok()
                    .and_then(|new| imm.fits(new).then_some(new))
                else {
                    // this needs to emit a helper instruction to materialize the offset
                    todo!("handle large stack offsets not fitting into the immediate")
                };
                *reg = MCReg::from_phys(Reg::sp);
                *offset = new_fitting;
            }
        }
        // TODO: either implement callee-saved register save here or handle it in regalloc
        // let used_regs = ir::mc::used_physical_registers::<Reg>(&ir, env);
        // let to_save =
        //     used_regs & self.abi.callee_saved() & !(Reg::sp.bit() | abi::FRAME_POINTER.bit());

        stack_size = stack_size.next_multiple_of(abi::CALL_STACK_ALIGN);
        let mut sp_decrement = stack_size;
        if !leaf {
            // space for x29, x30
            sp_decrement += 16;
        }

        let start = ir.get_original_block_start(BlockId::ENTRY);

        let sp = MCReg::from_phys(Reg::sp);
        let x29 = MCReg::from_phys(Reg::x29);
        let x30 = MCReg::from_phys(Reg::x30);

        // add/sub needs imm12, stp needs imm7 (imm10A8)
        if sp_decrement >= (1 << 7) {
            todo!("handle large stack frames");
        }
        let sp_decrement = sp_decrement as u32;

        if sp_decrement != 0 {
            ir.add_before(env, start, self.arm.sub_i64(sp, sp, sp_decrement));
            if !leaf {
                ir.add_before(
                    env,
                    start,
                    self.arm.stp64(x29, x30, MCRegOffset(sp, sp_decrement - 16)),
                );
                ir.add_before(env, start, self.arm.add_i64(x29, sp, sp_decrement - 16));
            }
        }
        for r in ir.refs() {
            let inst = ir.get_inst(r);
            if inst
                .as_module(self.arm)
                .is_some_and(|inst| inst.op().is_ret())
            {
                // epilogue
                if sp_decrement != 0 {
                    if !leaf {
                        ir.add_before(
                            env,
                            r,
                            self.arm.ldp64(x29, x30, MCRegOffset(sp, sp_decrement - 16)),
                        );
                    }
                    ir.add_before(env, r, self.arm.add_i64(sp, sp, sp_decrement));
                }
                if sp_decrement != 0 {}
            }
        }

        (ir.finish_and_compress(env), None)
    }
}
