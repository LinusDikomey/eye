use std::convert::Infallible;

use ir::{
    Argument, BlockId, Environment, MCReg, ModuleOf, Primitive, PrimitiveInfo, Ref, Type, Types,
    mc::{Mc, parallel_copy},
    modify::{Insert, IrModify},
    slots::Slots,
};

use crate::arch::x86::{
    X86,
    isa::{Reg, RegBits},
};

use super::Abi;

const ABI_PARAM_REGISTERS_INTEGER: [[Reg; 4]; 6] = [
    [Reg::rdi, Reg::edi, Reg::di, Reg::dil],
    [Reg::rsi, Reg::esi, Reg::si, Reg::sil],
    [Reg::rdx, Reg::edx, Reg::dx, Reg::dl],
    [Reg::rcx, Reg::ecx, Reg::cx, Reg::cl],
    [Reg::r8, Reg::r8d, Reg::r8w, Reg::r8b],
    [Reg::r9, Reg::r9d, Reg::r9w, Reg::r9b],
];
const RETURN_REGS: [[Reg; 4]; 2] = [
    [Reg::rax, Reg::eax, Reg::ax, Reg::al],
    [Reg::rdx, Reg::edx, Reg::dx, Reg::dl],
];

const CALLER_SAVED: [Reg; 9] = [
    Reg::rax,
    Reg::rcx,
    Reg::rdx,
    Reg::rsi,
    Reg::rdi,
    Reg::r8,
    Reg::r9,
    Reg::r10,
    Reg::r11,
];

const CALLEE_SAVED: [Reg; 7] = [
    Reg::rbx,
    Reg::rbp,
    Reg::rsp,
    Reg::r12,
    Reg::r13,
    Reg::r14,
    Reg::r15,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum AbiClass {
    None,
    Integer,
    Sse,
    Memory,
}
impl AbiClass {
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Memory, _) | (_, Self::Memory) => Self::Memory,
            (Self::Integer, _) | (_, Self::Integer) => Self::Integer,
            _ if self == other => self,
            _ => Self::Sse,
        }
    }
}
fn classify_primitive(p: Primitive) -> AbiClass {
    match p {
        Primitive::I1
        | Primitive::I8
        | Primitive::I16
        | Primitive::I32
        | Primitive::I64
        | Primitive::U8
        | Primitive::U16
        | Primitive::U32
        | Primitive::U64
        | Primitive::Ptr => AbiClass::Integer,
        Primitive::F32 | Primitive::F64 => AbiClass::Sse,
        Primitive::I128 | Primitive::U128 => AbiClass::Integer,
    }
}

fn classify(types: &Types, ty: Type, primitives: &[PrimitiveInfo]) -> [AbiClass; 2] {
    let layout = ir::type_layout(ty, types, primitives);
    match layout.size {
        0..=8 => {
            let mut class = AbiClass::None;
            ir::visit_primitives(ty, types, primitives, |p, _| {
                class = class.join(classify_primitive(Primitive::try_from(p).unwrap()));
            });
            [class, AbiClass::None]
        }
        9..=16 => {
            let mut a = AbiClass::None;
            let mut b = AbiClass::None;
            ir::visit_primitives(ty, types, primitives, |p, offset| {
                let class = classify_primitive(Primitive::try_from(p).unwrap());
                if offset < 8 {
                    a = a.join(class);
                } else {
                    b = b.join(class);
                }
            });
            if a == AbiClass::Memory || b == AbiClass::Memory {
                return [AbiClass::Memory, AbiClass::None];
            }
            [a, b]
        }
        _ => [AbiClass::Memory, AbiClass::None],
    }
}
pub struct SystemV;
impl Abi<X86> for SystemV {
    fn implement_params(
        &self,
        args: ir::Refs,
        ir: &mut IrModify,
        env: &Environment,
        mc: ModuleOf<Mc>,
        x86: ModuleOf<X86>,
        types: &Types,
        regs: &ir::slots::Slots<MCReg>,
    ) {
        let info = ir.get_block(BlockId::ENTRY);
        let before = Ref::index(info.body_idx);
        let mut integer_regs = ABI_PARAM_REGISTERS_INTEGER.into_iter();
        for arg in args.iter() {
            let [a, b] = classify(types, types[ir.get_ref_ty(arg)], env.primitives());
            let a_reg = match a {
                AbiClass::None => None,
                AbiClass::Integer => integer_regs.next().map(|r| r[0]),
                AbiClass::Sse => todo!("float params"),
                AbiClass::Memory => todo!("handle memory params"),
            };
            let b_reg = match b {
                AbiClass::None => None,
                AbiClass::Integer => integer_regs.next().map(|r| r[0]),
                AbiClass::Sse => todo!("float params"),
                AbiClass::Memory => unreachable!(),
            };
            extract_regs(
                ir,
                env,
                mc,
                x86,
                types,
                regs,
                before,
                Insert::Before,
                arg,
                a_reg,
                b_reg,
            );
        }
    }

    fn implement_call<'a>(
        &self,
        call_inst: Ref,
        ir: &mut IrModify,
        env: &Environment,
        mc: ModuleOf<Mc>,
        x86: ModuleOf<X86>,
        types: &Types,
        regs: &Slots<MCReg>,
        skip_first_arg: bool,
    ) {
        // first insert values into call registers
        {
            let inst = ir.get_inst(call_inst);
            let varargs = env[inst.module()][inst.function()].varargs().is_some();
            let args = ir
                .args_iter(inst, env)
                .map(|arg| {
                    let Argument::Ref(r) = arg else {
                        unreachable!("Call arguments should only be of type Ref");
                    };
                    r
                })
                .skip(skip_first_arg as usize);
            let mut integer_regs = ABI_PARAM_REGISTERS_INTEGER.into_iter();
            // PERF: collecting here to not borrow ir
            let args: Box<[Ref]> = args.collect();
            for arg in args {
                let classify = classify(types, types[ir.get_ref_ty(arg)], env.primitives());
                let [a, b] = classify;
                let a_reg = match a {
                    AbiClass::None => None,
                    AbiClass::Integer => integer_regs.next().map(|r| r[0]),
                    AbiClass::Sse => todo!("float params"),
                    AbiClass::Memory => todo!("handle memory params"),
                };
                let b_reg = match b {
                    AbiClass::None => None,
                    AbiClass::Integer => integer_regs.next().map(|r| r[0]),
                    AbiClass::Sse => todo!("float params"),
                    AbiClass::Memory => unreachable!(),
                };
                insert_regs(call_inst, ir, env, mc, x86, types, regs, arg, a_reg, b_reg);
            }
            if varargs {
                // al stores the number of vector registers used, zero it out since we don't currently
                // support them
                ir.add_before(
                    env,
                    call_inst,
                    x86.xor_rr32(MCReg::from_phys(Reg::eax), MCReg::from_phys(Reg::eax)),
                );
            }
        }

        // extract return value into value registers after the call
        {
            let mut integer_regs = RETURN_REGS.into_iter();
            let return_ty = types[ir.get_ref_ty(call_inst)];
            let [a, b] = classify(types, return_ty, env.primitives());
            let a_reg = match a {
                AbiClass::None => None,
                AbiClass::Integer => Some(integer_regs.next().unwrap()[0]),
                AbiClass::Sse => todo!("float params"),
                AbiClass::Memory => todo!("handle memory params"),
            };
            let b_reg = match b {
                AbiClass::None => None,
                AbiClass::Integer => Some(integer_regs.next().unwrap()[0]),
                AbiClass::Sse => todo!("float params"),
                AbiClass::Memory => unreachable!(),
            };
            extract_regs(
                ir,
                env,
                mc,
                x86,
                types,
                regs,
                call_inst,
                Insert::After,
                call_inst,
                a_reg,
                b_reg,
            );
        }
    }

    fn implement_return(
        &self,
        value: ir::Ref,
        ir: &mut ir::modify::IrModify,
        env: &Environment,
        mc: ModuleOf<Mc>,
        x86: ModuleOf<X86>,
        types: &Types,
        regs: &ir::slots::Slots<MCReg>,
        r: ir::Ref,
    ) {
        if value == ir::Ref::UNIT {
            ir.replace(env, r, x86.ret0());
            return;
        };
        let classify = classify(types, types[ir.get_ref_ty(value)], env.primitives());
        let [a, b] = classify;
        let mut integer_regs = RETURN_REGS.iter();
        let a_reg = match a {
            AbiClass::None => None,
            AbiClass::Integer => Some(integer_regs.next().unwrap()[0]),
            AbiClass::Sse => todo!("float params"),
            AbiClass::Memory => todo!("handle memory params"),
        };
        if b == AbiClass::None {
            ir.replace(env, r, x86.ret64());
        } else {
            ir.replace(env, r, x86.ret128());
        }
        let b_reg = match b {
            AbiClass::None => None,
            AbiClass::Integer => Some(integer_regs.next().unwrap()[0]),
            AbiClass::Sse => todo!("float params"),
            AbiClass::Memory => unreachable!(),
        };
        insert_regs(r, ir, env, mc, x86, types, regs, value, a_reg, b_reg);
    }

    fn caller_saved(&self) -> <<X86 as ir::mc::McInst>::Reg as ir::mc::Register>::RegisterBits {
        CALLER_SAVED.iter().fold(RegBits::new(), |a, b| a | b.bit())
    }

    fn callee_saved(&self) -> <<X86 as ir::mc::McInst>::Reg as ir::mc::Register>::RegisterBits {
        CALLEE_SAVED.iter().fold(RegBits::new(), |a, b| a | b.bit())
    }

    fn return_regs(
        &self,
        value_count: u32,
    ) -> <<X86 as ir::mc::McInst>::Reg as ir::mc::Register>::RegisterBits {
        RETURN_REGS[0..value_count as usize]
            .iter()
            .fold(RegBits::new(), |a, b| a | b[0].bit())
    }
}

fn extract_regs(
    ir: &mut IrModify,
    env: &Environment,
    mc: ModuleOf<Mc>,
    x86: ModuleOf<X86>,
    types: &Types,
    regs: &Slots<MCReg>,
    insert_at: Ref,
    position: Insert,
    arg: Ref,
    a_reg: Option<Reg>,
    b_reg: Option<Reg>,
) {
    _ = regs.visit_primitive_slots::<Infallible, _>(
        arg,
        types[ir.get_ref_ty(arg)],
        types,
        env.primitives(),
        |regs, _ty, offset| {
            let (src, reg_offset) = if offset >= 8 {
                (b_reg, (offset - 8) as u8)
            } else {
                (a_reg, offset as u8)
            };
            let src = src.expect("Handle stack-passed params");
            match regs {
                [] => {}
                &[dst] => extract(ir, env, mc, x86, insert_at, position, dst, src, reg_offset),
                &[dst_a, dst_b] => {
                    // any 128-bit primitive has to be exactly at offset 0 since the param
                    // would have been passed in memory otherwise
                    debug_assert_eq!(offset, 0);
                    let a_reg = a_reg.expect("Handle stack-passed params");
                    let b_reg = b_reg.expect("Handle stack-passed params");
                    extract(ir, env, mc, x86, insert_at, position, dst_a, a_reg, 0);
                    extract(ir, env, mc, x86, insert_at, position, dst_b, b_reg, 0);
                }
                _ => unreachable!(), // no primitive should use more than 2 regs
            };
            Ok(())
        },
    );
}

fn extract(
    ir: &mut IrModify,
    env: &Environment,
    mc: ModuleOf<Mc>,
    x86: ModuleOf<X86>,
    insert_at: Ref,
    position: Insert,
    to: MCReg,
    from: Reg,
    byte_offset: u8,
) {
    if byte_offset == 0 {
        ir.add_before_or_after(
            env,
            insert_at,
            position,
            parallel_copy(mc, &[to, MCReg::from_phys(from)]),
        );
        return;
    }
    // always interpret the target register as 64 bits first, mov the src in and shift it down.
    // Further uses will only use the lower bits.
    // CODEGEN: could use a smaller mov in some cases if higher bits aren't needed. Should be a
    // pretty simple check but maybe only downgrading to mov32 is worth it
    ir.add_before_or_after(
        env,
        insert_at,
        position,
        x86.mov_rr64(to, MCReg::from_phys(from)),
    );
    ir.add_before_or_after(
        env,
        insert_at,
        position,
        x86.shr_ri64(to, (byte_offset * 8).into()),
    );
}

fn insert_regs(
    before: Ref,
    ir: &mut IrModify,
    env: &Environment,
    mc: ModuleOf<Mc>,
    x86: ModuleOf<X86>,
    types: &Types,
    regs: &Slots<MCReg>,
    value: Ref,
    a_reg: Option<Reg>,
    b_reg: Option<Reg>,
) {
    let mut first_inserted_a = true;
    let mut first_inserted_b = true;
    _ = regs.visit_primitive_slots::<Infallible, _>(
        value,
        types[ir.get_ref_ty(value)],
        types,
        env.primitives(),
        |regs, _p, offset| {
            let (dst, reg_offset, first) = if offset >= 8 {
                let first = first_inserted_b;
                first_inserted_b = false;
                (b_reg, (offset - 8) as u8, first)
            } else {
                let first = first_inserted_a;
                first_inserted_a = false;
                (a_reg, offset as u8, first)
            };
            let dst = dst.expect("Handle stack-passed params");
            match regs {
                [] => {}
                &[src] => insert(ir, env, mc, x86, before, dst, src, reg_offset, first),
                &[src_a, src_b] => {
                    // any 128-bit primitive has to be exactly at offset 0 since the param
                    // would have been passed in memory otherwise
                    debug_assert_eq!(offset, 0);
                    let a_reg = a_reg.expect("Handle stack-passed params");
                    let b_reg = b_reg.expect("Handle stack-passed params");
                    insert(ir, env, mc, x86, before, a_reg, src_a, 0, first_inserted_a);
                    insert(ir, env, mc, x86, before, b_reg, src_b, 0, first_inserted_b);
                    first_inserted_a = false;
                    first_inserted_b = false;
                }
                _ => unreachable!(), // no primitive should use more than 2 regs
            };
            Ok(())
        },
    );
}

fn insert(
    ir: &mut IrModify,
    env: &Environment,
    mc: ModuleOf<Mc>,
    x86: ModuleOf<X86>,
    before: Ref,
    to: Reg,
    from: MCReg,
    byte_offset: u8,
    first_inserted: bool,
) {
    let to_class = to.class();
    let to = MCReg::from_phys(to);
    if first_inserted {
        ir.add_before(env, before, x86.mov_rr64(to, from));
        if byte_offset != 0 {
            ir.add_before(env, before, x86.shl_ri64(to, byte_offset as u32 * 8));
        }
    } else {
        let shifted = if byte_offset != 0 {
            // we need to copy to a tmp variable, shift it to the right place and then or it in
            let tmp = ir.new_reg::<Reg>(to_class);
            ir.add_before(env, before, parallel_copy(mc, &[tmp, from]));
            ir.add_before(env, before, x86.shl_ri64(tmp, byte_offset as u32 * 8));
            tmp
        } else {
            // no shift needed, we can or it in directly
            from
        };
        ir.add_before(env, before, x86.or_rr64(to, shifted));
    }
}
