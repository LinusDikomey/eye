use core::fmt;
use std::path::Path;

use ir::{
    GlobalId, MCReg, ModuleId,
    mc::{Abi, BackendState},
    pipeline::FunctionPass,
};

use crate::{
    arch::x86::X86,
    exe::elf::{SectionIdx, relocation::RelaWriter, symtab::SymtabIdx},
};

mod arch;
mod exe;

#[derive(Debug)]
pub enum Error {
    IO(std::io::Error),
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
        target: Option<&str>,
        out_file: &Path,
    ) -> Result<(), Error> {
        assert!(target.is_none(), "todo: check target");

        let mut writer = exe::elf::ElfObjectWriter::new();
        let mut symtab = exe::elf::symtab::SymtabWriter::new();

        let text = writer.section(
            exe::elf::SectionHeader {
                name: ".text".to_owned(),
                ty: exe::elf::SectionHeaderType::Progbits,
                flags: exe::elf::SectionHeaderFlags {
                    alloc: true,
                    execinstr: true,
                    ..Default::default()
                },
                addr: 0,
                link: SectionIdx::NONE,
                info: 0,
                addralign: 16,
                entsize: 0,
            },
            Vec::new(),
        );

        let rodata = writer.section(
            exe::elf::SectionHeader {
                name: ".rodata".to_owned(),
                ty: exe::elf::SectionHeaderType::Progbits,
                flags: exe::elf::SectionHeaderFlags {
                    alloc: true,
                    ..Default::default()
                },
                addr: 0,
                link: SectionIdx::NONE,
                info: 0,
                addralign: 8,
                entsize: 0,
            },
            Vec::new(),
        );

        let data = writer.section(
            exe::elf::SectionHeader {
                name: ".data".to_owned(),
                ty: exe::elf::SectionHeaderType::Progbits,
                flags: exe::elf::SectionHeaderFlags {
                    alloc: true,
                    write: true,
                    ..Default::default()
                },
                addr: 0,
                link: SectionIdx::NONE,
                info: 0,
                addralign: 8,
                entsize: 0,
            },
            Vec::new(),
        );

        // file entry
        let file_name = writer.add_str(env[module_id].name());
        symtab.entry(exe::elf::symtab::Entry {
            name_index: file_name,
            bind: exe::elf::symtab::Bind::Local,
            ty: exe::elf::symtab::Type::File,
            visibility: exe::elf::symtab::Visibility::Default,
            section_index: SectionIdx::ABSENT,
            value: 0,
            size: 0,
        });
        // section entry
        symtab.entry(exe::elf::symtab::Entry {
            name_index: 0,
            bind: exe::elf::symtab::Bind::Local,
            ty: exe::elf::symtab::Type::Section,
            visibility: exe::elf::symtab::Visibility::Default,
            section_index: text,
            value: 0,
            size: 0,
        });

        let isel = arch::x86::InstructionSelector::new(env);
        let mc = env.get_dialect_module::<ir::mc::Mc>();
        let x86 = isel.x86;
        let abi = arch::x86::get_target_abi();
        let mut relocations = Vec::new();
        let mut global_relocations: Vec<(GlobalId, u64)> = Vec::new();

        let mut function_offsets = vec![0u64; env[module_id].function_ids().len()];

        let mut pipeline = ir::pipeline::Pipeline::new("backend");
        pipeline.add_function_pass(Box::new(Isel {
            isel,
            module_id,
            abi,
        }));
        pipeline.add_function_pass(Box::new(ir::mc::Regalloc::<arch::x86::X86> {
            mc: isel.mc,
            preoccupied: arch::x86::PREOCCUPIED_REGISTERS,
            isa: x86,
            abi,
        }));
        pipeline.add_function_pass(Box::new(arch::x86::PrologueEpilogueInsertion { x86, abi }));

        // emit functions

        let symtab_entries: Box<[SymtabIdx]> = (env[module_id].function_ids())
            .zip(function_offsets.iter_mut())
            .map(|(id, function_offset)| {
                let func = &env[module_id][id];
                let (section_index, offset_in_section, size) = if let Some(ir) = func.ir() {
                    let offset = writer[text].len() as u64;
                    *function_offset = offset;
                    // PERF: cloning ir, types, name
                    let ir = ir.clone();
                    let mut types = func.types().clone();
                    let name = func.name.clone();
                    let mir = pipeline
                        .process_function_with_regs::<arch::x86::Reg>(env, ir, &mut types, &name);

                    tracing::debug!(target: "backend-ir",
                        function = name,
                        "Final machine IR:\n{}",
                        mir.display_with_phys_regs::<arch::x86::Reg>(env, &types)
                    );
                    arch::x86::write(
                        env,
                        mc,
                        x86,
                        &mir,
                        &mut writer[text],
                        &mut relocations,
                        &mut global_relocations,
                    );
                    let size = writer[text].len() as u64 - offset;
                    (text, offset, size)
                } else {
                    (SectionIdx::NONE, 0, 0)
                };
                let name_index = writer.add_str(&env[module_id][id].name);
                symtab.entry(exe::elf::symtab::Entry {
                    name_index,
                    bind: exe::elf::symtab::Bind::Global,
                    ty: exe::elf::symtab::Type::Function,
                    visibility: exe::elf::symtab::Visibility::Default,
                    section_index,
                    value: offset_in_section,
                    size,
                })
            })
            .collect();

        // emit globals

        let global_symtab_entries: Box<[SymtabIdx]> = env[module_id]
            .globals()
            .map(|global| {
                let name_index = writer.add_str(&global.name);
                let section = if global.readonly { rodata } else { data };
                let value = writer[section].len() as u64;
                writer[section].extend_from_slice(&global.value);
                symtab.entry(exe::elf::symtab::Entry {
                    name_index,
                    bind: exe::elf::symtab::Bind::Global,
                    ty: exe::elf::symtab::Type::Object,
                    visibility: exe::elf::symtab::Visibility::Default,
                    section_index: section,
                    value,
                    size: global.value.len() as u64,
                })
            })
            .collect();

        // emit relocations to elf

        let mut rela = RelaWriter::new();
        for (function_id, i) in relocations {
            debug_assert_eq!(function_id.module, module_id);
            let is_extern = env[module_id][function_id.function].ir().is_none();
            if is_extern {
                rela.entry(exe::elf::relocation::Rela {
                    r_offset: i,
                    sym: symtab_entries[function_id.function.idx()],
                    ty: exe::elf::relocation::RelaType::X86_64Plt32,
                    r_addend: -4, // call rel32, therefore offset by -4 since RIP is behind the instruction
                });
            } else {
                let offset = function_offsets[function_id.function.idx()]
                    .checked_signed_diff(i)
                    .and_then(|i| i.checked_sub(4))
                    .and_then(|i| i32::try_from(i).ok())
                    .expect("Function call is out of range for i32 offset");

                writer[text][i as usize..i as usize + 4].copy_from_slice(&offset.to_le_bytes());
            }
        }

        for (global_id, offset) in global_relocations {
            rela.entry(exe::elf::relocation::Rela {
                r_offset: offset,
                sym: global_symtab_entries[global_id.idx as usize],
                ty: exe::elf::relocation::RelaType::X86_64PC32,
                r_addend: -4,
            });
        }

        let (symtab_header, symtab_contents) = symtab.finish(writer.strtab_idx());

        let symtab_idx = writer.section(symtab_header, symtab_contents);
        let (rela_header, rela_contents) = rela.finish(text, symtab_idx);
        writer.section(rela_header, rela_contents);

        writer.write(out_file).map_err(Error::IO)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MCValue {
    /// this value doesn't have any runtime bits
    None,
    /// value is undefined and can be assumed to be any value at runtime
    Undef,
    /// an immediate (pointer-sized) constant value
    Imm(u64),
    /// value is located in a register
    Reg(MCReg),
    /// value is up to 16 bytes large and is spread across two registers (lower bits, upper bits)
    TwoRegs(MCReg, MCReg),
}

pub fn list_targets() -> Vec<String> {
    vec!["x86_64-unknown-linux".to_owned()]
}

struct Isel {
    isel: arch::x86::InstructionSelector,
    module_id: ModuleId,
    abi: &'static dyn Abi<X86>,
}
impl fmt::Debug for Isel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Isel")
    }
}
impl FunctionPass<BackendState> for Isel {
    fn run(
        &self,
        env: &ir::Environment,
        types: &ir::Types,
        ir: ir::FunctionIr,
        name: &str,
        state: &mut BackendState,
    ) -> (ir::FunctionIr, Option<ir::Types>) {
        let mut isel = self.isel;
        let (mir, types) = arch::x86::codegen(
            env,
            &ir,
            types,
            &mut isel,
            self.module_id,
            self.abi,
            state,
            name,
        );
        (mir, Some(types))
    }
}
