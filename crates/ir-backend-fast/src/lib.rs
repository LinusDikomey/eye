use core::fmt;
use std::path::Path;

use ir::{
    LocalFunctionId, ModuleId,
    mc::{Abi, BackendState, McInst},
    pipeline::FunctionPass,
};

mod arch;
mod exe;

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
            target::Arch::Aarch64 => todo!(),
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
        let (ir, types) = self
            .isel
            .codegen(env, &ir, types, self.module_id, self.abi, state, name);
        (ir, Some(types))
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
        function_name: &str,
    ) -> (ir::FunctionIr, ir::Types);
}
