use error::Error;

use crate::{
    Compiler, Type,
    compiler::Generics,
    hir::CastType,
    types::{BaseType, TypeFull},
};

pub fn check(
    from_ty: Type,
    to_ty: Type,
    compiler: &Compiler,
    generics: &Generics,
) -> (CastType, Option<Error>) {
    let mut error = None;
    if from_ty == to_ty {
        return (CastType::Noop, Some(Error::TrivialCast));
    }
    let cast = match (compiler.types.lookup(from_ty), compiler.types.lookup(to_ty)) {
        (TypeFull::Instance(BaseType::Invalid, _), _)
        | (_, TypeFull::Instance(BaseType::Invalid, _)) => CastType::Invalid,
        (TypeFull::Instance(a, _), TypeFull::Instance(b, _)) => {
            match (a.as_int(), a.as_float(), b.as_int(), b.as_float()) {
                _ if a == BaseType::Pointer && b == BaseType::Pointer => CastType::Noop,
                (Some(from), _, Some(to), _) => CastType::Int { from, to },
                (Some(from), _, _, Some(to)) => CastType::IntToFloat { from, to },
                (_, Some(from), Some(to), _) => CastType::FloatToInt { from, to },
                (_, Some(from), _, Some(to)) => CastType::Float { from, to },
                (Some(from), _, _, _) if b == BaseType::Pointer => CastType::IntToPtr { from },
                (_, _, Some(to), _) if a == BaseType::Pointer => CastType::PtrToInt { to },
                _ => CastType::Invalid,
            }
        }
        _ => {
            error = Some(Error::InvalidCast {
                from: compiler.display_type(from_ty, generics).to_string(),
                to: compiler.display_type(to_ty, generics).to_string(),
            });
            CastType::Invalid
        }
    };
    (cast, error)
}
