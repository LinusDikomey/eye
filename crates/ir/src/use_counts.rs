use std::{
    cell::Cell,
    ops::{Index, IndexMut},
};

use crate::{Environment, Ref, modify::IrModify};

pub struct UseCounts {
    use_counts: Box<[Cell<u32>]>,
}
impl Index<Ref> for UseCounts {
    type Output = Cell<u32>;
    fn index(&self, index: Ref) -> &Self::Output {
        &self.use_counts[index.idx()]
    }
}
impl IndexMut<Ref> for UseCounts {
    fn index_mut(&mut self, index: Ref) -> &mut Self::Output {
        &mut self.use_counts[index.idx()]
    }
}
impl UseCounts {
    pub fn empty() -> Self {
        Self {
            use_counts: Box::new([]),
        }
    }

    pub fn compute(ir: &IrModify, env: &Environment) -> Self {
        let mut use_counts = vec![Cell::new(0); ir.inst_count() as usize].into_boxed_slice();
        for i in 0..ir.inst_count() {
            let inst = ir.get_inst(Ref::index(i));
            for arg in ir.args_iter(inst, env) {
                match arg {
                    crate::Argument::Ref(r) => {
                        if let Some(r) = r.into_ref() {
                            *use_counts[r as usize].get_mut() += 1;
                        }
                    }
                    crate::Argument::BlockTarget(crate::BlockTarget(_block, args)) => {
                        for r in args.iter() {
                            if let Some(r) = r.into_ref() {
                                *use_counts[r as usize].get_mut() += 1;
                            }
                        }
                    }
                    crate::Argument::BlockId(_)
                    | crate::Argument::Int(_)
                    | crate::Argument::Float(_)
                    | crate::Argument::TypeId(_)
                    | crate::Argument::FunctionId(_)
                    | crate::Argument::GlobalId(_)
                    | crate::Argument::MCReg(_) => {}
                }
            }
        }
        Self { use_counts }
    }
}
