use std::collections::VecDeque;

use ir::{
    Argument, Bitmap, BlockId, Environment, FunctionId, FunctionIr, GlobalId, MCReg, ModuleOf,
    block_graph::Blocks,
    mc::{Mc, ParcopySolver, RegClass},
    parameter_types::Int32,
};

use crate::arch::x86::isa::Size;

use super::isa::{Reg, X86};

pub fn write(
    env: &Environment,
    mc: ModuleOf<Mc>,
    x86: ModuleOf<X86>,
    ir: &FunctionIr,
    text: &mut Vec<u8>,
    relocations: &mut Vec<(FunctionId, u64)>,
    global_relocations: &mut Vec<(GlobalId, u64)>,
) {
    let mut parcopy = ParcopySolver::new();
    let start = text.len();
    let mut block_queue = VecDeque::from([BlockId::ENTRY]);
    let mut queued_blocks = Bitmap::new(ir.block_count() as usize);
    queued_blocks.set(BlockId::ENTRY.idx(), true);
    let mut block_offsets: Box<[Option<u32>]> =
        vec![None; ir.block_count() as usize].into_boxed_slice();

    let mut missing_block_addrs: Vec<(u32, BlockId)> = Vec::new();

    while let Some(block) = block_queue.pop_front() {
        let offset = &mut block_offsets[block.idx()];
        if offset.is_some() {
            continue;
        }
        *offset = Some((text.len() - start) as u32);
        for succ in ir.successors(env, block) {
            if queued_blocks.get(succ.idx()) {
                continue;
            }
            queued_blocks.set(succ.idx(), true);
            block_queue.push_back(succ);
        }

        let mut block_iter = ir.get_block(block).peekable();
        while let Some((r, i)) = block_iter.next() {
            if let Some(inst) = i.as_module(mc) {
                match inst.op() {
                    Mc::IncomingBlockArgs => {}
                    Mc::Copy | Mc::AssignBlockArgs => {
                        let args = ir.args_iter(i, env).map(|arg| {
                            let Argument::MCReg(r) = arg else {
                                unreachable!()
                            };
                            r
                        });
                        parcopy.parcopy(
                            args,
                            |to, from| {
                                let to: Reg = to.phys().unwrap();
                                let from: Reg = from.phys().unwrap();
                                let size = match to.class() {
                                    RegClass::GP8 => Size::S8,
                                    RegClass::GP16 => Size::S16,
                                    RegClass::GP32 => Size::S32,
                                    RegClass::GP64 => Size::S64,
                                    RegClass::F32 => todo!(),
                                    RegClass::F64 => todo!(),
                                    RegClass::Flags => todo!(),
                                };
                                let ra = encode_reg(to);
                                let rb = encode_reg(from);
                                // HACK: currently checking the encoded registers to see if they
                                // are the same. This shouldn't be necessary but right now, the
                                // register size might be wrong after regalloc so only the encoded
                                // registers will be equal.
                                if ra != rb {
                                    mov_rr(text, size, (to, from));
                                }
                            },
                            MCReg::from_phys(super::TMP_REGISTER),
                        );
                    }
                }
                continue;
            }
            let Some(inst) = i.as_module(x86) else {
                panic!("expected x86 instruction but encountered other module at {r}");
            };

            use X86 as I;
            let op = inst.op();
            match op {
                I::or_rr8 | I::or_rr16 | I::or_rr32 | I::or_rr64 => {
                    inst_rr(text, op, &[0x08], &[0x09], ir.args(i, env))
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
                        text,
                        op,
                        &[0x80],
                        &[0x81],
                        ir.args(i, env),
                        if is_and { 4 } else { 1 },
                    )
                }
                I::and_rr8 | I::and_rr16 | I::and_rr32 | I::and_rr64 => {
                    inst_rr(text, op, &[0x20], &[0x21], ir.args(i, env))
                }
                I::push_r64 | I::pop_r64 => {
                    let r = encode_reg(ir.args(i, env));
                    let rex = encode_rex(false, false, false, r.ext(), r.force());
                    if rex != 0 {
                        text.push(rex);
                    }
                    let opcode = if op == I::push_r64 { 0x50 } else { 0x58 };
                    text.push(opcode + r.bits);
                }
                I::mov_ri8 => {
                    let (a, imm): (Reg, u32) = ir.args(i, env);
                    let imm8: i8 = (imm as i32).try_into().unwrap();
                    let ra = encode_reg(a);
                    let rex = encode_rex(false, false, false, ra.ext(), ra.force());
                    if rex != 0 {
                        text.push(rex);
                    }
                    text.extend([0xB0 + ra.bits, imm8 as u8]);
                }
                I::mov_ri16 => {
                    text.push(P16);
                    let (a, imm): (Reg, u32) = ir.args(i, env);
                    let imm16: i16 = (imm as i32).try_into().unwrap();
                    let ra = encode_reg(a);
                    let rex = encode_rex(false, false, false, ra.ext(), ra.ext());
                    if rex != 0 {
                        text.push(rex);
                    }
                    text.extend([0xB8 + ra.bits]);
                    text.extend(imm16.to_le_bytes());
                }
                I::mov_ri32 => {
                    let (a, imm): (Reg, u32) = ir.args(i, env);
                    let ra = encode_reg(a);
                    let rex = encode_rex(false, false, false, ra.ext(), ra.force());
                    if rex != 0 {
                        text.push(rex);
                    }
                    text.extend([0xB8 + ra.bits]);
                    text.extend(imm.to_le_bytes());
                }
                I::mov_ri64 => inst_ri32(text, &[0xC7], ir.args(i, env), true, 0),
                I::mov_rr8 | I::mov_rr16 | I::mov_rr32 | I::mov_rr64 => {
                    mov_rr(text, op.size(), ir.args(i, env));
                }
                I::mov_rm8 | I::mov_rm16 | I::mov_rm32 | I::mov_rm64 => {
                    inst_rm(text, op.size(), &[0x8A], &[0x8B], ir.args(i, env))
                }
                I::mov_mr8 | I::mov_mr16 | I::mov_mr32 | I::mov_mr64 => {
                    inst_mr(text, op.size(), &[0x88], &[0x89], ir.args(i, env))
                }
                I::ret0 | I::ret64 | I::ret128 => {
                    text.push(0xc3);
                }
                I::cmp_rr8 | I::cmp_rr16 | I::cmp_rr32 | I::cmp_rr64 => {
                    let (a, b) = ir.args(i, env);
                    // comparisons need to be emitted the other way around
                    inst_rr(text, op, &[0x3A], &[0x3B], (b, a))
                }
                I::test_rr8 => inst_rr_legacy(text, &[0x84], ir.args(i, env), false),
                I::jmp => {
                    let target = ir.args(i, env);
                    if block_queue.front().is_none_or(|&front| front != target)
                        || block_iter.peek().is_some()
                    {
                        emit_jmp(
                            &[0xEB],
                            &[0xE9],
                            target,
                            text,
                            start,
                            &block_offsets,
                            &mut missing_block_addrs,
                        );
                    }
                }
                I::je => {
                    let target = ir.args(i, env);
                    emit_jmp(
                        &[0x74, 0xCB],
                        &[0x0F, 0x84],
                        target,
                        text,
                        start,
                        &block_offsets,
                        &mut missing_block_addrs,
                    );
                }
                I::jne => {
                    let target = ir.args(i, env);
                    emit_jmp(
                        &[0x75, 0xCB],
                        &[0x0F, 0x85],
                        target,
                        text,
                        start,
                        &block_offsets,
                        &mut missing_block_addrs,
                    );
                }
                I::jl => {
                    let target = ir.args(i, env);
                    emit_jmp(
                        &[0x7C, 0xCB],
                        &[0x0F, 0x8C],
                        target,
                        text,
                        start,
                        &block_offsets,
                        &mut missing_block_addrs,
                    );
                }
                I::jge => {
                    let target = ir.args(i, env);
                    emit_jmp(
                        &[0x7D, 0xCB],
                        &[0x0F, 0x8D],
                        target,
                        text,
                        start,
                        &block_offsets,
                        &mut missing_block_addrs,
                    );
                }
                I::jle => {
                    let target = ir.args(i, env);
                    emit_jmp(
                        &[0x7E, 0xCB],
                        &[0x0F, 0x8E],
                        target,
                        text,
                        start,
                        &block_offsets,
                        &mut missing_block_addrs,
                    );
                }
                I::jg => {
                    let target = ir.args(i, env);
                    emit_jmp(
                        &[0x7F, 0xCB],
                        &[0x0F, 0x8F],
                        target,
                        text,
                        start,
                        &block_offsets,
                        &mut missing_block_addrs,
                    );
                }
                I::seto => inst_r(text, &[0x0F, 0x90], ir.args(i, env), 0, false),
                I::setno => inst_r(text, &[0x0F, 0x91], ir.args(i, env), 0, false),
                I::setc => inst_r(text, &[0x0F, 0x92], ir.args(i, env), 0, false),
                I::setnc => inst_r(text, &[0x0F, 0x93], ir.args(i, env), 0, false),
                I::sete => inst_r(text, &[0x0F, 0x94], ir.args(i, env), 0, false),
                I::setne => inst_r(text, &[0x0F, 0x95], ir.args(i, env), 0, false),
                I::setbe => inst_r(text, &[0x0F, 0x96], ir.args(i, env), 0, false),
                I::seta => inst_r(text, &[0x0F, 0x97], ir.args(i, env), 0, false),
                I::sets => inst_r(text, &[0x0F, 0x98], ir.args(i, env), 0, false),
                I::setns => inst_r(text, &[0x0F, 0x99], ir.args(i, env), 0, false),
                I::setp => inst_r(text, &[0x0F, 0x9A], ir.args(i, env), 0, false),
                I::setnp => inst_r(text, &[0x0F, 0x9B], ir.args(i, env), 0, false),
                I::setl => inst_r(text, &[0x0F, 0x9C], ir.args(i, env), 0, false),
                I::setge => inst_r(text, &[0x0F, 0x9D], ir.args(i, env), 0, false),
                I::setle => inst_r(text, &[0x0F, 0x9E], ir.args(i, env), 0, false),
                I::setg => inst_r(text, &[0x0F, 0x9F], ir.args(i, env), 0, false),

                I::add_rr8 | I::add_rr16 | I::add_rr32 | I::add_rr64 => {
                    inst_rr(text, op, &[0x00], &[0x01], ir.args(i, env));
                }
                I::add_ri8 | I::add_ri16 | I::add_ri32 | I::add_ri64 => {
                    inst_ri(text, op, &[0x80], &[0x81], ir.args(i, env), 0);
                }

                I::sub_rr8 => inst_rr_legacy(text, &[0x28], ir.args(i, env), false),
                I::sub_rr16 | I::sub_rr32 | I::sub_rr64 => {
                    if op == I::sub_rr16 {
                        text.push(P16);
                    }
                    inst_rr_legacy(text, &[0x29], ir.args(i, env), op == I::sub_rr64);
                }
                I::sub_ri8 | I::sub_ri16 | I::sub_ri32 | I::sub_ri64 => {
                    inst_ri(text, op, &[0x80], &[0x81], ir.args(i, env), 5)
                }
                I::imul_r8 => inst_r(text, &[0xF6], ir.args(i, env), 5, false),
                I::imul_rr16 | I::imul_rr32 | I::imul_rr64 => {
                    if op == I::imul_rr16 {
                        text.push(P16);
                    }
                    inst_rr_legacy(text, &[0x0F, 0xAF], ir.args(i, env), op == I::imul_rr64)
                }
                I::imul_rri16 | I::imul_rri32 | I::imul_rri64 => {
                    inst_rri(text, op.size(), &[], &[0x69], ir.args(i, env));
                }
                I::shl_ri8
                | I::shl_ri16
                | I::shl_ri32
                | I::shl_ri64
                | I::shr_ri8
                | I::shr_ri16
                | I::shr_ri32
                | I::shr_ri64 => {
                    let is_left =
                        matches!(op, I::shl_ri8 | I::shl_ri16 | I::shl_ri32 | I::shl_ri64);
                    let size = op.size();
                    if size == Size::S16 {
                        text.push(P16);
                    }
                    let (r, imm): (Reg, u32) = ir.args(i, env);
                    // shr always uses an 8-bit imm
                    let imm = imm as u8; // just truncating upper bits of shr here is fine
                    let modrm = encode_modrm_ri(r, size == Size::S64, if is_left { 4 } else { 5 });
                    if modrm.rex != 0 {
                        text.push(modrm.rex);
                    }
                    text.extend([if size == Size::S8 { 0xC0 } else { 0xC1 }, modrm.modrm, imm]);
                }
                I::neg_r8 => inst_r(text, &[0xF6], ir.args(i, env), 3, false),
                I::neg_r16 | I::neg_r32 | I::neg_r64 => {
                    if op == I::neg_r16 {
                        text.push(P16);
                    }
                    inst_r(text, &[0xF7], ir.args(i, env), 3, op == I::neg_r64);
                }
                I::xor_ri8 | I::xor_ri16 | I::xor_ri32 | I::xor_ri64 => {
                    inst_ri(text, op, &[0x80], &[0x81], ir.args(i, env), 6)
                }
                I::xor_rr8 | I::xor_rr16 | I::xor_rr32 | I::xor_rr64 => {
                    inst_rr(text, op, &[0x30], &[0x31], ir.args(i, env))
                }
                I::lea_rm32 | I::lea_rm64 => {
                    let opcode: &[u8] = &[0x8D];
                    let (reg_val, reg_ptr, off) = ir.args(i, env);
                    let wide = op == I::lea_rm64;
                    let off = OffsetClass::from_imm(off);
                    let a = encode_reg(reg_val);
                    let b = encode_reg(reg_ptr);
                    let rex = encode_rex(wide, a.ext(), false, b.ext(), a.force() || b.force());
                    if rex != 0 {
                        text.push(rex);
                    }
                    text.extend(opcode);
                    modrm_rm(text, off, a, b);
                }
                I::lea_function => {
                    let (dst, function) = ir.args(i, env);
                    // lea dst [rip + offset]
                    let relocation_offset = disp32(text, &[0x8D], dst);
                    relocations.push((function, relocation_offset));
                }
                I::lea_global => {
                    let (dst, global) = ir.args(i, env);
                    // lea dst [rip + offset]
                    let relocation_offset = disp32(text, &[0x8D], dst);
                    global_relocations.push((global, relocation_offset));
                }
                I::call_function => {
                    relocations.push((ir.args(i, env), text.len() as u64 + 1));
                    text.extend([0xE8, 0, 0, 0, 0]);
                }
                I::call_r64 => {
                    let r = ir.args(i, env);
                    let ra = encode_reg(r);
                    let rex = encode_rex(false, false, false, ra.ext(), ra.force());
                    debug_assert!(!(ra.prevents_rex() && rex != 0));
                    if rex != 0 {
                        text.push(rex);
                    }
                    text.extend([0xFF, MODRM_RR | (2 << 3) | ra.bits]);
                }
                I::movsx16_rr8 | I::movsx32_rr8 | I::movsx64_rr8 => {
                    let size = match op {
                        I::movsx16_rr8 => Size::S16,
                        I::movsx32_rr8 => Size::S32,
                        _ => Size::S64,
                    };
                    inst_rr_generic_inner(text, size, &[], &[0x0F, 0xBE], ir.args(i, env));
                }
                I::movsx32_rr16 | I::movsx64_rr16 => {
                    let size = if op == I::movsx32_rr16 {
                        Size::S32
                    } else {
                        Size::S64
                    };
                    inst_rr_generic_inner(text, size, &[], &[0x0F, 0xBF], ir.args(i, env));
                }
                I::movsx64_rr32 => {
                    inst_rr_generic_inner(text, Size::S64, &[], &[0x63], ir.args(i, env));
                }
                I::movzx16_rr8 | I::movzx32_rr8 => {
                    let size = if op == I::movzx16_rr8 {
                        Size::S16
                    } else {
                        Size::S32
                    };
                    inst_rr_generic_inner(text, size, &[], &[0x0F, 0xB6], ir.args(i, env));
                }
                I::movzx32_rr16 => {
                    inst_rr_generic_inner(text, Size::S32, &[], &[0x0F, 0xB7], ir.args(i, env));
                }
            }
        }
    }
    for (offset_location, block) in missing_block_addrs {
        let block_offset = block_offsets[block.idx()].unwrap();
        let offset: i32 = (block_offset as i64 - offset_location as i64 - 4)
            .try_into()
            .unwrap();
        let i = start + offset_location as usize;
        text[i..i + 4].copy_from_slice(&offset.to_le_bytes());
    }
}

fn mov_rr(text: &mut Vec<u8>, size: Size, (a, b): (Reg, Reg)) {
    inst_rr_generic_inner(text, size, &[0x88], &[0x89], (a, b))
}

fn modrm_rm(text: &mut Vec<u8>, mut offset: OffsetClass, reg: EncodedReg, rm: EncodedReg) {
    if rm.bits == 0b101 && matches!(offset, OffsetClass::Zero) {
        // becomes [disp32] otherwise
        offset = OffsetClass::Byte(0);
    }
    text.push(offset.modrm_bits() | (reg.bits << 3) | rm.bits);
    if rm.bits == 0b100 {
        // sib byte
        text.push(0x24);
    }
    offset.write(text);
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

fn inst_r(text: &mut Vec<u8>, opcode: &[u8], a: Reg, extension: u8, wide: bool) {
    let modrm = encode_modrm_r(a, wide, extension);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(opcode);
    text.push(modrm.modrm);
}

fn inst_rr_legacy(text: &mut Vec<u8>, opcode: &[u8], (a, b): (Reg, Reg), wide: bool) {
    let modrm = encode_modrm_rr(a, b, wide);
    if modrm.rex != 0 {
        text.push(modrm.rex);
    }
    text.extend(opcode);
    text.push(modrm.modrm);
}

fn inst_rr(text: &mut Vec<u8>, inst: X86, opcode_8: &[u8], opcode: &[u8], (a, b): (Reg, Reg)) {
    inst_rr_generic_inner(text, inst.size(), opcode_8, opcode, (a, b));
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
    (reg_val, reg_ptr, off): (Reg, Reg, Int32),
) {
    if size == Size::S16 {
        text.push(P16);
    }
    let off = OffsetClass::from_imm(off);
    let a = encode_reg(reg_val);
    let b = encode_reg(reg_ptr);
    let rex = encode_rex(
        size == Size::S64,
        a.ext(),
        false,
        b.ext(),
        a.force() || b.force(),
    );
    debug_assert!(!((a.prevents_rex() || b.prevents_rex()) && rex != 0));
    if rex != 0 {
        text.push(rex);
    }
    text.extend(if size == Size::S8 { opcode_8 } else { opcode });
    modrm_rm(text, off, a, b);
}

fn inst_mr(
    text: &mut Vec<u8>,
    size: Size,
    opcode_8: &[u8],
    opcode: &[u8],
    (reg_ptr, off, reg_val): (Reg, Int32, Reg),
) {
    // encoded exactly the same way, just swap the arguments around correctly
    inst_rm(text, size, opcode_8, opcode, (reg_val, reg_ptr, off));
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
    (a, b, imm): (Reg, Reg, u32),
) {
    if size == Size::S16 {
        text.push(P16);
    }
    let modrm = encode_modrm_rr(a, b, size == Size::S64);
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

fn emit_jmp(
    rel8_op: &[u8],
    rel32_op: &[u8],
    target: BlockId,
    text: &mut Vec<u8>,
    start: usize,
    block_offsets: &[Option<u32>],
    missing_block_addrs: &mut Vec<(u32, BlockId)>,
) {
    let my_rel8_offset = (text.len() + rel8_op.len() - start + 1) as u32;
    if let Some(known) = block_offsets[target.idx()] {
        let offset: i32 = (known as i64 - my_rel8_offset as i64).try_into().unwrap();
        let offset8: Result<i8, _> = offset.try_into();
        if let Ok(offset8) = offset8 {
            text.extend(rel8_op);
            text.push(offset8 as u8);
        } else {
            text.extend(rel32_op);
            let offset: i32 = (known as i64 - (text.len() - start + 4) as i64)
                .try_into()
                .unwrap();
            text.extend(offset.to_le_bytes());
        }
    } else {
        text.extend(rel32_op);
        let offset = (text.len() - start) as u32;
        missing_block_addrs.push((offset, target));
        text.extend(0i32.to_le_bytes());
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

fn encode_modrm_ri(reg: Reg, wide: bool, i: u8) -> Modrm {
    debug_assert!(i < 8);
    let r = encode_reg(reg);
    debug_assert!(!(r.prevents_rex() && wide));
    Modrm {
        rex: encode_rex(wide, r.ext(), false, false, r.force()),
        modrm: MODRM_RR | i << 3 | r.bits,
    }
}
