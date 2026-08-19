use crate::exe::macho::symtab::SymbolNum;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RelocationInfo {
    address: u32,
    info: u32,
}
impl RelocationInfo {
    pub const SIZE: u64 = 8;
    pub fn new(
        address: u32,
        symbolnum: SymbolNum,
        pcrel: bool,
        length: RelocationLength,
        extern_: bool,
        ty: impl RelocationType,
    ) -> Self {
        debug_assert!(symbolnum.0 < (1 << 24));
        let ty = ty.into_bits();
        debug_assert!(ty < (1 << 4));
        Self {
            address,
            info: symbolnum.0
                | (pcrel as u32) << 24
                | (length as u32) << 25
                | (extern_ as u32) << 27
                | (ty as u32) << 28,
        }
    }

    pub fn bytes(self) -> [u8; 8] {
        let [a, b, c, d] = self.address.to_le_bytes();
        let [e, f, g, h] = self.info.to_le_bytes();
        [a, b, c, d, e, f, g, h]
    }
}

#[repr(u8)]
#[allow(unused)]
pub enum RelocationLength {
    L1 = 0b00,
    L2 = 0b01,
    L4 = 0b10,
    L8 = 0b11,
}

pub trait RelocationType {
    /// convert to 4-bit integer
    fn into_bits(self) -> u8;
}

#[repr(u8)]
pub enum RelocationTypeX86_64 {
    Signed = 1,
    Branch = 2,
    /// signed with -4 addend
    Signed4 = 8,
}
impl RelocationType for RelocationTypeX86_64 {
    fn into_bits(self) -> u8 {
        self as u8
    }
}
