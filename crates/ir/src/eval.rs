use std::{
    fmt,
    ops::{Add, Div, Mul, Rem, Sub},
};

use crate::{
    Argument, BlockId, BlockInfo, BlockTarget, Environment, FunctionId, FunctionIr, ModuleOf,
    PrimitiveInfo, Ref, Type, TypeId, Types,
    dialect::Primitive,
    layout::{Layout, type_layout},
    slots::slot_count,
};

pub const BACKWARDS_JUMP_LIMIT: usize = 1000;
pub const STACK_SIZE: u32 = 8_000_000;
pub const STACK_FRAME_COUNT: usize = 10000;

#[derive(Clone, Debug)]
pub enum Val {
    Invalid,
    Unit,
    Int(u64),
    F32(f32),
    F64(f64),
    Ptr(Ptr),
    Array(Box<[Val]>),
    Tuple(Box<[Val]>),
}

const STACK_BIT: u32 = 1 << 31;

pub struct Mem {
    stack: Vec<u8>,
    heap: Vec<u8>,
}
impl Default for Mem {
    fn default() -> Self {
        Self::new()
    }
}
impl Mem {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            heap: vec![0], // to cover null pointers
        }
    }

    pub fn stack_alloc(&mut self, layout: Layout) -> Result<Ptr, Error> {
        let addr = self.stack.len();
        if self.stack.len() as u64 + layout.size > STACK_SIZE as u64 {
            return Err(Error::StackOverflow);
        }
        self.stack.resize_with(addr + layout.size as usize, || 0);
        Ok(Ptr {
            addr: addr as u32 | STACK_BIT,
            size: layout.size as u32,
        })
    }

    pub fn sp(&self) -> u32 {
        self.stack.len() as u32
    }

    pub fn restore_sp(&mut self, sp: u32) {
        debug_assert!(sp <= self.stack.len() as u32);
        self.stack.truncate(sp as usize);
    }
    pub fn malloc(&mut self, layout: Layout) -> Result<Ptr, OomError> {
        if self.heap.len() as u64 + layout.size >= STACK_BIT as u64 {
            return Err(OomError);
        }
        let addr = Ptr {
            addr: self.heap.len() as u32,
            size: layout.size as u32,
        };
        self.heap
            .extend(std::iter::repeat_n(0, layout.size as usize));
        Ok(addr)
    }

    pub fn load_n<const N: usize>(&mut self, mut ptr: Ptr) -> [u8; N] {
        let mem = if ptr.addr & STACK_BIT != 0 {
            ptr.addr &= !STACK_BIT;
            &mut self.stack
        } else {
            &mut self.heap
        };
        let mut arr = [0; N];
        arr.copy_from_slice(&mem[ptr.addr as usize..ptr.addr as usize + N]);
        arr
    }

    pub fn store(&mut self, mut ptr: Ptr, value: &[u8]) {
        let mem = if ptr.addr & STACK_BIT != 0 {
            ptr.addr &= !STACK_BIT;
            &mut self.stack
        } else {
            &mut self.heap
        };
        mem[ptr.addr as usize..ptr.addr as usize + value.len()].copy_from_slice(value);
    }

    pub fn free(&mut self, _ptr: Ptr, _layout: Layout) {
        // TODO: proper allocator that can free, also: verify that free call was valid
    }
}

pub struct OomError;

#[derive(Clone, Copy)]
pub struct Ptr {
    pub addr: u32,
    pub size: u32,
}
impl fmt::Debug for Ptr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ptr")
            .field(
                "addr",
                &if self.addr & STACK_BIT != 0 {
                    self.addr & !STACK_BIT
                } else {
                    self.addr
                },
            )
            .field(
                "location",
                &if self.addr & STACK_BIT != 0 {
                    "stack"
                } else {
                    "heap"
                },
            )
            .field("size", &self.size)
            .finish()
    }
}

impl Ptr {
    pub fn from_u64(x: u64) -> Self {
        Ptr {
            addr: (x >> 32) as u32,
            size: x as u32,
        }
    }

    pub fn into_u64(self) -> u64 {
        ((self.addr as u64) << 32) | self.size as u64
    }

    #[must_use = "returns a new pointer"]
    pub fn add_offset(self, offset: u32) -> Result<Self, ProvenanceError> {
        let size = self.size.checked_sub(offset).ok_or(ProvenanceError)?;
        Ok(Self {
            addr: self.addr + offset,
            size,
        })
    }
}

pub struct ProvenanceError;

#[derive(Debug, Clone)]
pub enum Error {
    InfiniteLoop,
    ExternCallFailed(Box<str>),
    StackOverflow,
    ProvenanceViolation,
    OutOfMemory,
}
impl From<ProvenanceError> for Error {
    fn from(_: ProvenanceError) -> Self {
        Self::ProvenanceViolation
    }
}

pub trait EvalEnvironment {
    fn env(&self) -> &Environment;
    fn debug(&self) -> bool;

    fn call_extern(
        &mut self,
        _id: FunctionId,
        _args: &[Val],
        _mem: &mut Mem,
    ) -> Result<Val, Box<str>> {
        Err("Can't evaluate extern functions".into())
    }
}
impl EvalEnvironment for Environment {
    fn env(&self) -> &Environment {
        self
    }

    fn debug(&self) -> bool {
        false
    }
}

struct Dialects {
    arith: ModuleOf<crate::dialect::Arith>,
    tuple: ModuleOf<crate::dialect::Tuple>,
    mem: ModuleOf<crate::dialect::Mem>,
    cf: ModuleOf<crate::dialect::Cf>,
}
impl Dialects {
    fn get(env: &crate::Environment) -> Self {
        // TODO: get rid of the unwraps, maybe let the user provide dialects
        Self {
            arith: env.get_dialect_module_if_present().unwrap(),
            tuple: env.get_dialect_module_if_present().unwrap(),
            mem: env.get_dialect_module_if_present().unwrap(),
            cf: env.get_dialect_module_if_present().unwrap(),
        }
    }
}

// TODO: validate params
// TODO: give errors a span by giving all IR instructions spans.
pub fn eval<E: EvalEnvironment>(
    top_level_ir: &FunctionIr,
    top_level_types: &Types,
    params: &[Val],
    env: &mut E,
) -> Result<Val, Error> {
    let top_level_function = (top_level_ir, top_level_types);
    if env.debug() {
        tracing::debug!(
            "Evaluating IR:\n{}",
            top_level_ir.display(env.env(), top_level_types)
        );
    }
    let dialects = Dialects::get(env.env());
    let mut mem = Mem::new();

    let mut values = Values::new(top_level_ir, top_level_types);
    let mut current_function: Option<FunctionId> = None;
    for (param, i) in params
        .iter()
        .zip(top_level_ir.get_block_args(BlockId::ENTRY).iter())
    {
        // TODO: check that the type fits
        // let ty = top_level_types[top_level_ir.inst[i as usize].ty];
        values.store(i.0, param);
    }

    let mut pc: u32 = top_level_ir.get_block(BlockId::ENTRY).next().unwrap().0.0;
    let mut call_stack: Vec<StackFrame> = vec![];
    let mut backwards_jumps = 0;

    let val = 'outer: loop {
        let (ir, types) = current_function.map_or(top_level_function, |id| {
            let function = &env.env()[id];
            (function.ir.get().unwrap(), &function.types)
        });

        let get_ref_and_ty = |values: &Values, r: Ref| -> (Val, Type) {
            match r {
                Ref::UNIT => (Val::Unit, Type::UNIT),
                Ref::FALSE => (Val::Int(0), Type::Primitive(Primitive::I1.id())),
                Ref::TRUE => (Val::Int(1), Type::Primitive(Primitive::I1.id())),
                _ => {
                    let ty = types[ir.get_inst(r).ty()];
                    (values.load(r.0, ty, types), ty)
                }
            }
        };
        let get_ref = |values: &Values, r: Ref| -> Val { get_ref_and_ty(values, r).0 };
        let get_int_ref = |values: &Values, r: Ref| -> u64 {
            debug_assert!(
                matches!(types[ir.get_inst(r).ty()], Type::Primitive(p) if Primitive::try_from(p).unwrap().is_int())
            );
            values.slots[values.slot_map[r.idx()] as usize]
        };
        let get_f32_ref = |values: &Values, r: Ref| -> f32 {
            debug_assert!(
                matches!(types[ir.get_inst(r).ty()], Type::Primitive(p) if Primitive::try_from(p).unwrap() == Primitive::F32)
            );
            f32::from_bits(values.slots[values.slot_map[r.idx()] as usize] as u32)
        };
        let get_f64_ref = |values: &Values, r: Ref| -> f64 {
            debug_assert!(
                matches!(types[ir.get_inst(r).ty()], Type::Primitive(p) if Primitive::try_from(p).unwrap() == Primitive::F64)
            );
            f64::from_bits(values.slots[values.slot_map[r.idx()] as usize])
        };
        let get_ptr_ref = |values: &Values, r: Ref| -> Ptr {
            debug_assert!(
                matches!(types[ir.get_inst(r).ty()], Type::Primitive(p) if Primitive::try_from(p).unwrap() == Primitive::Ptr)
            );
            Ptr::from_u64(values.slots[values.slot_map[r.idx()] as usize])
        };
        macro_rules! bin_op {
            ($float_f: ident, $inst: expr) => {{
                let (l, r): (Ref, Ref) = ir.args($inst, env.env());
                let l = get_ref(&values, l);
                let r = get_ref(&values, r);

                let Type::Primitive(ty) = types[$inst.ty()] else {
                    panic!("bin_op needs a primitive type")
                };
                let ty = Primitive::try_from(ty).unwrap();
                match ty {
                    Primitive::F32 => {
                        let Val::F32(l_val) = l else { panic!() };
                        let Val::F32(r_val) = r else { panic!() };
                        Val::F32(l_val.$float_f(r_val))
                    }
                    Primitive::F64 => {
                        let Val::F64(l_val) = l else { panic!() };
                        let Val::F64(r_val) = r else { panic!() };
                        Val::F64(l_val.$float_f(r_val))
                    }
                    t => panic!("Invalid type for binary operation: {t:?}"),
                }
            }};
        }
        // the inner loop should never break
        let _: std::convert::Infallible = loop {
            let inst = ir.get_inst(Ref(pc));
            //eprintln!("evaluating {} {:?} pc={pc}", inst.tag, inst.data);
            let invalid_ty =
                |ty: Type| -> ! { panic!("invalid type {ty:?} for {:?}", inst.function) };
            let value = if let Some(typed_inst) = inst.as_module(crate::BUILTIN) {
                use crate::Builtin as I;
                match typed_inst.op() {
                    I::Nothing => Val::Unit,
                    I::BlockArg => unreachable!("BlockArg inside function body encountered"),
                    I::Undef => {
                        // TODO: could track usage of undefined values and error
                        pc += 1;
                        continue;
                    }
                    I::Copy => unreachable!("Copy should only exist during optimization, not eval"),
                }
            } else if let Some(typed_inst) = inst.as_module(dialects.arith) {
                use crate::dialect::Arith as I;
                if let Some(binop) = typed_inst.op().int_binop(types[typed_inst.ty()]) {
                    let (l, r): (Ref, Ref) = ir.args(inst, env.env());
                    let l = get_int_ref(&values, l);
                    let r = get_int_ref(&values, r);
                    Val::Int((binop.fold)(l, r))
                } else {
                    match typed_inst.op() {
                        I::Shl | I::Shr | I::Rol | I::Ror => unreachable!("handled by int_binop"),
                        I::Int => {
                            let i: u64 = ir.args(inst, env.env());
                            Val::Int(i)
                        }
                        I::Float => {
                            let x: f64 = ir.args(inst, env.env());
                            match must_primitive(types[inst.ty()]) {
                                Primitive::F32 => Val::F32(x as f32),
                                Primitive::F64 => Val::F64(x),
                                _ => invalid_ty(types[inst.ty()]),
                            }
                        }
                        I::Neg => match get_ref(&values, ir.args(inst, env.env())) {
                            Val::Invalid => Val::Invalid,
                            Val::Int(val) => Val::Int(-(val as i64) as u64),
                            Val::F32(val) => Val::F32(-val),
                            Val::F64(val) => Val::F64(-val),
                            _ => panic!("Invalid value to negate"),
                        },
                        I::Not => {
                            let x = get_int_ref(&values, ir.args(inst, env.env()));
                            Val::Int(1 - x)
                        }
                        I::Add => bin_op!(add, inst),
                        I::Sub => bin_op!(sub, inst),
                        I::Mul => bin_op!(mul, inst),
                        I::Div => bin_op!(div, inst),
                        I::Rem => bin_op!(rem, inst),
                        I::Or => {
                            let (l, r): (Ref, Ref) = ir.args(inst, env.env());
                            let Val::Int(l) = get_ref(&values, l) else {
                                unreachable!()
                            };
                            let Val::Int(r) = get_ref(&values, r) else {
                                unreachable!()
                            };
                            Val::Int(((l != 0) || (r != 0)) as u64)
                        }
                        I::And => {
                            let (l, r) = ir.args(inst, env.env());
                            let l = get_int_ref(&values, l);
                            let r = get_int_ref(&values, r);
                            Val::Int(((l != 0) && (r != 0)) as u64)
                        }
                        op @ (I::Eq | I::NE | I::LT | I::GT | I::LE | I::GE) => {
                            let (l, r) = ir.args(inst, env.env());
                            let Type::Primitive(p) = types[inst.ty()] else {
                                unreachable!()
                            };
                            let p: Primitive = p.try_into().unwrap();
                            let res = match p {
                                Primitive::I1
                                | Primitive::I8
                                | Primitive::I16
                                | Primitive::I32
                                | Primitive::I64
                                | Primitive::U8
                                | Primitive::U16
                                | Primitive::U32
                                | Primitive::U64 => unreachable!(), // handled by int_bin_op
                                Primitive::I128 => todo!(),
                                Primitive::U128 => todo!(),
                                Primitive::F32 => {
                                    let l = get_f32_ref(&values, l);
                                    let r = get_f32_ref(&values, r);
                                    l.partial_cmp(&r)
                                }
                                Primitive::F64 => {
                                    let l = get_f64_ref(&values, l);
                                    let r = get_f64_ref(&values, r);
                                    l.partial_cmp(&r)
                                }
                                Primitive::Ptr => {
                                    let l = get_ptr_ref(&values, l);
                                    let r = get_ptr_ref(&values, r);
                                    Some(l.addr.cmp(&r.addr))
                                }
                            };
                            let val = match op {
                                I::Eq => res.is_some_and(|ord| ord.is_eq()),
                                I::NE => res.is_none_or(|ord| ord.is_ne()),
                                I::LT => res.is_some_and(|ord| ord.is_lt()),
                                I::LE => res.is_some_and(|ord| ord.is_le()),
                                I::GT => res.is_some_and(|ord| ord.is_gt()),
                                I::GE => res.is_some_and(|ord| ord.is_ge()),
                                _ => unreachable!(),
                            };
                            Val::Int(val as u64)
                        }
                        I::Xor => {
                            let (l, r) = ir.args(inst, env.env());
                            let l = get_int_ref(&values, l);
                            let r = get_int_ref(&values, r);
                            Val::Int(l ^ r)
                        }
                        I::CastInt => {
                            let r = ir.args(inst, env.env());
                            let x = get_int_ref(&values, r);
                            // TODO: probably not correct for casting, especially sign-extension
                            Val::Int(x)
                        }
                        I::CastFloat => {
                            let val = get_ref(&values, ir.args(inst, env.env()));
                            match (val, must_primitive(types[inst.ty()])) {
                                (Val::F32(x), Primitive::F32) => Val::F32(x),
                                (Val::F32(x), Primitive::F64) => Val::F64(x.into()),
                                (Val::F64(x), Primitive::F32) => Val::F32(x as f32),
                                (Val::F64(x), Primitive::F64) => Val::F64(x),
                                _ => panic!("invalid types for CastFloat"),
                            }
                        }
                        I::CastIntToFloat => {
                            let r = ir.args(inst, env.env());
                            let val = get_int_ref(&values, r);
                            let ty = must_primitive(types[ir.get_ref_ty(r)]);
                            match (ty, must_primitive(types[inst.ty()])) {
                                (Primitive::I1, Primitive::F32) => Val::F32((val & 1) as _),
                                (Primitive::U8, Primitive::F32) => Val::F32(val as u8 as _),
                                (Primitive::I8, Primitive::F32) => Val::F32(val as i8 as _),
                                (Primitive::U16, Primitive::F32) => Val::F32(val as u16 as _),
                                (Primitive::I16, Primitive::F32) => Val::F32(val as i16 as _),
                                (Primitive::U32, Primitive::F32) => Val::F32(val as u32 as _),
                                (Primitive::I32, Primitive::F32) => Val::F32(val as i32 as _),
                                (Primitive::U64, Primitive::F32) => Val::F32(val as _),
                                (Primitive::I64, Primitive::F32) => Val::F32(val as i64 as _),
                                (Primitive::U128, Primitive::F32) => todo!(),
                                (Primitive::I128, Primitive::F32) => todo!(),
                                (Primitive::I1, Primitive::F64) => Val::F64((val & 1) as _),
                                (Primitive::U8, Primitive::F64) => Val::F64(val as u8 as _),
                                (Primitive::I8, Primitive::F64) => Val::F64(val as i8 as _),
                                (Primitive::U16, Primitive::F64) => Val::F64(val as u16 as _),
                                (Primitive::I16, Primitive::F64) => Val::F64(val as i16 as _),
                                (Primitive::U32, Primitive::F64) => Val::F64(val as u32 as _),
                                (Primitive::I32, Primitive::F64) => Val::F64(val as i32 as _),
                                (Primitive::U64, Primitive::F64) => Val::F64(val as _),
                                (Primitive::I64, Primitive::F64) => Val::F64(val as i64 as _),
                                (Primitive::U128, Primitive::F64) => todo!(),
                                (Primitive::I128, Primitive::F64) => todo!(),
                                _ => panic!("invalid types for CastIntToFloat"),
                            }
                        }
                        I::CastFloatToInt => {
                            let val = get_ref(&values, ir.args(inst, env.env()));
                            Val::Int(match (val, must_primitive(types[inst.ty()])) {
                                (Val::F32(x), Primitive::I1) => (x as u8 & 1) as _,
                                (Val::F32(x), Primitive::U8) => (x as u8) as _,
                                (Val::F32(x), Primitive::I8) => (x as i8) as _,
                                (Val::F32(x), Primitive::U16) => (x as u16) as _,
                                (Val::F32(x), Primitive::I16) => (x as i16) as _,
                                (Val::F32(x), Primitive::U32) => (x as u32) as _,
                                (Val::F32(x), Primitive::I32) => (x as i32) as _,
                                (Val::F32(x), Primitive::U64) => (x as u64) as _,
                                (Val::F32(x), Primitive::I64) => (x as i64) as _,
                                (Val::F32(_), Primitive::U128) => todo!(),
                                (Val::F32(_), Primitive::I128) => todo!(),
                                (Val::F64(x), Primitive::I1) => (x as u8 & 1) as _,
                                (Val::F64(x), Primitive::U8) => (x as u8) as _,
                                (Val::F64(x), Primitive::I8) => (x as i8) as _,
                                (Val::F64(x), Primitive::U16) => (x as u16) as _,
                                (Val::F64(x), Primitive::I16) => (x as i16) as _,
                                (Val::F64(x), Primitive::U32) => (x as u32) as _,
                                (Val::F64(x), Primitive::I32) => (x as i32) as _,
                                (Val::F64(_), Primitive::U128) => todo!(),
                                (Val::F64(_), Primitive::I128) => todo!(),
                                _ => panic!("invalid types for CastFloatToInt"),
                            })
                        }
                    }
                }
            } else if let Some(typed_inst) = inst.as_module(dialects.tuple) {
                use crate::dialect::Tuple as I;
                match typed_inst.op() {
                    I::MemberValue => {
                        let (tuple, i): (Ref, u64) = ir.args(inst, env.env());
                        let Val::Tuple(t) = get_ref(&values, tuple) else {
                            unreachable!()
                        };
                        t[i as usize].clone()
                    }
                    I::InsertMember => {
                        let Type::Tuple(elem_types) = types[inst.ty()] else {
                            unreachable!()
                        };
                        let (tuple, insert_idx, value): (Ref, u64, Ref) = ir.args(inst, env.env());
                        let mut in_idx = values.slot_map[tuple.idx()];
                        let mut out_idx = values.slot_map[pc as usize];
                        for (elem_ty, i) in elem_types.iter().zip(0..) {
                            let slot_count = slot_count(types[elem_ty], types);
                            let src = if i == insert_idx {
                                values.slot_map[value.idx()]
                            } else {
                                in_idx
                            };
                            let src = src as usize..(src + slot_count) as usize;
                            values.slot_map.copy_within(src, out_idx as usize);
                            in_idx += slot_count;
                            out_idx += slot_count;
                        }
                        pc += 1;
                        continue;
                    }
                }
            } else if let Some(typed_inst) = inst.as_module(dialects.mem) {
                use crate::dialect::Mem as I;
                match typed_inst.op() {
                    I::Decl => {
                        let ty = ir.args(inst, env.env());
                        let layout = type_layout(types[ty], types, env.env().primitives());
                        Val::Ptr(mem.stack_alloc(layout)?)
                    }
                    I::Load => {
                        let ptr = values.load_ptr(ir.args(inst, env.env()));
                        values.load_primitives(
                            pc,
                            types[inst.ty],
                            types,
                            env.env().primitives(),
                            |size, offset| {
                                let ptr = ptr.add_offset(offset)?;
                                let val = match size {
                                    PrimitiveSize::S8 => u8::from_le_bytes(mem.load_n(ptr)) as u64,
                                    PrimitiveSize::S16 => {
                                        u16::from_le_bytes(mem.load_n(ptr)) as u64
                                    }
                                    PrimitiveSize::S32 => {
                                        u32::from_le_bytes(mem.load_n(ptr)) as u64
                                    }
                                    PrimitiveSize::S64 => u64::from_le_bytes(mem.load_n(ptr)),
                                    PrimitiveSize::S128 => todo!(),
                                };
                                Ok(val)
                            },
                        )?;
                        pc += 1;
                        continue;
                    }
                    I::Store => {
                        let (var, val) = ir.args(inst, env.env());
                        let ptr = get_ptr_ref(&values, var);
                        values.visit_primitives(
                            val,
                            ir,
                            types,
                            env.env().primitives(),
                            |p, offset| {
                                let offset = offset.try_into().expect("Support large offsets");
                                let ptr = ptr.add_offset(offset)?;
                                match p {
                                    PrimitiveVal::I8(x) => mem.store(ptr, &[x]),
                                    PrimitiveVal::I16(x) => mem.store(ptr, &x.to_le_bytes()),
                                    PrimitiveVal::I32(x) => mem.store(ptr, &x.to_le_bytes()),
                                    PrimitiveVal::I64(x) => mem.store(ptr, &x.to_le_bytes()),
                                    PrimitiveVal::I128(_) => todo!(),
                                }
                                Ok(())
                            },
                        )?;
                        Val::Invalid
                    }
                    I::MemberPtr => {
                        let (ptr, tuple_ty, i): (Ref, TypeId, u32) = ir.args(inst, env.env());
                        let Type::Tuple(elem_types) = types[tuple_ty] else {
                            panic!("tuple type expected for MemberPtr");
                        };
                        let ptr = values.load_ptr(ptr);
                        let offset =
                            crate::offset_in_tuple(elem_types, i, types, env.env().primitives());
                        Val::Ptr(ptr.add_offset(offset.try_into().unwrap())?)
                    }
                    I::Offset => {
                        let (ptr, offset): (Ref, u32) = ir.args(inst, env.env());
                        let ptr = values.load_ptr(ptr);
                        Val::Ptr(ptr.add_offset(offset)?)
                    }
                    I::IntToPtr => {
                        let i = get_int_ref(&values, ir.args(inst, env.env()));
                        // TODO: what to do with large addresses?
                        // TODO: what can be done with size
                        Val::Ptr(Ptr {
                            addr: i as u32,
                            size: u32::MAX,
                        })
                    }
                    I::PtrToInt => {
                        Val::Int(get_ptr_ref(&values, ir.args(inst, env.env())).addr.into())
                    }
                    I::FunctionPtr => todo!("function pointers"),
                    I::Global => todo!("globals"),
                    I::ArrayIndex => {
                        let (array_ptr, elem_ty, idx) = ir.args(inst, env.env());
                        let ptr = get_ptr_ref(&values, array_ptr);
                        let layout = type_layout(types[elem_ty], types, env.env().primitives());
                        let idx = get_int_ref(&values, idx);
                        let offset = idx * layout.stride();
                        Val::Ptr(ptr.add_offset(offset.try_into().map_err(|_| ProvenanceError)?)?)
                    }
                    I::CallPtr => todo!("indirect calls"),
                }
            } else if let Some(typed_inst) = inst.as_module(dialects.cf) {
                use crate::dialect::Cf as I;
                match typed_inst.op() {
                    I::Goto => {
                        let BlockTarget(target, args) = ir.args(inst, env.env());
                        let info = &ir.blocks[target.idx()];
                        let target_pos = info.args_idx + info.arg_count;
                        if target_pos <= pc {
                            backwards_jumps += 1;
                            if backwards_jumps > BACKWARDS_JUMP_LIMIT {
                                return Err(Error::InfiniteLoop);
                            }
                        }
                        values.copy_args(info, &args, ir, types);
                        pc = target_pos;
                        continue;
                    }
                    I::Branch => {
                        let (cond, a, b) = ir.args(inst, env.env());
                        let cond = get_int_ref(&values, cond) == 1;
                        let BlockTarget(target, args) = if cond { a } else { b };
                        let info = &ir.blocks[target.idx()];
                        let target_pos = info.args_idx + info.arg_count;
                        if target_pos <= pc {
                            backwards_jumps += 1;
                            if backwards_jumps > BACKWARDS_JUMP_LIMIT {
                                return Err(Error::InfiniteLoop);
                            }
                        }
                        values.copy_args(info, &args, ir, types);
                        pc = target_pos;
                        continue;
                    }
                    I::Ret => {
                        let return_val = get_ref(&values, ir.args(inst, env.env()));
                        let Some(mut frame) = call_stack.pop() else {
                            // the function that was originally evaluated returned, break to return the
                            // value from eval
                            break 'outer return_val;
                        };
                        current_function = frame.function;
                        pc = frame.pc;
                        mem.restore_sp(frame.sp);
                        frame.values.store(pc, &return_val);
                        values = frame.values;
                        pc += 1;
                        continue 'outer;
                    }
                }
            } else {
                let func = &env.env()[inst.function];
                let args: Vec<_> = ir
                    .args_iter(inst, env.env())
                    .map(|r| {
                        let Argument::Ref(r) = r else {
                            panic!("can't evaluate unknown instruction");
                        };
                        get_ref(&values, r)
                    })
                    .collect();
                if let Some(func_ir) = func.ir() {
                    // PERF: could copy args directly here without allocating
                    let mut new_values = Values::new(func_ir, &func.types);
                    let entry = BlockId::ENTRY;
                    let args_indices = func_ir.get_block_args(entry);
                    for (arg, val) in args_indices.iter().zip(args) {
                        new_values.store(arg.0, &val);
                    }
                    if call_stack.len() > STACK_FRAME_COUNT {
                        return Err(Error::StackOverflow);
                    }
                    call_stack.push(StackFrame {
                        function: current_function,
                        pc,
                        sp: mem.sp(),
                        values,
                    });
                    pc = func_ir.get_block(BlockId::ENTRY).next().unwrap().0.0;
                    values = new_values;
                    current_function = Some(inst.function);
                    // this will fetch ir and types again based on current_function
                    continue 'outer;
                } else {
                    let res = env
                        .call_extern(inst.function, &args, &mut mem)
                        .map_err(Error::ExternCallFailed)?;
                    values.store(pc, &res);
                    pc += 1;
                    continue 'outer;
                }
            };
            /*
            let value = match inst.tag {
                super::Tag::Nothing => Val::Invalid,
                super::Tag::Ret => {
                    let return_val = get_ref(&values, inst.data.un_op());
                    //eprintln!("  -> returning {return_val:?}");
                    let Some(mut frame) = call_stack.pop() else {
                        // the function that was originally evaluated returned, break to return the
                        // value from eval
                        break 'outer return_val;
                    };
                    current_function = frame.function;
                    pc = frame.pc;
                    mem.restore_sp(frame.sp);
                    frame.values.store(pc, &return_val);
                    values = frame.values;
                    pc += 1;
                    continue 'outer;
                }
                super::Tag::BlockArg => unreachable!("BlockArg should never exist inside a block"),
                super::Tag::Global => todo!("handle globals in const eval"),
                super::Tag::Int => Val::Int(inst.data.int()),
                super::Tag::LargeInt => {
                    let bytes =
                        &ir.extra[inst.data.extra() as usize..(inst.data.extra() + 16) as usize];
                    let mut bytes_arr = [0; 16];
                    bytes_arr.copy_from_slice(bytes);
                    todo!("support large ints")
                }
                super::Tag::Float => match types[inst.ty] {
                    IrType::F32 => Val::F32(inst.data.float() as f32),
                    IrType::F64 => Val::F64(inst.data.float()),
                    _ => panic!("invalid type"),
                },
                super::Tag::Decl => {
                    let layout = type_layout(types[inst.data.ty()], types);
                    Val::Ptr(mem.stack_alloc(layout)?)
                }
                super::Tag::Load => {
                    let ptr = values.load_ptr(inst.data.un_op());
                    values.load_primitives(pc, types[inst.ty], types, |size, offset| {
                        let ptr = ptr.add_offset(offset)?;
                        let val = match size {
                            PrimitiveSize::S8 => u8::from_le_bytes(mem.load_n(ptr)) as u64,
                            PrimitiveSize::S16 => u16::from_le_bytes(mem.load_n(ptr)) as u64,
                            PrimitiveSize::S32 => u32::from_le_bytes(mem.load_n(ptr)) as u64,
                            PrimitiveSize::S64 => u64::from_le_bytes(mem.load_n(ptr)),
                        };
                        //eprintln!("  .. read {} bytes at {ptr:?} -> {val}", size.byte_size());
                        Ok(val)
                    })?;
                    pc += 1;
                    continue;
                }
                super::Tag::Store => {
                    let (var, val) = inst.data.bin_op();
                    let Val::Ptr(mut ptr) = get_ref(&values, var) else {
                        panic!()
                    };
                    //let (val, ty) = get_ref_and_ty(&values, val);
                    //eprintln!("  storing {val:?} into {ptr:?}",);
                    values.visit_primitives(val, ir, types, |p| {
                        //eprintln!("  storing {p:?} {ptr:?}");
                        match p {
                            PrimitiveVal::I8(x) => {
                                let new_ptr = ptr.add_offset(1)?;
                                mem.store(ptr, &[x]);
                                ptr = new_ptr;
                            }
                            PrimitiveVal::I16(x) => {
                                let new_ptr = ptr.add_offset(2)?;
                                mem.store(ptr, &x.to_le_bytes());
                                ptr = new_ptr;
                            }
                            PrimitiveVal::I32(x) => {
                                let new_ptr = ptr.add_offset(4)?;
                                mem.store(ptr, &x.to_le_bytes());
                                ptr = new_ptr;
                            }
                            PrimitiveVal::I64(x) => {
                                let new_ptr = ptr.add_offset(8)?;
                                mem.store(ptr, &x.to_le_bytes());
                                ptr = new_ptr;
                            }
                        }
                        Ok(())
                    })?;
                    Val::Invalid
                }
                super::Tag::String => {
                    let _string = ir.extra[inst.data.extra_len().0 as usize
                        ..(inst.data.extra_len().0 + inst.data.extra_len().1) as usize]
                        .to_vec();
                    eprintln!("TODO: evaluate strings");
                    Val::Tuple(Box::new([Val::Ptr(Ptr { addr: 0, size: 0 }), Val::Int(0)]))
                }
                super::Tag::Call => {
                    let (func_id, args) = inst.data.call(&ir.extra);
                    let args: Vec<_> = args.map(|r| get_ref(&values, r)).collect();
                    let func = env.get_function(func_id);
                    if let Some(func_ir) = &func.ir {
                        // PERF: could copy args directly here without allocating
                        let mut new_values = Values::new(func_ir, &func.types);
                        let entry = BlockIndex::ENTRY;
                        let args_indices = func_ir.get_block_args(entry);
                        for (arg, val) in args_indices.range().zip(args) {
                            new_values.store(arg as _, &val);
                        }
                        if call_stack.len() > STACK_FRAME_COUNT {
                            return Err(Error::StackOverflow);
                        }
                        call_stack.push(StackFrame {
                            function: current_function,
                            pc,
                            sp: mem.sp(),
                            values,
                        });
                        pc = func_ir
                            .get_block_range(func_ir.blocks().next().unwrap())
                            .start;
                        values = new_values;
                        current_function = Some(func_id);
                        // this will fetch ir and types again based on current_function
                        continue 'outer;
                    } else {
                        let res = env
                            .call_extern(func_id, &args, &mut mem)
                            .map_err(Error::ExternCallFailed)?;
                        values.store(pc, &res);
                        pc += 1;
                        continue 'outer;
                    }
                }
                super::Tag::FunctionPtr | super::Tag::CallPtr => todo!("eval function pointers"),
                super::Tag::Neg => match get_ref(&values, inst.data.un_op()) {
                    Val::Invalid => Val::Invalid,
                    Val::Int(val) => Val::Int(-(val as i64) as u64),
                    Val::F32(val) => Val::F32(-val),
                    Val::F64(val) => Val::F64(-val),
                    _ => panic!("Invalid value to negate"),
                },
                super::Tag::Not => {
                    let val = get_ref(&values, inst.data.un_op());
                    match val {
                        Val::Int(0) => Val::Int(1),
                        Val::Int(1) => Val::Int(0),
                        _ => panic!(),
                    }
                }
                super::Tag::Add => bin_op!(+, inst),
                super::Tag::Sub => bin_op!(-, inst),
                super::Tag::Mul => bin_op!(*, inst),
                super::Tag::Div => bin_op!(/, inst),
                super::Tag::Rem => bin_op!(%, inst),
                super::Tag::Or => {
                    let (Val::Int(a), Val::Int(b)) = (
                        get_ref(&values, inst.data.bin_op().0),
                        get_ref(&values, inst.data.bin_op().1),
                    ) else {
                        panic!("Invalid values for or")
                    };
                    Val::Int((a != 0 || b != 0) as u64)
                }
                super::Tag::And => {
                    let (Val::Int(a), Val::Int(b)) = (
                        get_ref(&values, inst.data.bin_op().0),
                        get_ref(&values, inst.data.bin_op().1),
                    ) else {
                        panic!("Invalid values for and")
                    };
                    Val::Int((a != 0 && b != 0) as u64)
                }
                super::Tag::Eq => {
                    let l = get_ref(&values, inst.data.bin_op().0);
                    let r = get_ref(&values, inst.data.bin_op().1);
                    Val::Int(l.equals(&r) as u64)
                }
                super::Tag::NE => {
                    let l = get_ref(&values, inst.data.bin_op().0);
                    let r = get_ref(&values, inst.data.bin_op().1);
                    Val::Int(!l.equals(&r) as u64)
                }
                super::Tag::LT => cmp_op!(< , inst),
                super::Tag::GT => cmp_op!(> , inst),
                super::Tag::LE => cmp_op!(<=, inst),
                super::Tag::GE => cmp_op!(>=, inst),
                super::Tag::Xor => {
                    let (l, r) = inst.data.bin_op();
                    let (Val::Int(l), Val::Int(r)) = (get_ref(&values, l), get_ref(&values, r))
                    else {
                        panic!("invalid types for xor");
                    };
                    Val::Int(match types[inst.ty] {
                        IrType::I8 | IrType::U8 => ((l as u8) ^ (r as u8)) as u64,
                        IrType::I16 | IrType::U16 => ((l as u16) ^ (r as u16)) as u64,
                        IrType::I32 | IrType::U32 => ((l as u32) ^ (r as u32)) as u64,
                        IrType::I64 | IrType::U64 => l ^ r,
                        _ => panic!("invalid resulting type for xor"),
                    })
                }

                super::Tag::Rol | super::Tag::Ror => {
                    let (Val::Int(l), Val::Int(r)) = (
                        get_ref(&values, inst.data.bin_op().0),
                        get_ref(&values, inst.data.bin_op().1),
                    ) else {
                        panic!("invalid types for rol/ror");
                    };
                    let r = r as u32;
                    let res = if inst.tag == Tag::Rol {
                        match types[inst.ty] {
                            IrType::I8 | IrType::U8 => (l as u8).rotate_left(r) as u64,
                            IrType::I16 | IrType::U16 => (l as u16).rotate_left(r) as u64,
                            IrType::I32 | IrType::U32 => (l as u32).rotate_left(r) as u64,
                            IrType::I64 | IrType::U64 => l.rotate_left(r),
                            _ => panic!("invalid resulting type for rol"),
                        }
                    } else {
                        match types[inst.ty] {
                            IrType::I8 | IrType::U8 => (l as u8).rotate_right(r) as u64,
                            IrType::I16 | IrType::U16 => (l as u16).rotate_right(r) as u64,
                            IrType::I32 | IrType::U32 => (l as u16).rotate_right(r) as u64,
                            IrType::I64 | IrType::U64 => (l as u16).rotate_right(r) as u64,
                            _ => panic!("invalid resulting type for ror"),
                        }
                    };
                    Val::Int(res)
                }
                super::Tag::MemberPtr => {
                    let (ptr, elem_types, i) = inst.data.member_ptr(&ir.extra);
                    let ptr = values.load_ptr(ptr);
                    let offset = crate::offset_in_tuple(elem_types, i, types);
                    Val::Ptr(ptr.add_offset(offset.try_into().unwrap())?)
                }
                super::Tag::MemberValue => {
                    let (tuple, i) = inst.data.ref_int();
                    let Val::Tuple(t) = get_ref(&values, tuple) else {
                        unreachable!()
                    };
                    t[i as usize].clone()
                }
                super::Tag::InsertMember => {
                    let IrType::Tuple(elem_types) = types[inst.ty] else {
                        unreachable!()
                    };
                    let (tuple, i, value) = inst.data.ref_int_ref(&ir.extra);
                    let value = get_ref(&values, value);
                    Val::Tuple(if tuple == Ref::UNDEF {
                        let mut tuple_value: Box<_> =
                            (0..elem_types.count).map(|_| Val::Invalid).collect();
                        tuple_value[i as usize] = value;
                        tuple_value
                    } else {
                        let Val::Tuple(mut t) = get_ref(&values, tuple) else {
                            unreachable!()
                        };
                        t[i as usize] = value;
                        t
                    })
                }
                super::Tag::ArrayIndex => todo!(),
                super::Tag::CastInt => {
                    let (v, from_ty) = get_ref_and_ty(&values, inst.data.un_op());
                    debug_assert!(from_ty.is_int());
                    debug_assert!(types[inst.ty].is_int());
                    // integers values are always represented the same right now
                    v
                }
                super::Tag::CastFloat => {
                    let v = get_ref(&values, inst.data.un_op());
                    let to_ty = types[inst.ty];
                    match (v, to_ty) {
                        (Val::F32(v), IrType::F32) => Val::F32(v),
                        (Val::F32(v), IrType::F64) => Val::F64(v as f64),
                        (Val::F64(v), IrType::F32) => Val::F32(v as f32),
                        (Val::F64(v), IrType::F64) => Val::F64(v),
                        _ => panic!("invalid types for float cast"),
                    }
                }
                super::Tag::CastIntToFloat => {
                    let Val::Int(v) = get_ref(&values, inst.data.un_op()) else {
                        panic!("invalid type for CastIntToFloat");
                    };
                    let to_ty = types[inst.ty];
                    match to_ty {
                        IrType::F32 => Val::F32(v as f32),
                        IrType::F64 => Val::F64(v as f64),
                        _ => panic!("invalid target type for CastIntToFloat"),
                    }
                }
                super::Tag::CastFloatToInt => {
                    let v = match get_ref(&values, inst.data.un_op()) {
                        Val::F32(v) => v as f64, // casting f32 to f64 first should never be an issue
                        Val::F64(v) => v,
                        _ => panic!("invalid type for CastFloatToInt"),
                    };
                    let to_ty = types[inst.ty];
                    match to_ty {
                        IrType::U8 => Val::Int(v as u8 as u64),
                        IrType::U16 => Val::Int(v as u16 as u64),
                        IrType::U32 => Val::Int(v as u16 as u64),
                        IrType::U64 => Val::Int(v as u64),
                        IrType::I8 => Val::Int(v as i8 as u64),
                        IrType::I16 => Val::Int(v as i16 as u64),
                        IrType::I32 => Val::Int(v as i16 as u64),
                        IrType::I64 => Val::Int(v as i64 as u64),
                        IrType::U128 | IrType::I128 => {
                            todo!("evaluate 128 bit int values")
                        }
                        _ => panic!("invalid target type for CastIntToFloat"),
                    }
                }
                super::Tag::Goto => {
                    let (target, extra_idx) = inst.data.goto();
                    let args = decode_block_args(ir, target, extra_idx);
                    let target_pos = ir.blocks[target.idx() as usize].start;
                    if target_pos <= pc {
                        backwards_jumps += 1;
                        if backwards_jumps > BACKWARDS_JUMP_LIMIT {
                            return Err(Error::InfiniteLoop);
                        }
                    }
                    let arg_count = args.len() as u32;
                    for (r, i) in args.zip(target_pos - arg_count..target_pos) {
                        values.store(i, &get_ref(&values, r));
                    }
                    pc = target_pos;
                    continue;
                }
                super::Tag::Branch => {
                    let (cond, a, b, i) = inst.data.branch(&ir.extra);
                    let val = get_ref(&values, cond);
                    let val = match val {
                        Val::Int(0) => false,
                        Val::Int(1) => true,
                        _ => panic!("bool value expected"),
                    };
                    let a_arg_count = ir.blocks[a.idx() as usize].arg_count;
                    let b_arg_count = ir.blocks[b.idx() as usize].arg_count;
                    let (target, args_idx, arg_count) = if val {
                        (a, i, a_arg_count)
                    } else {
                        (b, i + a_arg_count as usize * 4, b_arg_count)
                    };
                    let target_pos = ir.blocks[target.idx() as usize].start;
                    if target_pos <= pc {
                        backwards_jumps += 1;
                        if backwards_jumps > BACKWARDS_JUMP_LIMIT {
                            return Err(Error::InfiniteLoop);
                        }
                    }
                    let args = decode_block_args(ir, target, args_idx);
                    for (r, i) in args.zip(target_pos - arg_count..target_pos) {
                        values.store(i, &get_ref(&values, r));
                    }
                    pc = target_pos;
                    continue;
                }
                super::Tag::IntToPtr => {
                    let Val::Int(addr) = get_ref(&values, inst.data.un_op()) else {
                        unreachable!()
                    };
                    // TODO: what to do with large addresses?
                    // TODO: what can be done with size
                    Val::Ptr(Ptr {
                        addr: addr.try_into().unwrap(),
                        size: u32::MAX,
                    })
                }
                super::Tag::PtrToInt => {
                    let ptr = values.load_ptr(inst.data.un_op());
                    Val::Int(ptr.addr as u64)
                }
                super::Tag::Asm => todo!(), // TODO: error handling
            };
            */
            //eprintln!("  -> got {value:?}");
            values.store(pc, &value);
            pc += 1;
        };
    };
    assert!(
        !matches!(val, Val::Invalid),
        "Constant evaluation yielded an invalid value, this is probably an internal error",
    );
    Ok(val)
}

struct StackFrame {
    /// if None, return to the original function that was passed to eval
    function: Option<FunctionId>,
    pc: u32,
    sp: u32,
    values: Values,
}

type Values = crate::slots::Slots<u64>;
impl Values {
    fn visit_primitives(
        &mut self,
        r: Ref,
        ir: &FunctionIr,
        types: &Types,
        primitives: &[PrimitiveInfo],
        mut visit: impl FnMut(PrimitiveVal, u64) -> Result<(), Error>,
    ) -> Result<(), Error> {
        match r {
            Ref::UNIT => Ok(()),
            Ref::TRUE => visit(PrimitiveVal::I8(1), 0),
            Ref::FALSE => visit(PrimitiveVal::I8(0), 0),
            _ => {
                let ty = types[ir.get_inst(r).ty];
                let slot_index = self.slot_map[r.0 as usize];
                self.visit_primitive_slots_inner(
                    &mut { slot_index },
                    ty,
                    types,
                    primitives,
                    &mut move |slots, p, offset| match p {
                        Primitive::I8 | Primitive::U8 | Primitive::I1 => {
                            visit(PrimitiveVal::I8(slots[0] as u8), offset)
                        }
                        Primitive::I16 | Primitive::U16 => {
                            visit(PrimitiveVal::I16(slots[0] as u16), offset)
                        }
                        Primitive::I32 | Primitive::U32 | Primitive::F32 => {
                            visit(PrimitiveVal::I32(slots[0] as u32), offset)
                        }
                        Primitive::I64 | Primitive::U64 | Primitive::F64 | Primitive::Ptr => {
                            visit(PrimitiveVal::I64(slots[0]), offset)
                        }
                        Primitive::I128 | Primitive::U128 => {
                            let value = ((slots[0] as u128) << 64) | slots[1] as u128;
                            visit(PrimitiveVal::I128(value), offset)
                        }
                    },
                    0,
                )
            }
        }
    }

    fn load_primitives(
        &mut self,
        i: u32,
        ty: Type,
        types: &Types,
        primitives: &[PrimitiveInfo],
        mut visit: impl FnMut(PrimitiveSize, u32) -> Result<u64, Error>,
    ) -> Result<(), Error> {
        let slot_index = self.slot_map[i as usize];
        self.load_primitives_inner(&mut { slot_index }, 0, ty, types, primitives, &mut visit)
    }

    fn load_primitives_inner(
        &mut self,
        i: &mut u32,
        offset: u32,
        ty: Type,
        types: &Types,
        primitives: &[PrimitiveInfo],
        visit: &mut impl FnMut(PrimitiveSize, u32) -> Result<u64, Error>,
    ) -> Result<(), Error> {
        match ty {
            Type::Primitive(p) => match Primitive::try_from(p).unwrap() {
                Primitive::I8 | Primitive::U8 | Primitive::I1 => {
                    self.slots[*i as usize] = visit(PrimitiveSize::S8, offset)?;
                    *i += 1;
                }
                Primitive::I16 | Primitive::U16 => {
                    self.slots[*i as usize] = visit(PrimitiveSize::S16, offset.div_ceil(2) * 2)?;
                    *i += 1;
                }
                Primitive::I32 | Primitive::U32 | Primitive::F32 => {
                    self.slots[*i as usize] = visit(PrimitiveSize::S32, offset.div_ceil(4) * 4)?;
                    *i += 1;
                }
                Primitive::I64 | Primitive::U64 | Primitive::F64 | Primitive::Ptr => {
                    self.slots[*i as usize] = visit(PrimitiveSize::S64, offset.div_ceil(8) * 8)?;
                    *i += 1;
                }
                Primitive::I128 | Primitive::U128 => todo!(),
            },
            Type::Array(elem, len) => {
                let elem_layout = type_layout(types[elem], types, primitives);
                if elem_layout.size == 0 {
                    return Ok(());
                }
                let align: u32 = elem_layout
                    .align
                    .get()
                    .try_into()
                    .map_err(|_| Error::OutOfMemory)?;
                let stride: u32 = elem_layout
                    .stride()
                    .try_into()
                    .map_err(|_| Error::OutOfMemory)?;
                let offset = offset.div_ceil(align) * align;
                for elem_idx in 0..len {
                    self.load_primitives_inner(
                        i,
                        offset + elem_idx * stride,
                        types[elem],
                        types,
                        primitives,
                        visit,
                    )?;
                }
            }
            Type::Tuple(elems) => {
                let layout = type_layout(Type::Tuple(elems), types, primitives);
                let align: u32 = layout
                    .align
                    .get()
                    .try_into()
                    .map_err(|_| Error::OutOfMemory)?;
                let offset = offset.div_ceil(align) * align;
                let mut layout = Layout::EMPTY;
                for elem in elems.iter() {
                    let elem_layout = type_layout(types[elem], types, primitives);
                    layout.align_for(elem_layout.align);
                    let tuple_offset: u32 = layout.size.try_into().unwrap();
                    self.load_primitives_inner(
                        i,
                        offset + tuple_offset,
                        types[elem],
                        types,
                        primitives,
                        visit,
                    )?;
                    layout.accumulate(type_layout(types[elem], types, primitives));
                }
            }
        }
        Ok(())
    }

    pub fn store(&mut self, i: u32, val: &Val) {
        let i = self.slot_map[i as usize];
        self.store_inner(&mut { i }, val);
    }

    pub fn load(&self, i: u32, ty: Type, types: &Types) -> Val {
        let i = self.slot_map[i as usize];
        self.load_inner(&mut { i }, ty, types)
    }

    pub fn load_ptr(&mut self, r: Ref) -> Ptr {
        Ptr::from_u64(self.slots[self.slot_map[r.idx()] as usize])
    }

    fn store_inner(&mut self, i: &mut u32, val: &Val) {
        match val {
            Val::Invalid | Val::Unit => {}
            &Val::Int(n) => {
                self.slots[*i as usize] = n;
                *i += 1;
            }
            &Val::F32(n) => {
                self.slots[*i as usize] = n.to_bits() as u64;
                *i += 1;
            }
            &Val::F64(n) => {
                self.slots[*i as usize] = n.to_bits();
                *i += 1;
            }
            &Val::Ptr(ptr) => {
                self.slots[*i as usize] = ptr.into_u64();
                *i += 1;
            }
            Val::Array(elems) | Val::Tuple(elems) => {
                for elem in elems {
                    self.store_inner(i, elem);
                }
            }
        }
    }

    fn load_inner(&self, i: &mut u32, ty: Type, types: &Types) -> Val {
        match ty {
            Type::Primitive(p) => match Primitive::try_from(p).unwrap() {
                Primitive::I1
                | Primitive::I8
                | Primitive::I16
                | Primitive::I32
                | Primitive::I64
                | Primitive::U8
                | Primitive::U16
                | Primitive::U32
                | Primitive::U64 => {
                    let val = Val::Int(self.slots[*i as usize]);
                    *i += 1;
                    val
                }
                Primitive::I128 | Primitive::U128 => todo!(),
                Primitive::F32 => {
                    let val = Val::F32(f32::from_bits(self.slots[*i as usize] as u32));
                    *i += 1;
                    val
                }
                Primitive::F64 => {
                    let val = Val::F64(f64::from_bits(self.slots[*i as usize]));
                    *i += 1;
                    val
                }
                Primitive::Ptr => {
                    let x = self.slots[*i as usize];
                    *i += 1;
                    Val::Ptr(Ptr::from_u64(x))
                }
            },
            Type::Array(elem, len) => Val::Array(
                (0..len)
                    .map(|_| self.load_inner(i, types[elem], types))
                    .collect(),
            ),
            Type::Tuple(elems) => Val::Tuple(
                elems
                    .iter()
                    .map(|elem| self.load_inner(i, types[elem], types))
                    .collect(),
            ),
        }
    }

    fn copy_args(&mut self, info: &BlockInfo, args: &[Ref], ir: &FunctionIr, types: &Types) {
        debug_assert_eq!(info.arg_count as usize, args.len());
        for (&r, i) in args
            .iter()
            .zip(info.args_idx..info.args_idx + info.arg_count)
        {
            let src = self.slot_map[r.idx()] as usize;
            let dst = self.slot_map[i as usize] as usize;
            let n = slot_count(types[ir.get_ref_ty(r)], types) as usize;
            self.slots.copy_within(src..src + n, dst);
        }
    }
}

fn must_primitive(ty: Type) -> Primitive {
    let Type::Primitive(p) = ty else {
        panic!("primitive expected");
    };
    p.try_into().expect("invalid primitive")
}

#[derive(Debug)]
enum PrimitiveVal {
    I8(u8),
    I16(u16),
    I32(u32),
    I64(u64),
    #[allow(unused)] // TODO: 128-bit support in eval
    I128(u128),
}

#[derive(Debug)]
enum PrimitiveSize {
    S8,
    S16,
    S32,
    S64,
    #[allow(unused)] // TODO: 128-bit support in eval
    S128,
}
