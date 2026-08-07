mod abi;
mod dialect;
mod parcopy;
mod regalloc;

#[doc(hidden)]
pub mod macros;

pub use abi::Abi;
pub use dialect::{Mc, McInsts};
use dmap::DHashMap;
pub use macros::registers;
pub use parcopy::ParcopySolver;
pub use regalloc::{Regalloc, regalloc};

use crate::Argument;
use crate::BlockGraph;
use crate::BlockId;
use crate::Environment;
use crate::FunctionId;
use crate::FunctionIr;
use crate::Inst;
use crate::Layout;
use crate::MCReg;
use crate::ModuleId;
use crate::ModuleOf;
use crate::Ref;
use crate::TypeId;
use crate::modify::IrModify;
use crate::slots::Slots;
use crate::use_counts::UseCounts;
use std::hash::Hash;
use std::ops::{BitAnd, BitOr, Not};

pub trait McInst: Inst {
    type Reg: Register;
    fn implicit_def(&self, abi: &'static dyn Abi<Self>) -> <Self::Reg as Register>::RegisterBits;
    fn implicit_use(&self, abi: &'static dyn Abi<Self>) -> <Self::Reg as Register>::RegisterBits;
}

pub trait Register: 'static + Copy + Eq + Hash {
    const DEFAULT: Self;
    type RegisterBits: Copy
        + BitAnd<Output = Self::RegisterBits>
        + Not<Output = Self::RegisterBits>
        + BitOr<Output = Self::RegisterBits>
        + Eq;
    type Class: Copy + Eq + Into<u8> + TryFrom<u8>;
    const NO_BITS: Self::RegisterBits;
    const ALL_BITS: Self::RegisterBits;

    fn to_str(self) -> &'static str;
    fn encode(self) -> u32;
    fn decode(value: u32) -> Self;

    fn bit_index(self) -> u8;
    fn get_bit(self, bits: &Self::RegisterBits) -> bool;
    fn set_bit(self, bits: &mut Self::RegisterBits, bit: bool);
    fn allocate_reg(free_bits: Self::RegisterBits, class: Self::Class) -> Option<Self>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UnknownRegister(u32);
impl Register for UnknownRegister {
    const DEFAULT: Self = Self(0);

    type RegisterBits = u8;
    type Class = u8;
    const NO_BITS: Self::RegisterBits = 0;
    const ALL_BITS: Self::RegisterBits = 0;

    fn to_str(self) -> &'static str {
        "unknown"
    }

    fn encode(self) -> u32 {
        self.0
    }

    fn decode(value: u32) -> Self {
        Self(value)
    }

    fn bit_index(self) -> u8 {
        // this just truncates the register but it's fine since it's only used for some small test
        // cases for parcopy.
        self.0 as u8
    }

    fn get_bit(self, _bits: &Self::RegisterBits) -> bool {
        false
    }

    fn set_bit(self, _bits: &mut Self::RegisterBits, _bit: bool) {}

    fn allocate_reg(_free_bits: Self::RegisterBits, _class: Self::Class) -> Option<Self> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpType {
    Non,
    Reg,
    Mem,
    Imm,
    Blk,
    Fun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpUsage {
    Def = 0b01,
    Use = 0b10,
    DefUse = 0b11,
}

pub struct StackSlot {
    pub offset: u32,
    pub size: u32,
}

pub fn parallel_copy(
    mc: ModuleOf<Mc>,
    args: &[MCReg],
) -> (FunctionId, impl crate::IntoArgs<'_>, TypeId) {
    let f = crate::FunctionId {
        module: mc.id(),
        function: Mc::Copy.id(),
    };
    (f, ((), args), TypeId::UNIT)
}

pub fn parallel_copy_args(
    mc: ModuleOf<Mc>,
    args: &[MCReg],
    unit: crate::TypeId,
) -> (FunctionId, impl crate::IntoArgs<'_>, crate::TypeId) {
    let f = crate::FunctionId {
        module: mc.id(),
        function: Mc::AssignBlockArgs.id(),
    };
    (f, ((), args), unit)
}

#[derive(Default)]
pub struct BackendState {
    pub stack_size: u32,
}
impl BackendState {
    /// creates a properly aligned stack index assuming the stack frame's total alignment is at least
    /// as large as the one from the Layout.
    /// Assumes a stack growing down, subtract layout.size if the stack should grow up.
    pub fn alloc_stack(&mut self, layout: Layout) -> u32 {
        let align = layout.align.get() as u32;
        self.stack_size = self.stack_size.next_multiple_of(align);
        self.stack_size += layout.size as u32;
        self.stack_size
    }
}

pub struct IselCtx<'a, I: McInst> {
    pub main_module: ModuleId,
    pub regs: Slots<MCReg>,
    pub abi: &'static dyn Abi<I>,
    pub unit: crate::TypeId,
    mc: ModuleOf<Mc>,
    use_counts: UseCounts,
    pub state: &'a mut BackendState,
    next_blocks: Box<[Option<BlockId>]>,
    pub stack_slots: &'a DHashMap<Ref, u32>,
}
impl<'a, I: McInst> IselCtx<'a, I> {
    pub fn new(
        main_module: ModuleId,
        env: &Environment,
        ir: &IrModify,
        regs: Slots<MCReg>,
        mc: ModuleOf<Mc>,
        unit: TypeId,
        abi: &'static dyn Abi<I>,
        state: &'a mut BackendState,
        block_graph: &BlockGraph<FunctionIr>,
        stack_slots: &'a DHashMap<Ref, u32>,
    ) -> Self {
        let use_counts = UseCounts::compute(ir, env);
        let mut next_blocks: Box<[Option<BlockId>]> = vec![None; ir.block_ids().len()].into();
        for order in block_graph.postorder().windows(2) {
            // after comes first since we actually care about rpo
            let after = order[0];
            let block = order[1];
            next_blocks[block.idx()] = Some(after);
        }
        Self {
            main_module,
            unit,
            regs,
            mc,
            use_counts,
            abi,
            state,
            next_blocks,
            stack_slots,
        }
    }

    pub fn remove_use(&mut self, r: Ref, ir: &mut IrModify, env: &Environment) {
        let count = self.use_counts[r].get() - 1;
        self.use_counts[r].set(count);
        let inst = ir.get_inst(r);
        if count == 0 && env[inst.function].flags.pure() {
            // last use of pure instruction was removed, remove uses from it's inputs and delete it

            // PERF: use small vec here
            let args: Box<[Ref]> = ir
                .args_iter(inst, env)
                .filter_map(|arg| {
                    if let Argument::Ref(r) = arg {
                        Some(r)
                    } else {
                        None
                    }
                })
                .collect();
            for arg in args {
                self.remove_use(arg, ir, env);
            }
            ir.replace_with(env, r, Ref::UNIT);
        }
    }

    pub fn add_use(&self, r: Ref) {
        self.use_counts[r].set(self.use_counts[r].get() + 1);
    }

    pub fn next_block(&self, block: BlockId) -> Option<BlockId> {
        self.next_blocks[block.idx()]
    }
}
impl<'a, I: McInst> crate::rewrite::RewriteCtx for IselCtx<'a, I> {
    fn begin_block(&mut self, env: &Environment, ir: &mut IrModify, block: BlockId) {
        if block == BlockId::ENTRY {
            return;
        }
        let info = ir.get_block(block);
        let args = self.regs.get_range(
            Ref::index(info.args_idx),
            Ref::index(info.args_idx + info.arg_count),
        );
        let f = FunctionId {
            module: self.mc.id(),
            function: Mc::IncomingBlockArgs.id(),
        };
        let start = ir.get_original_block_start(block);
        ir.add_before(env, start, (f, ((), args), self.unit));
    }
}

impl<'a, I: McInst> IselCtx<'a, I> {
    pub fn use_count(&self, r: Ref) -> u32 {
        if r.into_ref().is_some() {
            self.use_counts[r].get()
        } else {
            0
        }
    }

    pub fn single_use(&self, r: Ref) -> bool {
        self.use_counts[r].get() == 1
    }

    pub fn unused(&self, r: Ref) -> bool {
        self.use_counts[r].get() == 0
    }

    pub fn create_args_copy(
        &mut self,
        env: &Environment,
        before: Ref,
        mc: ModuleOf<Mc>,
        ir: &mut IrModify,
        target: BlockId,
        args: &[Ref],
        set_bool: impl Fn(&mut IrModify, &Environment, MCReg, bool),
    ) {
        let arg_refs = ir.get_block_args(target);
        debug_assert_eq!(args.len(), arg_refs.count() as usize);
        let copyargs: Vec<MCReg> = arg_refs
            .iter()
            .zip(args)
            .flat_map(|(to, &from)| {
                let from = if from.is_ref() {
                    self.regs.get(from)
                } else {
                    match from {
                        Ref::UNIT => &[],
                        Ref::TRUE | Ref::FALSE => {
                            let to = self.regs.get_one(to);
                            set_bool(ir, env, to, from == Ref::TRUE);
                            return None.into_iter().flatten();
                        }
                        _ => unreachable!(),
                    }
                };
                let to = self.regs.get(to);
                debug_assert_eq!(from.len(), to.len());
                Some(
                    to.iter()
                        .copied()
                        .zip(from.iter().copied())
                        .flat_map(|(a, b)| [a, b]),
                )
                .into_iter()
                .flatten()
            })
            .collect();
        if !copyargs.is_empty() {
            ir.add_before(env, before, parallel_copy_args(mc, &copyargs, self.unit));
        }
    }

    pub fn copy(
        &mut self,
        env: &Environment,
        before: Ref,
        mc: ModuleOf<Mc>,
        ir: &mut IrModify,
        args: &[MCReg],
    ) {
        ir.add_before(env, before, parallel_copy(mc, args));
    }
}

pub fn used_physical_registers<R: Register>(ir: &IrModify, env: &Environment) -> R::RegisterBits {
    let mut bits = R::NO_BITS;
    for i in 0..ir.inst_count() {
        let inst = ir.get_inst(Ref::index(i));
        for arg in ir.args_iter(inst, env) {
            let Argument::MCReg(r) = arg else {
                continue;
            };
            let Some(phys) = r.phys::<R>() else {
                continue;
            };
            phys.set_bit(&mut bits, true);
        }
    }
    bits
}
