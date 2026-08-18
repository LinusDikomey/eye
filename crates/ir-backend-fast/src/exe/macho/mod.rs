mod symtab;

use std::{
    fs::File,
    io::{self, BufWriter, Write},
    ops::BitOr,
    path::Path,
};

use crate::{exe::macho::symtab::SymTab, Error};

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

    let mut text = Vec::new();
    let mut relocations = Vec::new();
    let mut global_relocations = Vec::new();

    for id in env[module_id].function_ids() {
        let func = &env[module_id][id];
        if let Some(ir) = func.ir() {
            let offset = text.len() as u64;
            // *function_offset = offset;
            // PERF: cloning ir, types, name
            let ir = ir.clone();
            let types = func.types().clone();
            let name = func.name.clone();
            codegen(
                env,
                ir,
                types,
                &name,
                &mut text,
                &mut relocations,
                &mut global_relocations,
            );
            let n_strx = w.symtab.str(&name, true);
            w.symtab.sym(symtab::nlist_64 {
                n_strx,
                addr_type: symtab::SymbolAddrType::SecNumDefined,
                vis: symtab::SymbolVisibility::External,
                n_sect: SectionIdx(1),
                n_desc: 0,
                n_value: offset,
            });
            // let size = text.len() as u64 - offset;
        }
    }

    w.load_command(LoadCommand {
        necessary_for_loading: false,
        content: LoadCommandContent::Segment(SegmentLoad64 {
            segment_name: Name::TEXT_SEGMENT,
            vmaddr: 0,
            vmsize: text.len() as u64,
            max_vmem_prot: PermissionFlags::READ | PermissionFlags::EXEC,
            init_vmem_prot: PermissionFlags::READ | PermissionFlags::EXEC,
            flags: SegmentFlags::default(),
            sections: vec![SegmentSection64 {
                section_name: Name::TEXT_SECTION,
                section_addr: 0,
                alignment: 0,
                relocations_file_offset: 0,
                num_relocations: 0,
                flag: 0,
                contents: text,
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
    relocations_file_offset: u32,
    num_relocations: u32,
    flag: u32,
    contents: Vec<u8>,
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
        f.write_all(&offset.to_le_bytes())?;
        f.write_all(&self.alignment.to_le_bytes())?;
        f.write_all(&self.relocations_file_offset.to_le_bytes())?;
        f.write_all(&self.num_relocations.to_le_bytes())?;
        f.write_all(&self.flag.to_le_bytes())?;
        f.write_all(&[0u8; 12])?; // reserved
        *file_offset += self.contents.len() as u64;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Name([u8; 16]);
impl Name {
    const TEXT_SEGMENT: Self = Self(*b"__TEXT\0\0\0\0\0\0\0\0\0\0");
    const TEXT_SECTION: Self = Self(*b"__text\0\0\0\0\0\0\0\0\0\0");
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
            relocations_file_offset: 0,
            num_relocations: 0,
            flag: 0,
            contents: vec![0xAB, 0xCD, 0xEF, 0x12],
        };
        let mut buf = Vec::new();
        segment.write(&mut buf, Name::TEXT_SEGMENT, &mut 0).unwrap();
        assert_eq!(buf.len(), SegmentSection64::SIZE as usize);
    }
}
