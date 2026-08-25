use std::ops;

use ir::{
    Usage,
    mc::{Abi, McInst, Register},
};

use crate::Size;

ir::mc::registers! { RegBits
    GP64 => rax rbx rcx rdx rbp rsi rdi rsp rip r8  r9  r10  r11  r12  r13  r14  r15 none;
    GP32 => eax ebx ecx edx ebp esi edi esp     r8d r9d r10d r11d r12d r13d r14d r15d;
    GP16 => ax  bx  cx  dx  bp  si  di  sp      r8w r9w r10w r11w r12w r13w r14w r15w;
    GP8  => al  bl  cl  dl  bpl sil dil spl     r8b r9b r10b r11b r12b r13b r14b r15b
            ah  bh  ch  dh;
    Flags => eflags;
    F32  =>;
    F64  =>;
    !secondary:

    // these are variants of the generic registers but usable in index position
    // (they exclude rsp and r12 since they aren't allowed in SiB index encodings)
    GP64I => rax rbx rcx rdx rbp rsi rdi rip r8  r9  r10  r11  r13  r14  r15 none;
    GP32I => eax ebx ecx edx ebp esi edi     r8d r9d r10d r11d r13d r14d r15d;
    GP16I => ax  bx  cx  dx  bp  si  di      r8w r9w r10w r11w r13w r14w r15w;
    GP8I  => al  bl  cl  dl  bpl sil dil     r8b r9b r10b r11b r13b r14b r15b
             ah  bh  ch  dh;
}
impl RegClass {
    pub fn into_index(self) -> RegClass {
        match self {
            Self::GP64 | Self::GP64I => Self::GP64I,
            Self::GP32 | Self::GP32I => Self::GP32I,
            Self::GP16 | Self::GP16I => Self::GP16I,
            Self::GP8 | Self::GP8I => Self::GP8I,
            Self::Flags | Self::F32 | Self::F64 => {
                panic!("cannot convert reg class {self:?} into index class")
            }
        }
    }
}

pub const TMP_REGISTER: Reg = Reg::r15;
pub const PREOCCUPIED_REGISTERS: RegBits =
    RegBits(Reg::rbp.bit().0 | Reg::rsp.bit().0 | TMP_REGISTER.bit().0);

impl Reg {
    pub const fn index(self) -> u8 {
        use Reg::*;
        match self {
            rax | eax | ax | ah | al => 0,
            rbx | ebx | bx | bh | bl => 1,
            rcx | ecx | cx | ch | cl => 2,
            rdx | edx | dx | dh | dl => 3,
            rbp | ebp | bp | bpl => 4,
            rsi | esi | si | sil => 5,
            rdi | edi | di | dil => 6,
            rsp | esp | sp | spl => 7,
            r8 | r8d | r8w | r8b => 8,
            r9 | r9d | r9w | r9b => 9,
            r10 | r10d | r10w | r10b => 10,
            r11 | r11d | r11w | r11b => 11,
            r12 | r12d | r12w | r12b => 12,
            r13 | r13d | r13w | r13b => 13,
            r14 | r14d | r14w | r14b => 14,
            r15 | r15d | r15w | r15b => 15,
            eflags => 16,
            rip => 17,
            none => 31,
        }
    }

    pub const fn bit(self) -> RegBits {
        RegBits(1 << self.index() as u32)
    }

    pub const fn to_64_bits(self) -> Self {
        use Reg::*;
        match self {
            none => none,
            rax | eax | ax | al | ah => rax,
            rbx | ebx | bx | bl | bh => rbx,
            rcx | ecx | cx | cl | ch => rcx,
            rdx | edx | dx | dl | dh => rdx,
            rbp | ebp | bp | bpl => rbp,
            rsi | esi | si | sil => rsi,
            rdi | edi | di | dil => rdi,
            rsp | esp | sp | spl => rsp,
            rip => rip,
            r8 | r8d | r8w | r8b => r8,
            r9 | r9d | r9w | r9b => r9,
            r10 | r10d | r10w | r10b => r10,
            r11 | r11d | r11w | r11b => r11,
            r12 | r12d | r12w | r12b => r12,
            r13 | r13d | r13w | r13b => r13,
            r14 | r14d | r14w | r14b => r14,
            r15 | r15d | r15w | r15b => r15,
            eflags => eflags,
        }
    }

    pub const UNIQUE_BITS: [Self; 18] = [
        Self::rax,
        Self::rbx,
        Self::rcx,
        Self::rdx,
        Self::rbp,
        Self::rsi,
        Self::rdi,
        Self::rsp,
        Self::r8,
        Self::r9,
        Self::r10,
        Self::r11,
        Self::r12,
        Self::r13,
        Self::r14,
        Self::r15,
        Self::eflags,
        Self::rip,
    ];
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RegBits(u32);
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
        Self(0x0001FFFF)
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
    X86 "x86" X86Insts

    !all_return unit

    or_rr8  a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    or_rr16 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    or_rr32 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    or_rr64 a: MCReg(Usage::Use) b: MCReg(Usage::Use);

    or_ri8  a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    or_ri16 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    or_ri32 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    or_ri64 a: MCReg(Usage::Use) b: MCReg(Usage::Use);

    and_rr8  a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    and_rr16 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    and_rr32 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    and_rr64 a: MCReg(Usage::Use) b: MCReg(Usage::Use);

    and_ri8  a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    and_ri16 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    and_ri32 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    and_ri64 a: MCReg(Usage::Use) b: MCReg(Usage::Use);

    push_r64 reg: MCReg(Usage::Use);
    pop_r64 reg: MCReg(Usage::Def);

    mov_ri8 to: MCReg(Usage::Def) i: Int32;
    mov_ri16 to: MCReg(Usage::Def) i: Int32;
    mov_ri32 to: MCReg(Usage::Def) i: Int32;
    mov_ri64 to: MCReg(Usage::Def) i: Int32;

    mov_rr8  to: MCReg(Usage::Def) from: MCReg(Usage::Use);
    mov_rr16 to: MCReg(Usage::Def) from: MCReg(Usage::Use);
    mov_rr32 to: MCReg(Usage::Def) from: MCReg(Usage::Use);
    mov_rr64 to: MCReg(Usage::Def) from: MCReg(Usage::Use);

    mov_rm8  to: MCReg(Usage::Def) from: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32;
    mov_rm16 to: MCReg(Usage::Def) from: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32;
    mov_rm32 to: MCReg(Usage::Def) from: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32;
    mov_rm64 to: MCReg(Usage::Def) from: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32;

    mov_mr8  to: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32 from: MCReg(Usage::Use);
    mov_mr16 to: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32 from: MCReg(Usage::Use);
    mov_mr32 to: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32 from: MCReg(Usage::Use);
    mov_mr64 to: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32 from: MCReg(Usage::Use);

    ret0 !terminator;
    ret64 !terminator;
    ret128 !terminator;


    jmp addr: BlockId !terminator;
    je  addr: BlockId;
    jne addr: BlockId;
    jl  addr: BlockId;
    jge addr: BlockId;
    jle addr: BlockId;
    jg  addr: BlockId;

    /// overflow
    seto   r: MCReg(Usage::Def);
    /// not overflow
    setno  r: MCReg(Usage::Def);
    /// carry
    setc   r: MCReg(Usage::Def);
    /// not carry
    setnc  r: MCReg(Usage::Def);
    /// equal
    sete   r: MCReg(Usage::Def);
    /// not equal
    setne  r: MCReg(Usage::Def);
    /// below or equal
    setbe  r: MCReg(Usage::Def);
    /// above
    seta   r: MCReg(Usage::Def);
    /// signed
    sets   r: MCReg(Usage::Def);
    /// not signed
    setns  r: MCReg(Usage::Def);
    /// parity
    setp   r: MCReg(Usage::Def);
    /// not parity
    setnp  r: MCReg(Usage::Def);
    // less
    setl   r: MCReg(Usage::Def);
    /// greater or equal
    setge  r: MCReg(Usage::Def);
    /// less than or equal
    setle  r: MCReg(Usage::Def);
    /// greater
    setg   r: MCReg(Usage::Def);

    cmp_rr8  a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    cmp_rr16 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    cmp_rr32 a: MCReg(Usage::Use) b: MCReg(Usage::Use);
    cmp_rr64 a: MCReg(Usage::Use) b: MCReg(Usage::Use);

    test_rr8 a: MCReg(Usage::Use) b: MCReg(Usage::Use);

    add_rr8  a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    add_rr16 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    add_rr32 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    add_rr64 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);

    add_ri8  reg: MCReg(Usage::DefUse) imm: Int32;
    add_ri16 reg: MCReg(Usage::DefUse) imm: Int32;
    add_ri32 reg: MCReg(Usage::DefUse) imm: Int32;
    add_ri64 reg: MCReg(Usage::DefUse) imm: Int32;

    sub_rr8  a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    sub_rr16 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    sub_rr32 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    sub_rr64 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);

    sub_ri8  reg: MCReg(Usage::DefUse) imm: Int32;
    sub_ri16 reg: MCReg(Usage::DefUse) imm: Int32;
    sub_ri32 reg: MCReg(Usage::DefUse) imm: Int32;
    sub_ri64 reg: MCReg(Usage::DefUse) imm: Int32;

    /// ax = al * reg
    imul_r8   reg: MCReg(Usage::Use);
    imul_rr16 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    imul_rr32 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    imul_rr64 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);

    imul_rri16 dst: MCReg(Usage::Def) reg: MCReg(Usage::Use) imm: Int32;
    imul_rri32 dst: MCReg(Usage::Def) reg: MCReg(Usage::Use) imm: Int32;
    imul_rri64 dst: MCReg(Usage::Def) reg: MCReg(Usage::Use) imm: Int32;

    cbw;
    cwd;
    cdq;
    cqo;

    div_r8  reg: MCReg(Usage::Use);
    div_r16 reg: MCReg(Usage::Use);
    div_r32 reg: MCReg(Usage::Use);
    div_r64 reg: MCReg(Usage::Use);

    idiv_r8  reg: MCReg(Usage::Use);
    idiv_r16 reg: MCReg(Usage::Use);
    idiv_r32 reg: MCReg(Usage::Use);
    idiv_r64 reg: MCReg(Usage::Use);

    shl_ri8  reg: MCReg(Usage::DefUse) imm: Int32;
    shl_ri16 reg: MCReg(Usage::DefUse) imm: Int32;
    shl_ri32 reg: MCReg(Usage::DefUse) imm: Int32;
    shl_ri64 reg: MCReg(Usage::DefUse) imm: Int32;

    shr_ri8  reg: MCReg(Usage::DefUse) imm: Int32;
    shr_ri16 reg: MCReg(Usage::DefUse) imm: Int32;
    shr_ri32 reg: MCReg(Usage::DefUse) imm: Int32;
    shr_ri64 reg: MCReg(Usage::DefUse) imm: Int32;

    neg_r8 a: MCReg(Usage::DefUse);
    neg_r16 a: MCReg(Usage::DefUse);
    neg_r32 a: MCReg(Usage::DefUse);
    neg_r64 a: MCReg(Usage::DefUse);

    xor_rr8  a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    xor_rr16 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    xor_rr32 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);
    xor_rr64 a: MCReg(Usage::DefUse) b: MCReg(Usage::Use);

    xor_ri8  reg: MCReg(Usage::DefUse) imm: Int32;
    xor_ri16 reg: MCReg(Usage::DefUse) imm: Int32;
    xor_ri32 reg: MCReg(Usage::DefUse) imm: Int32;
    xor_ri64 reg: MCReg(Usage::DefUse) imm: Int32;

    lea_rm32 to: MCReg(Usage::Def) addr: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32;
    lea_rm64 to: MCReg(Usage::Def) addr: MCReg(Usage::Use) offset: Int32 index: MCReg(Usage::Use) scale: Int32;
    lea_function to: MCReg(Usage::Def) function: FunctionId;
    lea_global to: MCReg(Usage::Def) global: GlobalId;

    call_function function: FunctionId;
    call_r64 ptr: MCReg(Usage::Use);

    movsx16_rr8 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);
    movsx32_rr8 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);
    movsx64_rr8 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);
    movsx32_rr16 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);
    movsx64_rr16 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);
    movsx64_rr32 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);

    movzx16_rr8 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);
    movzx32_rr8 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);
    movzx32_rr16 dst: MCReg(Usage::Def) src: MCReg(Usage::Use);
}

impl X86 {
    pub const fn size(self) -> Size {
        match self {
            Self::or_rr8
            | Self::and_rr8
            | Self::or_ri8
            | Self::and_ri8
            | Self::mov_ri8
            | Self::mov_rm8
            | Self::mov_mr8
            | Self::cmp_rr8
            | Self::test_rr8
            | Self::add_rr8
            | Self::add_ri8
            | Self::sub_rr8
            | Self::sub_ri8
            | Self::imul_r8
            | Self::shl_ri8
            | Self::shr_ri8
            | Self::neg_r8
            | Self::xor_ri8
            | Self::mov_rr8
            | Self::xor_rr8
            | Self::movsx16_rr8
            | Self::movsx32_rr8
            | Self::movsx64_rr8
            | Self::movzx16_rr8
            | Self::movzx32_rr8
            | Self::cbw
            | Self::div_r8
            | Self::idiv_r8 => Size::S8,

            Self::or_rr16
            | Self::and_rr16
            | Self::or_ri16
            | Self::and_ri16
            | Self::mov_ri16
            | Self::mov_rm16
            | Self::mov_mr16
            | Self::cmp_rr16
            | Self::add_rr16
            | Self::add_ri16
            | Self::sub_rr16
            | Self::sub_ri16
            | Self::imul_rr16
            | Self::imul_rri16
            | Self::shl_ri16
            | Self::shr_ri16
            | Self::neg_r16
            | Self::xor_ri16
            | Self::mov_rr16
            | Self::xor_rr16
            | Self::movsx32_rr16
            | Self::movsx64_rr16
            | Self::movzx32_rr16
            | Self::cwd
            | Self::div_r16
            | Self::idiv_r16 => Size::S16,

            Self::or_rr32
            | Self::and_rr32
            | Self::or_ri32
            | Self::and_ri32
            | Self::mov_ri32
            | Self::mov_rr32
            | Self::mov_rm32
            | Self::mov_mr32
            | Self::cmp_rr32
            | Self::add_rr32
            | Self::add_ri32
            | Self::sub_rr32
            | Self::sub_ri32
            | Self::imul_rr32
            | Self::imul_rri32
            | Self::shl_ri32
            | Self::shr_ri32
            | Self::neg_r32
            | Self::xor_ri32
            | Self::lea_rm32
            | Self::xor_rr32
            | Self::movsx64_rr32
            | Self::cdq
            | Self::div_r32
            | Self::idiv_r32 => Size::S32,

            Self::or_rr64
            | Self::and_rr64
            | Self::or_ri64
            | Self::and_ri64
            | Self::push_r64
            | Self::pop_r64
            | Self::mov_ri64
            | Self::mov_rr64
            | Self::mov_rm64
            | Self::mov_mr64
            | Self::cmp_rr64
            | Self::add_rr64
            | Self::add_ri64
            | Self::sub_rr64
            | Self::sub_ri64
            | Self::imul_rr64
            | Self::imul_rri64
            | Self::shl_ri64
            | Self::shr_ri64
            | Self::neg_r64
            | Self::xor_ri64
            | Self::lea_rm64
            | Self::xor_rr64
            | Self::lea_function
            | Self::lea_global
            | Self::call_r64
            | Self::cqo
            | Self::div_r64
            | Self::idiv_r64 => Size::S64,

            Self::ret0
            | Self::ret64
            | Self::ret128
            | Self::jmp
            | Self::je
            | Self::jne
            | Self::jl
            | Self::jge
            | Self::jle
            | Self::jg
            | Self::seto
            | Self::setno
            | Self::setc
            | Self::setnc
            | Self::sete
            | Self::setne
            | Self::setbe
            | Self::seta
            | Self::sets
            | Self::setns
            | Self::setp
            | Self::setnp
            | Self::setl
            | Self::setge
            | Self::setle
            | Self::setg
            | Self::call_function => panic!("instruction doesn't have a size"),
        }
    }
}
impl McInst for X86 {
    type Reg = Reg;
    fn implicit_def(&self, abi: &'static dyn Abi<Self>) -> RegBits {
        match self {
            Self::push_r64 | Self::pop_r64 => Reg::rsp.bit(),
            Self::cmp_rr32 => Reg::eflags.bit(),
            #[rustfmt::skip]
            Self::add_rr8 | Self::add_rr16 | Self::add_rr32 | Self::add_rr64
            | Self::add_ri8 | Self::add_ri16 | Self::add_ri32 | Self::add_ri64
            | Self::sub_rr8 | Self::sub_rr16 | Self::sub_rr32 | Self::sub_rr64
            | Self::sub_ri8 | Self::sub_ri16 | Self::sub_ri32 | Self::sub_ri64
            | Self::imul_r8 | Self::imul_rr16 | Self::imul_rr32 | Self::imul_rr64
            | Self::imul_rri16 | Self::imul_rri64 => Reg::eflags.bit(),
            Self::div_r8 | Self::idiv_r8 => Reg::ax.bit() | Reg::eflags.bit(),
            Self::div_r16
            | Self::div_r32
            | Self::div_r64
            | Self::idiv_r16
            | Self::idiv_r32
            | Self::idiv_r64 => Reg::rax.bit() | Reg::rdx.bit() | Reg::eflags.bit(),
            Self::cbw => Reg::al.bit(),
            Self::cwd => Reg::dx.bit() | Reg::ax.bit(),
            Self::cdq | Self::cqo => Reg::rdx.bit() | Reg::rax.bit(),
            Self::call_function | Self::call_r64 => abi.caller_saved(),
            _ => Reg::NO_BITS,
        }
    }

    fn implicit_use(&self, abi: &'static dyn Abi<Self>) -> RegBits {
        match self {
            Self::push_r64 | Self::pop_r64 => Reg::rsp.bit(),
            Self::jl => Reg::eflags.bit(),
            Self::ret64 => abi.return_regs(1),
            Self::ret128 => abi.return_regs(2),
            Self::div_r8 | Self::idiv_r8 => Reg::ax.bit() | Reg::eflags.bit(),
            Self::div_r16
            | Self::div_r32
            | Self::div_r64
            | Self::idiv_r16
            | Self::idiv_r32
            | Self::idiv_r64 => Reg::rax.bit() | Reg::rdx.bit() | Reg::eflags.bit(),
            Self::cbw => Reg::al.bit(),
            Self::cwd => Reg::ax.bit(),
            Self::cdq | Self::cqo => Reg::rax.bit(),
            _ => Reg::NO_BITS,
        }
    }
}
impl X86 {
    pub fn is_ret(&self) -> bool {
        matches!(self, Self::ret0 | Self::ret64 | Self::ret128)
    }
}
