use std::{fmt, num::NonZeroU32};

mod float;
mod int;

pub use float::FloatType;
pub use int::IntType;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum Primitive {
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    Type,
}
impl Primitive {
    pub fn as_int(self) -> Option<IntType> {
        use Primitive::*;
        Some(match self {
            I8 => IntType::I8,
            I16 => IntType::I16,
            I32 => IntType::I32,
            I64 => IntType::I64,
            I128 => IntType::I128,
            U8 => IntType::U8,
            U16 => IntType::U16,
            U32 => IntType::U32,
            U64 => IntType::U64,
            U128 => IntType::U128,
            _ => return None,
        })
    }
    pub fn is_int(self) -> bool {
        self.as_int().is_some()
    }

    pub fn as_float(self) -> Option<FloatType> {
        use Primitive::*;
        Some(match self {
            F32 => FloatType::F32,
            F64 => FloatType::F64,
            _ => return None,
        })
    }
    pub fn is_float(self) -> bool {
        self.as_float().is_some()
    }

    pub fn token_len(self) -> NonZeroU32 {
        use Primitive as P;
        let len = match self {
            P::I8 | P::U8 => NonZeroU32::new(2).unwrap(),
            P::I16 | P::I32 | P::I64 | P::U16 | P::U32 | P::U64 | P::F32 | P::F64 => {
                NonZeroU32::new(3).unwrap()
            }
            P::I128 | P::U128 | P::Type => NonZeroU32::new(4).unwrap(),
        };
        debug_assert_eq!(Into::<&'static str>::into(self).len(), len.get() as usize);
        len
    }

    pub const fn into_str(self) -> &'static str {
        use Primitive::*;
        match self {
            I8 => "i8",
            U8 => "u8",
            I16 => "i16",
            U16 => "u16",
            I32 => "i32",
            U32 => "u32",
            I64 => "i64",
            U64 => "u64",
            I128 => "i128",
            U128 => "u128",
            F32 => "f32",
            F64 => "f64",
            Type => "type",
        }
    }
}
impl From<Primitive> for &'static str {
    fn from(value: Primitive) -> Self {
        value.into_str()
    }
}
impl fmt::Display for Primitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.into_str())
    }
}
