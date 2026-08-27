use std::convert::Infallible;

use ir::{
    Argument, BlockId, Environment, MCReg, ModuleOf, Primitive, PrimitiveInfo, Ref, Type, Types,
    mc::{Abi, Mc, parallel_copy},
    modify::{Insert, IrModify},
    slots::Slots,
};

use crate::arch::arm::isa::{Arm, Reg, RegBits, TMP_REGISTER};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ArmAbi {
    Arm32,
    Arm64,
    Darwin64,
}
impl Abi<Arm> for ArmAbi {
    fn implement_params(
        &self,
        args: ir::Refs,
        ir: &mut IrModify,
        env: &Environment,
        mc: ModuleOf<ir::mc::Mc>,
        i: ModuleOf<Arm>,
        types: &ir::Types,
        regs: &ir::slots::Slots<ir::MCReg>,
    ) {
        let before = Ref::index(ir.get_block(BlockId::ENTRY).body_idx);
        let mut param_alloc = ParamAllocator::new();
        for arg in args.iter() {
            let location = classify(types, types[ir.get_ref_ty(arg)], env.primitives());
            let storage = param_alloc.alloc(location, *self);
            extract_regs(
                ir,
                env,
                mc,
                i,
                types,
                regs,
                Insert::Before(before),
                arg,
                storage,
            );
        }
    }

    fn implement_call(
        &self,
        call_inst: Ref,
        ir: &mut IrModify,
        env: &Environment,
        mc: ModuleOf<ir::mc::Mc>,
        arm: ModuleOf<Arm>,
        types: &ir::Types,
        regs: &ir::slots::Slots<ir::MCReg>,
        skip_first_arg: bool,
    ) {
        let inst = ir.get_inst(call_inst);
        let args = ir
            .args_iter(inst, env)
            .skip(skip_first_arg as usize)
            .map(|arg| {
                let Argument::Ref(r) = arg else {
                    unreachable!()
                };
                r
            });
        // PERF: collecting here to not borrow ir
        let args: Box<[Ref]> = args.collect();
        let mut alloc = ParamAllocator::new();
        for arg in args {
            let location = classify(types, types[ir.get_ref_ty(arg)], env.primitives());
            let storage = alloc.alloc(location, *self);
            insert_regs(
                ir,
                env,
                mc,
                arm,
                types,
                regs,
                Insert::Before(call_inst),
                arg,
                storage,
            );
        }
        let ret_location = classify(types, types[ir.get_ref_ty(call_inst)], env.primitives());
        let storage = ParamAllocator::new().alloc(ret_location, *self);
        extract_regs(
            ir,
            env,
            mc,
            arm,
            types,
            regs,
            Insert::After(call_inst),
            call_inst,
            storage,
        );
    }

    fn implement_return(
        &self,
        value: Ref,
        ir: &mut IrModify,
        env: &Environment,
        mc: ModuleOf<ir::mc::Mc>,
        arm: ModuleOf<Arm>,
        types: &ir::Types,
        regs: &ir::slots::Slots<ir::MCReg>,
        r: Ref,
    ) {
        let location = classify(types, types[ir.get_ref_ty(value)], env.primitives());
        let mut alloc = ParamAllocator::new();
        let storage = alloc.alloc(location, *self);
        insert_regs(
            ir,
            env,
            mc,
            arm,
            types,
            regs,
            Insert::Before(r),
            value,
            storage,
        );
        ir.replace(env, r, arm.ret0(MCReg::from_phys(Reg::x30)));
    }

    fn caller_saved(&self) -> RegBits {
        CALLER_SAVED.iter().fold(RegBits::new(), |a, b| a | b.bit())
    }

    fn callee_saved(&self) -> RegBits {
        CALLEE_SAVED.iter().fold(RegBits::new(), |a, b| a | b.bit())
    }

    fn return_regs(&self, value_count: u32) -> RegBits {
        RETURN_REGS[0..value_count as usize]
            .iter()
            .fold(RegBits::new(), |a, b| a | b[0].bit())
    }
}
impl ArmAbi {
    pub fn preoccupied_regs(self) -> RegBits {
        let mut bits = Reg::sp.bit() | FRAME_POINTER.bit() | TMP_REGISTER.bit();
        if self == Self::Darwin64 {
            // reserved on darwin
            bits = bits | Reg::x18.bit();
        }
        bits
    }
}

pub const CALL_STACK_ALIGN: u64 = 16;

const ABI_PARAM_REGISTERS_INTEGER: [[Reg; 2]; 8] = [
    [Reg::x0, Reg::w0],
    [Reg::x1, Reg::w1],
    [Reg::x2, Reg::w2],
    [Reg::x3, Reg::x3],
    [Reg::x4, Reg::w4],
    [Reg::x5, Reg::w5],
    [Reg::x6, Reg::w6],
    [Reg::x7, Reg::w7],
];

const ABI_PARAM_REGISTERS_SIMD: [[Reg; 2]; 8] = [
    [Reg::d0, Reg::s0],
    [Reg::d1, Reg::s1],
    [Reg::d2, Reg::s2],
    [Reg::d3, Reg::s3],
    [Reg::d4, Reg::s4],
    [Reg::d5, Reg::s5],
    [Reg::d6, Reg::s6],
    [Reg::d7, Reg::s7],
];

pub const FRAME_POINTER: Reg = Reg::x29;

const RETURN_REGS: [[Reg; 2]; 2] = [[Reg::x0, Reg::w0], [Reg::x1, Reg::w1]];

#[derive(Default)]
struct ParamAllocator {
    /// next general register number
    ngrn: u8,
    /// next simd (and floating point) register number
    nsrn: u8,
    // nprn: u8,
    /// next stack argument address
    nsaa: u32,
}
impl ParamAllocator {
    fn new() -> Self {
        Self::default()
    }

    fn alloc(&mut self, location: Location, target: ArmAbi) -> Storage {
        match location {
            Location::Registers {
                classes: [class, AbiClass::None],
                align_16: _,
            } => {
                let reg = match class {
                    AbiClass::None => Reg::xzr,
                    AbiClass::Core(width) => {
                        if self.ngrn == 8 {
                            todo!("stack params")
                        }
                        let reg = ABI_PARAM_REGISTERS_INTEGER[self.ngrn as usize][width as usize];
                        self.ngrn += 1;
                        reg
                    }
                    AbiClass::SimdFp(width) => {
                        if self.nsrn == 8 {
                            todo!("stack params")
                        }
                        let reg = ABI_PARAM_REGISTERS_SIMD[self.nsrn as usize][width as usize];
                        self.nsrn += 1;
                        reg
                    }
                };
                Storage::Registers([reg, Reg::xzr])
            }
            Location::Registers {
                classes: [a, b],
                align_16,
            } => {
                std::debug_assert_matches!(
                    (a, b),
                    (AbiClass::Core(_), AbiClass::Core(_)),
                    "Should only have 2 core classes for larger types/aggregates"
                );
                if align_16 && target != ArmAbi::Darwin64 {
                    // align up to next even register number
                    self.ngrn = self.ngrn.next_multiple_of(2);
                }
                if self.ngrn >= 7 {
                    todo!("stack params")
                }
                let regs = [
                    ABI_PARAM_REGISTERS_INTEGER[self.ngrn as usize][0],
                    ABI_PARAM_REGISTERS_INTEGER[self.ngrn as usize + 1][0],
                ];
                self.ngrn += 2;
                Storage::Registers(regs)
            }
            Location::HfaF32 { count } | Location::HfaF64 { count } => {
                let size_index = matches!(location, Location::HfaF32 { .. }) as usize;
                if self.nsrn + count < 8 {
                    let mut regs = [Reg::xzr; 4];
                    for i in 0..count {
                        regs[i as usize] = ABI_PARAM_REGISTERS_SIMD[self.nsrn as usize][size_index];
                        self.nsrn += 1;
                    }
                    Storage::Hfa(regs)
                } else {
                    self.nsrn = 8;
                    let size = if matches!(location, Location::HfaF32 { .. }) {
                        4
                    } else {
                        8
                    };
                    self.nsaa = self.nsaa.next_multiple_of(size);
                    let i = self.nsaa;
                    self.nsaa += size;
                    Storage::Stack(i)
                }
            }
            Location::Memory => todo!("memory params"),
        }
    }
}

fn extract_regs(
    ir: &mut IrModify,
    env: &Environment,
    mc: ModuleOf<Mc>,
    arm: ModuleOf<Arm>,
    types: &Types,
    regs: &Slots<MCReg>,
    position: Insert,
    arg: Ref,
    storage: Storage,
) {
    let ty = types[ir.get_ref_ty(arg)];
    match storage {
        Storage::Registers([a, b]) => {
            regs.visit_primitive_slots::<Infallible, _>(
                arg,
                ty,
                types,
                env.primitives(),
                |regs, _ty, offset| {
                    match regs {
                        [] => {}
                        &[dst] => {
                            let (src, reg_offset) = if offset >= 8 {
                                (b, (offset - 8) as u8)
                            } else {
                                (a, offset as u8)
                            };
                            extract(ir, env, mc, arm, position, dst, src, reg_offset);
                        }
                        &[dst_a, dst_b] => {
                            debug_assert_eq!(offset, 0);
                            ir.add_before_or_after(
                                env,
                                position,
                                parallel_copy(
                                    mc,
                                    &[dst_a, MCReg::from_phys(a), dst_b, MCReg::from_phys(b)],
                                ),
                            );
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                },
            );
        }
        Storage::Hfa(hfa) => {
            let mut hfa = hfa.into_iter();
            // PERF: vec allocation
            let mut copies = Vec::new();
            _ = regs.visit_primitive_slots::<Infallible, _>(
                arg,
                ty,
                types,
                env.primitives(),
                |regs, ty, _| {
                    let &[to] = regs else { unreachable!() };
                    let from = hfa.next().unwrap();
                    copies.extend([to, MCReg::from_phys(from)]);
                    debug_assert!(ty.is_float());
                    hfa.next();
                    Ok(())
                },
            );
            ir.add_before_or_after(env, position, parallel_copy(mc, &copies));
        }
        Storage::Stack(_) => todo!(),
    }
}

fn extract(
    ir: &mut IrModify,
    env: &Environment,
    mc: ModuleOf<Mc>,
    _arm: ModuleOf<Arm>,
    position: Insert,
    to: MCReg,
    from: Reg,
    byte_offset: u8,
) {
    if byte_offset == 0 {
        ir.add_before_or_after(
            env,
            position,
            parallel_copy(mc, &[to, MCReg::from_phys(from)]),
        );
        return;
    }
    todo!("offset abi extraction")
}

fn insert_regs(
    ir: &mut IrModify,
    env: &Environment,
    mc: ModuleOf<Mc>,
    arm: ModuleOf<Arm>,
    types: &Types,
    regs: &Slots<MCReg>,
    position: Insert,
    value: Ref,
    storage: Storage,
) {
    if value == Ref::UNIT {
        return;
    }
    let Storage::Registers([dst_a, dst_b]) = storage else {
        todo!()
    };
    let mut first_inserted_a = true;
    let mut first_inserted_b = true;
    regs.visit_primitive_slots::<Infallible, _>(
        value,
        types[ir.get_ref_ty(value)],
        types,
        env.primitives(),
        |regs, _p, offset| {
            match regs {
                [] => {}
                &[src] => {
                    let first;
                    let (dst, reg_offset) = if offset >= 8 {
                        first = first_inserted_b;
                        first_inserted_b = false;
                        (dst_b, (offset - 8) as u8)
                    } else {
                        first = first_inserted_a;
                        first_inserted_a = false;
                        (dst_a, offset as u8)
                    };
                    insert(ir, env, mc, arm, position, dst, src, reg_offset, first)
                }
                &[src_a, src_b] => {
                    debug_assert_eq!(offset, 0);
                    debug_assert!(first_inserted_a && first_inserted_b);
                    ir.add_before_or_after(
                        env,
                        position,
                        parallel_copy(
                            mc,
                            &[
                                MCReg::from_phys(dst_a),
                                src_a,
                                MCReg::from_phys(dst_b),
                                src_b,
                            ],
                        ),
                    );
                }
                _ => unreachable!(),
            }
            Ok(())
        },
    );
}

fn insert(
    ir: &mut IrModify,
    env: &Environment,
    mc: ModuleOf<Mc>,
    _arm: ModuleOf<Arm>,
    position: Insert,
    dst: Reg,
    src: MCReg,
    reg_offset: u8,
    first: bool,
) {
    let dst = MCReg::from_phys(dst);
    if !first || reg_offset != 0 {
        todo!()
    }
    ir.add_before_or_after(env, position, parallel_copy(mc, &[dst, src]));
}

enum Storage {
    Registers([Reg; 2]),
    Hfa([Reg; 4]),
    Stack(u32),
}

const CALLER_SAVED: [Reg; 18] = [
    Reg::x0,
    Reg::x1,
    Reg::x2,
    Reg::x3,
    Reg::x4,
    Reg::x5,
    Reg::x6,
    Reg::x7,
    Reg::x8,
    Reg::x9,
    Reg::x10,
    Reg::x11,
    Reg::x12,
    Reg::x13,
    Reg::x14,
    Reg::x15,
    Reg::x16,
    Reg::x17,
];

const CALLEE_SAVED: [Reg; 10] = [
    Reg::x19,
    Reg::x20,
    Reg::x21,
    Reg::x22,
    Reg::x23,
    Reg::x24,
    Reg::x25,
    Reg::x26,
    Reg::x27,
    Reg::x28,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AbiClass {
    None,
    Core(RegWidth),
    SimdFp(RegWidth),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// used to index into abi register lists to select the correct width
enum RegWidth {
    W64 = 0,
    W32 = 1,
}

enum Location {
    Registers {
        classes: [AbiClass; 2],
        align_16: bool,
    },
    HfaF32 {
        count: u8,
    },
    HfaF64 {
        count: u8,
    },
    Memory,
}

fn classify(types: &Types, ty: Type, primitives: &[PrimitiveInfo]) -> Location {
    match ty {
        Type::Primitive(p) => match Primitive::try_from(p).unwrap() {
            Primitive::I1
            | Primitive::I8
            | Primitive::I16
            | Primitive::I32
            | Primitive::U8
            | Primitive::U16
            | Primitive::U32 => Location::Registers {
                classes: [AbiClass::Core(RegWidth::W32), AbiClass::None],
                align_16: false,
            },
            Primitive::I64 | Primitive::U64 | Primitive::Ptr => Location::Registers {
                classes: [AbiClass::Core(RegWidth::W64), AbiClass::None],
                align_16: false,
            },
            Primitive::I128 | Primitive::U128 => Location::Registers {
                classes: [AbiClass::Core(RegWidth::W64); 2],
                align_16: true,
            },
            Primitive::F32 => Location::Registers {
                classes: [AbiClass::SimdFp(RegWidth::W32), AbiClass::None],
                align_16: false,
            },
            Primitive::F64 => Location::Registers {
                classes: [AbiClass::SimdFp(RegWidth::W64), AbiClass::None],
                align_16: false,
            },
        },
        _ => {
            let mut core: u64 = 0;
            let mut f32s: u64 = 0;
            let mut f64s: u64 = 0;
            // check for HFA
            // PERF: this keeps scanning and should really just bail once any core primitive
            // is found
            ir::visit_primitives(ty, types, primitives, |p, _| {
                match Primitive::try_from(p).unwrap() {
                    Primitive::F32 => f32s += 1,
                    Primitive::F64 => f64s += 1,
                    _ => core += 1,
                }
            });
            match (core, f32s, f64s) {
                (0, count @ ..=4, 0) => Location::HfaF32 { count: count as _ },
                (0, 0, count @ ..=4) => Location::HfaF64 { count: count as _ },
                _ => {
                    let layout = ir::type_layout(ty, types, primitives);
                    match layout.size {
                        0 => Location::Registers {
                            classes: [AbiClass::None; 2],
                            align_16: false,
                        },
                        1..=4 => Location::Registers {
                            classes: [AbiClass::Core(RegWidth::W32), AbiClass::None],
                            align_16: false,
                        },
                        5..=8 => Location::Registers {
                            classes: [AbiClass::Core(RegWidth::W64), AbiClass::None],
                            align_16: false,
                        },
                        9..=16 => Location::Registers {
                            classes: [AbiClass::Core(RegWidth::W64); 2],
                            align_16: layout.align.get() > 8,
                        },
                        17.. => Location::Memory,
                    }
                }
            }
        }
    }
}
