use crate::exe::elf::{SectionHeader, SectionIdx, symtab::SymtabIdx};

pub struct RelaWriter {
    section: Vec<u8>,
    entry_count: u32,
}
impl RelaWriter {
    pub fn new() -> Self {
        Self {
            section: Vec::new(),
            entry_count: 0,
        }
    }

    pub fn entry(&mut self, entry: Rela) {
        self.section.extend(entry.r_offset.to_le_bytes());
        let info = ((entry.sym.0 as u64) << 32) | entry.ty as u64;
        self.section.extend(info.to_le_bytes());
        self.section.extend(entry.r_addend.to_le_bytes());
        self.entry_count += 1;
    }

    pub fn finish(self, text: SectionIdx, symtab: SectionIdx) -> (SectionHeader, Vec<u8>) {
        (
            SectionHeader {
                name: ".rela.text".to_owned(),
                ty: super::SectionHeaderType::RelA,
                flags: super::SectionHeaderFlags::default(),
                addr: 0,
                link: symtab,
                info: text.0.into(),
                addralign: 8,
                entsize: 24,
            },
            self.section,
        )
    }
}

#[derive(Copy, Clone)]
#[repr(u32)]
pub enum RelaType {
    X86_64PC32 = 2,
    X86_64Plt32 = 4,
}

pub struct Rela {
    pub r_offset: u64,
    pub sym: SymtabIdx,
    pub ty: RelaType,
    pub r_addend: i64,
}
