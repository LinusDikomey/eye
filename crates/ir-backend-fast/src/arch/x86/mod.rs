mod abi;
mod emit;
mod isa;
mod isel;
mod prologue_epilogue;

use abi::get_target_abi;
use isa::{PREOCCUPIED_REGISTERS, Reg, TMP_REGISTER, X86};
use prologue_epilogue::PrologueEpilogueInsertion;

use crate::CodegenFn;

pub fn init_codegen(env: &mut ir::Environment, module_id: ir::ModuleId) -> CodegenFn {
    let isel = isel::InstructionSelector::new(env);
    let mc = isel.mc;
    let x86 = isel.x86;
    let abi = get_target_abi();

    let mut pipeline = ir::pipeline::Pipeline::new("backend");
    pipeline.add_function_pass(Box::new(crate::Isel {
        isel,
        module_id,
        abi,
    }));
    pipeline.add_function_pass(Box::new(ir::mc::Regalloc::<X86> {
        mc,
        preoccupied: PREOCCUPIED_REGISTERS,
        isa: x86,
        abi,
    }));
    pipeline.add_function_pass(Box::new(PrologueEpilogueInsertion { x86, abi }));

    Box::new(move |env, ir, mut types, name, text, relocations| {
        let mir = pipeline.process_function_with_regs::<Reg>(env, ir, &mut types, name);

        tracing::debug!(target: "backend-ir",
            function = name,
            "Final machine IR:\n{}",
            mir.display_with_phys_regs::<Reg>(env, &types)
        );
        crate::emit(env, mc, x86, &mir, text, relocations);
    })
}
