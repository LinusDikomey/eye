use std::fmt;

use ir::{BlockId, MCReg, ModuleOf, mc::Abi, modify::IrModify, pipeline::FunctionPass};

use crate::{
    BackendState,
    arch::x86::{Reg, X86},
};

pub struct PrologueEpilogueInsertion {
    pub x86: ModuleOf<X86>,
    pub abi: &'static dyn Abi<X86>,
}
impl fmt::Debug for PrologueEpilogueInsertion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "x86::PrologueEpilogueInsertion")
    }
}
impl FunctionPass<BackendState> for PrologueEpilogueInsertion {
    fn run(
        &self,
        env: &ir::Environment,
        _types: &ir::Types,
        ir: ir::FunctionIr,
        name: &str,
        state: &mut BackendState,
    ) -> (ir::FunctionIr, Option<ir::Types>) {
        // HACK: don't insert a prologue/epilogue for _start (used on linux only) since it's
        // supposed to be a naked function
        if name == "_start" {
            return (ir, None);
        }
        let mut ir = IrModify::new(ir);

        let used_regs = ir::mc::used_physical_registers::<Reg>(&ir, env);
        let to_save = used_regs & self.abi.callee_saved() & !(Reg::rsp.bit() | Reg::rbp.bit());

        let start = ir.get_original_block_start(BlockId::ENTRY);
        let x86 = self.x86;

        let callee_saved_size = Reg::UNIQUE_BITS
            .iter()
            .filter(|reg| to_save & reg.bit() != super::isa::RegBits::default())
            .count() as u32
            * 8;

        if state.stack_size > 0 {
            ir.add_before(env, start, x86.push_r64(MCReg::from_phys(Reg::rbp)));
            ir.add_before(
                env,
                start,
                x86.mov_rr64(MCReg::from_phys(Reg::rbp), MCReg::from_phys(Reg::rsp)),
            );
            let final_stack_size = (state.stack_size + callee_saved_size).next_multiple_of(16);
            state.stack_size = final_stack_size - callee_saved_size;
            ir.add_before(
                env,
                start,
                x86.sub_ri64(MCReg::from_phys(Reg::rsp), state.stack_size),
            );
        }

        for reg in Reg::UNIQUE_BITS {
            if to_save & reg.bit() != super::isa::RegBits::default() {
                ir.add_before(env, start, x86.push_r64(MCReg::from_phys(reg)));
            }
        }

        for r in ir.refs() {
            let inst = ir.get_inst(r);
            if inst
                .as_module(self.x86)
                .is_some_and(|inst| inst.op().is_ret())
            {
                // insert epilogue before return
                for reg in Reg::UNIQUE_BITS.into_iter().rev() {
                    if to_save & reg.bit() != super::isa::RegBits::default() {
                        ir.add_before(env, r, x86.pop_r64(MCReg::from_phys(reg)));
                    }
                }
                if state.stack_size != 0 {
                    ir.add_before(
                        env,
                        r,
                        x86.add_ri64(MCReg::from_phys(Reg::rsp), state.stack_size),
                    );
                    ir.add_before(env, r, x86.pop_r64(MCReg::from_phys(Reg::rbp)));
                }
            }
        }

        (ir.finish_and_compress(env), None)
    }
}
