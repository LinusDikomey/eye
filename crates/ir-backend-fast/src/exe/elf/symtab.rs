use crate::exe::elf::SectionIdx;

use super::SectionHeader;

pub struct SymtabWriter {
    section: Vec<u8>,
    entry_count: u32,
    first_non_local: Option<u32>,
}
impl SymtabWriter {
    pub fn new() -> Self {
        let mut writer = Self {
            section: Vec::new(),
            entry_count: 0,
            first_non_local: None,
        };
        writer.entry(Entry {
            name_index: 0,
            bind: Bind::Local,
            ty: Type::None,
            visibility: Visibility::Default,
            section_index: SectionIdx::NONE,
            value: 0,
            size: 0,
        });
        writer
    }

    pub fn entry(&mut self, entry: Entry) -> SymtabIdx {
        let idx = self.entry_count;
        if matches!(entry.bind, Bind::Local) {
            assert!(
                self.first_non_local.is_none(),
                "ELF local symbols must precede non-local symbols"
            );
        } else if self.first_non_local.is_none() {
            self.first_non_local = Some(idx);
        }
        self.entry_count = self
            .entry_count
            .checked_add(1)
            .expect("too many entries in symtab");
        self.section.extend(entry.name_index.to_le_bytes());
        let info = ((entry.bind as u8) << 4) | (entry.ty as u8);
        self.section.extend([info, entry.visibility as u8]);
        self.section.extend(entry.section_index.0.to_le_bytes());
        self.section.extend(entry.value.to_le_bytes());
        self.section.extend(entry.size.to_le_bytes());
        SymtabIdx(idx)
    }

    pub fn finish(self, strtab: SectionIdx) -> (SectionHeader, Vec<u8>) {
        let first_non_local = self.first_non_local.unwrap_or(self.entry_count);
        (
            SectionHeader {
                name: ".symtab".to_owned(),
                ty: super::SectionHeaderType::SymTab,
                flags: super::SectionHeaderFlags::default(),
                addr: 0,
                link: strtab,
                info: first_non_local,
                addralign: 8,
                entsize: 24,
            },
            self.section,
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SymtabIdx(pub u32);

pub struct Entry {
    pub name_index: u32,
    pub bind: Bind,
    pub ty: Type,
    pub visibility: Visibility,
    pub section_index: SectionIdx,
    pub value: u64,
    pub size: u64,
}

#[repr(u8)]
#[allow(unused)]
pub enum Bind {
    Local = 0x00,
    Global = 0x01,
    Weak = 0x02,
    Num = 0x03,
}

#[repr(u8)]
#[allow(unused)]
pub enum Type {
    None = 0x00,
    Object = 0x01,
    Function = 0x02,
    Section = 0x03,
    File = 0x04,
    Common = 0x05,
    Tls = 0x06,
    NumTypes = 0x07,
}

#[repr(u8)]
#[allow(unused)]
pub enum Visibility {
    Default = 0x00,
    Internal = 0x01,
    Hiddden = 0x02,
    Protected = 0x03,
}
