#![recursion_limit = "256"]

use core::fmt;
use std::path::Path;

use ir::{
    LocalFunctionId, ModuleId, Primitive, Ref, Type, Types,
    mc::{Abi, BackendState, McInst},
    modify::IrModify,
    pipeline::FunctionPass,
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
            target::Arch::X86_64 => arch::x86::init_codegen(env, module_id),
            target::Arch::Arm32 => todo!(),
            target::Arch::Aarch64 => arch::arm::init_codegen(env, module_id, &ArmAbi::Darwin64),
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
        name: &str,
        state: &mut BackendState,
    ) -> (ir::FunctionIr, Option<ir::Types>) {
        let _enter = tracing::span!(
            target: "isel",
            tracing::Level::INFO,
            "function",
            function = name,
        )
        .entered();
        let ir = self
            .isel
            .codegen(env, &ir, types, self.module_id, self.abi, state);
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
