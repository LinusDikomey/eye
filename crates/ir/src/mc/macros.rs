#[macro_export]
macro_rules! ident_count {
    () => {
        0
    };
    ($i: ident $($rest: ident)*) => {
        1 + $crate::mc::macros::ident_count!($($rest)*)
    };
}
pub use crate::ident_count;

#[macro_export]
macro_rules! first_reg {
    () => {
        compile_error!("Register list can't be empty");
    };
    ($id: ident $($rest: ident)*) => {
        Self::$id
    };
}
pub use crate::first_reg;

#[macro_export]
macro_rules! registers {
    (
        $bits: ident
        $($size: ident => $($variant: ident)*;)*
        !secondary:
        $($secondary_size: ident => $($secondary_variant: ident)*;)*
    ) => {
        #[rustfmt::skip]
        #[allow(non_camel_case_types)]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        #[repr(u8)]
        pub enum Reg {
            $($($variant,)*)*
        }
        impl Reg {
            pub fn class(self) -> RegClass {
                match self {
                    $($(Self::$variant => RegClass::$size,)*)*
                }
            }
        }

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        #[repr(u8)]
        pub enum RegClass {
            $($size,)*
            $($secondary_size,)*
        }
        impl From<RegClass> for u8 {
            fn from(value: RegClass) -> u8 {
                value as u8
            }
        }
        impl ::core::convert::TryFrom<u8> for RegClass {
            type Error = ();

            fn try_from(value: u8) -> ::core::result::Result<Self, ()> {
                let count = $crate::mc::macros::ident_count!($($($variant)*)*);
                if value >= count {
                    return Err(());
                }
                unsafe { std::mem::transmute(value) }
            }
        }

        impl $crate::mc::Register for Reg {
            const DEFAULT: Self = $crate::mc::macros::first_reg!($($($variant)*)*);
            const NO_BITS: RegBits = RegBits::new();
            const ALL_BITS: RegBits = RegBits::all();
            type RegisterBits = RegBits;
            type Class = RegClass;

            fn to_str(self) -> &'static str {
                match self {
                    $($(Self::$variant => stringify!($variant),)*)*
                }
            }

            fn encode(self) -> u32 {
                self as u32
            }

            fn decode(value: u32) -> Self {
                let count = $crate::mc::macros::ident_count!($($($variant)*)*);
                assert!(value < count, "can't decode invalid physical register {}", value);
                unsafe { std::mem::transmute(value as u8) }
            }

            fn bit_index(self) -> u8 {
                self.index()
            }

            fn get_bit(self, bits: &RegBits) -> bool {
                bits.get(self)
            }

            fn set_bit(self, bits: &mut RegBits, set: bool) {
                bits.set(self, set);
            }

            fn allocate_reg(free: Self::RegisterBits, class: RegClass) -> Option<Self> {
                match class {
                    $(
                        RegClass::$size => {
                            $(if Self::$variant.get_bit(&free) {
                                return Some(Self::$variant)
                            })*
                        }
                    )*
                    $(
                        RegClass::$secondary_size => {
                            $(if Self::$secondary_variant.get_bit(&free) {
                                return Some(Self::$secondary_variant)
                            })*
                        }
                    )*
                }
                None
            }
        }
        impl ::core::fmt::Display for Reg {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(f, "{}", $crate::mc::Register::to_str(*self))
            }
        }


        impl ::core::convert::TryFrom<$crate::Argument<'_>> for Reg {
            type Error = $crate::ArgError;
            fn try_from(value: $crate::Argument<'_>) -> Result<Self, Self::Error> {
                let $crate::Argument::MCReg(value) = value else {
                    return Err($crate::ArgError {
                        expected: $crate::Parameter::MCReg($crate::Usage::Def),
                        found: value.parameter_ty(),
                    });
                };
                Ok(value
                    .phys()
                    .expect("expected physical register, found virtual"))
            }
        }

        pub struct RegOffset(pub Reg, pub u32);
        impl ::core::convert::TryFrom<$crate::Argument<'_>> for RegOffset {
            type Error = $crate::ArgError;
            fn try_from(value: $crate::Argument<'_>) -> Result<Self, Self::Error> {
                let $crate::Argument::MCRegOffset(value) = value else {
                    return Err($crate::ArgError {
                        expected: $crate::Parameter::MCRegOffset($crate::Usage::Use, $crate::Imm::I32),
                        found: value.parameter_ty(),
                    });
                };
                Ok(RegOffset(value.0.phys().expect("expected physical register, found virtual"), value.1))
            }
        }
    };
}
pub use crate::registers;
