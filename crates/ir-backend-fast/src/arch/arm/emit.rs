use ir::parameter_types::Int32;

use crate::arch::arm::isa::{Arm, Reg, RegClass};

type Emitter<'a> = crate::emit::Emitter<'a, Reg>;

impl crate::Emit for Arm {
    const TMP: Self::Reg = super::isa::TMP_REGISTER;

    fn implement_copy(e: &mut Emitter, to: Self::Reg, from: Self::Reg) {
        let checked_reg = if to == super::isa::TMP_REGISTER {
            from
        } else {
            to
        };
        let (fp, sf) = match checked_reg.class() {
            RegClass::GP32 => (false, false),
            RegClass::GP64 => (false, true),
            RegClass::FP32 => (true, false),
            RegClass::FP64 => (true, true),
            RegClass::ZR => unreachable!("shouldn't emit copy with ZR"),
        };
        if fp {
            e.text.extend(fmov_reg((to, from), sf).to_le_bytes());
        } else {
            let zr = if sf { Reg::xzr } else { Reg::wzr };
            e.text.extend(orr((to, from, zr), sf).to_le_bytes());
        }
    }

    fn emit(e: &mut Emitter, inst: ir::TypedInstruction<Self>) {
        use Arm as I;
        let op: u32 = match inst.op() {
            I::orr32 => orr(e.ir.typed_args(&inst), false),
            I::orr64 => orr(e.ir.typed_args(&inst), true),
            I::movz32 | I::movz64 => mov_imm(0b10, e.ir.typed_args(&inst), inst.op() == I::movz64),
            I::movk32 | I::movk64 => mov_imm(0b11, e.ir.typed_args(&inst), inst.op() == I::movk64),
            I::ldr8 | I::ldr16 | I::ldr32 | I::ldr64 => {
                let size = match inst.op() {
                    I::ldr8 => Size::S1,
                    I::ldr16 => Size::S2,
                    I::ldr32 => Size::S4,
                    I::ldr64 => Size::S8,
                    _ => unreachable!(),
                };
                let (dst, src, offset): (Reg, Reg, Int32) = e.ir.typed_args(&inst);
                if offset > (1 << 12) {
                    panic!("ldr offset too large");
                }
                load_store_unsigned_imm(size, offset as u16, src, dst)
            }
            I::ldp32 | I::ldp64 => {
                let (dst1, dst2, ptr, imm): (Reg, Reg, Reg, Int32) = e.ir.typed_args(&inst);
                load_store_pair_signed_imm(
                    if inst.op() == I::ldp64 { 0b10 } else { 0b00 },
                    true,
                    imm as i8,
                    dst2,
                    ptr,
                    dst1,
                )
            }
            I::ret0 | I::ret64 | I::ret128 => ret(e.ir.typed_args(&inst)),
        };
        e.text.extend(op.to_le_bytes());
    }
}

macro_rules! assert_num_equal {
    ($a: expr, $b: expr) => {
        let _: [(); $a] = [(); $b];
    };
}

const fn correct_length_binary_literal(s: &str, len: usize) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0] == b'0' && b[1] == b'b' && b.len() - 2 == len
}
macro_rules! encode {
    (@parse $prev_bit: literal) => { 0u32 };
    (@parse $prev_bit: literal $hi: literal .. $lo: literal $lit: literal, $($rest: tt)*) => {
        ({
            assert_num_equal!($prev_bit, $hi+ 1);
            const _: () = assert!(correct_length_binary_literal(
                stringify!($lit),
                $hi - $lo + 1
            ));
            $lit << $lo
        }) | encode!(@parse $lo $($rest)*)
    };
    (@parse $prev_bit: literal $hi: literal .. $lo: literal $value: expr, $($rest: tt)*) => {
        ({
            assert_num_equal!($prev_bit, $hi+ 1);
            let value = u32::from($value);
            let width = $hi - $lo + 1;
            debug_assert!(width == 32 || value < (1 << width));
            value << $lo
        }) | encode!(@parse $lo $($rest)*)
    };
    (@parse $prev_bit: literal $bit: literal $value: expr, $($rest: tt)*) => {
        ({
            let v: bool = $value;
            assert_num_equal!($prev_bit, $bit + 1);
            u32::from(v) << $bit
        }) | encode!(@parse $bit $($rest)*)
    };
    (@parse $($invalid:tt)*) => {
        compile_error!("invalid bitfield syntax")
    };
    ($($rest: tt)*) => { encode!(@parse 32 $($rest)*) };
}

fn orr((rd, rn, rm): (Reg, Reg, Reg), sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b01, false, rm, 0, rn, rd)
}

fn fmov_reg((rd, rn): (Reg, Reg), sf: bool) -> u32 {
    ((sf as u32) << 22)
        | (0b11110 << 24)
        | (0b100000 << 10)
        | ((rn.index() as u32) << 5)
        | rd.index() as u32
}

fn logical_shifted_reg(sf: bool, opc: u8, n: bool, rm: Reg, imm6: u8, rn: Reg, rd: Reg) -> u32 {
    encode! {
        31     sf,
        30..29 opc,
        28..24 0b01010,
        23..22 0b00, // shift
        21     n,
        20..16 rm.index(),
        15..10 imm6,
         9.. 5 rn.index(),
         4.. 0 rd.index(),
    }
}

#[repr(u8)]
enum Size {
    S1 = 0b00,
    S2 = 0b01,
    S4 = 0b10,
    S8 = 0b11,
}

fn load_store_unsigned_imm(size: Size, imm12: u16, rn: Reg, rt: Reg) -> u32 {
    debug_assert!(imm12 < 1 << 12);
    ((size as u32) << 30)
        | (0b111001 << 24)
        | (0b01 << 22)
        | ((imm12 as u32) << 10)
        | ((rn.index() as u32) << 5)
        | rt.index() as u32
}

fn load_store_pair_signed_imm(opc: u8, load: bool, imm7: i8, rt2: Reg, rn: Reg, rt: Reg) -> u32 {
    encode! {
        31..30 opc,
        29..23 0b1010010,
        22     load,
        21..15 imm7 as u8,
        14..10 rt2.index(),
        9 .. 5 rn.index(),
        4 .. 0 rt.index(),
    }
}

fn mov_imm(opc: u8, (rd, hw, imm16): (Reg, Int32, Int32), sf: bool) -> u32 {
    encode! {
        31     sf,
        30..29 opc,
        28..23 0b100101,
        22..21 hw,
        20..5  imm16,
         4..0  rd.index(),
    }
}

fn ret(rn: Reg) -> u32 {
    encode! {
        31..10 0b1101011001011111000000,
         9..5  rn.index(),
         4..0 0b00000,
    }
}
