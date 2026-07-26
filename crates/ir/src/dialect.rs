use std::num::NonZeroU64;

use crate::{Parameter, Type, instructions, primitives};

primitives! {
    I1 = 1
    I8 = 1
    I16 = 2
    I32 = 4
    I64 = 8
    I128 = 16
    U8 = 1
    U16 = 2
    U32 = 4
    U64 = 8
    U128 = 16
    F32 = 4
    F64 = 8
    Ptr = 8
}
impl Primitive {
    pub fn byte_size(self) -> NonZeroU64 {
        NonZeroU64::new(match self {
            Self::I1 | Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            // FIXME(target ptr size)
            Self::I64 | Self::U64 | Self::F64 | Self::Ptr => 8,
            Self::I128 | Self::U128 => 16,
        })
        .unwrap()
    }

    pub fn is_int(self) -> bool {
        self.is_signed_int() || self.is_unsigned_int()
    }

    pub fn is_signed_int(self) -> bool {
        matches!(
            self,
            Self::I1 | Self::I8 | Self::I16 | Self::I32 | Self::I64
        )
    }

    pub fn is_unsigned_int(self) -> bool {
        matches!(self, Self::U8 | Self::U16 | Self::U32 | Self::U64)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    #[must_use]
    /// returns the unsigned integer type of the same size
    pub fn into_unsigned(self) -> Self {
        match self {
            Self::I1 | Self::I8 | Self::U8 => Self::U8,
            Self::I16 | Self::U16 => Self::U16,
            Self::I32 | Self::U32 | Self::F32 => Self::U32,
            // FIXME(target ptr size)
            Self::I64 | Self::U64 | Self::F64 | Self::Ptr => Self::U64,
            Self::I128 | Self::U128 => Self::U128,
        }
    }
}

pub fn register_all(env: &mut crate::Environment) {
    env.get_dialect_module::<Arith>();
    env.get_dialect_module::<Tuple>();
    env.get_dialect_module::<Mem>();
    env.get_dialect_module::<Cf>();
}

instructions! {
    Arith "arith" ArithInsts

    Int value: Int !pure;
    Float value: Float !pure;

    Neg value: Ref !pure;
    Not value: Ref !pure;

    Add l: Ref r: Ref !pure;
    Sub l: Ref r: Ref !pure;
    Mul l: Ref r: Ref !pure;
    Div l: Ref r: Ref !pure;
    Rem l: Ref r: Ref !pure;

    Or  l: Ref r: Ref !pure;
    And l: Ref r: Ref !pure;
    Eq  l: Ref r: Ref !pure;
    NE  l: Ref r: Ref !pure;
    LT  l: Ref r: Ref !pure;
    GT  l: Ref r: Ref !pure;
    LE  l: Ref r: Ref !pure;
    GE  l: Ref r: Ref !pure;

    Xor l: Ref r: Ref !pure;
    Shl l: Ref r: Ref !pure;
    Shr l: Ref r: Ref !pure;
    Rol l: Ref r: Ref !pure;
    Ror l: Ref r: Ref !pure;

    CastInt value: Ref !pure;
    CastFloat value: Ref !pure;
    CastIntToFloat value: Ref !pure;
    CastFloatToInt value: Ref !pure;
}

impl Arith {
    pub fn int_binop(self, ty: Type) -> Option<ArithIntBinOp> {
        let Type::Primitive(p) = ty else {
            return None;
        };
        let p = Primitive::try_from(p).unwrap();
        if !p.is_int() {
            return None;
        }
        let signed = p.is_signed_int();
        Some(match self {
            Self::Int
            | Self::Float
            | Self::Neg
            | Self::Not
            | Self::CastInt
            | Self::CastFloat
            | Self::CastIntToFloat
            | Self::CastFloatToInt => return None,
            Self::Add => ArithIntBinOp::new(|a, b| a.wrapping_add(b))
                .commutative()
                .associative()
                .identity(0),
            Self::Sub => ArithIntBinOp::new(|a, b| a.wrapping_sub(b)).rhs_identity(0),
            Self::Mul => ArithIntBinOp::new(if signed {
                |a, b| (a as i64).wrapping_mul(b as i64) as u64
            } else {
                |a, b| a.wrapping_mul(b)
            })
            .commutative()
            .associative()
            .identity(1),
            Self::Div => ArithIntBinOp::new(if signed {
                |a, b| (a as i64).wrapping_div(b as i64) as u64
            } else {
                |a, b| a.wrapping_div(b)
            })
            .rhs_identity(1),
            Self::Rem => ArithIntBinOp::new(|a, b| a.wrapping_rem(b)).rhs_identity(1),
            Self::Or => ArithIntBinOp::new(|a, b| a | b)
                .commutative()
                .associative()
                .identity(0),
            Self::And => ArithIntBinOp::new(|a, b| a & b)
                .commutative()
                .identity(u64::MAX),
            Self::Eq => ArithIntBinOp::new(|a, b| (a == b) as u64).commutative(),
            Self::NE => ArithIntBinOp::new(|a, b| (a != b) as u64).commutative(),
            Self::LT => ArithIntBinOp::new(if signed {
                |a, b| ((a as i64) < b as i64) as u64
            } else {
                |a, b| (a < b) as u64
            }),
            Self::GT => ArithIntBinOp::new(if signed {
                |a, b| ((a as i64) > b as i64) as u64
            } else {
                |a, b| (a > b) as u64
            }),
            Self::LE => ArithIntBinOp::new(if signed {
                |a, b| ((a as i64) <= b as i64) as u64
            } else {
                |a, b| (a <= b) as u64
            }),
            Self::GE => ArithIntBinOp::new(if signed {
                |a, b| ((a as i64) >= b as i64) as u64
            } else {
                |a, b| (a >= b) as u64
            }),
            Arith::Xor => ArithIntBinOp::new(|a, b| a ^ b)
                .commutative()
                .associative()
                .identity(0),
            Arith::Shl => ArithIntBinOp::new(|a, b| a.wrapping_shl(b as u32)).rhs_identity(0),
            Arith::Shr => ArithIntBinOp::new(|a, b| a.wrapping_shr(b as u32)).rhs_identity(0),
            // TODO: need to do rotate on the correct integer width
            Arith::Rol => ArithIntBinOp::new(|a, b| a.rotate_left(b as u32)).rhs_identity(0),
            Arith::Ror => ArithIntBinOp::new(|a, b| a.rotate_right(b as u32)).rhs_identity(0),
        })
    }
}

pub struct ArithIntBinOp {
    pub fold: fn(u64, u64) -> u64,
    pub commutative: bool,
    pub associative: bool,
    pub lhs_identity: Option<u64>,
    pub rhs_identity: Option<u64>,
}
impl ArithIntBinOp {
    fn new(fold: fn(u64, u64) -> u64) -> Self {
        Self {
            fold,
            commutative: false,
            associative: false,
            lhs_identity: None,
            rhs_identity: None,
        }
    }

    fn commutative(mut self) -> Self {
        self.commutative = true;
        self
    }

    fn associative(mut self) -> Self {
        self.associative = true;
        self
    }

    fn identity(mut self, identity: u64) -> Self {
        self.lhs_identity = Some(identity);
        self.rhs_identity = Some(identity);
        self
    }

    fn rhs_identity(mut self, identity: u64) -> Self {
        self.rhs_identity = Some(identity);
        self
    }
}

instructions! {
    Tuple "tuple" TupleInsts

    MemberValue tuple: Ref element: Int32 !pure;
    InsertMember tuple: Ref element: Int32 value: Ref !pure;
}

instructions! {
    Mem "mem" MemInsts

    Decl ty: TypeId !pure;
    Load ptr: Ref !pure;
    Store ptr: Ref value: Ref => unit;
    MemberPtr ptr: Ref ty: TypeId idx: Int32 !pure;
    IntToPtr value: Ref !pure;
    PtrToInt value: Ref !pure;
    Global global: GlobalId !pure;
    ArrayIndex array_ptr: Ref elem_ty: TypeId a: Ref !pure;
    /// interprets the offset as i64
    Offset ptr: Ref offset: Int !pure;
    FunctionPtr function: FunctionId !pure;
    CallPtr ptr: Ref !varargs = Some(Parameter::Ref);
}

instructions! {
    Cf "cf" CfInsts

    Goto<'a> target: BlockTarget<'a> !terminator => unit;
    Branch cond: Ref on_true: BlockTarget<'static> on_false: BlockTarget<'static> !terminator => unit;
    Ret value: Ref !terminator => unit;
}
