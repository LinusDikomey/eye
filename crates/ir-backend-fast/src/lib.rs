#![recursion_limit = "256"]

use core::fmt;
use std::path::Path;

use dmap::DHashMap;
use ir::{
    Argument, BlockGraph, BlockId, Environment, FunctionId, FunctionIr, Layout, LocalFunctionId,
    MCReg, ModuleId, ModuleOf, Primitive, Ref, Type, TypeId, Types,
    mc::{Abi, Mc, McInst, parallel_copy, parallel_copy_args},
    modify::IrModify,
    pipeline::FunctionPass,
    slots::Slots,
    use_counts::UseCounts,
};

mod arch;
mod emit;
mod exe;

use emit::{Emit, emit};

use crate::arch::arm::ArmAbi;

type CodegenFn = Box<
    dyn Fn(&ir::Environment, ir::FunctionIr, ir::Types, &str, &mut Vec<u8>, &mut Vec<Relocation>),
>;

#[derive(Debug)]
pub enum Error {
    IO(std::io::Error),
    UnsupportedArch,
}

enum Relocation {
    FunctionCall(LocalFunctionId, u64),
    FunctionAddr(LocalFunctionId, u64),
    GlobalAddr(u32, u64),
}

#[derive(Default)]
pub struct Backend {}
impl Backend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn emit_module(
        &self,
        env: &mut ir::Environment,
        module_id: ir::ModuleId,
        target: &target::Target,
        out_file: &Path,
    ) -> Result<(), Error> {
        let codegen: CodegenFn = match &target.arch {
            target::Arch::X86_64 => arch::x86::init_codegen(env, module_id, target),
            target::Arch::Arm32 => match target.os {
                target::Os::Linux => {
                    arch::arm::init_codegen(env, module_id, target, &ArmAbi::Arm32)
                }
                _ => return Err(Error::UnsupportedArch),
            },
            target::Arch::Aarch64 => arch::arm::init_codegen(
                env,
                module_id,
                target,
                match target.os {
                    target::Os::Linux => &ArmAbi::Arm64,
                    target::Os::Darwin => &ArmAbi::Darwin64,
                    _ => return Err(Error::UnsupportedArch),
                },
            ),
            target::Arch::Other(other) => unimplemented!("Unsupported architecture {other}"),
        };

        match target.os {
            target::Os::Linux => exe::elf::emit(env, module_id, out_file, codegen),
            target::Os::Darwin => exe::macho::emit(env, module_id, out_file, &target.arch, codegen),
            _ => unimplemented!("fast backend target for os {}", target.os),
        }
    }
}

pub fn list_targets() -> Vec<String> {
    vec!["x86_64-unknown-linux".to_owned()]
}

struct Isel<I: McInst, S> {
    isel: S,
    module_id: ModuleId,
    abi: &'static dyn Abi<I>,
    target: target::Target,
}
impl<I: McInst, S> fmt::Debug for Isel<I, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Isel")
    }
}
impl<I: McInst, S: InstructionSelector<I>> FunctionPass<BackendState> for Isel<I, S> {
    fn run(
        &self,
        env: &ir::Environment,
        types: &ir::Types,
        ir: ir::FunctionIr,
        _name: &str,
        state: &mut BackendState,
    ) -> (ir::FunctionIr, Option<ir::Types>) {
        let ir = self.isel.codegen(
            env,
            &ir,
            types,
            self.module_id,
            self.abi,
            &self.target,
            state,
        );
        (ir, None)
    }
}

trait InstructionSelector<I: McInst>: Copy {
    fn codegen(
        &self,
        env: &ir::Environment,
        body: &ir::FunctionIr,
        types: &ir::Types,
        main_module: ModuleId,
        abi: &'static dyn Abi<I>,
        target: &target::Target,
        state: &mut BackendState,
    ) -> ir::FunctionIr;
}

fn primitive_of_ref(r: Ref, ir: &IrModify, types: &Types) -> Primitive {
    let Type::Primitive(p) = types[ir.get_ref_ty(r)] else {
        unreachable!()
    };
    p.try_into().expect("Invalid primitive encountered")
}

fn int_size_of_ref(r: Ref, ir: &IrModify, types: &Types) -> Size {
    arith_class(r, ir, types).1
}

pub enum ArithClass {
    Signed,
    Unsigned,
    Float,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Size {
    S8,
    S16,
    S32,
    S64,
    S128,
}

pub fn arith_class(r: Ref, ir: &IrModify, types: &Types) -> (ArithClass, Size) {
    match primitive_of_ref(r, ir, types) {
        Primitive::I1 | Primitive::I8 => (ArithClass::Signed, Size::S8),
        Primitive::I16 => (ArithClass::Signed, Size::S16),
        Primitive::I32 => (ArithClass::Signed, Size::S32),
        Primitive::I64 => (ArithClass::Signed, Size::S64),
        Primitive::I128 => (ArithClass::Signed, Size::S128),
        Primitive::U8 => (ArithClass::Unsigned, Size::S8),
        Primitive::U16 => (ArithClass::Unsigned, Size::S16),
        Primitive::U32 => (ArithClass::Unsigned, Size::S32),
        Primitive::U64 => (ArithClass::Unsigned, Size::S64),
        Primitive::U128 => (ArithClass::Unsigned, Size::S128),
        Primitive::F32 => (ArithClass::Float, Size::S32),
        Primitive::F64 => (ArithClass::Float, Size::S64),
        Primitive::Ptr => unreachable!("unsupported type Ptr for arithmetic"),
    }
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
    pub target: &'a target::Target,
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
        abi: &'static dyn Abi<I>,
        target: &'a target::Target,
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
            regs,
            mc,
            use_counts,
            abi,
            target,
            state,
            next_blocks,
            stack_slots,
        }
    }

    pub fn remove_use(&mut self, r: Ref, ir: &mut IrModify, env: &Environment) {
        let count = self.use_counts[r].get() - 1;
        self.use_counts[r].set(count);
        let inst = ir.get_inst(r);
        if count == 0 && env[inst.function_id()].flags().pure() {
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
impl<'a, I: McInst> ir::rewrite::RewriteCtx for IselCtx<'a, I> {
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
        ir.add_before(env, start, (f, ((), args), TypeId::UNIT));
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
            ir.add_before(env, before, parallel_copy_args(mc, &copyargs, TypeId::UNIT));
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
