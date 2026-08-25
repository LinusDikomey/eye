use std::ops;

use ir::{
    Usage,
    mc::{McInst, Register},
};

ir::mc::registers! { RegBits
    GP64 => x0 x1 x2 x3 x4 x5 x6 x7 x8 x9 x10 x11 x12 x13 x14 x15 x16 x17 x18 x19 x20 x21 x22 x23 x24 x25 x26 x27 x28 x29 x30  sp;
    GP32 => w0 w1 w2 w3 w4 w5 w6 w7 w8 w9 w10 w11 w12 w13 w14 w15 w16 w17 w18 w19 w20 w21 w22 w23 w24 w25 w26 w27 w28 w29 w30 wsp;
    FP32 => s0 s1 s2 s3 s4 s5 s6 s7 s8 s9 s10 s11 s12 s13 s14 s15 s16 s17 s18 s19 s20 s21 s22 s23 s24 s25 s26 s27 s28 s29 s30 s31;
    FP64 => d0 d1 d2 d3 d4 d5 d6 d7 d8 d9 d10 d11 d12 d13 d14 d15 d16 d17 d18 d19 d20 d21 d22 d23 d24 d25 d26 d27 d28 d29 d30 d31;
    ZR   => xzr wzr;
    !secondary:
}
impl Reg {
    pub const fn index(self) -> u8 {
        use Reg::*;
        match self {
            x0 | w0 => 0,
            x1 | w1 => 1,
            x2 | w2 => 2,
            x3 | w3 => 3,
            x4 | w4 => 4,
            x5 | w5 => 5,
            x6 | w6 => 6,
            x7 | w7 => 7,
            x8 | w8 => 8,
            x9 | w9 => 9,
            x10 | w10 => 10,
            x11 | w11 => 11,
            x12 | w12 => 12,
            x13 | w13 => 13,
            x14 | w14 => 14,
            x15 | w15 => 15,
            x16 | w16 => 16,
            x17 | w17 => 17,
            x18 | w18 => 18,
            x19 | w19 => 19,
            x20 | w20 => 20,
            x21 | w21 => 21,
            x22 | w22 => 22,
            x23 | w23 => 23,
            x24 | w24 => 24,
            x25 | w25 => 25,
            x26 | w26 => 26,
            x27 | w27 => 27,
            x28 | w28 => 28,
            x29 | w29 => 29,
            x30 | w30 => 30,
            sp | wsp | xzr | wzr => 31,
            s0 | d0 => 32,
            s1 | d1 => 32 + 1,
            s2 | d2 => 32 + 2,
            s3 | d3 => 32 + 3,
            s4 | d4 => 32 + 4,
            s5 | d5 => 32 + 5,
            s6 | d6 => 32 + 6,
            s7 | d7 => 32 + 7,
            s8 | d8 => 32 + 8,
            s9 | d9 => 32 + 9,
            s10 | d10 => 32 + 10,
            s11 | d11 => 32 + 11,
            s12 | d12 => 32 + 12,
            s13 | d13 => 32 + 13,
            s14 | d14 => 32 + 14,
            s15 | d15 => 32 + 15,
            s16 | d16 => 32 + 16,
            s17 | d17 => 32 + 17,
            s18 | d18 => 32 + 18,
            s19 | d19 => 32 + 19,
            s20 | d20 => 32 + 20,
            s21 | d21 => 32 + 21,
            s22 | d22 => 32 + 22,
            s23 | d23 => 32 + 23,
            s24 | d24 => 32 + 24,
            s25 | d25 => 32 + 25,
            s26 | d26 => 32 + 26,
            s27 | d27 => 32 + 27,
            s28 | d28 => 32 + 28,
            s29 | d29 => 32 + 29,
            s30 | d30 => 32 + 30,
            s31 | d31 => 32 + 31,
        }
    }

    pub const fn bit(self) -> RegBits {
        RegBits(1 << self.index() as u64)
    }
}

pub const TMP_REGISTER: Reg = Reg::x16; // IP0
pub const PREOCCUPIED: RegBits = RegBits(Reg::sp.bit().0 | Reg::x29.bit().0 | TMP_REGISTER.bit().0);

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RegBits(u64);
impl ops::Not for RegBits {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}
impl ops::BitAnd for RegBits {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}
impl ops::BitOr for RegBits {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}
impl RegBits {
    pub const fn new() -> Self {
        Self(0)
    }

    const fn all() -> Self {
        Self(u64::MAX)
    }

    fn get(&self, r: Reg) -> bool {
        self.0 & r.bit().0 != 0
    }

    fn set(&mut self, r: Reg, set: bool) {
        let bit = r.bit();
        if set {
            self.0 |= bit.0;
        } else {
            self.0 &= !bit.0;
        }
    }
}

ir::instructions! {
    Arm "arm" ArmInsts

    !all_return unit

    orr32 x: MCReg(Usage::Def) a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    orr64 x: MCReg(Usage::Def) a: MCReg(Usage::Use) b: MCReg(Usage::Use);

    // shift = hw * 16
    movz32 dst: MCReg(Usage::Def) hw: Int32 imm16: Int32;
    movz64 dst: MCReg(Usage::Def) hw: Int32 imm16: Int32;
    movk32 dst: MCReg(Usage::Def) hw: Int32 imm16: Int32;
    movk64 dst: MCReg(Usage::Def) hw: Int32 imm16: Int32;

    ldr8  dst: MCReg(Usage::Def) ptr: MCReg(Usage::Use) offset: Int32;
    ldr16 dst: MCReg(Usage::Def) ptr: MCReg(Usage::Use) offset: Int32;
    ldr32 dst: MCReg(Usage::Def) ptr: MCReg(Usage::Use) offset: Int32;
    ldr64 dst: MCReg(Usage::Def) ptr: MCReg(Usage::Use) offset: Int32;

    ldp32 dst1: MCReg(Usage::Def) dst2: MCReg(Usage::Def) ptr: MCReg(Usage::Use) imm7: Int32;
    ldp64 dst1: MCReg(Usage::Def) dst2: MCReg(Usage::Def) ptr: MCReg(Usage::Use) imm7: Int32;

    ret0   target: MCReg(Usage::Use);
    ret64  target: MCReg(Usage::Use);
    ret128 target: MCReg(Usage::Use);
}

impl McInst for Arm {
    type Reg = Reg;

    fn implicit_def(
        &self,
        _abi: &'static dyn ir::mc::Abi<Self>,
    ) -> <Self::Reg as ir::mc::Register>::RegisterBits {
        Reg::NO_BITS
    }

    fn implicit_use(
        &self,
        _abi: &'static dyn ir::mc::Abi<Self>,
    ) -> <Self::Reg as ir::mc::Register>::RegisterBits {
        Reg::NO_BITS
    }
}
