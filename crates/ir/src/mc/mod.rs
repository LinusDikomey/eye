mod abi;
mod dialect;
mod parcopy;
mod regalloc;

#[doc(hidden)]
pub mod macros;

pub use abi::Abi;
pub use dialect::{Mc, McInsts};
pub use macros::registers;
pub use parcopy::ParcopySolver;
pub use regalloc::{Regalloc, regalloc};

use crate::Argument;
use crate::Environment;
use crate::FunctionId;
use crate::Inst;
use crate::MCReg;
use crate::MCRegOffset;
use crate::ModuleOf;
use crate::Ref;
use crate::TypeId;
use crate::modify::IrModify;
use std::hash::Hash;
use std::ops::{BitAnd, BitOr, Not};

pub trait McInst: Inst {
    type Reg: Register;
    fn implicit_def(&self, abi: &'static dyn Abi<Self>) -> <Self::Reg as Register>::RegisterBits;
    fn implicit_use(&self, abi: &'static dyn Abi<Self>) -> <Self::Reg as Register>::RegisterBits;
}

pub trait Register: 'static + Copy + Eq + Hash {
    const DEFAULT: Self;
    type RegisterBits: Copy
        + BitAnd<Output = Self::RegisterBits>
        + Not<Output = Self::RegisterBits>
        + BitOr<Output = Self::RegisterBits>
        + Eq;
    type Class: Copy + Eq + Into<u8> + TryFrom<u8>;
    const NO_BITS: Self::RegisterBits;
    const ALL_BITS: Self::RegisterBits;

    fn to_str(self) -> &'static str;
    fn encode(self) -> u32;
    fn decode(value: u32) -> Self;

    fn bit_index(self) -> u8;
    fn get_bit(self, bits: &Self::RegisterBits) -> bool;
    fn set_bit(self, bits: &mut Self::RegisterBits, bit: bool);
    fn allocate_reg(free_bits: Self::RegisterBits, class: Self::Class) -> Option<Self>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UnknownRegister(u32);
impl Register for UnknownRegister {
    const DEFAULT: Self = Self(0);

    type RegisterBits = u8;
    type Class = u8;
    const NO_BITS: Self::RegisterBits = 0;
    const ALL_BITS: Self::RegisterBits = 0;

    fn to_str(self) -> &'static str {
        "unknown"
    }

    fn encode(self) -> u32 {
        self.0
    }

    fn decode(value: u32) -> Self {
        Self(value)
    }

    fn bit_index(self) -> u8 {
        // this just truncates the register but it's fine since it's only used for some small test
        // cases for parcopy.
        self.0 as u8
    }

    fn get_bit(self, _bits: &Self::RegisterBits) -> bool {
        false
    }

    fn set_bit(self, _bits: &mut Self::RegisterBits, _bit: bool) {}

    fn allocate_reg(_free_bits: Self::RegisterBits, _class: Self::Class) -> Option<Self> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpType {
    Non,
    Reg,
    Mem,
    Imm,
    Blk,
    Fun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpUsage {
    Def = 0b01,
    Use = 0b10,
    DefUse = 0b11,
}

pub fn parallel_copy(
    mc: ModuleOf<Mc>,
    args: &[MCReg],
) -> (FunctionId, impl crate::IntoArgs<'_>, TypeId) {
    let f = crate::FunctionId {
        module: mc.id(),
        function: Mc::Copy.id(),
    };
    (f, ((), args), TypeId::UNIT)
}

pub fn parallel_copy_args(
    mc: ModuleOf<Mc>,
    args: &[MCReg],
    unit: crate::TypeId,
) -> (FunctionId, impl crate::IntoArgs<'_>, crate::TypeId) {
    let f = crate::FunctionId {
        module: mc.id(),
        function: Mc::AssignBlockArgs.id(),
    };
    (f, ((), args), unit)
}

pub fn used_physical_registers<R: Register>(ir: &IrModify, env: &Environment) -> R::RegisterBits {
    let mut bits = R::NO_BITS;
    for i in 0..ir.inst_count() {
        let inst = ir.get_inst(Ref::index(i));
        for arg in ir.args_iter(inst, env) {
            let (Argument::MCReg(r) | Argument::MCRegOffset(MCRegOffset(r, _))) = arg else {
                continue;
            };
            let Some(phys) = r.phys::<R>() else {
                continue;
            };
            phys.set_bit(&mut bits, true);
        }
    }
    bits
}
