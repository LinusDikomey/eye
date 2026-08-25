use ir::{BlockId, FunctionId, GlobalId, TypedInstruction, parameter_types::Int32};

use crate::{Emit, Relocation, Size, arch::x86::isa::RegClass, emit::Emitter};

use super::isa::{Reg, X86};

impl Emit for X86 {
    const TMP: Self::Reg = super::TMP_REGISTER;

    fn implement_copy(e: &mut Emitter<Reg>, to: Self::Reg, from: Self::Reg) {
        let checked_reg = if to == super::TMP_REGISTER { from } else { to };
        let size = match checked_reg.class() {
            RegClass::GP8 | RegClass::GP8I => Size::S8,
            RegClass::GP16 | RegClass::GP16I => Size::S16,
            RegClass::GP32 | RegClass::GP32I => Size::S32,
            RegClass::GP64 | RegClass::GP64I => Size::S64,
            RegClass::F32 => todo!(),
            RegClass::F64 => todo!(),
            RegClass::Flags => {
                unreachable!("flags register should not be allocated")
            }
        };
        let ra = encode_reg(to);
        let rb = encode_reg(from);
        // HACK: currently checking the encoded registers to see if they
        // are the same. This shouldn't be necessary but right now, the
        // register size might be wrong after regalloc so only the encoded
        // registers will be equal.
        if ra != rb {
            mov_rr(e, size, (to, from));
        }
    }

    fn emit(e: &mut Emitter<Reg>, inst: TypedInstruction<Self>) {
        use X86 as I;
        let op = inst.op();
        match op {
            I::or_rr8 | I::or_rr16 | I::or_rr32 | I::or_rr64 => {
                inst_rr(e.text, op, &[0x08], &[0x09], e.ir.typed_args(&inst))
            }
            I::or_ri8
            | I::or_ri16
            | I::or_ri32
            | I::or_ri64
            | I::and_ri8
            | I::and_ri16
            | I::and_ri32
            | I::and_ri64 => {
                let is_and = matches!(op, I::and_ri8 | I::and_ri16 | I::and_ri32 | I::and_ri64);
                inst_ri(
                    e.text,
                    op,
                    &[0x80],
                    &[0x81],
                    e.ir.typed_args(&inst),
                    if is_and { 4 } else { 1 },
                )
            }
            I::and_rr8 | I::and_rr16 | I::and_rr32 | I::and_rr64 => {
                inst_rr(e.text, op, &[0x20], &[0x21], e.ir.typed_args(&inst))
            }
            I::push_r64 | I::pop_r64 => {
                let r = encode_reg(e.ir.typed_args(&inst));
                let rex = encode_rex(false, false, false, r.ext(), r.force());
                if rex != 0 {
                    e.text.push(rex);
                }
                let opcode = if op == I::push_r64 { 0x50 } else { 0x58 };
                e.text.push(opcode + r.bits);
            }
            I::mov_ri8 => {
                let (a, imm): (Reg, u32) = e.ir.typed_args(&inst);
                let imm8: i8 = (imm as i32).try_into().unwrap();
                let ra = encode_reg(a);
                let rex = encode_rex(false, false, false, ra.ext(), ra.force());
                if rex != 0 {
                    e.text.push(rex);
                }
                e.text.extend([0xB0 + ra.bits, imm8 as u8]);
            }
            I::mov_ri16 => {
                e.text.push(P16);
                let (a, imm): (Reg, u32) = e.ir.typed_args(&inst);
                let imm16: i16 = (imm as i32).try_into().unwrap();
                let ra = encode_reg(a);
                let rex = encode_rex(false, false, false, ra.ext(), ra.ext());
                if rex != 0 {
                    e.text.push(rex);
                }
                e.text.extend([0xB8 + ra.bits]);
                e.text.extend(imm16.to_le_bytes());
            }
            I::mov_ri32 => {
                let (a, imm): (Reg, u32) = e.ir.typed_args(&inst);
                let ra = encode_reg(a);
                let rex = encode_rex(false, false, false, ra.ext(), ra.force());
                if rex != 0 {
                    e.text.push(rex);
                }
                e.text.extend([0xB8 + ra.bits]);
                e.text.extend(imm.to_le_bytes());
            }
            I::mov_ri64 => inst_ri32(e.text, &[0xC7], e.ir.typed_args(&inst), true, 0),
            I::mov_rr8 | I::mov_rr16 | I::mov_rr32 | I::mov_rr64 => {
                mov_rr(e, op.size(), e.ir.typed_args(&inst));
            }
            I::mov_rm8 | I::mov_rm16 | I::mov_rm32 | I::mov_rm64 => {
                inst_rm(e.text, op.size(), &[0x8A], &[0x8B], e.ir.typed_args(&inst))
            }
            I::mov_mr8 | I::mov_mr16 | I::mov_mr32 | I::mov_mr64 => {
                inst_mr(e.text, op.size(), &[0x88], &[0x89], e.ir.typed_args(&inst))
            }
            I::ret0 | I::ret64 | I::ret128 => {
                e.text.push(0xc3);
            }
            I::cmp_rr8 | I::cmp_rr16 | I::cmp_rr32 | I::cmp_rr64 => {
                inst_rr(e.text, op, &[0x3A], &[0x3B], swap(e.ir.typed_args(&inst)))
            }
            I::test_rr8 => inst_rr_legacy(e.text, &[0x84], swap(e.ir.typed_args(&inst)), false),
            I::jmp => {
                let target = e.ir.typed_args(&inst);
                if !e.is_next(target) {
                    emit_jmp(e, &[0xEB], &[0xE9], target);
                }
            }
            I::je => {
                let target = e.ir.typed_args(&inst);
                emit_jmp(e, &[0x74, 0xCB], &[0x0F, 0x84], target);
            }
            I::jne => {
                let target = e.ir.typed_args(&inst);
                emit_jmp(e, &[0x75, 0xCB], &[0x0F, 0x85], target);
            }
            I::jl => {
                let target = e.ir.typed_args(&inst);
                emit_jmp(e, &[0x7C, 0xCB], &[0x0F, 0x8C], target);
            }
            I::jge => {
                let target = e.ir.typed_args(&inst);
                emit_jmp(e, &[0x7D, 0xCB], &[0x0F, 0x8D], target);
            }
            I::jle => {
                let target = e.ir.typed_args(&inst);
                emit_jmp(e, &[0x7E, 0xCB], &[0x0F, 0x8E], target);
            }
            I::jg => {
                let target = e.ir.typed_args(&inst);
                emit_jmp(e, &[0x7F, 0xCB], &[0x0F, 0x8F], target);
            }
            I::seto => inst_r_legacy(e.text, &[0x0F, 0x90], e.ir.typed_args(&inst), 0, false),
            I::setno => inst_r_legacy(e.text, &[0x0F, 0x91], e.ir.typed_args(&inst), 0, false),
            I::setc => inst_r_legacy(e.text, &[0x0F, 0x92], e.ir.typed_args(&inst), 0, false),
            I::setnc => inst_r_legacy(e.text, &[0x0F, 0x93], e.ir.typed_args(&inst), 0, false),
            I::sete => inst_r_legacy(e.text, &[0x0F, 0x94], e.ir.typed_args(&inst), 0, false),
            I::setne => inst_r_legacy(e.text, &[0x0F, 0x95], e.ir.typed_args(&inst), 0, false),
            I::setbe => inst_r_legacy(e.text, &[0x0F, 0x96], e.ir.typed_args(&inst), 0, false),
            I::seta => inst_r_legacy(e.text, &[0x0F, 0x97], e.ir.typed_args(&inst), 0, false),
            I::sets => inst_r_legacy(e.text, &[0x0F, 0x98], e.ir.typed_args(&inst), 0, false),
            I::setns => inst_r_legacy(e.text, &[0x0F, 0x99], e.ir.typed_args(&inst), 0, false),
            I::setp => inst_r_legacy(e.text, &[0x0F, 0x9A], e.ir.typed_args(&inst), 0, false),
            I::setnp => inst_r_legacy(e.text, &[0x0F, 0x9B], e.ir.typed_args(&inst), 0, false),
            I::setl => inst_r_legacy(e.text, &[0x0F, 0x9C], e.ir.typed_args(&inst), 0, false),
            I::setge => inst_r_legacy(e.text, &[0x0F, 0x9D], e.ir.typed_args(&inst), 0, false),
            I::setle => inst_r_legacy(e.text, &[0x0F, 0x9E], e.ir.typed_args(&inst), 0, false),
            I::setg => inst_r_legacy(e.text, &[0x0F, 0x9F], e.ir.typed_args(&inst), 0, false),

            I::add_rr8 | I::add_rr16 | I::add_rr32 | I::add_rr64 => {
                inst_rr(e.text, op, &[0x00], &[0x01], e.ir.typed_args(&inst));
            }
            I::add_ri8 | I::add_ri16 | I::add_ri32 | I::add_ri64 => {
                inst_ri(e.text, op, &[0x80], &[0x81], e.ir.typed_args(&inst), 0);
            }
            I::sub_rr8 | I::sub_rr16 | I::sub_rr32 | I::sub_rr64 => {
                inst_rr(e.text, op, &[0x28], &[0x29], e.ir.typed_args(&inst));
            }
            I::sub_ri8 | I::sub_ri16 | I::sub_ri32 | I::sub_ri64 => {
                inst_ri(e.text, op, &[0x80], &[0x81], e.ir.typed_args(&inst), 5)
            }
            I::imul_r8 => inst_r_legacy(e.text, &[0xF6], e.ir.typed_args(&inst), 5, false),
            I::imul_rr16 | I::imul_rr32 | I::imul_rr64 => {
                if op == I::imul_rr16 {
                    e.text.push(P16);
                }
                inst_rr_legacy(
                    e.text,
                    &[0x0F, 0xAF],
                    swap(e.ir.typed_args(&inst)),
                    op == I::imul_rr64,
                )
            }
            I::imul_rri16 | I::imul_rri32 | I::imul_rri64 => {
                inst_rri(e.text, op.size(), &[], &[0x69], e.ir.typed_args(&inst));
            }
            I::cbw => e.text.push(0x98),
            I::cwd => e.text.extend([P16, 0x99]),
            I::cdq => e.text.push(0x99),
            I::cqo => e
                .text
                .extend([encode_rex(true, false, false, false, false), 0x99]),
            I::div_r8 | I::div_r16 | I::div_r32 | I::div_r64 => inst_r(
                e.text,
                op.size(),
                &[0xF6],
                &[0xF7],
                e.ir.typed_args(&inst),
                6,
            ),
            I::idiv_r8 | I::idiv_r16 | I::idiv_r32 | I::idiv_r64 => inst_r(
                e.text,
                op.size(),
                &[0xF6],
                &[0xF7],
                e.ir.typed_args(&inst),
                7,
            ),
            I::shl_ri8
            | I::shl_ri16
            | I::shl_ri32
            | I::shl_ri64
            | I::shr_ri8
            | I::shr_ri16
            | I::shr_ri32
            | I::shr_ri64 => {
                let is_left = matches!(op, I::shl_ri8 | I::shl_ri16 | I::shl_ri32 | I::shl_ri64);
                let size = op.size();
                if size == Size::S16 {
                    e.text.push(P16);
                }
                let (r, imm): (Reg, u32) = e.ir.typed_args(&inst);
                // shr always uses an 8-bit imm
                let imm = imm as u8; // just truncating upper bits of shr here is fine
                let modrm = encode_modrm_ri(r, size == Size::S64, if is_left { 4 } else { 5 });
                if modrm.rex != 0 {
                    e.text.push(modrm.rex);
                }
                e.text
                    .extend([if size == Size::S8 { 0xC0 } else { 0xC1 }, modrm.modrm, imm]);
            }
            I::neg_r8 => inst_r_legacy(e.text, &[0xF6], e.ir.typed_args(&inst), 3, false),
            I::neg_r16 | I::neg_r32 | I::neg_r64 => {
                if op == I::neg_r16 {
                    e.text.push(P16);
                }
                inst_r_legacy(e.text, &[0xF7], e.ir.typed_args(&inst), 3, op == I::neg_r64);
            }
            I::xor_ri8 | I::xor_ri16 | I::xor_ri32 | I::xor_ri64 => {
                inst_ri(e.text, op, &[0x80], &[0x81], e.ir.typed_args(&inst), 6)
            }
            I::xor_rr8 | I::xor_rr16 | I::xor_rr32 | I::xor_rr64 => {
                inst_rr(e.text, op, &[0x30], &[0x31], e.ir.typed_args(&inst))
            }
            I::lea_rm32 | I::lea_rm64 => {
                inst_rm(e.text, op.size(), &[], &[0x8D], e.ir.typed_args(&inst));
            }
            I::lea_function => {
                let (dst, function): (Reg, FunctionId) = e.ir.typed_args(&inst);
                // lea dst [rip + offset]
                let relocation_offset = disp32(e.text, &[0x8D], dst);
                e.relocations.push(Relocation::FunctionAddr(
                    function.function,
                    relocation_offset,
                ));
            }
            I::lea_global => {
                let (dst, global): (Reg, GlobalId) = e.ir.typed_args(&inst);
                // lea dst [rip + offset]
                let relocation_offset = disp32(e.text, &[0x8D], dst);
                e.relocations
                    .push(Relocation::GlobalAddr(global.idx, relocation_offset));
            }
            I::call_function => {
                let function: FunctionId = e.ir.typed_args(&inst);
                e.relocations.push(Relocation::FunctionCall(
                    function.function,
                    e.text.len() as u64 + 1,
                ));
                e.text.extend([0xE8, 0, 0, 0, 0]);
            }
            I::call_r64 => {
                let r = e.ir.typed_args(&inst);
                let ra = encode_reg(r);
                let rex = encode_rex(false, false, false, ra.ext(), ra.force());
                debug_assert!(!(ra.prevents_rex() && rex != 0));
                if rex != 0 {
                    e.text.push(rex);
                }
                e.text.extend([0xFF, MODRM_RR | (2 << 3) | ra.bits]);
            }
            I::movsx16_rr8 | I::movsx32_rr8 | I::movsx64_rr8 => {
                let size = match op {
                    I::movsx16_rr8 => Size::S16,
                    I::movsx32_rr8 => Size::S32,
                    _ => Size::S64,
                };
                inst_rr_generic_inner(
                    e.text,
                    size,
                    &[],
                    &[0x0F, 0xBE],
                    swap(e.ir.typed_args(&inst)),
                );
            }
            I::movsx32_rr16 | I::movsx64_rr16 => {
                let size = if op == I::movsx32_rr16 {
                    Size::S32
                } else {
                    Size::S64
                };
                inst_rr_generic_inner(
                    e.text,
                    size,
                    &[],
                    &[0x0F, 0xBF],
                    swap(e.ir.typed_args(&inst)),
                );
            }
            I::movsx64_rr32 => {
                inst_rr_generic_inner(
                    e.text,
                    Size::S64,
                    &[],
                    &[0x63],
                    swap(e.ir.typed_args(&inst)),
                );
            }
            I::movzx16_rr8 | I::movzx32_rr8 => {
                let size = if op == I::movzx16_rr8 {
                    Size::S16
                } else {
                    Size::S32
                };
                inst_rr_generic_inner(
                    e.text,
                    size,
                    &[],
                    &[0x0F, 0xB6],
                    swap(e.ir.typed_args(&inst)),
                );
            }
            I::movzx32_rr16 => {
                inst_rr_generic_inner(
                    e.text,
                    Size::S32,
                    &[],
                    &[0x0F, 0xB7],
                    swap(e.ir.typed_args(&inst)),
                );
            }
        }
    }
}

fn swap((a, b): (Reg, Reg)) -> (Reg, Reg) {
    (b, a)
}

fn mov_rr(e: &mut Emitter<Reg>, size: Size, (a, b): (Reg, Reg)) {
    inst_rr_generic_inner(e.text, size, &[0x88], &[0x89], (a, b))
}

/// emits an instruction with a placeholder disp32 (rip-relative address) and returns the offset
/// of that offset placeholder to be used for emitting a relocation
fn disp32(text: &mut Vec<u8>, opcode: &[u8], reg: Reg) -> u64 {
    let r = encode_reg(reg);
    let rex = encode_rex(true, r.ext(), false, false, r.force());
    if rex != 0 {
        text.push(rex);
    }
    text.extend_from_slice(opcode);
    text.push(r.bits << 3 | 0b101);
    let relocation_offset = text.len().try_into().expect("text segment is too large");
    text.extend([0; 4]);
    relocation_offset
}

fn inst_r(text: &mut Vec<u8>, size: Size, opcode_8: &[u8], opcode: &[u8], a: Reg, extension: u8) {
    if size == Size::S16 {
        text.push(P16);
    }
    let modrm = encode_modrm_r(a, size == Size::S64, extension);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(if size == Size::S8 { opcode_8 } else { opcode });
    text.push(modrm.modrm);
}

fn inst_r_legacy(text: &mut Vec<u8>, opcode: &[u8], a: Reg, extension: u8, wide: bool) {
    let modrm = encode_modrm_r(a, wide, extension);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(opcode);
    text.push(modrm.modrm);
}

fn inst_rr_legacy(text: &mut Vec<u8>, opcode: &[u8], (reg, rm): (Reg, Reg), wide: bool) {
    let modrm = encode_modrm_rr(reg, rm, wide);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(opcode);
    text.push(modrm.modrm);
}

fn inst_rr(text: &mut Vec<u8>, inst: X86, opcode_8: &[u8], opcode: &[u8], (rm, reg): (Reg, Reg)) {
    inst_rr_generic_inner(text, inst.size(), opcode_8, opcode, (rm, reg));
}

fn inst_rr_generic_inner(
    text: &mut Vec<u8>,
    size: Size,
    opcode_8: &[u8],
    opcode: &[u8],
    (rm, reg): (Reg, Reg),
) {
    if size == Size::S16 {
        text.push(P16);
    }
    let modrm = encode_modrm_rr(reg, rm, size == Size::S64);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(if size == Size::S8 { opcode_8 } else { opcode });
    text.push(modrm.modrm);
}

fn inst_rm(
    text: &mut Vec<u8>,
    size: Size,
    opcode_8: &[u8],
    opcode: &[u8],
    (reg_val, reg_ptr, off, index_reg, scale): (Reg, Reg, Int32, Reg, Int32),
) {
    if size == Size::S16 {
        text.push(P16);
    }
    let mut off = OffsetClass::from_imm(off);
    let a = encode_reg(reg_val);
    let b = encode_reg(reg_ptr);
    let index = encode_reg(index_reg);
    debug_assert!(index.bits != 0b100);
    let rex = encode_rex(
        size == Size::S64,
        a.ext(),
        index.ext(),
        b.ext(),
        a.force() || b.force() || index.force(),
    );
    debug_assert!(!((a.prevents_rex() || b.prevents_rex() || index.prevents_rex()) && rex != 0));
    if rex != 0 {
        text.push(rex);
    }
    text.extend(if size == Size::S8 { opcode_8 } else { opcode });
    if b.bits == 0b101 && matches!(off, OffsetClass::Zero) {
        // becomes [disp32] otherwise
        off = OffsetClass::Byte(0);
    }
    let need_sib = index_reg != Reg::none || b.bits == 0b100 || reg_ptr == Reg::none;

    if need_sib {
        text.extend([
            off.modrm_bits() | (a.bits << 3) | 0b100,
            sib(scale, (index_reg != Reg::none).then_some(index), b),
        ]);
    } else {
        text.push(off.modrm_bits() | (a.bits << 3) | b.bits);
    }
    off.write(text);
}

fn sib(scale: u32, index: Option<EncodedReg>, base: EncodedReg) -> u8 {
    let scale_bits = match scale {
        1 => 0b00,
        2 => 0b01,
        4 => 0b10,
        8 => 0b11,
        _ => unreachable!("invalid scale for memory operand: {scale}"),
    };
    let index_bits = index.map_or(0b100, |index| index.bits);
    (scale_bits << 6) | (index_bits << 3) | base.bits
}
fn inst_mr(
    text: &mut Vec<u8>,
    size: Size,
    opcode_8: &[u8],
    opcode: &[u8],
    (reg_ptr, off, index, scale, reg_val): (Reg, Int32, Reg, Int32, Reg),
) {
    // encoded exactly the same way, just swap the arguments around correctly
    inst_rm(
        text,
        size,
        opcode_8,
        opcode,
        (reg_val, reg_ptr, off, index, scale),
    );
}

fn inst_ri(
    text: &mut Vec<u8>,
    op: X86,
    opcode_8: &[u8],
    opcode: &[u8],
    (r, imm): (Reg, u32),
    i: u8,
) {
    let size = op.size();
    if size == Size::S16 {
        text.push(P16);
    }
    let modrm = encode_modrm_ri(r, size == Size::S64, i);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(if size == Size::S8 { opcode_8 } else { opcode });
    text.push(modrm.modrm);
    match size {
        Size::S8 => text.push(imm as u8),
        Size::S16 => text.extend((imm as u16).to_le_bytes()),
        Size::S32 | Size::S64 => text.extend(imm.to_le_bytes()),
        Size::S128 => unreachable!(), // no 128-bit register instructions used
    }
}

fn inst_rri(
    text: &mut Vec<u8>,
    size: Size,
    opcode_8: &[u8],
    opcode: &[u8],
    (reg, rm, imm): (Reg, Reg, u32),
) {
    if size == Size::S16 {
        text.push(P16);
    }
    let modrm = encode_modrm_rr(reg, rm, size == Size::S64);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(if size == Size::S8 { opcode_8 } else { opcode });
    text.push(modrm.modrm);
    match size {
        Size::S8 => text.push(imm as u8),
        Size::S16 => text.extend((imm as u16).to_le_bytes()),
        Size::S32 | Size::S64 => text.extend(imm.to_le_bytes()),
        Size::S128 => unreachable!(), // no 128-bit register instructions used
    }
}

fn inst_ri32(text: &mut Vec<u8>, opcode: &[u8], (r, imm): (Reg, u32), wide: bool, i: u8) {
    let modrm = encode_modrm_ri(r, wide, i);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(opcode);
    text.push(modrm.modrm);
    text.extend(imm.to_le_bytes());
}

fn emit_jmp(e: &mut Emitter<Reg>, rel8_op: &[u8], rel32_op: &[u8], target: BlockId) {
    let my_rel8_offset = e.offset_in_function() + rel8_op.len() as u32 - 1;
    if let Some(known) = e.block_offset(target) {
        let offset: i32 = (known as i64 - my_rel8_offset as i64).try_into().unwrap();
        let offset8: Result<i8, _> = offset.try_into();
        if let Ok(offset8) = offset8 {
            e.text.extend(rel8_op);
            e.text.push(offset8 as u8);
        } else {
            e.text.extend(rel32_op);
            let offset: i32 = known
                .checked_signed_diff(e.offset_in_function() + 4)
                .unwrap();
            e.text.extend(offset.to_le_bytes());
        }
    } else {
        e.text.extend(rel32_op);
        let offset = e.offset_in_function();
        e.missing_block_addrs.push((offset, target));
        e.text.extend(0i32.to_le_bytes());
    }
}

/// 16-bit instruction prefix
const P16: u8 = 0x66;
const MODRM_RR: u8 = 0b1100_0000;

#[derive(Debug, Clone, Copy)]
enum OffsetClass {
    Zero,
    Byte(i8),
    DWord(i32),
}
impl OffsetClass {
    fn from_imm(value: u32) -> Self {
        let value = value as i32;
        if value == 0 {
            Self::Zero
        } else if let Ok(b) = value.try_into() {
            Self::Byte(b)
        } else {
            Self::DWord(value)
        }
    }

    fn modrm_bits(self) -> u8 {
        (match self {
            OffsetClass::Zero => 0b00,
            OffsetClass::Byte(_) => 0b01,
            OffsetClass::DWord(_) => 0b10,
        }) << 6
    }

    fn write(self, text: &mut Vec<u8>) {
        match self {
            OffsetClass::Zero => {}
            OffsetClass::Byte(b) => text.push(b as u8),
            OffsetClass::DWord(dw) => text.extend(dw.to_le_bytes()),
        }
    }
}

#[derive(Debug)]
struct Modrm {
    rex: u8,
    modrm: u8,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RegKind {
    Normal,
    High,
    ForceRex,
    Extended,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct EncodedReg {
    // three-bit register encoding
    bits: u8,
    kind: RegKind,
}
impl EncodedReg {
    /// if the encoding requires an extension bit somewhere
    pub fn ext(self) -> bool {
        matches!(self.kind, RegKind::Extended)
    }

    /// if the encoding forces a rex prefix
    fn force(self) -> bool {
        matches!(self.kind, RegKind::ForceRex)
    }

    /// if the encoding forbids emitting a rex prefix
    fn prevents_rex(self) -> bool {
        matches!(self.kind, RegKind::High)
    }
}
#[rustfmt::skip]
fn encode_reg(r: Reg) -> EncodedReg {
    use Reg::*;

    let (bits, kind) = match r {
        none => (0, RegKind::Normal),
        al | ax | rax | eax => (0, RegKind::Normal),
        cl | cx | rcx | ecx => (1, RegKind::Normal),
        dl | dx | rdx | edx => (2, RegKind::Normal),
        bl | bx | rbx | ebx => (3, RegKind::Normal),

        ah => (4, RegKind::High),
        ch => (5, RegKind::High),
        dh => (6, RegKind::High),
        bh => (7, RegKind::High),

        spl => (4, RegKind::ForceRex),
        bpl => (5, RegKind::ForceRex),
        sil => (6, RegKind::ForceRex),
        dil => (7, RegKind::ForceRex),

        sp | rsp | esp => (4, RegKind::Normal),
        bp | rbp | ebp => (5, RegKind::Normal),
        si | rsi | esi => (6, RegKind::Normal),
        di | rdi | edi => (7, RegKind::Normal),

        r8b  | r8w  | r8  | r8d  => (0, RegKind::Extended),
        r9b  | r9w  | r9  | r9d  => (1, RegKind::Extended),
        r10b | r10w | r10 | r10d => (2, RegKind::Extended),
        r11b | r11w | r11 | r11d => (3, RegKind::Extended),
        r12b | r12w | r12 | r12d => (4, RegKind::Extended),
        r13b | r13w | r13 | r13d => (5, RegKind::Extended),
        r14b | r14w | r14 | r14d => (6, RegKind::Extended),
        r15b | r15w | r15 | r15d => (7, RegKind::Extended),

        eflags | rip => unreachable!(),
    };
    EncodedReg { bits, kind }
}

fn encode_rex(w: bool, r: bool, x: bool, b: bool, force: bool) -> u8 {
    if w || r || x || b || force {
        0b_0100_0000 | ((w as u8) << 3) | ((r as u8) << 2) | ((x as u8) << 1) | b as u8
    } else {
        0
    }
}

fn encode_modrm_r(r: Reg, wide: bool, extension: u8) -> Modrm {
    debug_assert!(extension < 8);
    let ra = encode_reg(r);
    let rex = encode_rex(wide, ra.ext(), false, false, ra.force());
    debug_assert!(!(ra.prevents_rex() && rex != 0));
    Modrm {
        rex,
        modrm: MODRM_RR | (extension << 3) | ra.bits,
    }
}

fn encode_modrm_rr(reg: Reg, rm: Reg, wide: bool) -> Modrm {
    let reg = encode_reg(reg);
    let rm = encode_reg(rm);
    let rex = encode_rex(wide, reg.ext(), false, rm.ext(), reg.force() || rm.force());
    debug_assert!(!((reg.prevents_rex() || rm.prevents_rex()) && rex != 0));
    Modrm {
        rex,
        modrm: MODRM_RR | (reg.bits << 3) | rm.bits,
    }
}

fn encode_modrm_ri(rm: Reg, wide: bool, i: u8) -> Modrm {
    debug_assert!(i < 8);
    let rm = encode_reg(rm);
    debug_assert!(!(rm.prevents_rex() && wide));
    Modrm {
        rex: encode_rex(wide, false, false, rm.ext(), rm.force()),
        modrm: MODRM_RR | i << 3 | rm.bits,
    }
}
