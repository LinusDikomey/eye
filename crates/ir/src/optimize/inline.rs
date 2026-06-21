use std::{borrow::Cow, cmp::min, fmt};

use crate::{
    Argument, Bitmap, BlockGraph, BlockId, Environment, Function, FunctionIr, LocalFunctionId,
    Module, ModuleId, ModuleOf, Ref, TypeId,
    dialect::Cf,
    modify::{Insert, IrModify},
    pipeline::ModulePass,
    rewrite::RenameTable,
};

pub struct Inline {
    cf: ModuleOf<Cf>,
}
impl Inline {
    pub fn new(env: &mut Environment) -> Self {
        Self {
            cf: env.get_dialect_module(),
        }
    }

    pub fn inline_function_calls(
        &self,
        env: &mut Environment,
        module: ModuleId,
        function: LocalFunctionId,
        scc: &[LocalFunctionId],
    ) {
        let Some(ir) = env.modules[module.idx()].functions[function.idx()]
            .ir
            .take()
        else {
            return;
        };
        // PERF: don't always clone types here
        let mut types = env.modules[module.idx()].functions[function.idx()]
            .types
            .clone();
        let mut ir = IrModify::new(ir);
        for block in ir.block_ids() {
            for call_ref in ir.get_original_block_refs(block).iter() {
                let call_inst = *ir.get_inst(call_ref);
                if call_inst.function.module != module || scc.contains(&call_inst.function()) {
                    continue;
                }
                let Some(callee_ir) = env[call_inst.function].ir() else {
                    continue;
                };
                let callee_types = env[call_inst.function].types();
                if !should_inline(&ir, callee_ir) {
                    continue;
                }
                // inline!
                let name = &env.modules[module.idx()].functions[function.idx()].name;
                let inlined_name = &env[call_inst.function].name;
                tracing::debug!(target: "pass::inline", "Inlining {inlined_name} into {name}");

                let old_types_offset = types.types.len() as u32;
                types.types.extend(
                    callee_types
                        .types
                        .iter()
                        .map(|ty| ty.apply_offset(old_types_offset)),
                );
                let mut renames = RenameTable::new(callee_ir, old_types_offset);
                let graph = BlockGraph::calculate(callee_ir, env);

                let (after_block, call_site_insert_point) = ir.split_block_before(block, call_ref);

                // add the functions return type as a block argument to the new block
                let mut return_ty = env[call_inst.function]
                    .return_type
                    .expect("Function missing return type");
                return_ty.0 += old_types_offset;
                let (return_val, _) = ir.add_block_arg(env, after_block, return_ty);
                ir.replace_with(env, call_ref, return_val);

                for &block in graph.postorder().iter().rev() {
                    let (new_block, _, _) = ir.add_block(
                        callee_ir
                            .get_block_args(block)
                            .iter()
                            .map(|r| TypeId(callee_ir.get_ref_ty(r).0 + old_types_offset)),
                    );
                    if block == BlockId::ENTRY {
                        let call_args = ir.args_iter(&call_inst, env);
                        let args: Vec<_> = callee_ir
                            .get_block_args(block)
                            .iter()
                            .zip(call_args)
                            .map(|(_block_arg, call_arg)| {
                                let Argument::Ref(call_arg) = call_arg else {
                                    unreachable!()
                                };
                                call_arg
                            })
                            .collect();
                        ir.add_after(
                            env,
                            call_site_insert_point,
                            self.cf.Goto(crate::BlockTarget(new_block, args.into())),
                        );
                    }
                    renames.rename_block(block, new_block);
                }

                // reverse postorder ensures defs are visited before uses
                for &block in graph.postorder().iter().rev() {
                    let new_block = renames.get_block(block);
                    ir.add_manual_preds(
                        new_block,
                        callee_ir.preds(block).iter().map(|&b| renames.get_block(b)),
                    );
                    ir.add_manual_succs(
                        new_block,
                        callee_ir.succs(block).iter().map(|&b| renames.get_block(b)),
                    );
                    let args = ir.get_block_args(new_block);
                    let insert_idx = ir.get_block_insert_point(new_block);
                    for (arg, renamed) in callee_ir.get_block_args(block).iter().zip(args.iter()) {
                        renames.rename(arg, renamed);
                    }
                    for (r, inst) in callee_ir.get_block(block) {
                        let mut inst = *inst;
                        renames.visit_inst(
                            &mut ir.ir.extra,
                            &ir.ir.blocks,
                            Some(&callee_ir.extra),
                            &mut inst,
                            env,
                        );
                        if inst
                            .as_module(self.cf)
                            .is_some_and(|inst| inst.op() == Cf::Ret)
                        {
                            // Ret becomes a Goto to the block after the inlined call
                            let return_value: Ref = ir.args(&inst, env);
                            ir.add_after(
                                env,
                                insert_idx,
                                self.cf.Goto(crate::BlockTarget(
                                    after_block,
                                    Cow::Borrowed(&[return_value]),
                                )),
                            );
                        } else {
                            let renamed =
                                ir.add_inst_before_or_after(insert_idx, Insert::After, inst);
                            renames.rename(r, renamed);
                        }
                    }
                }
            }
        }
        let ir = ir.finish_and_compress(env);
        let ir_function = &mut env.modules[module.idx()].functions[function.idx()];
        ir_function
            .ir
            .set(ir)
            .unwrap_or_else(|_| panic!("ir set twice"));
        ir_function.types = types;
    }
}
impl fmt::Debug for Inline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Inline")
    }
}
impl ModulePass for Inline {
    fn run(&self, env: &mut Environment, module: ModuleId) {
        let mut sccs = Sccs::new(&env[module], module);
        sccs.compute(|scc| {
            for &id in scc {
                self.inline_function_calls(env, module, id, scc);
            }
        });
    }
}

fn should_inline(_caller: &IrModify, callee: &FunctionIr) -> bool {
    // TODO: better inlining metric
    callee.refs().count() < 20
}

fn called(f: &Function, module: ModuleId) -> Box<[LocalFunctionId]> {
    let Some(ir) = f.ir() else {
        return Box::new([]);
    };
    let mut called = Vec::new();
    for r in ir.refs() {
        let inst = ir.get_inst(r);
        if inst.module() == module {
            // call in module
            called.push(inst.function());
        }
    }
    called.into_boxed_slice()
}

/// Computes strongly-connetcted components
struct Sccs {
    edges: Box<[Box<[LocalFunctionId]>]>,
    indices: Box<[Option<u32>]>,
    lowlinks: Box<[u32]>,
    index: u32,
    stack: Vec<LocalFunctionId>,
    on_stack: Bitmap,
}
impl Sccs {
    pub fn new(module: &Module, module_id: ModuleId) -> Self {
        Self {
            edges: module
                .function_ids()
                .map(|id| called(&module[id], module_id))
                .collect(),
            indices: vec![None; module.function_ids().len()].into_boxed_slice(),
            lowlinks: vec![0; module.function_ids().len()].into_boxed_slice(),
            index: 0,
            stack: Vec::new(),
            on_stack: Bitmap::new(module.function_ids().len()),
        }
    }

    pub fn compute(&mut self, mut on_scc: impl FnMut(&[LocalFunctionId])) {
        for id in (0..self.edges.len() as u32).map(LocalFunctionId) {
            if self.indices[id.0 as usize].is_none() {
                self.strong_connect(id, &mut on_scc);
            }
        }
    }

    fn strong_connect(&mut self, v: LocalFunctionId, on_scc: &mut impl FnMut(&[LocalFunctionId])) {
        self.indices[v.idx()] = Some(self.index);
        self.lowlinks[v.idx()] = self.index;
        let stack_pre_length = self.stack.len();
        self.stack.push(v);
        self.on_stack.set(v.idx(), true);
        self.index += 1;

        for i in 0..self.edges[v.idx()].len() {
            let w = self.edges[v.idx()][i];
            if let Some(w_index) = self.indices[w.idx()] {
                if self.on_stack.get(w.idx()) {
                    // Successor w is in stack and hence in the current SCC
                    self.lowlinks[v.idx()] = min(self.lowlinks[v.idx()], w_index);
                }
            } else {
                self.strong_connect(w, on_scc);
                self.lowlinks[v.idx()] = min(self.lowlinks[v.idx()], self.lowlinks[w.idx()]);
            }
        }
        if self.lowlinks[v.idx()] == self.indices[v.idx()].unwrap() {
            on_scc(&self.stack[stack_pre_length..]);
            for v in self.stack.drain(stack_pre_length..) {
                self.on_stack.set(v.idx(), false);
            }
        }
    }
}
