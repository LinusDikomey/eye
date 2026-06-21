use dmap::{DHashMap, DHashSet};

use crate::{
    Argument, Bitmap, BlockGraph, BlockId, BlockInfo, BlockTarget, Builtin, Environment,
    FunctionId, FunctionIr, INLINE_ARGS, Inst, Instruction, IntoArgs, MCReg, Parameter, Ref, Refs,
    TypeId, TypedInstruction, argument, builder::write_args, decode_args, dialect::Cf,
    mc::RegClass, rewrite::RenameTable,
};

#[derive(Debug)]
pub struct IrModify {
    pub(crate) ir: FunctionIr,
    additional: Vec<AdditionalInst>,
}
impl IrModify {
    pub fn new(ir: FunctionIr) -> Self {
        Self {
            ir,
            additional: Vec::new(),
        }
    }

    pub fn inst_count(&self) -> u32 {
        (self.ir.insts.len() + self.additional.len()) as _
    }

    pub fn refs(&self) -> impl use<> + ExactSizeIterator<Item = Ref> {
        (0..(self.ir.insts.len() + self.additional.len()) as u32).map(Ref)
    }

    pub fn block_ids(&self) -> impl ExactSizeIterator<Item = BlockId> + use<> {
        self.ir.block_ids()
    }

    pub fn get_ref_ty(&self, r: Ref) -> TypeId {
        match r {
            Ref::UNIT => TypeId::I1,
            Ref::TRUE | Ref::FALSE => TypeId::I1,
            _ => {
                if r.idx() < self.ir.insts.len() {
                    self.ir.insts[r.idx()].ty
                } else {
                    self.additional[r.idx() - self.ir.insts.len()].inst.ty
                }
            }
        }
    }

    pub fn args<'a, A: argument::Args<'a>>(
        &'a self,
        inst: &'a Instruction,
        env: &'a Environment,
    ) -> A {
        self.ir.args(inst, env)
    }

    pub fn typed_args<'a, A: argument::Args<'a>, I: Inst>(
        &'a self,
        inst: &'a TypedInstruction<I>,
    ) -> A {
        self.ir.typed_args(inst)
    }

    pub fn args_n<'a, I: Inst + 'static, const N: usize>(
        &'a self,
        inst: &'a TypedInstruction<I>,
    ) -> [Argument<'a>; N] {
        self.ir.args_n(inst)
    }

    pub fn args_n_ignore_extra<'a, I: Inst + 'static, const N: usize>(
        &'a self,
        inst: &'a TypedInstruction<I>,
    ) -> [Argument<'a>; N] {
        self.ir.args_n_ignore_extra(inst)
    }

    pub(crate) fn try_get_inst(&self, r: Ref) -> Result<&Instruction, ()> {
        debug_assert!(
            r.is_ref(),
            "Tried to get an instruction from a Ref value: {r}"
        );
        if !r.is_ref() {
            return Err(());
        }
        let i = r.0 as usize;
        Ok(if i < self.ir.insts.len() {
            &self.ir.insts[i]
        } else {
            &self.additional[i - self.ir.insts.len()].inst
        })
    }

    pub fn get_inst(&self, r: Ref) -> &Instruction {
        self.try_get_inst(r)
            .expect("Retrieved instruction was deleted")
    }

    pub fn visit_block_targets_mut(
        &mut self,
        inst: Ref,
        env: &Environment,
        mut f: impl FnMut(BlockId, &mut [Ref]),
    ) {
        let inst = if (inst.0 as usize) < self.ir.insts.len() {
            self.ir.insts[inst.idx()]
        } else {
            self.ir.insts[inst.idx() - self.ir.insts.len()]
        };
        let params = env[inst.function].params();
        let slot_count: usize = params.iter().map(|p| p.slot_count()).sum();
        if slot_count <= INLINE_ARGS {
            let mut idx = 0;
            for param in params {
                if *param == Parameter::BlockTarget {
                    let block = BlockId(inst.args[idx]);
                    let args_idx = inst.args[idx + 1] as usize;
                    let arg_count = self.ir.blocks[block.idx()].arg_count as usize;
                    let args: &mut [u32] = &mut self.ir.extra[args_idx..args_idx + arg_count];
                    // SAFETY: reinterprets the u32s as Refs, Refs are repr(transparent)
                    let args: &mut [Ref] = unsafe {
                        std::slice::from_raw_parts_mut(args.as_mut_ptr().cast(), args.len())
                    };
                    f(block, args)
                }
                idx += param.slot_count();
            }
        } else {
            let mut idx = inst.args[0] as usize;
            for param in params {
                if *param == Parameter::BlockTarget {
                    let block = BlockId(self.ir.extra[idx]);
                    let args_idx = self.ir.extra[idx + 1] as usize;
                    let arg_count = self.ir.blocks[block.idx()].arg_count as usize;
                    let args: &mut [u32] = &mut self.ir.extra[args_idx..args_idx + arg_count];
                    // SAFETY: reinterprets the u32s as Refs, Refs are repr(transparent)
                    let args: &mut [Ref] = unsafe {
                        std::slice::from_raw_parts_mut(args.as_mut_ptr().cast(), args.len())
                    };
                    f(block, args)
                }
                idx += param.slot_count();
            }
        }
    }

    /// adds a block argument of the specified type to the block and returns the Ref to the new arg
    /// as well as it's index in the arg list of that block
    pub fn add_block_arg(&mut self, env: &Environment, block: BlockId, ty: TypeId) -> (Ref, u32) {
        let r = Ref((self.ir.insts.len() + self.additional.len()) as u32);
        let block_info = &mut self.ir.blocks[block.idx()];
        let before = Ref(block_info.args_idx); // before the first instruction of the body block
        let prev_arg_count = block_info.arg_count;
        block_info.arg_count += 1;
        self.additional.push(AdditionalInst {
            insert_at: before,
            position: Insert::Before,
            inst: crate::builtins::block_arg_inst(ty),
        });
        for pred in &self.ir.blocks[block.idx()].preds {
            let pred_info = &self.ir.blocks[pred.idx()];
            let terminator_idx = pred_info.body_idx + pred_info.len - 1;
            let terminator = &mut self.ir.insts[terminator_idx as usize];
            let func = &env[terminator.function];
            let param_count: usize = func.params.iter().map(|p| p.slot_count()).sum();
            let visit_block_target =
                |target_block: BlockId, extra_idx: u32, extra: &mut Vec<u32>| -> u32 {
                    if target_block != block {
                        return extra_idx;
                    }
                    let i = extra_idx as usize;
                    let new_idx = if extra.len() != i + prev_arg_count as usize {
                        let new_idx = extra.len() as u32;
                        // if the old args aren't already at the end of extra, copy them to the end so we
                        // can add an arg
                        extra.extend_from_within(i..i + prev_arg_count as usize);
                        new_idx
                    } else {
                        extra_idx
                    };
                    extra.push(Ref::UNIT.0);
                    new_idx
                };
            if param_count <= INLINE_ARGS && func.varargs.is_none() {
                let mut idx = 0;
                for param in &func.params {
                    if *param == Parameter::BlockTarget {
                        terminator.args[idx + 1] = visit_block_target(
                            BlockId(terminator.args[idx]),
                            terminator.args[idx + 1],
                            &mut self.ir.extra,
                        );
                    }
                    idx += param.slot_count();
                }
            } else {
                let mut idx = terminator.args[0] as usize;
                for param in &func.params {
                    if *param == Parameter::BlockTarget {
                        self.ir.extra[idx + 1] = visit_block_target(
                            BlockId(self.ir.extra[idx]),
                            self.ir.extra[idx + 1],
                            &mut self.ir.extra,
                        );
                    }
                    idx += param.slot_count();
                }
            }
        }
        (r, prev_arg_count)
    }

    pub fn add_before<'r>(
        &mut self,
        env: &Environment,
        before: Ref,
        inst: (FunctionId, impl IntoArgs<'r>, TypeId),
    ) -> Ref {
        self.add_before_or_after(env, before, Insert::Before, inst)
    }

    pub fn add_after<'r>(
        &mut self,
        env: &Environment,
        r: Ref,
        inst: (FunctionId, impl IntoArgs<'r>, TypeId),
    ) -> Ref {
        self.add_before_or_after(env, r, Insert::After, inst)
    }

    pub fn add_before_or_after<'r>(
        &mut self,
        env: &Environment,
        insert_at: Ref,
        position: Insert,
        (function, args, ty): (FunctionId, impl IntoArgs<'r>, TypeId),
    ) -> Ref {
        let insert_at_old = insert_at.idx() < self.ir.insts.len();
        let insert_at = if insert_at_old {
            insert_at
        } else {
            self.additional[insert_at.idx() - self.ir.insts.len()].insert_at
        };
        let def = &env[function];
        debug_assert!(
            !def.flags.terminator() || !insert_at_old || position == Insert::After,
            "can't add a terminator before another instruction"
        );
        // PERF: iterating blocks every time is bad, somehow avoid this
        let block = BlockId(
            self.ir
                .blocks
                .iter()
                .position(|info| {
                    if info.body_idx < self.ir.insts.len() as _ {
                        let i = info.body_idx;
                        (i..i + info.len).contains(&insert_at.0)
                    } else {
                        info.body_idx == insert_at.0
                    }
                })
                .unwrap_or_else(|| panic!("no block found for instruction {insert_at}"))
                as _,
        );
        let args = write_args(
            &mut self.ir.extra,
            block,
            &mut self.ir.blocks,
            &def.params,
            def.varargs,
            args,
        );
        self.add_inst_before_or_after(insert_at, position, Instruction { function, args, ty })
    }

    pub fn add_inst_before_or_after(
        &mut self,
        insert_at: Ref,
        position: Insert,
        inst: Instruction,
    ) -> Ref {
        let r = Ref((self.ir.insts.len() + self.additional.len())
            .try_into()
            .expect("too many instructions"));
        self.additional.push(AdditionalInst {
            insert_at,
            position,
            inst,
        });
        r
    }

    pub fn finish_and_compress(self, env: &Environment) -> FunctionIr {
        let cf = env.get_dialect_module_if_present::<Cf>();
        let Self { mut ir, additional } = self;
        // TODO: IrModify should keep track of number of instructions replaced with Nothing/Copy so
        // that we can skip compressing. Also could preallocate the new insts buffer perfectly
        // if additional.is_empty() {
        //     return ir;
        // }

        let inst_count = (ir.insts.len() + additional.len()) as u32;
        let mut renames = RenameTable::with_counts(inst_count, ir.block_count());

        let block_graph = BlockGraph::calculate(&ir, env);

        let inst_count = ir.insts.len() + additional.len();
        let mut insts = Vec::with_capacity(inst_count);
        let new_refs = (ir.insts.len() as u32..).map(Ref);
        let old_insts = ir.insts;
        let mut new_insts: Vec<_> = additional.into_iter().zip(new_refs).collect();
        new_insts.sort_by(|(a, _), (b, _)| {
            a.insert_at
                .0
                .cmp(&b.insert_at.0)
                .then(a.position.cmp(&b.position))
        });

        let mut visited_blocks = Bitmap::new(ir.blocks.len());
        let mut removed_blocks = DHashSet::default();

        for &current_block in block_graph.postorder().iter().rev() {
            if visited_blocks.get(current_block.idx()) {
                continue;
            }

            let mut arg_renames: Option<std::vec::IntoIter<Ref>> = None;
            let mut appending_block = None;

            let mut current_block = current_block;

            let mut compress_goto = |inst: &Instruction,
                                     cf: Option<crate::ModuleOf<Cf>>,
                                     extra: &[u32],
                                     blocks: &mut [BlockInfo],
                                     current_block: &mut BlockId,
                                     appending_block: &mut Option<BlockId>,
                                     visited_blocks: &Bitmap,
                                     arg_renames: &mut Option<std::vec::IntoIter<Ref>>,
                                     renames: &mut RenameTable,
                                     goto_ref: Ref|
             -> bool {
                let Some(cf) = cf else {
                    return false;
                };
                let Some(cf_inst) = inst.as_module(cf) else {
                    return false;
                };
                if cf_inst.op() != Cf::Goto {
                    return false;
                }
                let params = cf_inst.inst.params();
                let varargs = cf_inst.inst.varargs();
                let mut args_iter = decode_args(&inst.args, params, varargs, blocks, extra);
                let Some(Argument::BlockTarget(BlockTarget(target, args))) = args_iter.next()
                else {
                    unreachable!()
                };
                debug_assert!(args_iter.next().is_none());
                let target_info = &mut blocks[target.idx()];
                if target_info.preds.len() != 1 {
                    return false;
                }
                // Goto whose target has only a single predecessor:
                // combine into one block
                debug_assert_eq!(
                    appending_block.unwrap_or(*current_block),
                    *target_info.preds.first().unwrap()
                );
                debug_assert_eq!(args.len(), target_info.arg_count as usize);
                debug_assert!(!visited_blocks.get(target.idx()), "RPO is wrong");

                // successors of the next block become successors of the current block
                let succs = std::mem::take(&mut target_info.succs);
                let appending = appending_block.unwrap_or(*current_block);
                // update the predecessor infos of the successors
                for &succ in &succs {
                    blocks[succ.idx()].replace_pred(target, appending);
                }
                let info = &mut blocks[appending.idx()];
                debug_assert_eq!(info.succs.len(), 1);
                debug_assert_eq!(info.succs[0], target);
                info.succs = succs;

                removed_blocks.insert(target);

                // trivial goto is removed
                blocks[appending_block.unwrap_or(*current_block).idx()].len -= 1;

                // continue appending the successor to the current block
                *arg_renames = Some(args.into_owned().into_iter());
                if appending_block.is_none() {
                    *appending_block = Some(*current_block);
                }
                *current_block = target;
                // goto returns a unit so this rename is correct
                renames.rename(goto_ref, Ref::UNIT);
                true
            };

            'add_block_contents: loop {
                visited_blocks.set(current_block.idx(), true);
                let info = &mut ir.blocks[current_block.idx()];
                let mut new_idx = match new_insts
                    .binary_search_by_key(&info.args_idx, |(add, _)| add.insert_at.0)
                {
                    Ok(mut new_idx) => {
                        while new_idx > 0 && new_insts[new_idx - 1].0.insert_at.0 == info.args_idx {
                            new_idx -= 1;
                        }
                        new_idx
                    }
                    Err(i) => i,
                };

                // check if block was added after and all insts will be in additional
                let is_new_block = info.args_idx >= old_insts.len() as _;
                let inst_range = if is_new_block {
                    // new blocks only contain a single Nothing "insert point" in their body
                    debug_assert_eq!(
                        info.len, 1,
                        "Can't have a previous length other than 1 in new block"
                    );
                    info.len = 0;
                    info.args_idx..info.body_idx + 1
                } else {
                    info.args_idx..info.body_idx + info.len
                }
                .map(Ref);
                info.args_idx = insts.len() as u32;
                info.body_idx = info.args_idx + info.arg_count;
                for old_ref in inst_range {
                    'before: while let Some((added, r)) = new_insts.get(new_idx)
                        && added.insert_at == old_ref
                        && added.position == Insert::Before
                    {
                        let mut inst = added.inst;
                        if inst
                            .as_module(crate::BUILTIN)
                            .is_some_and(|inst| inst.op() == Builtin::Nothing)
                        {
                            new_idx += 1;
                            continue 'before;
                        }
                        renames.visit_inst(&mut ir.extra, &ir.blocks, None, &mut inst, env);
                        new_idx += 1;
                        if added
                            .inst
                            .as_module(crate::BUILTIN)
                            .is_some_and(|inst| inst.op() == Builtin::BlockArg)
                        {
                            if let Some(renamed) =
                                arg_renames.as_mut().and_then(|renames| renames.next())
                            {
                                renames.rename(*r, renamed);
                                continue 'before;
                            }
                        } else {
                            ir.blocks[appending_block.unwrap_or(current_block).idx()].len += 1;
                            if compress_goto(
                                &inst,
                                cf,
                                &ir.extra,
                                &mut ir.blocks,
                                &mut current_block,
                                &mut appending_block,
                                &visited_blocks,
                                &mut arg_renames,
                                &mut renames,
                                *r,
                            ) {
                                continue 'add_block_contents;
                            }
                        }
                        let new_r = Ref(insts.len() as _);
                        renames.rename(*r, new_r);
                        insts.push(inst);
                    }

                    if !is_new_block {
                        let mut inst = old_insts[old_ref.idx()];
                        renames.visit_inst(&mut ir.extra, &ir.blocks, None, &mut inst, env);
                        if let Some(inst) = inst.as_module(crate::BUILTIN) {
                            if matches!(inst.op(), Builtin::Nothing | Builtin::Copy) {
                                let new = if inst.op() == Builtin::Nothing {
                                    Ref::UNIT
                                } else {
                                    Ref(inst.args[0])
                                };
                                renames.rename(old_ref, new);
                                if appending_block.is_none() {
                                    ir.blocks[current_block.idx()].len -= 1;
                                }
                                continue;
                            } else if inst.op() == Builtin::BlockArg
                                && let Some(renamed) =
                                    arg_renames.as_mut().and_then(|renames| renames.next())
                            {
                                renames.rename(old_ref, renamed);
                                continue;
                            }
                        }
                        if compress_goto(
                            &inst,
                            cf,
                            &mut ir.extra,
                            &mut ir.blocks,
                            &mut current_block,
                            &mut appending_block,
                            &visited_blocks,
                            &mut arg_renames,
                            &mut renames,
                            old_ref,
                        ) {
                            continue 'add_block_contents;
                        }
                        let new_r = Ref(insts.len() as _);
                        renames.rename(old_ref, new_r);
                        if let Some(appending) = appending_block {
                            ir.blocks[appending.idx()].len += 1;
                        }
                        insts.push(inst);
                    }
                    'after: while let Some((added, r)) = new_insts.get(new_idx)
                        && added.insert_at == old_ref
                    {
                        debug_assert_eq!(
                            added.position,
                            Insert::After,
                            "Insert::Before should already be consumed for the instruction"
                        );
                        let mut inst = added.inst;
                        if inst
                            .as_module(crate::BUILTIN)
                            .is_some_and(|inst| inst.op() == Builtin::Nothing)
                        {
                            new_idx += 1;
                            continue 'after;
                        }
                        renames.visit_inst(&mut ir.extra, &ir.blocks, None, &mut inst, env);
                        new_idx += 1;
                        if added
                            .inst
                            .as_module(crate::BUILTIN)
                            .is_some_and(|inst| inst.op() == Builtin::BlockArg)
                        {
                            if let Some(renamed) =
                                arg_renames.as_mut().and_then(|renames| renames.next())
                            {
                                renames.rename(*r, renamed);
                                continue 'after;
                            }
                        } else {
                            ir.blocks[appending_block.unwrap_or(current_block).idx()].len += 1;
                            if compress_goto(
                                &inst,
                                cf,
                                &ir.extra,
                                &mut ir.blocks,
                                &mut current_block,
                                &mut appending_block,
                                &visited_blocks,
                                &mut arg_renames,
                                &mut renames,
                                *r,
                            ) {
                                continue 'add_block_contents;
                            }
                        }
                        let new_r = Ref(insts.len() as _);
                        renames.rename(*r, new_r);
                        insts.push(inst);
                    }
                }
                break 'add_block_contents;
            }
        }
        visited_blocks.visit_unset_bits(|i| {
            if i < ir.blocks.len() {
                removed_blocks.insert(BlockId(i as _));
            }
        });

        let mut renamed_blocks = DHashMap::default();
        // let mut removed_block_iter = removed_blocks.iter().copied().peekable();
        'remove_blocks: while !removed_blocks.is_empty() {
            let last = BlockId(ir.blocks.len() as u32 - 1);
            if removed_blocks.contains(&last) {
                ir.blocks.pop();
                removed_blocks.remove(&last);
                continue 'remove_blocks;
            }
            let Some(&to_remove) = removed_blocks.iter().next() else {
                // no more blocks to remove after popping
                break;
            };

            ir.blocks.swap_remove(to_remove.idx());
            removed_blocks.remove(&to_remove);
            renamed_blocks.insert(BlockId(ir.blocks.len() as u32), to_remove);
        }

        if !renamed_blocks.is_empty() {
            for inst in &mut insts {
                crate::update_inst_refs(
                    inst,
                    env,
                    &mut ir.extra,
                    &ir.blocks,
                    std::convert::identity,
                    |block| renamed_blocks.get(&block).copied().unwrap_or(block),
                );
            }
            for block in &mut ir.blocks {
                for b in block.succs.iter_mut().chain(block.preds.iter_mut()) {
                    *b = renamed_blocks.get(b).copied().unwrap_or(*b);
                }
            }
        }

        ir.insts = insts;
        ir
    }

    pub fn new_reg(&mut self, class: RegClass) -> MCReg {
        self.ir.new_reg(class)
    }

    pub fn add_block(&mut self, args: impl IntoIterator<Item = TypeId>) -> (BlockId, Refs, Ref) {
        let id = BlockId(self.ir.blocks.len() as _);
        let args_idx = (self.ir.insts.len() + self.additional.len()) as u32;
        self.additional
            .extend(
                args.into_iter()
                    .zip(args_idx..)
                    .map(|(arg_ty, idx)| AdditionalInst {
                        // add all arguments "after" the index of themselves so they all get grouped properly
                        // with the ability to insert after and before them preserved
                        insert_at: Ref(idx),
                        position: Insert::After,
                        inst: Instruction {
                            function: crate::FunctionId {
                                module: crate::ModuleId::BUILTINS,
                                function: Builtin::BlockArg.id(),
                            },
                            args: [0, 0],
                            ty: arg_ty,
                        },
                    }),
            );
        let body_idx = (self.ir.insts.len() + self.additional.len()) as u32;
        let arg_count = body_idx - args_idx;
        self.ir.blocks.push(BlockInfo {
            arg_count,
            args_idx,
            body_idx,
            len: 1,
            preds: Vec::new(),
            succs: Vec::new(),
        });
        // insert point instruction
        self.additional.push(AdditionalInst {
            insert_at: Ref(body_idx),
            position: Insert::After,
            inst: Instruction::NOTHING,
        });
        let arg_refs = Refs {
            idx: args_idx,
            count: arg_count,
        };
        (id, arg_refs, Ref(body_idx))
    }

    pub fn get_block_insert_point(&self, block: BlockId) -> Ref {
        let info = &self.ir.blocks[block.idx()];
        debug_assert!(
            info.len > 0,
            "Can't determine insert point for an empty block, this should never happen"
        );
        Ref(info.body_idx + info.len - 1)
    }

    pub fn replace_with(&mut self, env: &Environment, r: Ref, new: Ref) {
        let block = self.inst_block(r);
        let ty = self.get_ref_ty(new);
        self.on_inst_remove(env, r, block);
        let inst = if (r.0 as usize) < self.ir.insts.len() {
            &mut self.ir.insts[r.0 as usize]
        } else {
            &mut self.additional[r.0 as usize - self.ir.insts.len()].inst
        };
        inst.function = FunctionId {
            module: crate::ModuleId::BUILTINS,
            function: Builtin::Copy.id(),
        };
        inst.args = [new.0, 0];
        inst.ty = ty;
    }

    pub fn delete(&mut self, env: &Environment, r: Ref) {
        let block = self.inst_block(r);
        self.on_inst_remove(env, r, block);
        let inst = if (r.0 as usize) < self.ir.insts.len() {
            &mut self.ir.insts[r.0 as usize]
        } else {
            &mut self.additional[r.0 as usize - self.ir.insts.len()].inst
        };
        *inst = Instruction::NOTHING;
    }

    pub fn inst_block(&self, r: Ref) -> BlockId {
        let r = if r.idx() < self.ir.insts.len() {
            r
        } else {
            self.additional[r.idx()].insert_at
        };
        // PERF: iterating blocks every time is bad, avoid this maybe with a BTreeMap in IrModify
        BlockId(
            self.ir
                .blocks
                .iter()
                .position(|info| {
                    // FIXME: arg_count does not work as the offset after adding block args
                    (info.args_idx..info.args_idx + info.arg_count + info.len).contains(&r.0)
                })
                .unwrap_or_else(|| panic!("no block found for ref {r}")) as _,
        )
    }

    pub fn replace<'r>(
        &mut self,
        env: &Environment,
        r: Ref,
        (function, args, ty): (FunctionId, impl IntoArgs<'r>, TypeId),
    ) {
        let block = self.inst_block(r);
        let def = &env[function];
        let args = write_args(
            &mut self.ir.extra,
            block,
            &mut self.ir.blocks,
            &def.params,
            def.varargs,
            args,
        );
        self.replace_with_inst(env, r, Instruction { function, args, ty });
    }

    pub fn replace_with_inst(&mut self, env: &Environment, r: Ref, new_inst: Instruction) {
        let block = self.inst_block(r);
        self.on_inst_remove(env, r, block);
        if r.idx() < self.ir.insts.len() {
            self.ir.insts[r.idx()] = new_inst;
        } else {
            self.additional[r.idx() - self.ir.insts.len()].inst = new_inst;
        }
    }

    fn on_inst_remove(&mut self, env: &Environment, r: Ref, block: BlockId) {
        let mut succs = std::mem::take(&mut self.ir.blocks[block.idx()].succs);
        let inst = self.get_inst(r);
        let def = &env[inst.function];
        let args = inst.args_inner(def.params(), def.varargs(), &self.ir.blocks, &self.ir.extra);
        let mut removed = Vec::new();
        for arg in args {
            if let Argument::BlockTarget(BlockTarget(target, _)) = arg {
                let i = succs
                    .iter()
                    .position(|&succ| succ == target)
                    .expect("Successor was not present");
                succs.swap_remove(i);
                removed.push(target);
            }
        }
        debug_assert!(self.ir.blocks[block.idx()].succs.is_empty());
        self.ir.blocks[block.idx()].succs = succs;
        for target in removed {
            let info = &mut self.ir.blocks[target.idx()];
            let i = info
                .preds
                .iter()
                .position(|&pred| pred == block)
                .expect("Predecessor was not present");
            info.preds.swap_remove(i);
        }
    }

    pub fn get_block(&self, block: BlockId) -> &BlockInfo {
        &self.ir.blocks[block.idx()]
    }

    /// Gets the Ref to the first instruction in the given block before potential modifications.
    pub fn get_original_block_start(&self, block: BlockId) -> Ref {
        let info = &self.ir.blocks[block.idx()];
        Ref(info.args_idx + info.arg_count)
    }

    pub fn get_block_args(&self, block: BlockId) -> Refs {
        // FIXME: this is incorrect since block args can be changed
        self.ir.get_block_args(block)
    }

    pub fn get_original_block_refs(&self, block: BlockId) -> Refs {
        self.ir.get_block_refs(block)
    }

    pub fn prepare_instruction<'a, A: crate::IntoArgs<'a>>(
        &mut self,
        params: &[Parameter],
        varargs: Option<Parameter>,
        block: BlockId,
        arg: (FunctionId, A, TypeId),
    ) -> Instruction {
        self.ir.prepare_instruction(params, varargs, block, arg)
    }

    pub fn args_iter<'a>(
        &'a self,
        inst: &'a Instruction,
        env: &'a Environment,
    ) -> crate::ArgsIter<'a, 'a> {
        self.ir.args_iter(inst, env)
    }

    /// Splits a block into two before the specified instruction Ref, returning the id of the new
    /// block as well as the insert point of the existing block. The existing block will be left
    /// without a terminator.
    pub fn split_block_before(&mut self, block: BlockId, r: Ref) -> (BlockId, Ref) {
        let new_block_id = BlockId(self.ir.blocks.len() as _);
        let info = &mut self.ir.blocks[block.idx()];
        debug_assert!(
            (info.body_idx..info.body_idx + info.len).contains(&r.0),
            "Ref is outside of the block"
        );
        let split_offset = r.0 - info.body_idx;
        let new_block_idx = info.body_idx + split_offset;
        let new_block_len = info.len - split_offset;
        info.len = split_offset;
        let insert_point = if split_offset == 0 && info.arg_count == 0 {
            info.args_idx = (self.ir.insts.len() + self.additional.len()) as u32;
            info.body_idx = info.args_idx;
            info.len = 1;
            let insert_point = Ref(info.body_idx);
            // insert point instruction
            self.additional.push(AdditionalInst {
                insert_at: insert_point,
                position: Insert::After,
                inst: Instruction::NOTHING,
            });
            insert_point
        } else {
            Ref(info.body_idx + split_offset - 1)
        };
        let succs = {
            let mut succs = std::mem::take(&mut info.succs);
            for succ in &mut succs {
                if *succ == block {
                    // block has itself as successor, can just replace it directly with the split block id
                    *succ = new_block_id;
                } else {
                    self.ir.blocks[succ.idx()].replace_pred(block, new_block_id);
                }
            }
            succs
        };
        self.ir.blocks.push(BlockInfo {
            arg_count: 0,
            args_idx: new_block_idx,
            body_idx: new_block_idx,
            len: new_block_len,
            preds: Vec::new(),
            succs,
        });
        (new_block_id, insert_point)
    }

    pub fn add_manual_preds(&mut self, block: BlockId, preds: impl Iterator<Item = BlockId>) {
        self.ir.blocks[block.idx()].succs.extend(preds);
    }
    pub fn add_manual_succs(&mut self, block: BlockId, succs: impl Iterator<Item = BlockId>) {
        self.ir.blocks[block.idx()].succs.extend(succs);
    }
}

#[derive(Debug)]
struct AdditionalInst {
    insert_at: Ref,
    position: Insert,
    inst: Instruction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Insert {
    Before,
    After,
}

#[cfg(test)]
mod tests {
    use crate::{BlockId, Environment, Primitive, dialect, modify::IrModify, verify};

    #[test]
    fn add_block_arg() {
        let mut env = Environment::new(Primitive::create_infos());
        env.get_dialect_module::<dialect::Arith>();
        env.get_dialect_module::<dialect::Cf>();
        let src = r#"
            b0:
                %0 = arith.Int 0 :: I32
                     cf.Goto b1
            b1:
                %3 = arith.Int 10 :: I32
                %4 = arith.LT %0 %3 :: I1
                     cf.Branch %4 b2 b3
            b2:
                %7 = arith.Int 1 :: I32
                %8 = arith.Add %0 %7 :: I32
                     cf.Goto b1
            b3:
                     cf.Ret unit
        "#;
        let (function, mut types) = crate::parse::parse_function_body(&env, src);
        verify::function_body(&env, &function, &types, "test");
        let mut ir = IrModify::new(function);
        let i64 = types.add(Primitive::I64);
        let (_, n) = ir.add_block_arg(&env, BlockId(1), i64);
        assert_eq!(n, 0);
        let function = ir.finish_and_compress(&env);
        println!("{}", function.display(&env, &types));
        verify::function_body(&env, &function, &types, "test");
    }
}
