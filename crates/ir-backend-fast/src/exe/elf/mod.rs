pub mod relocation;
pub mod symtab;

use std::{
    fs::File,
    io::{self, BufWriter, Write},
    ops,
    path::Path,
};

use crate::{Error, exe::elf::symtab::SymtabIdx};

pub fn emit(
    env: &mut ir::Environment,
    module_id: ir::ModuleId,
    out_file: &Path,
    codegen: crate::CodegenFn,
) -> Result<(), Error> {
    let mut writer = ElfObjectWriter::new();
    let mut symtab = symtab::SymtabWriter::new();

    let text = writer.section(
        SectionHeader {
            name: ".text".to_owned(),
            ty: SectionHeaderType::Progbits,
            flags: SectionHeaderFlags {
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
        SectionHeader {
            name: ".rodata".to_owned(),
            ty: SectionHeaderType::Progbits,
            flags: SectionHeaderFlags {
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
        SectionHeader {
            name: ".data".to_owned(),
            ty: SectionHeaderType::Progbits,
            flags: SectionHeaderFlags {
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
    symtab.entry(symtab::Entry {
        name_index: file_name,
        bind: symtab::Bind::Local,
        ty: symtab::Type::File,
        visibility: symtab::Visibility::Default,
        section_index: SectionIdx::ABSENT,
        value: 0,
        size: 0,
    });
    // section entry
    symtab.entry(symtab::Entry {
        name_index: 0,
        bind: symtab::Bind::Local,
        ty: symtab::Type::Section,
        visibility: symtab::Visibility::Default,
        section_index: text,
        value: 0,
        size: 0,
    });

    let mut relocations = Vec::new();
    let mut global_relocations = Vec::new();

    let mut function_offsets = vec![0u64; env[module_id].function_ids().len()];

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
                let types = func.types().clone();
                let name = func.name.clone();
                codegen(
                    env,
                    ir,
                    types,
                    &name,
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
            symtab.entry(symtab::Entry {
                name_index,
                bind: symtab::Bind::Global,
                ty: symtab::Type::Function,
                visibility: symtab::Visibility::Default,
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
            symtab.entry(symtab::Entry {
                name_index,
                bind: symtab::Bind::Global,
                ty: symtab::Type::Object,
                visibility: symtab::Visibility::Default,
                section_index: section,
                value,
                size: global.value.len() as u64,
            })
        })
        .collect();

    // emit relocations to elf

    let mut rela = relocation::RelaWriter::new();
    for (function_id, i) in relocations {
        debug_assert_eq!(function_id.module, module_id);
        let is_extern = env[module_id][function_id.function].ir().is_none();
        if is_extern {
            rela.entry(relocation::Rela {
                r_offset: i,
                sym: symtab_entries[function_id.function.idx()],
                ty: relocation::RelaType::X86_64Plt32,
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
        rela.entry(relocation::Rela {
            r_offset: offset,
            sym: global_symtab_entries[global_id.idx as usize],
            ty: relocation::RelaType::X86_64PC32,
            r_addend: -4,
        });
    }

    let (symtab_header, symtab_contents) = symtab.finish(writer.strtab_idx());

    let symtab_idx = writer.section(symtab_header, symtab_contents);
    let (rela_header, rela_contents) = rela.finish(text, symtab_idx);
    writer.section(rela_header, rela_contents);

    writer.write(out_file).map_err(Error::IO)
}

#[repr(u8)]
#[derive(Clone, Copy)]
#[allow(unused)]
pub enum Format {
    B32 = 1,
    B64 = 2,
}

#[repr(u8)]
#[allow(unused)]
pub enum Endianness {
    Little = 1,
    Big = 2,
}

#[repr(u8)]
pub enum Abi {
    SystemV = 0x00,
}

#[derive(Clone, Copy)]
#[allow(unused)]
pub enum ObjectFileType {
    Unknown,
    Relocatable,
    Executable,
    Shared,
    Core,
    OSSpecific(u8),
    ProcessorSpecific(u8),
}
impl ObjectFileType {
    fn to_bytes(self) -> [u8; 2] {
        match self {
            Self::Unknown => 0x00u16.to_le_bytes(),
            Self::Relocatable => 0x01u16.to_le_bytes(),
            Self::Executable => 0x02u16.to_le_bytes(),
            Self::Shared => 0x03u16.to_le_bytes(),
            Self::Core => 0x04u16.to_le_bytes(),
            Self::OSSpecific(b) => [0xFE, b],
            Self::ProcessorSpecific(b) => [0xFF, b],
        }
    }
}

#[derive(Debug)]
pub struct ElfObjectWriter {
    strtab: String,
    sections: Vec<(SectionHeader, Vec<u8>)>,
}
impl ElfObjectWriter {
    pub fn new() -> Self {
        Self {
            strtab: "\0".to_owned(),
            sections: vec![
                // null section
                (
                    SectionHeader {
                        name: String::new(),
                        ty: SectionHeaderType::Null,
                        flags: SectionHeaderFlags::default(),
                        addr: 0,
                        link: SectionIdx::NONE,
                        info: 0,
                        addralign: 0,
                        entsize: 0,
                    },
                    Vec::new(),
                ),
                // strtab section
                (
                    SectionHeader {
                        name: ".strtab".to_owned(),
                        ty: SectionHeaderType::StrTab,
                        flags: SectionHeaderFlags::default(),
                        addr: 0,
                        link: SectionIdx::NONE,
                        info: 0,
                        addralign: 1,
                        entsize: 0,
                    },
                    Vec::new(),
                ),
            ],
        }
    }

    pub fn strtab_idx(&self) -> SectionIdx {
        SectionIdx(1) // always stored at idx 1
    }

    pub fn write(mut self, path: &Path) -> io::Result<()> {
        let mut file = BufWriter::new(File::create(path)?);
        let format = Format::B64;
        let endianness = Endianness::Little;
        let abi = Abi::SystemV;
        let [file_a, file_b] = ObjectFileType::Relocatable.to_bytes();
        let [isa_a, isa_b] = 0x3eu16.to_le_bytes(); // x86-64
        #[rustfmt::skip]
        file.write_all(&[
            0x7f, b'E', b'L', b'F',
            format as u8,
            endianness as u8,
            1,                      // current ELF version
            abi as u8,
            0,                      // ABI version
            0, 0, 0, 0, 0, 0, 0,    // padding bytes
            file_a, file_b,         // object file type
            isa_a, isa_b,           // isa
            1, 0, 0, 0,             // version of ELF
        ])?;
        // entry point, program header table offset, section header table offset
        let (mut offset, section_header_len): (u64, u16) = match format {
            Format::B32 => {
                file.write_all(&[0; 4])?;
                file.write_all(&[0; 4])?;
                let offset: u32 = 0x34;
                file.write_all(&offset.to_le_bytes())?;
                (offset as u64, 0x28)
            }
            Format::B64 => {
                file.write_all(&[0; 8])?;
                file.write_all(&[0; 8])?;
                let offset: u64 = 0x40;
                file.write_all(&offset.to_le_bytes())?;
                (offset, 0x40)
            }
        };
        let ehsize: u16 = 64;
        let section_header_count = self.sections.len() as u16;
        let strtab_index = self.strtab_idx();

        file.write_all(&[0, 0, 0, 0])?; // e_flags: target-specific flags
        file.write_all(&ehsize.to_le_bytes())?; // e_ehsize
        file.write_all(&0x38u16.to_le_bytes())?; // // e_phentsize
        file.write_all(&0u16.to_le_bytes())?; // e_phnum: program header table entry count
        file.write_all(&section_header_len.to_le_bytes())?; // e_shentsize
        file.write_all(&section_header_count.to_le_bytes())?; // e_shnum
        file.write_all(&strtab_index.0.to_le_bytes())?; // e_shstrndx

        let section_header_names: Vec<u32> = self
            .sections
            .iter()
            .map(|(header, _)| {
                if header.name.is_empty() {
                    // null section doesn't have a name
                    0 // there is a nul byte at the start of strtab for this
                } else {
                    let index = self.strtab.len().try_into().expect("strtab is too long");
                    self.strtab.push_str(&header.name);
                    self.strtab.push('\0');
                    index
                }
            })
            .collect();

        // Write section headers. Track final offset to give the section bodies the correct offset
        offset += section_header_count as u64 * section_header_len as u64;

        // put in the final strtab
        self.sections[1].1 = self.strtab.into_bytes();

        for ((header, content), name_index) in self.sections.iter().zip(section_header_names) {
            let len = content.len() as u64;
            let section_offset = if content.is_empty() { 0 } else { offset };
            header.write(&mut file, name_index, section_offset, len)?;
            offset += len;
        }

        for (_, content) in &self.sections {
            file.write_all(content)?;
        }

        Ok(())
    }

    pub fn section(&mut self, header: SectionHeader, contents: Vec<u8>) -> SectionIdx {
        let idx = self.sections.len() as u16;
        self.sections.push((header, contents));
        // null section and strtab section come before any other sections
        SectionIdx(idx)
    }

    pub fn add_str(&mut self, s: &str) -> u32 {
        let index = self
            .strtab
            .len()
            .try_into()
            .expect("strtab section is too long");
        self.strtab.push_str(s);
        self.strtab.push('\0');
        index
    }
}
impl ops::Index<SectionIdx> for ElfObjectWriter {
    type Output = Vec<u8>;

    fn index(&self, index: SectionIdx) -> &Self::Output {
        &self.sections[index.0 as usize].1
    }
}
impl ops::IndexMut<SectionIdx> for ElfObjectWriter {
    fn index_mut(&mut self, index: SectionIdx) -> &mut Self::Output {
        &mut self.sections[index.0 as usize].1
    }
}

#[derive(Debug)]
pub struct SectionHeader {
    pub name: String,
    pub ty: SectionHeaderType,
    pub flags: SectionHeaderFlags,
    pub addr: u64,
    pub link: SectionIdx,
    pub info: u32,
    pub addralign: u64,
    pub entsize: u64,
}
impl SectionHeader {
    /// currently assumes 64-bit elf
    fn write(
        &self,
        file: &mut BufWriter<File>,
        name_index: u32,
        offset: u64,
        size: u64,
    ) -> io::Result<()> {
        self.write_with_name_index(file, name_index, offset, size)
    }

    fn write_with_name_index(
        &self,
        file: &mut BufWriter<File>,
        name_index: u32,
        offset: u64,
        size: u64,
    ) -> io::Result<()> {
        file.write_all(&name_index.to_le_bytes())?;

        file.write_all(&(self.ty as u32).to_le_bytes())?;
        file.write_all(&self.flags.to_bytes64())?;
        file.write_all(&self.addr.to_le_bytes())?;
        file.write_all(&offset.to_le_bytes())?;
        file.write_all(&size.to_le_bytes())?;
        file.write_all(&(self.link.0 as u32).to_le_bytes())?;
        file.write_all(&self.info.to_le_bytes())?;
        file.write_all(&self.addralign.to_le_bytes())?;
        file.write_all(&self.entsize.to_le_bytes())?;

        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SectionIdx(pub u16);
impl SectionIdx {
    pub const NONE: Self = Self(0);
    pub const ABSENT: Self = Self(0xfff1);
}

#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum SectionHeaderType {
    Null = 0,
    Progbits = 1,
    SymTab = 2,
    StrTab = 3,
    RelA = 4,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct SectionHeaderFlags {
    pub write: bool,
    pub alloc: bool,
    pub execinstr: bool,
    pub merge: bool,
    pub strings: bool,
}
impl SectionHeaderFlags {
    fn _to_bytes32(self) -> [u8; 4] {
        let mut bits: u32 = 0;
        if self.write {
            bits |= 1 << 0;
        }
        if self.alloc {
            bits |= 1 << 1;
        }
        if self.execinstr {
            bits |= 1 << 2;
        }
        if self.merge {
            bits |= 1 << 3;
        }
        if self.strings {
            bits |= 1 << 4;
        }
        bits.to_le_bytes()
    }

    fn to_bytes64(self) -> [u8; 8] {
        let mut bits: u64 = 0;
        if self.write {
            bits |= 1 << 0;
        }
        if self.alloc {
            bits |= 1 << 1;
        }
        if self.execinstr {
            bits |= 1 << 2;
        }
        if self.merge {
            bits |= 1 << 3;
        }
        if self.strings {
            bits |= 1 << 4;
        }
        bits.to_le_bytes()
    }
}
