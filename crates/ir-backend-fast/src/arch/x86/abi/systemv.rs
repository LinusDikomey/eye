use std::convert::Infallible;

use ir::{
    Argument, BlockId, Environment, MCReg, ModuleOf, Primitive, Ref, Type, TypeId, Types, mc::Mc,
    modify::IrModify, slots::Slots,
};

use crate::arch::x86::{
    X86,
    isa::{Reg, RegisterBits},
};

use super::Abi;

const ABI_PARAM_REGISTERS: [[Reg; 4]; 6] = [
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

pub struct SystemV;
impl Abi<X86> for SystemV {
    fn implement_params(
        &self,
        args: ir::Refs,
        ir: &mut ir::modify::IrModify,
        env: &Environment,
        mc: ModuleOf<Mc>,
        types: &Types,
        regs: &ir::slots::Slots<MCReg>,
        unit: TypeId,
    ) {
        let info = ir.get_block(BlockId::ENTRY);
        let before = Ref::index(info.args_idx + info.arg_count);
        let mut abi_regs = ABI_PARAM_REGISTERS.into_iter();
        for arg in args.iter() {
            _ = regs.visit_primitive_slots::<Infallible, _>(
                arg,
                types[ir.get_ref_ty(arg)],
                types,
                |regs, ty| {
                    let mut copy = |args| {
                        ir.add_before(env, before, ir::mc::parallel_copy(mc, args, unit));
                    };
                    match ty {
                        Primitive::I1 | Primitive::I8 | Primitive::U8 => {
                            copy(&[regs[0], MCReg::from_phys(abi_regs.next().unwrap()[3])]);
                        }
                        Primitive::I16 | Primitive::U16 => {
                            copy(&[regs[0], MCReg::from_phys(abi_regs.next().unwrap()[2])]);
                        }
                        Primitive::I32 | Primitive::U32 => {
                            copy(&[regs[0], MCReg::from_phys(abi_regs.next().unwrap()[1])]);
                        }
                        Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                            copy(&[regs[0], MCReg::from_phys(abi_regs.next().unwrap()[0])]);
                        }
                        Primitive::I128 | Primitive::U128 => {
                            copy(&[
                                regs[0],
                                MCReg::from_phys(abi_regs.next().unwrap()[0]),
                                regs[1],
                                MCReg::from_phys(abi_regs.next().unwrap()[1]),
                            ]);
                        }
                        Primitive::F32 | Primitive::F64 => todo!("SystemV float parameter abi"),
                    }
                    Ok(())
                },
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
        unit: TypeId,
    ) {
        let inst = ir.get_inst(call_inst);
        let function_id = ir::FunctionId {
            module: inst.module(),
            function: inst.function(),
        };
        let args = ir.args_iter(inst, env).map(|arg| {
            let Argument::Ref(r) = arg else {
                unreachable!("Call arguments should only be of type Ref");
            };
            r
        });
        let mut abi_regs = ABI_PARAM_REGISTERS.into_iter();
        let mut copies = Vec::new();
        for arg in args {
            _ = regs.visit_primitive_slots::<Infallible, _>(
                arg,
                types[ir.get_ref_ty(arg)],
                types,
                |regs, p| {
                    match p {
                        Primitive::I1 | Primitive::I8 | Primitive::U8 => {
                            copies.extend([MCReg::from_phys(abi_regs.next().unwrap()[3]), regs[0]]);
                        }
                        Primitive::I16 | Primitive::U16 => {
                            copies.extend([MCReg::from_phys(abi_regs.next().unwrap()[2]), regs[0]]);
                        }
                        Primitive::I32 | Primitive::U32 => {
                            copies.extend([MCReg::from_phys(abi_regs.next().unwrap()[1]), regs[0]]);
                        }
                        Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                            copies.extend([MCReg::from_phys(abi_regs.next().unwrap()[0]), regs[0]]);
                        }
                        Primitive::I128 | Primitive::U128 => {
                            copies.extend([
                                MCReg::from_phys(abi_regs.next().unwrap()[0]),
                                regs[0],
                                MCReg::from_phys(abi_regs.next().unwrap()[0]),
                                regs[1],
                            ]);
                        }
                        Primitive::F32 | Primitive::F64 => todo!("float arguments"),
                    };
                    Ok(())
                },
            );
        }
        if !copies.is_empty() {
            ir.add_before(env, call_inst, ir::mc::parallel_copy(mc, &copies, unit));
        }
        let return_ty = types[ir.get_ref_ty(call_inst)];
        let mut copy = |args| ir.add_after(env, call_inst, ir::mc::parallel_copy(mc, args, unit));
        match return_ty {
            Type::Primitive(p) => match Primitive::try_from(p).unwrap() {
                Primitive::I1 | Primitive::I8 | Primitive::U8 => {
                    copy(&[regs.get_one(call_inst), MCReg::from_phys(RETURN_REGS[0][3])]);
                }
                Primitive::I16 | Primitive::U16 => {
                    copy(&[regs.get_one(call_inst), MCReg::from_phys(RETURN_REGS[0][2])]);
                }
                Primitive::I32 | Primitive::U32 => {
                    copy(&[regs.get_one(call_inst), MCReg::from_phys(RETURN_REGS[0][1])]);
                }
                Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                    copy(&[regs.get_one(call_inst), MCReg::from_phys(RETURN_REGS[0][0])]);
                }
                Primitive::I128 | Primitive::U128 => {
                    let r = regs.get(call_inst);
                    copy(&[
                        r[0],
                        MCReg::from_phys(RETURN_REGS[0][0]),
                        r[1],
                        MCReg::from_phys(RETURN_REGS[1][0]),
                    ]);
                }
                Primitive::F32 | Primitive::F64 => todo!("float return types"),
            },
            Type::Tuple(elems) if elems.count() == 0 => {}
            _ => todo!("abi: aggregate return types"),
        }
        ir.replace(env, call_inst, x86.call_function(function_id));
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
        unit: TypeId,
    ) {
        if value == ir::Ref::UNIT {
            ir.replace(env, r, x86.ret0());
            return;
        };
        let ty = types[ir.get_ref_ty(value)];
        let mut copy = |args| ir.add_before(env, r, ir::mc::parallel_copy(mc, args, unit));
        match ty {
            Type::Primitive(p) => match Primitive::try_from(p).unwrap() {
                Primitive::I1 | Primitive::I8 | Primitive::U8 => {
                    copy(&[MCReg::from_phys(RETURN_REGS[0][3]), regs.get_one(value)]);
                    ir.replace(env, r, x86.ret64());
                }
                Primitive::I16 | Primitive::U16 => {
                    copy(&[MCReg::from_phys(RETURN_REGS[0][2]), regs.get_one(value)]);
                    ir.replace(env, r, x86.ret64());
                }
                Primitive::I32 | Primitive::U32 => {
                    copy(&[MCReg::from_phys(RETURN_REGS[0][1]), regs.get_one(value)]);
                    ir.replace(env, r, x86.ret64());
                }
                Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                    copy(&[MCReg::from_phys(RETURN_REGS[0][0]), regs.get_one(value)]);
                    ir.replace(env, r, x86.ret64());
                }
                Primitive::I128 | Primitive::U128 => {
                    let &[a, b] = regs.get(value) else {
                        unreachable!()
                    };
                    copy(&[
                        MCReg::from_phys(RETURN_REGS[0][0]),
                        a,
                        MCReg::from_phys(RETURN_REGS[1][0]),
                        b,
                    ]);
                    ir.replace(env, r, x86.ret128());
                }
                Primitive::F32 | Primitive::F64 => todo!("abi: float return values"),
            },
            Type::Array(_, _) => todo!("abi: array return values"),
            Type::Tuple(elems) if elems.is_empty() => ir.replace(env, r, x86.ret0()),
            Type::Tuple(_) => todo!("abi: tuple return values"),
        }
    }

    fn caller_saved(&self) -> <<X86 as ir::mc::McInst>::Reg as ir::mc::Register>::RegisterBits {
        CALLER_SAVED
            .iter()
            .fold(RegisterBits::new(), |a, b| a | b.bit())
    }

    fn callee_saved(&self) -> <<X86 as ir::mc::McInst>::Reg as ir::mc::Register>::RegisterBits {
        CALLEE_SAVED
            .iter()
            .fold(RegisterBits::new(), |a, b| a | b.bit())
    }

    fn return_regs(
        &self,
        value_count: u32,
    ) -> <<X86 as ir::mc::McInst>::Reg as ir::mc::Register>::RegisterBits {
        RETURN_REGS[0..value_count as usize]
            .iter()
            .fold(RegisterBits::new(), |a, b| a | b[0].bit())
    }
}
