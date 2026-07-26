use std::{collections::BTreeMap, fmt, num::NonZeroU64};

use dmap::DHashMap;

use crate::{
    Argument, BUILTIN, BlockGraph, BlockTarget, Environment, FunctionIr, ModuleOf, Primitive, Ref,
    Type, TypeId, Types,
    dialect::{Arith, Mem, Tuple},
    layout,
    modify::IrModify,
    pipeline::FunctionPass,
};

pub struct SROA {
    arith: ModuleOf<Arith>,
    mem: ModuleOf<Mem>,
    tuple: ModuleOf<Tuple>,
}

impl SROA {
    pub fn new(env: &mut crate::Environment) -> Self {
        Self {
            arith: env.get_dialect_module(),
            mem: env.get_dialect_module(),
            tuple: env.get_dialect_module(),
        }
    }
}
impl fmt::Debug for SROA {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SROA")
    }
}
impl FunctionPass for SROA {
    fn run(
        &self,
        env: &crate::Environment,
        types: &crate::Types,
        ir: crate::FunctionIr,
        function: &str,
        _state: &mut (),
    ) -> (crate::FunctionIr, Option<crate::Types>) {
        // TODO: disqualify Decls if the pointers are used in any unaccounted instructions
        // this isn't trivial since some MemberPtrs may be SROAd away but some might not
        // approach: track direct and indirect uses of Decls, if all direct and indirect (through MemberPtr)
        // uses are only used by Store/Load, a Decl qualifies for SROA
        let block_graph = BlockGraph::calculate(&ir, env);

        let mut typed_accesses: DHashMap<Ref, (Vec<Access>, bool, Vec<Ref>)> = dmap::new();

        let mut zero_sized_accesses = Vec::new();

        for &block in block_graph.postorder().iter().rev() {
            for (r, inst) in ir.get_block(block) {
                if let Some(mem) = inst.as_module(self.mem) {
                    match mem.op() {
                        Mem::Load => {
                            let ptr: Ref = ir.typed_args(&mem);
                            if let Some((decl, offset)) =
                                self.find_decl_offset_of_ptr(&ir, types, env, ptr)
                            {
                                let ty = ir.get_ref_ty(r);
                                let (accesses, disqualified, _) =
                                    typed_accesses.entry(decl).or_default();
                                let before_count = accesses.len();
                                self.split_accesses(
                                    types,
                                    env,
                                    ty,
                                    offset,
                                    |size, offset, index, ty| {
                                        if let Type::Primitive(ty) = types[ty] {
                                            accesses.push(Access {
                                                access: AccessType::Load,
                                                location: r,
                                                size,
                                                offset,
                                                index,
                                                ty: ty.try_into().unwrap(),
                                            });
                                        } else {
                                            // currently arrays aren't traversed with split_accesses
                                            *disqualified = true;
                                        }
                                    },
                                );
                                if accesses.len() == before_count && !*disqualified {
                                    // zero-sized load
                                    zero_sized_accesses.push(r);
                                }
                                continue;
                            }
                        }
                        Mem::Store => {
                            let (ptr, val): (Ref, Ref) = ir.typed_args(&mem);
                            if let Some((decl, offset)) =
                                self.find_decl_offset_of_ptr(&ir, types, env, ptr)
                            {
                                let ty = ir.get_ref_ty(val);
                                let (accesses, disqualified, _) =
                                    typed_accesses.entry(decl).or_default();
                                let before_count = accesses.len();
                                self.split_accesses(
                                    types,
                                    env,
                                    ty,
                                    offset,
                                    |size, offset, index, ty| {
                                        if let Type::Primitive(ty) = types[ty] {
                                            accesses.push(Access {
                                                access: AccessType::Store,
                                                location: r,
                                                size,
                                                offset,
                                                index,
                                                ty: ty.try_into().unwrap(),
                                            });
                                        } else {
                                            // currently arrays aren't traversed with split_accesses
                                            *disqualified = true;
                                        }
                                    },
                                );
                                if accesses.len() == before_count {
                                    // zero-sized store
                                    zero_sized_accesses.push(r);
                                }

                                // still disqualify Decl used in value
                                if let Some((decl, _)) =
                                    self.find_decl_offset_of_ptr(&ir, types, env, val)
                                {
                                    typed_accesses.entry(decl).or_default().1 = true;
                                }
                                continue;
                            }
                        }
                        Mem::MemberPtr => {
                            let (ptr, _, _): (Ref, TypeId, u32) = ir.typed_args(&mem);
                            if let Some((decl, _)) =
                                self.find_decl_offset_of_ptr(&ir, types, env, ptr)
                            {
                                let (_, _, uses) = typed_accesses.entry(decl).or_default();
                                uses.push(r);
                                continue;
                            }
                        }
                        Mem::ArrayIndex => {} // TODO
                        _ => {}
                    }
                }
                // disqualify all Decls referenced (indirectly) from SROA
                // since this instruction is neither a Load
                for arg in ir.args_iter(inst, env) {
                    match arg {
                        Argument::Ref(arg) => {
                            if let Some((decl, _)) =
                                self.find_decl_offset_of_ptr(&ir, types, env, arg)
                            {
                                tracing::debug!(target: "pass::sroa", function=function, "{r} uses decl {decl} from {arg}");
                                typed_accesses.entry(decl).or_default().1 = true;
                            }
                        }
                        Argument::BlockTarget(BlockTarget(_, args)) => {
                            for &r in args.iter() {
                                if let Some((decl, _)) =
                                    self.find_decl_offset_of_ptr(&ir, types, env, r)
                                {
                                    typed_accesses.entry(decl).or_default().1 = true;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        tracing::debug!(target: "pass::sroa", function = function, "Accesses: {typed_accesses:#?}");

        let mut ir = IrModify::new(ir);
        let mut types = types.clone();

        for r in zero_sized_accesses {
            ir.delete(env, r);
        }

        'decls: for (decl, (accesses, disqualified, uses)) in typed_accesses {
            if disqualified {
                continue;
            }
            // partition map of scalars at offset with size and SubType
            let mut subranges: BTreeMap<u64, (NonZeroU64, SubType, Ref)> = BTreeMap::new();
            let stores = accesses
                .iter()
                .filter(|access| access.access == AccessType::Store);
            for store in stores {
                let mut in_range =
                    subranges.range_mut(store.offset..store.offset + store.size.get());
                match (in_range.next(), in_range.next()) {
                    (None, None) => {
                        if let Some((offset, (size, ty, _))) =
                            subranges.range_mut(..store.offset).next_back()
                            && offset + size.get() > store.offset
                        {
                            // overlapping store before
                            let suboffset = store.offset - *offset;
                            let end_size = store.size.checked_add(suboffset).unwrap();
                            *ty = SubType::Int;
                            if *size < end_size {
                                // expand the range to the right
                                *size = end_size;
                            }
                        } else {
                            subranges.insert(
                                store.offset,
                                (store.size, SubType::Unique(store.ty), Ref::UNIT),
                            );
                        }
                    }
                    (Some((offset, (size, ty, _))), None) => {
                        let mut new_offset = None;
                        debug_assert!(*offset >= store.offset);
                        if *offset == store.offset {
                            *ty = ty.join(SubType::Unique(store.ty))
                        } else {
                            let diff = *offset - store.offset;
                            // expand the range to the left
                            new_offset = Some(store.offset);
                            *size = size.checked_add(diff).unwrap();
                            *ty = SubType::Int;
                        }
                        if *size < store.size {
                            // expand the range to the right
                            *size = store.size;
                        }
                        if let Some(new_offset) = new_offset {
                            let offset = *offset;
                            let size = subranges.remove(&offset).unwrap();
                            subranges.insert(new_offset, size);
                        }
                    }
                    (Some(_), Some(_)) => {
                        // If any store overlaps two subelements, don't perform SROA for now
                        // TODO
                        continue 'decls;
                    }
                    (None, Some(_)) => unreachable!(),
                }
            }
            tracing::debug!(target: "pass::sroa", function = function, "Got subrange map for {decl}: {subranges:#?}");
            if subranges
                .iter()
                .any(|(_, &(size, ty, _))| matches!(ty, SubType::Int) && size.get() > 8)
            {
                // Don't handle int types larger than 8 bytes for now since that makes the offset
                // math more complex
                // TODO
                continue 'decls;
            }
            // we decided on a partitioning of the Decl, create the new decls now
            for (size, ty, subdecl) in &mut subranges.values_mut() {
                let ptr_ty = ir.get_ref_ty(decl);
                let subdecl_ty = subdecl_ty(*size, *ty);
                *subdecl = ir.add_before(env, decl, self.mem.Decl(types.add(subdecl_ty), ptr_ty));
            }

            let mut load_element_tuples: DHashMap<Ref, Ref> = dmap::new();
            let mut stores_to_delete = dmap::new_set();

            for access in accesses {
                let subrange = subranges.range(0..=access.offset).next_back();
                match access.access {
                    AccessType::Load => {
                        let value =
                            subrange.and_then(|(&offset, &(subrange_size, subtype, subdecl))| {
                                let access_offset = access.offset - offset;
                                let subdecl_ty = subdecl_ty(subrange_size, subtype);
                                self.load_access(
                                    env,
                                    &mut ir,
                                    &mut types,
                                    subdecl,
                                    subdecl_ty,
                                    &access,
                                    access_offset,
                                )
                            });
                        if let Some(index) = access.index {
                            let tuple_ty = ir.get_ref_ty(access.location);
                            // Note that the tuple is still created even if we don't have a value!
                            let element_tuple = load_element_tuples
                                .entry(access.location)
                                .or_insert_with(|| {
                                    ir.add_before(env, access.location, BUILTIN.Undef(tuple_ty))
                                });
                            if let Some(value) = value {
                                let new_tuple = ir.add_before(
                                    env,
                                    access.location,
                                    self.tuple
                                        .InsertMember(*element_tuple, index, value, tuple_ty),
                                );
                                *element_tuple = new_tuple;
                            }
                        } else if let Some(value) = value {
                            ir.replace_with(env, access.location, value);
                        } else {
                            let ty = ir.get_ref_ty(access.location);
                            ir.replace(env, access.location, BUILTIN.Undef(ty));
                        }
                    }
                    AccessType::Store => {
                        let Some((&offset, &(subrange_size, subtype, subdecl))) = subrange else {
                            unreachable!(
                                "Subranges are defined by stores so there should always be a subrange here"
                            );
                        };
                        let access_offset = access.offset - offset;
                        let subdecl_ty = subdecl_ty(subrange_size, subtype);
                        let inst = ir.get_inst(access.location).as_module(self.mem).unwrap();
                        debug_assert_eq!(inst.op(), Mem::Store);
                        let (_, value): (Ref, Ref) = ir.typed_args(&inst);
                        stores_to_delete.insert(access.location);
                        let value = if let Some(idx) = access.index {
                            self.get_or_extract_index(
                                env,
                                &mut ir,
                                &mut types,
                                value,
                                idx,
                                access.ty,
                                access.location,
                            )
                        } else {
                            value
                        };
                        self.store_access(
                            env,
                            &mut ir,
                            &mut types,
                            subdecl,
                            subdecl_ty,
                            value,
                            &access,
                            access_offset,
                        );
                    }
                }
            }
            for r in uses {
                ir.delete(env, r);
            }
            // replace aggregate loads with the final tuple values
            for (location, tuple) in load_element_tuples {
                ir.replace_with(env, location, tuple);
            }
            // delete all old stores that weren't replaced immediately
            for r in stores_to_delete {
                ir.delete(env, r);
            }
            // delete original Decl
            ir.delete(env, decl);
        }

        (ir.finish_and_compress(env), Some(types))
    }
}
impl SROA {
    /// Find the Decl location and offset of a ptr Ref
    fn find_decl_offset_of_ptr(
        &self,
        ir: &FunctionIr,
        types: &Types,
        env: &Environment,
        mut r: Ref,
    ) -> Option<(Ref, u64)> {
        let mut offset: u64 = 0;
        if !r.is_ref() {
            return None;
        }
        loop {
            return if let Some(mem) = ir.get_inst(r).as_module(self.mem) {
                match mem.op() {
                    Mem::Decl => return Some((r, offset)),
                    Mem::ArrayIndex => None, // TODO
                    Mem::MemberPtr => {
                        let (ptr, ty, idx): (Ref, TypeId, u32) = ir.typed_args(&mem);
                        let Type::Tuple(elems) = types[ty] else {
                            unreachable!()
                        };
                        if idx != 0 {
                            let tuple_offset: u64 =
                                layout::offset_in_tuple(elems, idx, types, env.primitives());
                            offset += tuple_offset;
                        };
                        r = ptr;
                        continue;
                    }
                    _ => None,
                }
            } else {
                None
            };
        }
    }

    fn split_accesses(
        &self,
        types: &Types,
        env: &Environment,
        type_id: TypeId,
        offset: u64,
        mut on_elem: impl FnMut(NonZeroU64, u64, Option<u32>, TypeId),
    ) {
        match types[type_id] {
            Type::Tuple(elems) => {
                for (elem, i) in elems.iter().zip(0..) {
                    let elem_offset = layout::offset_in_tuple(elems, i, types, env.primitives());
                    let layout = layout::type_layout(types[elem], types, env.primitives());
                    if let Some(size) = NonZeroU64::new(layout.size) {
                        on_elem(size, offset + elem_offset, Some(i), elem);
                    }
                }
            } // TODO: Arrays
            ty => {
                let layout = layout::type_layout(ty, types, env.primitives());
                if let Some(size) = NonZeroU64::new(layout.size) {
                    on_elem(size, offset, None, type_id);
                }
            }
        }
    }

    fn get_or_extract_index(
        &self,
        env: &Environment,
        ir: &mut IrModify,
        types: &mut Types,
        r: Ref,
        idx: u32,
        ty: Primitive,
        insert_before: Ref,
    ) -> Ref {
        // try to look through InsertMember ops first to see if the value was inserted before
        let mut current = r;
        loop {
            if let Some(tuple) = ir.get_inst(current).as_module(self.tuple)
                && tuple.op() == Tuple::InsertMember
            {
                let (old_tuple, insert_idx, value): (Ref, u32, Ref) = ir.typed_args(&tuple);
                if insert_idx == idx {
                    return value;
                }
                current = old_tuple;
            } else {
                break;
            }
        }
        ir.add_before(
            env,
            insert_before,
            self.tuple.MemberValue(r, idx, types.add(ty)),
        )
    }

    fn load_access(
        &self,
        env: &Environment,
        ir: &mut IrModify,
        types: &mut Types,
        ptr: Ref,
        mut ty: Primitive,
        access: &Access,
        offset: u64,
    ) -> Option<Ref> {
        debug_assert_eq!(access.access, AccessType::Load);

        if offset >= ty.byte_size().get() {
            // loading fully undefined memory (no stores write to it)
            return None;
        }

        let mut loaded = ir.add_before(env, access.location, self.mem.Load(ptr, types.add(ty)));

        if ty == access.ty {
            // simplest case: we have the right type and don't need to reconstruct a tuple
            // so we can replace the access directly
            return Some(loaded);
        }

        // TODO(endianness): on big-endian, when access.ty.size() != ty.size(), we will need to adjust
        // the offset
        let bit_offset = offset * 8;
        if bit_offset != 0 {
            let uint_ty = if ty.is_unsigned_int() {
                types.add(ty)
            } else {
                let src_uint = ty.into_unsigned();
                let new_ty = types.add(src_uint);
                if ty.is_float() {
                    todo!("bitcast float to int")
                } else if ty.is_int() {
                    loaded =
                        ir.add_before(env, access.location, self.arith.CastInt(loaded, new_ty));
                } else {
                    debug_assert_eq!(ty, Primitive::Ptr);
                    loaded = ir.add_before(env, access.location, self.mem.PtrToInt(loaded, new_ty));
                }
                ty = src_uint;
                new_ty
            };
            let bit_offset =
                ir.add_before(env, access.location, self.arith.Int(bit_offset, uint_ty));
            loaded = ir.add_before(
                env,
                access.location,
                self.arith.Shr(loaded, bit_offset, uint_ty),
            );
        }
        Some(self.convert_primitive(env, ir, types, loaded, ty, access.ty, access.location))
    }

    fn store_access(
        &self,
        env: &Environment,
        ir: &mut IrModify,
        types: &mut Types,
        ptr: Ref,
        ty: Primitive,
        value: Ref,
        access: &Access,
        offset: u64,
    ) {
        if ty == access.ty {
            ir.add_before(env, access.location, self.mem.Store(ptr, value));
            return;
        }
        debug_assert!(
            ty.is_unsigned_int(),
            "Stores should either fit the type or ensure an unsigned storage type",
        );
        let ty_id = types.add(ty);
        let partial = offset != 0 || access.ty.byte_size() != ty.byte_size();
        let mut value =
            self.convert_primitive(env, ir, types, value, access.ty, ty, access.location);
        // TODO(endianness): on big-endian, when dst.size() != src.size(), we will need to adjust
        let bit_offset = offset * 8;
        let bit_size = access.ty.byte_size().get() * 8;
        debug_assert!(bit_size <= 64);
        let mask = (u64::MAX >> (64 - bit_size)) << bit_offset;
        if bit_offset != 0 {
            let bit_offset = ir.add_before(env, access.location, self.arith.Int(bit_offset, ty_id));
            value = ir.add_before(
                env,
                access.location,
                self.arith.Shl(value, bit_offset, ty_id),
            );
        }
        if partial {
            // load the previous value, mask out the existing bits and combine it with the new value
            let existing = ir.add_before(env, access.location, self.mem.Load(ptr, ty_id));
            let not_mask = match ty {
                Primitive::U8 => !(mask as u8) as u64,
                Primitive::U16 => !(mask as u16) as u64,
                Primitive::U32 => !(mask as u32) as u64,
                Primitive::U64 => !mask,
                _ => unreachable!(),
            };
            let not_mask = ir.add_before(env, access.location, self.arith.Int(not_mask, ty_id));
            let existing = ir.add_before(
                env,
                access.location,
                self.arith.And(existing, not_mask, ty_id),
            );
            // or distinct like llvm could be useful here in the future maybe?
            value = ir.add_before(env, access.location, self.arith.Or(existing, value, ty_id));
        }
        ir.add_before(env, access.location, self.mem.Store(ptr, value));
    }

    fn convert_primitive(
        &self,
        env: &Environment,
        ir: &mut IrModify,
        types: &mut Types,
        value: Ref,
        src: Primitive,
        dst: Primitive,
        before: Ref,
    ) -> Ref {
        if src == dst {
            return value;
        }
        let dst_ty = types.add(dst);
        match (src, dst) {
            _ if src.is_float() || dst.is_float() => todo!("bitcast float/int"),
            (_, Primitive::Ptr) => ir.add_before(env, before, self.mem.IntToPtr(value, dst_ty)),
            (Primitive::Ptr, _) => ir.add_before(env, before, self.mem.PtrToInt(value, dst_ty)),
            _ => ir.add_before(env, before, self.arith.CastInt(value, dst_ty)),
        }
    }
}

fn subdecl_ty(size: NonZeroU64, ty: SubType) -> Primitive {
    match ty {
        SubType::Int => {
            match size.get() {
                1 => Primitive::U8,
                2 => Primitive::U16,
                3..=4 => Primitive::U32,
                5..=8 => Primitive::U64,
                _ => unreachable!(), // checked before
            }
        }
        SubType::Unique(primitive) => primitive,
    }
}

struct Access {
    access: AccessType,
    location: Ref,
    size: NonZeroU64,
    offset: u64,
    ty: Primitive,
    index: Option<u32>,
}
impl fmt::Debug for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.location)?;
        if let Some(index) = self.index {
            write!(f, ".{index}")?;
        } else {
            write!(f, "  ")?;
        }
        let pad = if self.access == AccessType::Load {
            " "
        } else {
            ""
        };
        write!(
            f,
            " = {:?}{} +{} size={} : {:?}",
            self.access, pad, self.offset, self.size, self.ty,
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessType {
    Load,
    Store,
}

/// Type of a subrange of an aggregate
#[derive(Clone, Copy, Debug)]
enum SubType {
    Int,
    Unique(Primitive),
}
impl SubType {
    #[must_use]
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unique(a), Self::Unique(b)) if a == b => self,
            _ => SubType::Int,
        }
    }
}
