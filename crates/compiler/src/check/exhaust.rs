use dmap::DHashMap;

use crate::{
    Compiler, InvalidTypeError, Type, compiler::Instance, helpers::IteratorExt, types::TypeFull,
};

#[derive(Clone, Copy)]
pub struct SignedInt(pub u128, pub bool);
#[derive(Clone, Debug, PartialEq, Eq)]
// FIXME: exhaustion of tuples/enum arguments is wrong
#[derive(Default)]
pub enum Exhaustion {
    /// no values exhausted
    #[default]
    None,
    /// all values exhausted
    Full,
    UnsignedInt(Vec<Range>),
    SignedInt {
        neg: Vec<Range>,
        pos: Vec<Range>,
    },
    Bool {
        true_: bool,
        false_: bool,
    },
    Enum(DHashMap<String, Vec<Exhaustion>>),
    Tuple(Vec<Exhaustion>),
    Invalid,
}
impl Exhaustion {
    pub fn is_trivially_exhausted(&self) -> bool {
        match self {
            Self::Full
            | Self::Bool {
                true_: true,
                false_: true,
            } => true,
            Self::Tuple(_) => false, //TODO: with proper tuple exhaustion, check trivial tuples
            _ => false,
        }
    }

    /// Return value:
    /// Ok(true) means it's exhausted.
    /// Ok(false) means it's not exhausted.
    /// Err means a type is mismatched/invalid
    pub fn is_exhausted(&self, ty: Type, compiler: &Compiler) -> Result<bool, InvalidTypeError> {
        let types = &compiler.types;
        Ok(match self {
            Exhaustion::None => compiler.is_uninhabited(ty, &Instance::EMPTY)?,
            Exhaustion::Full => true,
            Exhaustion::UnsignedInt(ranges) => match types.lookup(ty) {
                TypeFull::Instance(base, _) if base.is_int() => {
                    let int = base.as_int().unwrap();
                    !int.is_signed()
                        && ranges
                            .first()
                            .is_some_and(|r| r.start == 0 && r.end >= int.max())
                }
                _ => return Err(InvalidTypeError),
            },
            Exhaustion::SignedInt { neg, pos } => match types.lookup(ty) {
                TypeFull::Instance(base, _) if base.is_int() => {
                    let int = base.as_int().unwrap();

                    pos.first()
                        .is_some_and(|r| r.start == 0 && r.end >= int.max())
                        && neg
                            .first()
                            .is_some_and(|r| r.start == 0 && r.end >= int.min())
                }
                _ => return Err(InvalidTypeError),
            },
            Exhaustion::Enum(_) => {
                todo!("check enums")
                /*
                match ty {
                    TypeInfo::Resolved(symbol, generics) => {
                        let ResolvedTypeBody::Enum(enum_def) = &symbols.get_type(symbol).body else { return None };
                        for (name, (_, _, arg_types)) in &enum_def.variants {
                            let Some(args) = exhausted_variants.get(name) else { return Some(false) };
                            if args.len() != arg_types.len() { return None };
                            for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
                                let arg_ty = arg_ty.as_info(types, |i| generics.nth(i as u32).into());
                                let arg_ty = types.get_info_or_idx(arg_ty);
                                if !arg.is_exhausted(arg_ty, types, symbols)? {
                                    return Some(false)
                                }
                            }
                        }
                    }
                    TypeInfo::Enum { .. } => {
                        for i in 0..count as usize {
                            let TypeInfo::Tuple(variant)
                            let (name, arg_types) = &types.get_enum_variants(variants)[i];
                            let Some(args) = exhausted_variants.get(name) else { return Some(false) };
                            if args.len() != arg_types.len() { return None };
                            for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
                                if !arg.is_exhausted(types[arg_ty], types, symbols)? {
                                    return Some(false)
                                }
                            }
                        }
                    }
                    _ => return None
                }
                true
                */
            }
            &Exhaustion::Bool { true_, false_ } => true_ && false_,
            Exhaustion::Tuple(members) => {
                let member_types = match types.lookup(ty) {
                    TypeFull::Tuple {
                        members: member_types,
                        named_members: named_member_types,
                    } => {
                        if member_types.len() + named_member_types.len() != members.len() {
                            return Err(InvalidTypeError);
                        };
                        member_types
                            .iter()
                            .copied()
                            .chain(named_member_types.iter().map(|&(_, ty)| ty))
                    }
                    _ => return Err(InvalidTypeError),
                };

                members
                    .iter()
                    .zip(member_types)
                    .try_all(|(member, ty)| member.is_exhausted(ty, compiler))?
            }
            Exhaustion::Invalid => return Err(InvalidTypeError),
        })
    }

    pub fn exhaust_full(&mut self) {
        *self = Exhaustion::Full;
    }

    /// exhaust an integer value and return true if it wasn't covered yet
    pub fn exhaust_int(&mut self, x: SignedInt) -> bool {
        self.exhaust_int_range(x, x)
    }

    /// exhaust an integer range and return true if it wasn't fully covered yet
    pub fn exhaust_int_range(&mut self, mut start: SignedInt, mut end: SignedInt) -> bool {
        fn exhaust(ranges: &mut Vec<Range>, start: u128, end: u128) -> bool {
            if ranges.is_empty() {
                ranges.push(Range { start, end });
                true
            } else {
                if end + 1 < ranges.first().unwrap().start {
                    ranges.insert(0, Range { start, end });
                    return true;
                }
                for i in 0..ranges.len() - 1 {
                    let next_range = ranges[i + 1];
                    let range = &mut ranges[i];
                    if let Some(merged) = merge_ranges(*range, Range { start, end }) {
                        if let Some(merged) = merge_ranges(merged, next_range) {
                            debug_assert!(merged.start < range.start || merged.end > range.end);
                            *range = merged;
                            ranges.remove(i + 1);
                            return true;
                        }
                        debug_assert!(merged.start < range.start || merged.end > range.end);
                        *range = merged;
                        return true;
                    } else if end < next_range.start {
                        ranges.insert(i, Range { start, end });
                        return true;
                    }
                }
                let last = ranges.last_mut().unwrap();
                if let Some(merged) = merge_ranges(Range { start, end }, *last) {
                    debug_assert!(
                        merged.start < last.start || merged.end > last.end || (merged == *last)
                    );
                    *last = merged;
                    true
                } else {
                    ranges.push(Range { start, end });
                    true
                }
            }
        }
        if start.1 && start.0 == 0 {
            start = SignedInt(0, false);
        }
        if end.1 && end.0 == 0 {
            end = SignedInt(0, false);
        }
        match self {
            Self::None => {
                *self = match (start.1, end.1) {
                    (false, false) => Self::UnsignedInt(vec![Range::new(start.0, end.0)]),
                    (true, false) => Self::SignedInt {
                        neg: vec![Range::new(0, start.0)],
                        pos: vec![Range::new(0, end.0)],
                    },
                    (true, true) => Self::SignedInt {
                        neg: vec![Range::new(start.0, end.0)],
                        pos: vec![],
                    },
                    (false, true) => unreachable!(),
                };
                true
            }
            Self::UnsignedInt(ranges) => {
                if start.1 {
                    let Self::UnsignedInt(mut ranges) = std::mem::take(self) else {
                        unreachable!()
                    };
                    *self = Self::SignedInt {
                        neg: vec![Range::new(if end.1 { end.0 } else { 0 }, start.0)],
                        pos: if end.1 {
                            ranges
                        } else {
                            exhaust(&mut ranges, 0, end.0);
                            ranges
                        },
                    };
                    true
                } else {
                    exhaust(ranges, start.0, end.0)
                }
            }
            Self::SignedInt { neg, pos } => {
                if start.1 {
                    let mut new_values = exhaust(neg, if end.1 { end.0 } else { 0 }, start.0);
                    if !end.1 {
                        new_values |= exhaust(pos, 0, end.0);
                    }
                    new_values
                } else {
                    exhaust(pos, start.0, end.0)
                }
            }
            Self::Full => false,
            _ => {
                *self = Self::Invalid;
                false
            }
        }
    }

    pub fn exhaust_bool(&mut self, b: bool) -> bool {
        match self {
            Self::None => {
                *self = Self::Bool {
                    true_: b,
                    false_: !b,
                };
                true
            }
            Self::Bool { true_, false_ } => {
                if b {
                    let prev = *true_;
                    *true_ = true;
                    prev
                } else {
                    let prev = *false_;
                    *false_ = true;
                    prev
                }
            }
            Self::Full => false,
            _ => {
                *self = Self::Invalid;
                false
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: u128,
    pub end: u128,
}
impl Range {
    pub fn new(start: u128, end: u128) -> Self {
        Self { start, end }
    }
}
fn merge_ranges(a: Range, b: Range) -> Option<Range> {
    if a.start < b.start {
        if a.end < b.start.saturating_sub(1) {
            None
        } else {
            Some(Range::new(a.start, a.end.max(b.end)))
        }
    } else if b.end < a.start.saturating_sub(1) {
        None
    } else {
        Some(Range::new(b.start, b.end.max(a.end)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        let mut e = Exhaustion::None;
        e.exhaust_int(SignedInt(6, false));
        e.exhaust_int(SignedInt(1, false));
        e.exhaust_int(SignedInt(3, false));
        e.exhaust_int(SignedInt(2, false));
        e.exhaust_int(SignedInt(5, true));
        e.exhaust_int(SignedInt(8, true));
        e.exhaust_int_range(SignedInt(7, true), SignedInt(6, true));
        debug_assert_eq!(
            e,
            Exhaustion::SignedInt {
                neg: vec![Range::new(5, 8)],
                pos: vec![Range::new(1, 3), Range::new(6, 6)]
            }
        );

        let mut e = Exhaustion::None;
        e.exhaust_int_range(SignedInt(3, true), SignedInt(5, false));
        debug_assert_eq!(
            e,
            Exhaustion::SignedInt {
                neg: vec![Range::new(0, 3)],
                pos: vec![Range::new(0, 5)]
            }
        );
        e.exhaust_int_range(SignedInt(5, true), SignedInt(4, true));
        debug_assert_eq!(
            e,
            Exhaustion::SignedInt {
                neg: vec![Range::new(0, 5)],
                pos: vec![Range::new(0, 5)]
            }
        );
    }
}
