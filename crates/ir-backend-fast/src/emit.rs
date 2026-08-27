use std::collections::VecDeque;

use ir::{
    Argument, Bitmap, BlockId, Environment, FunctionIr, MCReg, ModuleOf, StackSlot,
    TypedInstruction,
    block_graph::Blocks,
    mc::{Mc, McInst, ParcopySolver, Register},
};

use crate::Relocation;

pub trait Emit: McInst {
    const TMP: Self::Reg;

    fn implement_copy(text: &mut Vec<u8>, to: Self::Reg, from: Self::Reg);
    fn implement_stack_addr(text: &mut Vec<u8>, to: Self::Reg, slot: StackSlot);
    fn emit(e: &mut Emitter<Self::Reg>, inst: TypedInstruction<Self>);
}

pub struct Emitter<'a, R: Register> {
    pub ir: &'a FunctionIr,
    pub text: &'a mut Vec<u8>,
    pub relocations: &'a mut Vec<Relocation>,
    start: usize,
    parcopy: ParcopySolver<R>,
    block_queue: VecDeque<BlockId>,
    queued_blocks: Bitmap,
    block_offsets: Box<[Option<u32>]>,
    pub missing_block_addrs: Vec<(u32, BlockId)>,
}
impl<'a, R: Register> Emitter<'a, R> {
    pub fn offset_in_function(&self) -> u32 {
        (self.text.len() - self.start)
            .try_into()
            .expect("Function machine code too long")
    }

    pub fn is_next(&self, block: BlockId) -> bool {
        self.block_queue.front().is_some_and(|&b| b == block)
    }

    pub fn block_offset(&self, block: BlockId) -> Option<u32> {
        self.block_offsets[block.idx()]
    }

    pub fn parcopy<I: Emit<Reg = R>>(&mut self, args: impl Clone + IntoIterator<Item = R>) {
        self.parcopy.parcopy(
            args,
            |to, from| I::implement_copy(self.text, to, from),
            I::TMP,
        );
    }

    pub fn materialize_stack_addr<I: Emit<Reg = R>>(&mut self, reg: MCReg, slot: StackSlot) {
        I::implement_stack_addr(self.text, reg.phys().unwrap(), slot);
    }
}

pub fn emit<I: Emit>(
    env: &Environment,
    mc: ModuleOf<Mc>,
    m: ModuleOf<I>,
    ir: &FunctionIr,
    text: &mut Vec<u8>,
    relocations: &mut Vec<Relocation>,
) {
    let mut emitter = Emitter {
        ir,
        start: text.len(),
        text,
        relocations,
        parcopy: ParcopySolver::<I::Reg>::new(),
        block_queue: VecDeque::from([BlockId::ENTRY]),
        queued_blocks: Bitmap::new(ir.block_count() as usize),
        block_offsets: vec![None; ir.block_count() as usize].into_boxed_slice(),
        missing_block_addrs: Vec::new(),
    };
    emitter.queued_blocks.set(BlockId::ENTRY.idx(), true);

    while let Some(block) = emitter.block_queue.pop_front() {
        let current_offset = emitter.offset_in_function();
        let offset = &mut emitter.block_offsets[block.idx()];
        if offset.is_some() {
            continue;
        }
        *offset = Some(current_offset);
        for succ in ir.successors(env, block) {
            if emitter.queued_blocks.get(succ.idx()) {
                continue;
            }
            emitter.queued_blocks.set(succ.idx(), true);
            emitter.block_queue.push_back(succ);
        }

        for (r, i) in ir.get_block(block) {
            if let Some(inst) = i.as_module(mc) {
                match inst.op() {
                    Mc::IncomingBlockArgs => {}
                    Mc::Copy | Mc::AssignBlockArgs => {
                        let args = ir.args_iter(i, env).map(|arg| {
                            let Argument::MCReg(r) = arg else {
                                unreachable!()
                            };
                            r.phys::<I::Reg>().expect("need physical registers")
                        });
                        emitter.parcopy::<I>(args);
                    }
                    Mc::StackValue => {
                        let (reg, slot): (MCReg, u32) = ir.typed_args(&inst);
                        let slot = StackSlot::new(slot);
                        emitter.materialize_stack_addr::<I>(reg, slot);
                    }
                }
                continue;
            }

            let Some(inst) = i.as_module(m) else {
                let module = env[i.module()].name();
                panic!("expected machine instruction but encountered module '{module}' at {r}");
            };
            I::emit(&mut emitter, inst);
        }
    }

    for (offset_location, block) in emitter.missing_block_addrs {
        // TODO: this should depend on the emitted architecture
        let block_offset = emitter.block_offsets[block.idx()].unwrap();
        let offset: i32 = (block_offset as i64 - offset_location as i64 - 4)
            .try_into()
            .unwrap();
        let i = emitter.start + offset_location as usize;
        emitter.text[i..i + 4].copy_from_slice(&offset.to_le_bytes());
    }
}
