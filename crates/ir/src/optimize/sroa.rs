use std::{collections::BTreeMap, fmt, num::NonZeroU64};

use dmap::DHashMap;

use crate::{
    Argument, BUILTIN, BlockGraph, BlockTarget, Environment, FunctionIr, ModuleOf, Primitive, Ref,
    Type, TypeId, Types,
    dialect::{Mem, Tuple},
    layout,
    modify::IrModify,
    pipeline::FunctionPass,
};

pub struct SROA {
    mem: ModuleOf<Mem>,
    tuple: ModuleOf<Tuple>,
}

impl SROA {
    pub fn new(env: &mut crate::Environment) -> Self {
        Self {
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
                                let (accesses, _, _) = typed_accesses.entry(decl).or_default();
                                let before_count = accesses.len();
                                self.split_accesses(
                                    types,
                                    env,
                                    ty,
                                    offset,
                                    |size, offset, index, ty| {
                                        accesses.push(Access {
                                            access: AccessType::Load,
                                            location: r,
                                            size,
                                            offset,
                                            index,
                                            ty,
                                        });
                                    },
                                );
                                if accesses.len() == before_count {
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
                                let (accesses, _, _) = typed_accesses.entry(decl).or_default();
                                let before_count = accesses.len();
                                self.split_accesses(
                                    types,
                                    env,
                                    ty,
                                    offset,
                                    |size, offset, index, ty| {
                                        accesses.push(Access {
                                            access: AccessType::Store,
                                            location: r,
                                            size,
                                            offset,
                                            index,
                                            ty,
                                        });
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
                            *ty = ty.join(SubType::Unique(store.ty), &types)
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
                let subdecl_ty = match *ty {
                    SubType::Int => {
                        let primitive_ty = match size.get() {
                            1 => Primitive::I8,
                            2 => Primitive::I16,
                            3..=4 => Primitive::I32,
                            5..=8 => Primitive::I64,
                            _ => unreachable!(), // checked above
                        };
                        types.add(Type::Primitive(primitive_ty.into()))
                    }
                    SubType::Unique(type_id) => type_id,
                };
                *subdecl = ir.add_before(env, decl, self.mem.Decl(subdecl_ty, ptr_ty));
            }

            let mut load_element_tuples: DHashMap<Ref, Ref> = dmap::new();
            let mut stores_to_delete = dmap::new_set();

            for access in accesses {
                let (&offset, &(_, _, subdecl)) =
                    subranges.range(0..=access.offset).next_back().unwrap();
                let access_offset: u32 = (access.offset - offset).try_into().unwrap();
                let ptr = if access_offset == 0 {
                    subdecl
                } else {
                    let ptr_ty = ir.get_ref_ty(subdecl);
                    ir.add_before(
                        env,
                        access.location,
                        self.mem
                            .Offset(subdecl, i64::from(access_offset) as u64, ptr_ty),
                    )
                };
                match access.access {
                    AccessType::Load => {
                        let load = self.mem.Load(ptr, access.ty);
                        if let Some(index) = access.index {
                            let value = ir.add_before(env, access.location, load);

                            let tuple_ty = ir.get_ref_ty(access.location);
                            let element_tuple = load_element_tuples
                                .entry(access.location)
                                .or_insert_with(|| {
                                    ir.add_before(env, access.location, BUILTIN.Undef(tuple_ty))
                                });
                            let new_tuple = ir.add_before(
                                env,
                                access.location,
                                self.tuple
                                    .InsertMember(*element_tuple, index, value, tuple_ty),
                            );
                            *element_tuple = new_tuple;
                        } else {
                            ir.replace(env, access.location, load);
                        }
                    }
                    AccessType::Store => {
                        let inst = ir.get_inst(access.location).as_module(self.mem).unwrap();
                        debug_assert_eq!(inst.op(), Mem::Store);
                        let (_, value): (Ref, Ref) = ir.typed_args(&inst);
                        if let Some(idx) = access.index {
                            stores_to_delete.insert(access.location);
                            let value = self.get_or_extract_index(
                                env,
                                &mut ir,
                                value,
                                idx,
                                access.ty,
                                access.location,
                            );
                            ir.add_before(env, access.location, self.mem.Store(ptr, value));
                        } else {
                            ir.replace(env, access.location, self.mem.Store(ptr, value));
                        };
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
        r: Ref,
        idx: u32,
        ty: TypeId,
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
        ir.add_before(env, insert_before, self.tuple.MemberValue(r, idx, ty))
    }
}

#[derive(Debug)]
struct Access {
    access: AccessType,
    location: Ref,
    size: NonZeroU64,
    offset: u64,
    ty: TypeId,
    index: Option<u32>,
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
    Unique(TypeId),
}
impl SubType {
    #[must_use]
    fn join(self, other: Self, types: &Types) -> Self {
        match (self, other) {
            (Self::Unique(a), Self::Unique(b)) if types.are_equal(a, b) => self,
            _ => SubType::Int,
        }
    }
}
