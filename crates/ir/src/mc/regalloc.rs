use std::{collections::VecDeque, fmt};

use crate::{
    Argument, ArgumentMut, Bitmap, BlockGraph, BlockId, Environment, FunctionIr, MCReg,
    MCRegOffset, ModuleOf, Ref, Types, Usage,
    mc::{Abi, McInst},
    pipeline::FunctionPass,
};

use super::{Mc, Register};

pub struct Regalloc<I: McInst> {
    pub mc: ModuleOf<crate::mc::Mc>,
    pub preoccupied: <I::Reg as Register>::RegisterBits,
    pub abi: &'static dyn Abi<I>,
    pub isa: ModuleOf<I>,
}
impl<I: McInst> fmt::Debug for Regalloc<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Regalloc<{}>", std::any::type_name::<I>())
    }
}
impl<I: McInst, State> FunctionPass<State> for Regalloc<I> {
    fn run(
        &self,
        env: &Environment,
        types: &Types,
        mut ir: FunctionIr,
        name: &str,
        _: &mut State,
    ) -> (FunctionIr, Option<Types>) {
        regalloc(
            env,
            self.mc,
            &mut ir,
            self.isa,
            self.abi,
            types,
            self.preoccupied,
            name,
        );
        (ir, None)
    }
}

pub fn regalloc<I: McInst>(
    env: &Environment,
    mc: ModuleOf<Mc>,
    ir: &mut FunctionIr,
    isa: ModuleOf<I>,
    abi: &'static dyn Abi<I>,
    types: &crate::Types,
    preoccupied_bits: <I::Reg as Register>::RegisterBits,
    function: &str,
) {
    let graph = BlockGraph::calculate(ir, env);
    let mut intersecting_precolored = vec![I::Reg::NO_BITS; ir.mc_reg_count() as usize];
    let mut liveins: Box<[Bitmap]> = (0..ir.block_count())
        .map(|_| Bitmap::new(ir.mc_reg_count() as usize))
        .collect();
    analyze_liveness(
        env,
        mc,
        ir,
        isa,
        abi,
        &graph,
        &mut liveins,
        &mut intersecting_precolored,
    );
    if tracing::enabled!(target: "regalloc", tracing::Level::DEBUG) {
        for (i, liveins) in liveins.iter().enumerate() {
            let mut liveins_list = Vec::new();
            liveins.visit_set_bits(|vreg| liveins_list.push(format!("${vreg}")));
            tracing::debug!(target: "regalloc", function, "liveins at bb{i}: {liveins_list:#?}");
        }
        tracing::debug!(target: "regalloc",
            function,
            "IR after liveness analysis:\n{}",
            ir.display_with_phys_regs::<I::Reg>(env, types)
        )
    }
    perform_regalloc::<I::Reg>(
        env,
        mc,
        ir,
        &graph,
        &intersecting_precolored,
        &liveins,
        preoccupied_bits,
    );
}

fn analyze_liveness<I: McInst>(
    env: &Environment,
    mc: ModuleOf<Mc>,
    ir: &mut FunctionIr,
    isa: ModuleOf<I>,
    abi: &'static dyn Abi<I>,
    graph: &BlockGraph<FunctionIr>,
    liveins: &mut [Bitmap],
    intersecting_precolored: &mut [<I::Reg as Register>::RegisterBits],
) {
    let mut workqueue: VecDeque<_> = graph.postorder().iter().copied().collect();
    let mut workqueue_set: Bitmap = Bitmap::new_with_ones(ir.block_count() as usize);
    let mut liveouts = liveins.to_vec();

    while let Some(block) = workqueue.pop_front() {
        workqueue_set.set(block.idx(), false);
        // PERF: just reuse one bitmap in the future and copy over
        let mut live = liveouts[block.idx()].clone();
        let mut live_precolored = <I::Reg as Register>::NO_BITS;
        for r in ir.block_body_refs(block).iter().rev() {
            analyze_inst_liveness::<I>(
                env,
                mc,
                ir,
                isa,
                abi,
                &mut live,
                &mut live_precolored,
                intersecting_precolored,
                r,
            );
        }
        for pred in graph.preds(block) {
            if liveouts[pred.idx()].union_with(&live) && !workqueue_set.get(pred.idx()) {
                workqueue_set.set(pred.idx(), true);
                workqueue.push_back(pred);
            }
        }
        liveins[block.idx()] = live;
    }
}

fn analyze_inst_liveness<I: McInst>(
    env: &Environment,
    mc: ModuleOf<Mc>,
    ir: &mut FunctionIr,
    isa: ModuleOf<I>,
    abi: &'static dyn Abi<I>,
    live: &mut Bitmap,
    live_precolored: &mut <I::Reg as Register>::RegisterBits,
    intersecting_precolored: &mut [<I::Reg as Register>::RegisterBits],
    inst_r: Ref,
) {
    if let Some(inst) = ir.get_inst(inst_r).as_module(mc) {
        match inst.op() {
            Mc::IncomingBlockArgs | Mc::StackValue => {}
            Mc::Copy | Mc::AssignBlockArgs => {
                // to
                for arg in ir.args_mut(inst_r, env).step_by(2) {
                    let ArgumentMut::MCReg(_, r) = arg else {
                        unreachable!();
                    };
                    if let Some(i) = r.virt() {
                        if !live.get(i as usize) {
                            r.set_dead();
                        }
                        live.set(i as usize, false);
                    } else if !r.phys::<I::Reg>().unwrap().get_bit(live_precolored) {
                        r.phys::<I::Reg>().unwrap().set_bit(live_precolored, false);
                        r.set_dead();
                    }
                }
                // from
                for arg in ir.args_mut(inst_r, env).skip(1).step_by(2) {
                    let ArgumentMut::MCReg(_, r) = arg else {
                        unreachable!();
                    };
                    if let Some(i) = r.virt()
                        && !live.get(i as usize)
                    {
                        live.set(i as usize, true);
                        r.set_dead();
                    }
                }
                return;
            }
        }
    }

    for arg in ir.args_mut(inst_r, env) {
        let (ArgumentMut::MCReg(usage, r) | ArgumentMut::MCRegOffset(usage, _, r, _)) = arg else {
            continue;
        };
        if r.is_stack_slot() {
            continue;
        }
        if let Some(p) = r.phys::<I::Reg>() {
            if !p.get_bit(live_precolored) {
                r.set_dead();
            }
            p.set_bit(live_precolored, usage != Usage::Def);
            live.visit_set_bits(|vreg| {
                p.set_bit(&mut intersecting_precolored[vreg], true);
            });
        } else {
            let i = r.virt().unwrap() as usize;
            if !live.get(i) {
                live.set(i, true);
                r.set_dead();
            } else if usage == Usage::Def {
                live.set(i, false);
            }
        }
    }

    // make implicit defs available again *after* handling all normal arg liveness so the registers
    // don't ge reused too early
    if let Some(inst) = ir.get_inst(inst_r).as_module(isa) {
        let defs = inst.inst.implicit_def(abi);
        let uses = inst.inst.implicit_use(abi);
        // all physical regs used here are now alive
        *live_precolored = *live_precolored | uses;
        let defs_and_uses = defs | uses;
        if defs_and_uses != I::Reg::NO_BITS {
            live.visit_set_bits(|vreg| {
                intersecting_precolored[vreg] = intersecting_precolored[vreg] | defs_and_uses;
            });
        }
    }
}

fn perform_regalloc<R: Register>(
    env: &Environment,
    mc: ModuleOf<Mc>,
    ir: &mut FunctionIr,
    graph: &BlockGraph<FunctionIr>,
    intersecting_precolored: &[R::RegisterBits],
    liveins: &[Bitmap],
    preoccupied_bits: R::RegisterBits,
) {
    // PERF: cloning the reg classes here due to borrowing problems with the current design
    // (iterating arguments)
    let classes: Box<[u8]> = ir.mc_reg_classes().into();
    let get_class =
        |i| R::Class::try_from(classes[i]).unwrap_or_else(|_| panic!("invalid reg class"));
    tracing::debug!(target: "regalloc", "Classes: {classes:#?}");

    let default_free = R::ALL_BITS & !preoccupied_bits;
    let mut chosen = vec![R::DEFAULT; ir.mc_reg_count() as usize];

    // first choose the registers for all block arguments
    for &block in graph.postorder().iter() {
        if block == BlockId::ENTRY {
            continue;
        }
        let mut free = default_free;
        let incoming = ir.get_block(block).next().unwrap().1;
        debug_assert_eq!(
            incoming.function,
            crate::FunctionId {
                module: env
                    .get_dialect_module_if_present::<crate::mc::Mc>()
                    .unwrap()
                    .id(),
                function: crate::mc::Mc::IncomingBlockArgs.id(),
            },
            "Block does not begin with IncomingBlockArgs"
        );
        for arg in ir.args_iter(incoming, env) {
            let (Argument::MCReg(r) | Argument::MCRegOffset(MCRegOffset(r, _))) = arg else {
                unreachable!()
            };
            let i = r.virt().unwrap() as usize;
            let avail = free & !intersecting_precolored[i] & !preoccupied_bits;
            let chosen_reg = R::allocate_reg(avail, get_class(i))
                .expect("register allocation failed, TODO: spilling");
            chosen_reg.set_bit(&mut free, false);
            chosen[i] = chosen_reg;
        }
    }

    // then go over the blocks again to fill in the remaining registers
    for &block in graph.postorder().iter().rev() {
        let mut free = default_free;
        liveins[block.idx()].visit_set_bits(|livein| {
            chosen[livein].set_bit(&mut free, false);
        });
        for block_ref in ir.block_body_refs(block).iter() {
            if let Some(inst) = ir.get_inst(block_ref).as_module(mc) {
                match inst.op() {
                    Mc::IncomingBlockArgs | Mc::StackValue => {}
                    Mc::Copy | Mc::AssignBlockArgs => {
                        let is_block_args = inst.op() == Mc::AssignBlockArgs;
                        // handle source arguments first
                        for arg in ir.args_mut(block_ref, env).skip(1).step_by(2) {
                            let (ArgumentMut::MCReg(_, r) | ArgumentMut::MCRegOffset(_, _, r, _)) =
                                arg
                            else {
                                unreachable!();
                            };
                            if let Some(i) = r.virt() {
                                let chosen = chosen[i as usize];
                                let dead = r.is_dead();
                                *r = MCReg::from_phys(chosen);
                                if dead {
                                    chosen.set_bit(&mut free, true);
                                    // always preserve the dead bit
                                    r.set_dead();
                                }
                            } else if r.is_dead() {
                                r.phys::<R>().unwrap().set_bit(&mut free, true);
                            }
                        }
                        // then handle destinations so that the dead dead source registers can be reused
                        // TODO: could try to fill in trivial copies (dest = dead src) first to
                        // always maximize reusing registers
                        for arg in ir.args_mut(block_ref, env).step_by(2) {
                            let (ArgumentMut::MCReg(_, r) | ArgumentMut::MCRegOffset(_, _, r, _)) =
                                arg
                            else {
                                unreachable!();
                            };
                            if let Some(i) = r.virt() {
                                let dead = r.is_dead();
                                let chosen_reg;
                                if is_block_args {
                                    // in the case of block args, the registers were already assigned
                                    chosen_reg = chosen[i as usize];
                                } else {
                                    let occupied = intersecting_precolored[i as usize];
                                    let avail = free & !occupied & !preoccupied_bits;
                                    let class = classes[r.virt().unwrap() as usize];
                                    chosen_reg = R::allocate_reg(
                                        avail,
                                        R::Class::try_from(class)
                                            .unwrap_or_else(|_| panic!("invalid reg class")),
                                    )
                                    .expect("register allocation failed, TODO: spilling");
                                    chosen[i as usize] = chosen_reg;
                                };
                                *r = MCReg::from_phys(chosen_reg);
                                if dead {
                                    r.set_dead();
                                } else {
                                    chosen_reg.set_bit(&mut free, false);
                                }
                            } else {
                                r.phys::<R>().unwrap().set_bit(&mut free, false);
                            }
                        }
                        continue;
                    }
                }
            }

            for arg in ir.args_mut(block_ref, env) {
                match arg {
                    ArgumentMut::MCReg(Usage::Def, _)
                    | ArgumentMut::MCRegOffset(Usage::Def, _, _, _) => {}
                    ArgumentMut::MCReg(_, reg) | ArgumentMut::MCRegOffset(_, _, reg, _) => {
                        if let Some(r) = reg.virt() {
                            // Update Def/DefUse with the chosen register and set the free bit if it's dead.
                            let dead = reg.is_dead();
                            let chosen_reg = chosen[r as usize];
                            *reg = MCReg::from_phys(chosen_reg);
                            if dead {
                                chosen_reg.set_bit(&mut free, true);
                                // always preserve the dead bit
                                reg.set_dead();
                            }
                        }
                    }
                    _ => {}
                }
            }
            for arg in ir.args_mut(block_ref, env).rev() {
                if let ArgumentMut::MCReg(usage, r) | ArgumentMut::MCRegOffset(usage, _, r, _) = arg
                {
                    if r.is_stack_slot() {
                        continue;
                    }
                    if let Some(phys) = r.phys::<R>() {
                        phys.set_bit(&mut free, r.is_dead());
                    } else if usage == Usage::Def {
                        let i = r.virt().unwrap() as usize;
                        // TODO: spilling
                        let occupied = intersecting_precolored[i];
                        let avail = free & !occupied & !preoccupied_bits;
                        let chosen_reg = R::allocate_reg(
                            avail,
                            R::Class::try_from(classes[i])
                                .unwrap_or_else(|_| panic!("invalid reg class")),
                        )
                        .expect("register allocation failed, TODO: spilling");
                        chosen_reg.set_bit(&mut free, false);
                        chosen[i] = chosen_reg;
                        let dead = r.is_dead();
                        *r = MCReg::from_phys(chosen_reg);
                        // preserve the dead bit
                        if dead {
                            r.set_dead();
                        }
                    }
                }
            }
        }
    }
}
