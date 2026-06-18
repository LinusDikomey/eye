use crate::{FunctionIr, Primitive, PrimitiveInfo, Ref, Type, Types};

pub struct Slots<V> {
    pub slots: Vec<V>,
    pub slot_map: Vec<u32>,
}
impl<V: Copy> Slots<V> {
    pub fn with_default(ir: &FunctionIr, types: &Types, default: V) -> Self {
        let mut slots = Vec::new();
        let slot_map = ir
            .insts
            .iter()
            .map(|inst| {
                let count = slot_count(types[inst.ty], types);
                let idx = slots.len() as u32;
                slots.extend(std::iter::repeat_n(default, count as usize));
                idx
            })
            .collect();
        Self { slots, slot_map }
    }

    pub fn get(&self, r: Ref) -> &[V] {
        let r = r.into_ref().expect("Can't get slots for value ref") as usize;
        let start = self.slot_map[r] as usize;
        let end = self
            .slot_map
            .get(r + 1)
            .map(|i| *i as usize)
            .unwrap_or(self.slots.len());
        &self.slots[start..end]
    }

    pub fn get_one(&self, r: Ref) -> V {
        let r = r.into_ref().expect("Can't get slot for value ref") as usize;
        let slot = self.slot_map[r] as usize;
        debug_assert_eq!(
            self.slot_map
                .get(r + 1)
                .map(|i| *i as usize)
                .unwrap_or(self.slots.len()),
            slot + 1,
            "Tried to get_one a Value in a slot not containing a single value"
        );
        self.slots[slot]
    }

    // gets an exclusive range of values
    pub fn get_range(&self, start: Ref, end: Ref) -> &[V] {
        let start = self.slot_map[start.into_ref().expect("Can't get slots for value ref") as usize]
            as usize;
        let end = self
            .slot_map
            .get(end.into_ref().expect("Can't get slots for value ref") as usize)
            .map(|i| *i as usize)
            .unwrap_or(self.slots.len());
        &self.slots[start..end]
    }

    pub fn visit_primitive_slots<E, F: FnMut(&[V], Primitive, u64) -> Result<(), E>>(
        &self,
        r: Ref,
        ty: Type,
        types: &Types,
        primitives: &[PrimitiveInfo],
        visit: F,
    ) -> Result<(), E> {
        self.visit_primitive_slots_inner(
            &mut { self.slot_map[r.into_ref().expect("Can't get slots for value Ref") as usize] },
            ty,
            types,
            primitives,
            &mut { visit },
            0,
        )
    }

    pub fn visit_primitive_slots_inner<E, F: FnMut(&[V], Primitive, u64) -> Result<(), E>>(
        &self,
        i: &mut u32,
        ty: Type,
        types: &Types,
        primitives: &[PrimitiveInfo],
        visit: &mut F,
        mut offset: u64,
    ) -> Result<(), E> {
        match ty {
            Type::Primitive(id) => {
                let p = Primitive::try_from(id).unwrap();
                let slot_count = primitive_slot_count(p);
                visit(
                    &self.slots[*i as usize..(*i + slot_count) as usize],
                    p,
                    offset,
                )?;
                *i += slot_count;
            }
            Type::Array(elem, len) => {
                let elem_layout = crate::type_layout(types[elem], types, primitives);
                for _ in 0..len {
                    self.visit_primitive_slots_inner(
                        i,
                        types[elem],
                        types,
                        primitives,
                        visit,
                        offset,
                    )?;
                    offset += elem_layout.stride();
                }
            }
            Type::Tuple(elems) => {
                for elem in elems.iter() {
                    let elem_layout = crate::type_layout(types[elem], types, primitives);
                    self.visit_primitive_slots_inner(
                        i,
                        types[elem],
                        types,
                        primitives,
                        visit,
                        offset,
                    )?;
                    offset += elem_layout.stride();
                }
            }
        }
        Ok(())
    }

    pub fn visit_primitive_slots_mut<E, F: FnMut(&mut [V], Primitive, u64) -> Result<(), E>>(
        &mut self,
        r: Ref,
        ty: Type,
        types: &Types,
        primitives: &[PrimitiveInfo],
        visit: F,
    ) -> Result<(), E> {
        self.visit_primitive_slots_mut_inner(
            &mut { self.slot_map[r.into_ref().expect("Can't get slots for value Ref") as usize] },
            ty,
            types,
            primitives,
            &mut { visit },
            0,
        )
    }

    pub fn visit_primitive_slots_mut_inner<
        E,
        F: FnMut(&mut [V], Primitive, u64) -> Result<(), E>,
    >(
        &mut self,
        i: &mut u32,
        ty: Type,
        types: &Types,
        primitives: &[PrimitiveInfo],
        visit: &mut F,
        mut offset: u64,
    ) -> Result<(), E> {
        match ty {
            Type::Primitive(id) => {
                let p = Primitive::try_from(id).unwrap();
                let slot_count = primitive_slot_count(p);
                visit(
                    &mut self.slots[*i as usize..(*i + slot_count) as usize],
                    p,
                    offset,
                )?;
                *i += slot_count;
            }
            Type::Array(elem, len) => {
                let elem_layout = crate::type_layout(types[elem], types, primitives);
                for _ in 0..len {
                    self.visit_primitive_slots_mut_inner(
                        i,
                        types[elem],
                        types,
                        primitives,
                        visit,
                        offset,
                    )?;
                    offset += elem_layout.stride();
                }
            }
            Type::Tuple(elems) => {
                for elem in elems.iter() {
                    let elem_layout = crate::type_layout(types[elem], types, primitives);
                    offset = offset.div_ceil(elem_layout.align.get()) * elem_layout.align.get();
                    self.visit_primitive_slots_mut_inner(
                        i,
                        types[elem],
                        types,
                        primitives,
                        visit,
                        offset,
                    )?;
                    offset += elem_layout.size;
                }
            }
        }
        Ok(())
    }
}
impl<V: Copy + Default> Slots<V> {
    pub fn new(ir: &FunctionIr, types: &Types) -> Self {
        Self::with_default(ir, types, V::default())
    }
}

pub fn slot_count(ty: Type, types: &Types) -> u32 {
    match ty {
        Type::Primitive(p) => primitive_slot_count(Primitive::try_from(p).unwrap()),
        Type::Array(type_ref, count) => slot_count(types[type_ref], types) * count,
        Type::Tuple(type_refs) => type_refs
            .iter()
            .map(|ty| slot_count(types[ty], types))
            .sum(),
    }
}

pub fn primitive_slot_count(p: Primitive) -> u32 {
    match p {
        Primitive::I128 | Primitive::U128 => 2,
        _ => 1,
    }
}
