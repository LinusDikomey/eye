mod abi;
mod emit;
mod isa;
mod isel;

use crate::CodegenFn;

pub use abi::ArmAbi;

pub fn init_codegen(
    env: &mut ir::Environment,
    module_id: ir::ModuleId,
    target: &target::Target,
    abi: &'static ArmAbi,
) -> CodegenFn {
    let isel = isel::InstructionSelector::new(env);
    let mc = isel.mc;
    let arm = isel.arm;

    let mut pipeline = ir::pipeline::Pipeline::new("backend");
    pipeline.add_function_pass(Box::new(crate::Isel {
        isel,
        module_id,
        abi,
        target: target.clone(),
    }));
    pipeline.add_function_pass(Box::new(ir::mc::Regalloc::<isa::Arm> {
        mc,
        preoccupied: abi.preoccupied_regs(),
        isa: arm,
        abi,
    }));
    // TODO: add PrologueEpilogueInsertion pass
    Box::new(move |env, ir, mut types, name, text, relocations| {
        let mir = pipeline.process_function_with_regs::<isa::Reg>(env, ir, &mut types, name);
        tracing::debug!(target: "backend-ir",
            function = name,
            "Final machine IR:\n{}",
            mir.display_with_phys_regs::<isa::Reg>(env, &types)
        );
        crate::emit(env, mc, arm, &mir, text, relocations);
    })
}
