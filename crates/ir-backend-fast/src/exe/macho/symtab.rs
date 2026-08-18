use std::{
    fs::File,
    io::{self, BufWriter, Write},
};

use crate::exe::macho::SectionIdx;

pub struct SymTab {
    symbols: Vec<nlist_64>,
    s: String,
}
impl SymTab {
    pub fn new() -> Self {
        Self {
            symbols: Vec::new(),
            s: "\0".to_owned(),
        }
    }

    pub const COMMAND_SIZE: u32 = 16;

    pub fn sym(&mut self, symbol: nlist_64) {
        self.symbols.push(symbol);
    }

    pub fn str(&mut self, s: &str, underscore_prefix: bool) -> StrIdx {
        let i = StrIdx(self.s.len().try_into().expect("strtab too long"));
        if underscore_prefix {
            self.s.push('_');
        }
        self.s += s;
        self.s.push('\0');
        i
    }

    pub fn write(&self, f: &mut BufWriter<File>, file_offset: &mut u64) -> io::Result<()> {
        let symoff = u32::try_from(*file_offset).expect("mach-o file too large");
        f.write_all(&symoff.to_le_bytes())?;
        let nsyms = u32::try_from(self.symbols.len()).expect("too many symbols");
        *file_offset += self.symbols.len() as u64 * nlist_64::SIZE;
        f.write_all(&nsyms.to_le_bytes())?;
        let stroff = u32::try_from(*file_offset).expect("mach-o file too large");
        f.write_all(&stroff.to_le_bytes())?;
        let strsize = u32::try_from(self.s.len()).expect("strtab too long");
        f.write_all(&strsize.to_le_bytes())?;
        *file_offset += u64::from(strsize);
        Ok(())
    }

    pub fn write_content(&self, f: &mut BufWriter<File>) -> io::Result<()> {
        for sym in &self.symbols {
            sym.write(f)?;
        }
        f.write_all(self.s.as_bytes())
    }
}

pub struct StrIdx(u32);

#[allow(non_camel_case_types)]
pub struct nlist_64 {
    pub n_strx: StrIdx,
    pub addr_type: SymbolAddrType,
    pub vis: SymbolVisibility,
    pub n_sect: SectionIdx,
    pub n_desc: u16,
    pub n_value: u64,
}
impl nlist_64 {
    pub const SIZE: u64 = 16;
    pub fn write(&self, f: &mut BufWriter<File>) -> io::Result<()> {
        f.write_all(&self.n_strx.0.to_le_bytes())?;
        let n_type = self.addr_type as u8 | self.vis as u8;
        f.write_all(&[n_type, self.n_sect.0])?;
        f.write_all(&self.n_desc.to_le_bytes())?;
        f.write_all(&self.n_value.to_le_bytes())
    }
}

#[rustfmt::skip]
#[derive(Clone, Copy)]
#[allow(unused)]
pub enum SymbolAddrType {
    Undefined     = 0b0000,
    Absolute      = 0b0010,
    Indirect      = 0b1010,
    PreboundUndef = 0b1100,
    SecNumDefined = 0b1110,
}

#[rustfmt::skip]
#[derive(Clone, Copy)]
#[allow(unused)]
pub enum SymbolVisibility {
    Private  = 0b0001_0000,
    External = 0b0000_0001,
}
