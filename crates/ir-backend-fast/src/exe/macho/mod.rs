mod relocation;
mod symtab;

use std::{
    fs::File,
    io::{self, BufWriter, Write},
    ops::BitOr,
    path::Path,
};

use crate::{Error, Relocation, exe::macho::symtab::SymTab};

pub fn emit(
    env: &mut ir::Environment,
    module_id: ir::ModuleId,
    out_file: &Path,
    arch: &target::Arch,
    codegen: crate::CodegenFn,
) -> Result<(), Error> {
    let cpu_type = match arch {
        target::Arch::X86_64 => CpuType::X86_64,
        target::Arch::Aarch64 => CpuType::Arm64,
        target::Arch::Other(_) => return Err(Error::UnsupportedArch),
    };
    let mut w = MachoObjectWriter::new();

    // section indices are 1-indexed
    let text_section = SectionIdx(1);
    let const_section = SectionIdx(2);
    let data_section = SectionIdx(3);

    let mut text = Vec::new();
    let mut consts = Vec::new();
    let mut data = Vec::new();

    let mut relocations = Vec::new();

    let function_symbols: Vec<_> = env[module_id]
        .function_ids()
        .map(|id| {
            let func = &env[module_id][id];
            let n_strx = w.symtab.str(&func.name, true);
            if let Some(ir) = func.ir() {
                let offset = text.len() as u64;
                // *function_offset = offset;
                // PERF: cloning ir, types, name
                let ir = ir.clone();
                let types = func.types().clone();
                let name = func.name.clone();
                codegen(env, ir, types, &name, &mut text, &mut relocations);
                let symbolnum = w.symtab.sym(symtab::nlist_64 {
                    n_strx,
                    addr_type: symtab::SymbolAddrType::SecNumDefined,
                    vis: symtab::SymbolVisibility::External,
                    n_sect: text_section,
                    n_desc: 0,
                    n_value: offset,
                });
                // let size = text.len() as u64 - offset;
                (Some(offset), symbolnum)
            } else {
                let symbolnum = w.symtab.sym(symtab::nlist_64 {
                    n_strx,
                    addr_type: symtab::SymbolAddrType::Undefined,
                    vis: symtab::SymbolVisibility::External,
                    n_sect: SectionIdx(0),
                    n_desc: 0,
                    n_value: 0,
                });
                (None, symbolnum)
            }
        })
        .collect();

    let global_symbols: Vec<_> = env[module_id]
        .globals()
        .map(|global| {
            let name = w.symtab.str(&global.name, true);
            let (section, section_idx) = if global.readonly {
                (&mut consts, const_section)
            } else {
                (&mut data, data_section)
            };
            let offset = section.len() as u64;
            section.extend_from_slice(&global.value);

            w.symtab.sym(symtab::nlist_64 {
                n_strx: name,
                addr_type: symtab::SymbolAddrType::SecNumDefined,
                vis: symtab::SymbolVisibility::External,
                n_sect: section_idx,
                n_desc: 0,
                n_value: offset,
            })
        })
        .collect();

    let relocations = relocations
        .iter()
        .filter_map(|relocation| match *relocation {
            Relocation::FunctionCall(id, offset) | Relocation::FunctionAddr(id, offset) => {
                let (target_offset, symbol) = function_symbols[id.idx()];
                let ty = if matches!(relocation, Relocation::FunctionCall(_, _)) {
                    relocation::RelocationTypeX86_64::Branch
                } else {
                    relocation::RelocationTypeX86_64::Signed
                };
                if let Some(target_offset) = target_offset {
                    let offset = target_offset
                        .checked_signed_diff(offset)
                        .and_then(|i| i.checked_sub(4))
                        .and_then(|i| i32::try_from(i).ok())
                        .expect("Function call is out of range for i32 offset");

                    text[offset as usize..offset as usize + 4]
                        .copy_from_slice(&offset.to_le_bytes());
                    None
                } else {
                    let offset = u32::try_from(offset).unwrap();
                    Some(relocation::RelocationInfo::new(
                        offset,
                        symbol,
                        true,
                        relocation::RelocationLength::L4,
                        true,
                        ty,
                    ))
                }
            }
            Relocation::GlobalAddr(id, offset) => Some(relocation::RelocationInfo::new(
                offset.try_into().unwrap(),
                global_symbols[id as usize],
                true,
                relocation::RelocationLength::L4,
                true,
                relocation::RelocationTypeX86_64::Signed,
            )),
        })
        .collect();

    w.load_command(LoadCommand {
        necessary_for_loading: false,
        content: LoadCommandContent::Segment(SegmentLoad64 {
            segment_name: Name::TEXT_SEGMENT,
            vmaddr: 0,
            vmsize: (text.len() + consts.len()) as u64,
            max_vmem_prot: PermissionFlags::READ | PermissionFlags::EXEC,
            init_vmem_prot: PermissionFlags::READ | PermissionFlags::EXEC,
            flags: SegmentFlags::default(),
            sections: vec![
                SegmentSection64 {
                    section_name: Name::TEXT_SECTION,
                    section_addr: 0,
                    alignment: 0,
                    flag: 0,
                    contents: text,
                    relocations,
                },
                SegmentSection64 {
                    section_name: Name::CONST_SECTION,
                    section_addr: 0,
                    alignment: 0, // TODO: const alignment?
                    flag: 0,
                    contents: consts,
                    relocations: Vec::new(),
                },
            ],
        }),
    });
    w.load_command(LoadCommand {
        necessary_for_loading: false,
        content: LoadCommandContent::Segment(SegmentLoad64 {
            segment_name: Name::DATA_SEGMENT,
            vmaddr: 0,
            vmsize: data.len() as u64,
            max_vmem_prot: PermissionFlags::READ | PermissionFlags::WRITE,
            init_vmem_prot: PermissionFlags::READ | PermissionFlags::WRITE,
            flags: SegmentFlags::default(),
            sections: vec![SegmentSection64 {
                section_name: Name::DATA_SECTION,
                section_addr: 0,
                alignment: 0, // TODO: data alignment?
                flag: 0,
                contents: data,
                relocations: Vec::new(),
            }],
        }),
    });
    let mut writer = BufWriter::new(File::create(out_file).map_err(Error::IO)?);
    w.write(cpu_type, &mut writer).map_err(Error::IO)
}

pub struct MachoObjectWriter {
    symtab: SymTab,
    /// every load command except the first one which is always the symtab
    load_commands: Vec<LoadCommand>,
}
impl MachoObjectWriter {
    pub fn new() -> Self {
        Self {
            symtab: SymTab::new(),
            load_commands: Vec::new(),
        }
    }

    fn load_command(&mut self, load_command: LoadCommand) -> SectionIdx {
        self.load_commands.push(load_command);
        SectionIdx(
            self.load_commands
                .len()
                .try_into()
                .expect("too many sections"),
        )
    }

    pub fn write(self, cpu_type: CpuType, f: &mut BufWriter<File>) -> io::Result<()> {
        let symtab_load = LoadCommand {
            necessary_for_loading: false,
            content: LoadCommandContent::SymTab(self.symtab),
        };
        let magic: u32 = 0xfeedfacf;
        f.write_all(&magic.to_le_bytes())?;
        f.write_all(&(cpu_type as u32).to_le_bytes())?;
        f.write_all(&(CpuSubtypeArm::All as u32).to_le_bytes())?;
        f.write_all(&(FileType::RelocatableObject as u32).to_le_bytes())?;
        // number of load commands
        f.write_all(&(self.load_commands.len() as u32 + 1).to_le_bytes())?;
        let load_commands_size: u32 =
            symtab_load.size() + self.load_commands.iter().map(|cmd| cmd.size()).sum::<u32>();
        f.write_all(&load_commands_size.to_le_bytes())?;
        let flags = 0u32;
        f.write_all(&flags.to_le_bytes())?;

        // reserved on 64-bit binaries
        f.write_all(&0u32.to_le_bytes())?;

        let mut content_offset = u64::from(8 * 4 + load_commands_size);

        let load_commands = std::iter::once(&symtab_load).chain(&self.load_commands);

        for command in load_commands.clone() {
            command.write(f, &mut content_offset)?;
        }
        for command in load_commands {
            match &command.content {
                LoadCommandContent::Segment(segment) => {
                    for section in &segment.sections {
                        f.write_all(&section.contents)?;
                        for relocation in &section.relocations {
                            f.write_all(&relocation.bytes())?;
                        }
                    }
                }
                LoadCommandContent::SymTab(symtab) => {
                    symtab.write_content(f)?;
                }
            }
        }

        Ok(())
    }
}

#[repr(u32)]
#[rustfmt::skip]
#[allow(unused)]
pub enum CpuType {
    X86    = 0x00000007,
    X86_64 = 0x01000007,
    Arm    = 0x0000000C,
    Arm64  = 0x0100000C,
}

#[repr(u32)]
pub enum CpuSubtypeArm {
    All = 0x00000000,
}

#[repr(u32)]
enum FileType {
    RelocatableObject = 1,
}

enum LoadCommandContent {
    Segment(SegmentLoad64),
    SymTab(SymTab),
}
impl LoadCommandContent {
    fn size(&self) -> u32 {
        match self {
            LoadCommandContent::Segment(segment) => segment.size(),
            LoadCommandContent::SymTab(_) => SymTab::COMMAND_SIZE,
        }
    }
}

struct LoadCommand {
    necessary_for_loading: bool,
    content: LoadCommandContent,
}
impl LoadCommand {
    const START_SIZE: u32 = 8;
    fn size(&self) -> u32 {
        Self::START_SIZE + self.content.size()
    }

    fn write(&self, f: &mut BufWriter<File>, file_offset: &mut u64) -> io::Result<()> {
        let ty = match &self.content {
            LoadCommandContent::Segment(_) => LoadCommandType::SegmentLoad64,
            LoadCommandContent::SymTab(_) => LoadCommandType::SymTab,
        } as u32
            | if self.necessary_for_loading {
                0x80000000
            } else {
                0
            };
        f.write_all(&ty.to_le_bytes())?;
        f.write_all(&self.size().to_le_bytes())?;
        match &self.content {
            LoadCommandContent::Segment(segment) => segment.write(f, file_offset),
            LoadCommandContent::SymTab(symtab) => symtab.write(f, file_offset),
        }
    }
}

#[derive(Clone, Copy)]
struct SectionIdx(u8);

#[repr(u32)]
#[derive(Clone, Copy)]
#[allow(unused)]
enum LoadCommandType {
    SegmentLoad32 = 0x01,
    SegmentLoad64 = 0x19,
    SymTab = 0x02,
}

struct SegmentLoad64 {
    segment_name: Name,
    vmaddr: u64,
    vmsize: u64,
    max_vmem_prot: PermissionFlags,
    init_vmem_prot: PermissionFlags,
    flags: SegmentFlags,
    sections: Vec<SegmentSection64>,
}
impl SegmentLoad64 {
    fn size(&self) -> u32 {
        16 + 8 + 8 + 8 + 8 + 4 + 4 + 4 + 4 + self.sections.len() as u32 * SegmentSection64::SIZE
    }

    fn write(&self, f: &mut BufWriter<File>, file_offset: &mut u64) -> io::Result<()> {
        f.write_all(&self.segment_name.0)?;
        f.write_all(&self.vmaddr.to_le_bytes())?;
        f.write_all(&self.vmsize.to_le_bytes())?;
        let fileoff: u64 = *file_offset;
        f.write_all(&fileoff.to_le_bytes())?;
        let content_size: u64 = self
            .sections
            .iter()
            .map(|section| section.contents.len())
            .sum::<usize>()
            .try_into()
            .expect("mach-o sections too large");
        let filesize = content_size;
        f.write_all(&filesize.to_le_bytes())?;
        f.write_all(&self.max_vmem_prot.0.to_le_bytes())?;
        f.write_all(&self.init_vmem_prot.0.to_le_bytes())?;
        f.write_all(&(self.sections.len() as u32).to_le_bytes())?;
        f.write_all(&self.flags.0.to_le_bytes())?;
        for section in &self.sections {
            section.write(f, self.segment_name, file_offset)?;
        }
        Ok(())
    }
}

struct PermissionFlags(u32);
#[allow(unused)]
impl PermissionFlags {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const EXEC: Self = Self(1 << 2);
}
impl BitOr for PermissionFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SegmentFlags(u32);

struct SegmentSection64 {
    section_name: Name,
    section_addr: u64,
    alignment: u32,
    flag: u32,
    contents: Vec<u8>,
    relocations: Vec<relocation::RelocationInfo>,
}
impl SegmentSection64 {
    const SIZE: u32 = 16 + 16 + 8 + 8 + 4 + 4 + 4 + 4 + 4 + 12;

    fn write<W: Write>(
        &self,
        f: &mut W,
        segment_name: Name,
        file_offset: &mut u64,
    ) -> io::Result<()> {
        f.write_all(&self.section_name.0)?;
        f.write_all(&segment_name.0)?;
        f.write_all(&self.section_addr.to_le_bytes())?;
        let size = self.contents.len() as u64;
        f.write_all(&size.to_le_bytes())?;
        let offset = u32::try_from(*file_offset).expect("mach-o file section offset too large");
        *file_offset += self.contents.len() as u64;
        f.write_all(&offset.to_le_bytes())?;
        f.write_all(&self.alignment.to_le_bytes())?;
        let relocations_file_offset =
            u32::try_from(*file_offset).expect("mach-o file section offset too large");
        *file_offset += self.relocations.len() as u64 * relocation::RelocationInfo::SIZE;
        f.write_all(&relocations_file_offset.to_le_bytes())?;
        let num_relocations = u32::try_from(self.relocations.len()).expect("too many relocations");
        f.write_all(&num_relocations.to_le_bytes())?;
        f.write_all(&self.flag.to_le_bytes())?;
        f.write_all(&[0u8; 12])?; // reserved
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Name([u8; 16]);
impl Name {
    const TEXT_SEGMENT: Self = Self(*b"__TEXT\0\0\0\0\0\0\0\0\0\0");
    const TEXT_SECTION: Self = Self(*b"__text\0\0\0\0\0\0\0\0\0\0");
    const CONST_SECTION: Self = Self(*b"__const\0\0\0\0\0\0\0\0\0");
    const DATA_SEGMENT: Self = Self(*b"__DATA\0\0\0\0\0\0\0\0\0\0");
    const DATA_SECTION: Self = Self(*b"__data\0\0\0\0\0\0\0\0\0\0");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_size() {
        let segment = SegmentSection64 {
            section_name: Name::TEXT_SECTION,
            section_addr: 0xAA,
            alignment: 8,
            relocations: Vec::new(),
            flag: 0,
            contents: vec![0xAB, 0xCD, 0xEF, 0x12],
        };
        let mut buf = Vec::new();
        segment.write(&mut buf, Name::TEXT_SEGMENT, &mut 0).unwrap();
        assert_eq!(buf.len(), SegmentSection64::SIZE as usize);
    }
}
