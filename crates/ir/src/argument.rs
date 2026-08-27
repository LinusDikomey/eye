use crate::{
    ArgsIter, BlockId, BlockTarget, FunctionId, GlobalId, MCReg, MCRegOffset, Parameter, Ref,
    TypeId, Usage,
};

#[derive(Debug, Clone)]
pub enum Argument<'a> {
    Ref(Ref),
    BlockId(BlockId),
    BlockTarget(BlockTarget<'a>),
    Int(u64),
    Float(f64),
    TypeId(TypeId),
    FunctionId(FunctionId),
    GlobalId(GlobalId),
    MCReg(MCReg),
    MCRegOffset(MCRegOffset),
}
impl Argument<'_> {
    /// Gets the parameter type based on the value. Note that the exact type may differ
    /// since integer width and usage metadata are not clear based on a single value
    pub fn parameter_ty(&self) -> Parameter {
        match self {
            Argument::Ref(_) => Parameter::Ref,
            Argument::BlockId(_) => Parameter::BlockId,
            Argument::BlockTarget(_) => Parameter::BlockTarget,
            Argument::Int(_) => Parameter::Int,
            Argument::Float(_) => Parameter::Float,
            Argument::TypeId(_) => Parameter::TypeId,
            Argument::FunctionId(_) => Parameter::FunctionId,
            Argument::GlobalId(_) => Parameter::GlobalId,
            Argument::MCReg(_) => Parameter::MCReg(crate::Usage::Def), // TODO: setting def here ok?
            Argument::MCRegOffset(_) => Parameter::MCRegOffset(crate::Usage::Use, crate::Imm::U32),
        }
    }
}
impl From<Ref> for Argument<'_> {
    fn from(value: Ref) -> Self {
        Self::Ref(value)
    }
}
impl TryFrom<Argument<'_>> for Ref {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::Ref(value) = value else {
            return Err(ArgError {
                expected: Parameter::Ref,
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}
impl From<BlockId> for Argument<'_> {
    fn from(value: BlockId) -> Self {
        Self::BlockId(value)
    }
}
impl TryFrom<Argument<'_>> for BlockId {
    type Error = ArgError;
    fn try_from(value: Argument<'_>) -> Result<Self, Self::Error> {
        let Argument::BlockId(value) = value else {
            return Err(ArgError {
                expected: Parameter::BlockId,
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}

impl<'a> From<BlockTarget<'a>> for Argument<'a> {
    fn from(value: BlockTarget<'a>) -> Self {
        Self::BlockTarget(value)
    }
}
impl<'a> TryFrom<Argument<'a>> for BlockTarget<'a> {
    type Error = ArgError;
    fn try_from(value: Argument<'a>) -> Result<Self, Self::Error> {
        let Argument::BlockTarget(value) = value else {
            return Err(ArgError {
                expected: Parameter::BlockTarget,
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}
impl From<u64> for Argument<'_> {
    fn from(value: u64) -> Self {
        Self::Int(value)
    }
}
impl TryFrom<Argument<'_>> for u64 {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::Int(value) = value else {
            return Err(ArgError {
                expected: Parameter::Int,
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}
impl From<u32> for Argument<'_> {
    fn from(value: u32) -> Self {
        Self::Int(value.into())
    }
}
impl TryFrom<Argument<'_>> for u32 {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::Int(value) = value else {
            return Err(ArgError {
                expected: Parameter::Int32,
                found: value.parameter_ty(),
            });
        };
        value.try_into().map_err(|_| ArgError {
            expected: Parameter::Int32,
            found: Parameter::Int,
        })
    }
}
impl From<f64> for Argument<'_> {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}
impl TryFrom<Argument<'_>> for f64 {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::Float(value) = value else {
            return Err(ArgError {
                expected: Parameter::Float,
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}
impl From<TypeId> for Argument<'_> {
    fn from(value: TypeId) -> Self {
        Self::TypeId(value)
    }
}
impl TryFrom<Argument<'_>> for TypeId {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::TypeId(value) = value else {
            return Err(ArgError {
                expected: Parameter::TypeId,
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}
impl From<FunctionId> for Argument<'_> {
    fn from(value: FunctionId) -> Self {
        Self::FunctionId(value)
    }
}
impl TryFrom<Argument<'_>> for FunctionId {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::FunctionId(value) = value else {
            return Err(ArgError {
                expected: Parameter::FunctionId,
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}
impl From<GlobalId> for Argument<'_> {
    fn from(value: GlobalId) -> Self {
        Self::GlobalId(value)
    }
}
impl TryFrom<Argument<'_>> for GlobalId {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::GlobalId(value) = value else {
            return Err(ArgError {
                expected: Parameter::GlobalId,
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}
impl From<MCReg> for Argument<'_> {
    fn from(value: MCReg) -> Self {
        Self::MCReg(value)
    }
}
impl TryFrom<Argument<'_>> for MCReg {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::MCReg(value) = value else {
            return Err(ArgError {
                expected: Parameter::MCReg(Usage::Def),
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}
impl From<MCRegOffset> for Argument<'_> {
    fn from(value: MCRegOffset) -> Self {
        Self::MCRegOffset(value)
    }
}
impl TryFrom<Argument<'_>> for MCRegOffset {
    type Error = ArgError;
    fn try_from(value: Argument) -> Result<Self, Self::Error> {
        let Argument::MCRegOffset(value) = value else {
            return Err(ArgError {
                expected: Parameter::MCRegOffset(Usage::Use, crate::Imm::I32),
                found: value.parameter_ty(),
            });
        };
        Ok(value)
    }
}

pub trait IntoArgs<'a> {
    type Args: ExactSizeIterator<Item = Argument<'a>>;

    fn into_args(self) -> Self::Args;
}
impl<'a> IntoArgs<'a> for Vec<Ref> {
    // PERF: specifying function pointer here in Map type might worsen performance since it becomes
    // a dynamic call
    type Args = std::iter::Map<std::vec::IntoIter<Ref>, fn(Ref) -> Argument<'a>>;
    fn into_args(self) -> Self::Args {
        self.into_iter().map(Argument::Ref)
    }
}
impl<'a> IntoArgs<'a> for Vec<Argument<'a>> {
    type Args = std::vec::IntoIter<Argument<'a>>;
    fn into_args(self) -> Self::Args {
        self.into_iter()
    }
}
impl<'a> IntoArgs<'a> for () {
    type Args = std::array::IntoIter<Argument<'a>, 0>;

    fn into_args(self) -> Self::Args {
        [].into_iter()
    }
}
impl<'a, T: Into<Argument<'a>>> IntoArgs<'a> for T {
    type Args = std::array::IntoIter<Argument<'a>, 1>;

    fn into_args(self) -> Self::Args {
        [self.into()].into_iter()
    }
}

impl<'a, T: Into<Argument<'a>>> IntoArgs<'a> for (T,) {
    type Args = std::array::IntoIter<Argument<'a>, 1>;

    fn into_args(self) -> Self::Args {
        [self.0.into()].into_iter()
    }
}

impl<'a, A0: Into<Argument<'a>>, A1: Into<Argument<'a>>> IntoArgs<'a> for (A0, A1) {
    type Args = std::array::IntoIter<Argument<'a>, 2>;

    fn into_args(self) -> Self::Args {
        [self.0.into(), self.1.into()].into_iter()
    }
}
impl<'a, A0: Into<Argument<'a>>, A1: Into<Argument<'a>>, A2: Into<Argument<'a>>> IntoArgs<'a>
    for (A0, A1, A2)
{
    type Args = std::array::IntoIter<Argument<'a>, 3>;

    fn into_args(self) -> Self::Args {
        [self.0.into(), self.1.into(), self.2.into()].into_iter()
    }
}
impl<
    'a,
    A0: Into<Argument<'a>>,
    A1: Into<Argument<'a>>,
    A2: Into<Argument<'a>>,
    A3: Into<Argument<'a>>,
> IntoArgs<'a> for (A0, A1, A2, A3)
{
    type Args = std::array::IntoIter<Argument<'a>, 4>;

    fn into_args(self) -> Self::Args {
        [self.0.into(), self.1.into(), self.2.into(), self.3.into()].into_iter()
    }
}
impl<
    'a,
    A0: Into<Argument<'a>>,
    A1: Into<Argument<'a>>,
    A2: Into<Argument<'a>>,
    A3: Into<Argument<'a>>,
    A4: Into<Argument<'a>>,
> IntoArgs<'a> for (A0, A1, A2, A3, A4)
{
    type Args = std::array::IntoIter<Argument<'a>, 5>;

    fn into_args(self) -> Self::Args {
        [
            self.0.into(),
            self.1.into(),
            self.2.into(),
            self.3.into(),
            self.4.into(),
        ]
        .into_iter()
    }
}
impl<'a, A: IntoArgs<'a>, V: Copy + Into<Argument<'a>>> IntoArgs<'a> for (A, &'a [V]) {
    type Args = VarargsIntoArgs<'a, A, V>;

    fn into_args(self) -> Self::Args {
        VarargsIntoArgs {
            a: self.0.into_args(),
            v: self.1,
        }
    }
}

pub struct VarargsIntoArgs<'a, A: IntoArgs<'a>, V> {
    a: A::Args,
    v: &'a [V],
}
impl<'a, A: IntoArgs<'a>, V: Copy + Into<Argument<'a>>> Iterator for VarargsIntoArgs<'a, A, V> {
    type Item = Argument<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.a.next().or_else(|| {
            self.v.split_first().map(|(x, xs)| {
                self.v = xs;
                (*x).into()
            })
        })
    }
}
impl<'a, A: IntoArgs<'a>, V: Copy + Into<Argument<'a>>> ExactSizeIterator
    for VarargsIntoArgs<'a, A, V>
{
    fn len(&self) -> usize {
        self.a.len() + self.v.len()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ArgsError {
    CountMismatch { expected: usize, found: usize },
    InvalidType { index: usize, error: ArgError },
}

#[derive(Debug, Clone, Copy)]
pub struct ArgError {
    pub expected: Parameter,
    pub found: Parameter,
}

pub trait Args<'args>: Sized {
    fn get<'a>(args: ArgsIter<'a, 'args>) -> Result<Self, ArgsError>;
}

macro_rules! count {
    () => {
        0
    };
    ($t: ident $($rest: ident)*) => {
       1 + count!($($rest)*)
    };
}
macro_rules! args_impl {
    ($($t: ident)*) => {
        #[allow(unused_parens, unused_assignments)]
        impl<'args, $($t),*> Args<'args> for ($($t),*)
        where
        $(
            $t: TryFrom<Argument<'args>, Error = ArgError>,
        )*
        {
            fn get<'a>(
                mut it: ArgsIter<'a, 'args>,
            ) -> Result<Self, ArgsError> {
                let expected_count = count!($($t)*);
                let mut found_so_far = 0;
                let value = (
                    $(
                        {
                            let arg: $t = it
                                .next()
                                .ok_or(ArgsError::CountMismatch {
                                    expected: expected_count,
                                    found: found_so_far,
                                })
                                ?
                                .try_into()
                                .map_err(|error| ArgsError::InvalidType { index: 0, error })?;
                            found_so_far += 1;
                            arg
                        }
                    ),*
                );
                let remaining_count = it.count();
                if remaining_count > 0 {
                    return Err(ArgsError::CountMismatch {
                        expected: 1,
                        found: expected_count + remaining_count,
                    });
                }
                Ok(value)
            }
        }
    };
}
args_impl!(A);
args_impl!(A B);
args_impl!(A B C);
args_impl!(A B C D);
args_impl!(A B C D E);
args_impl!(A B C D E F);
