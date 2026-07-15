use crate::{
    Argument, BUILTIN, Bitmap, BlockTarget, Builtin, Ref, modify::IrModify, pipeline::FunctionPass,
};

#[derive(Debug)]
pub struct DeadCodeElimination;
impl FunctionPass for DeadCodeElimination {
    fn run(
        &self,
        env: &crate::Environment,
        _types: &crate::Types,
        ir: crate::FunctionIr,
        name: &str,
        _state: &mut (),
    ) -> (crate::FunctionIr, Option<crate::Types>) {
        let mut alive_insts = Bitmap::new(ir.inst_count() as usize);
        let mut ir = IrModify::new(ir);

        loop {
            let mut changed = false;
            for block in ir.block_ids() {
                let info = ir.get_block(block);
                if info.arg_count == 0 {
                    // mark insert point instruction as alive so it doesn't get killed
                    alive_insts.set(info.args_idx as usize, true);
                }
                for r in info.all_refs() {
                    let inst = ir.get_inst(r);
                    let f = &env[inst.function];
                    if !f.flags.pure()
                        || inst
                            .as_module(BUILTIN)
                            .is_some_and(|inst| inst.op() == Builtin::BlockArg)
                    {
                        // don't delete block args for now, they will need special treatment
                        // all impure operations and block arguments are set to alive
                        alive_insts.set(r.idx(), true);
                    }
                    if alive_insts.get(r.idx()) {
                        let args = ir.args_iter(inst, env);
                        for arg in args {
                            match arg {
                                Argument::Ref(r) => {
                                    if let Some(i) = r.into_ref()
                                        && !alive_insts.get(i as usize)
                                    {
                                        changed = true;
                                        alive_insts.set(i as usize, true);
                                    }
                                }
                                Argument::BlockTarget(BlockTarget(_, args)) => {
                                    for arg in args.iter() {
                                        let Some(i) = arg.into_ref() else { continue };
                                        if !alive_insts.get(i as usize) {
                                            changed = true;
                                            alive_insts.set(i as usize, true);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        alive_insts.visit_unset_bits(|dead_inst| {
            // Bitmap doesn't track exact length so this can happen for the remaining (up to 7) bits
            if dead_inst >= ir.inst_count() as usize {
                return;
            }
            let dead_inst = Ref(dead_inst as u32);
            tracing::debug!(target: "pass::dce", function = name, "Dead instruction {dead_inst}");
            ir.replace_with(env, dead_inst, Ref::UNIT);
        });
        (ir.finish_and_compress(env), None)
    }
}
