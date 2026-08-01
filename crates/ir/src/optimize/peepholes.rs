use std::{borrow::Cow, fmt};

use crate::{
    BUILTIN, BlockId, BlockTarget, Builtin, Environment, FunctionIr, Instruction, ModuleOf,
    Primitive, Ref, Type, Types,
    dialect::{Arith, Cf, Mem, Tuple},
    modify::IrModify,
    pipeline::FunctionPass,
    rewrite::{self, IntoVisit, LinearRewriteOrder, Rewrite},
    use_counts::UseCounts,
};

pub struct Peepholes {
    peephole: Peephole,
}
impl fmt::Debug for Peepholes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Peepholes")
    }
}
impl Peepholes {
    pub fn new(env: &mut Environment) -> Self {
        Self {
            peephole: Peephole::new(env),
        }
    }
}
impl FunctionPass for Peepholes {
    fn run(
        &self,
        env: &crate::Environment,
        types: &crate::Types,
        ir: FunctionIr,
        _name: &str,
        _state: &mut (),
    ) -> (crate::FunctionIr, Option<crate::Types>) {
        let mut ir = IrModify::new(ir);
        let mut ctx = PeepholeCtx {
            use_counts: UseCounts::compute(&ir, env),
        };
        rewrite::rewrite_in_place(
            &mut ir,
            types,
            env,
            &mut ctx,
            self,
            LinearRewriteOrder::new(),
        );
        (ir.finish_and_compress(env), None)
    }
}
struct PeepholeCtx {
    use_counts: UseCounts,
}
impl PeepholeCtx {
    fn single_use(&self, r: Ref) -> bool {
        self.use_counts[r].get() == 1
    }
}
impl rewrite::RewriteCtx for PeepholeCtx {}

impl rewrite::Visitor<PeepholeCtx> for Peepholes {
    type Output = Rewrite;

    fn visit_instruction(
        &self,
        ir: &mut IrModify,
        types: &Types,
        env: &Environment,
        inst: &crate::Instruction,
        r: crate::Ref,
        block: crate::BlockId,
        ctx: &mut PeepholeCtx,
    ) -> Option<Self::Output> {
        if let Some(rewrite) = pre_peephole(&self.peephole, ir, types, env, inst, r, block, ctx) {
            return Some(rewrite);
        }
        self.peephole
            .visit_instruction(ir, types, env, inst, r, block, ctx)
    }
}

fn pre_peephole(
    dialects: &Peephole,
    ir: &mut IrModify,
    types: &Types,
    env: &Environment,
    inst: &Instruction,
    r: Ref,
    block: BlockId,
    _ctx: &mut PeepholeCtx,
) -> Option<Rewrite> {
    if let Some(arith) = inst.as_module(dialects.arith) {
        let ty = types[inst.ty()];
        if let Some(binop) = arith.inst.int_binop(ty) {
            let (lhs, rhs): (Ref, Ref) = ir.typed_args(&arith);
            // if any of the operands of one of these instructions is undef, the whole
            // instruction becomes undef
            for arg in [lhs, rhs] {
                if !arg.is_ref() {
                    continue;
                }
                if ir
                    .get_inst(arg)
                    .as_module(BUILTIN)
                    .is_some_and(|inst| inst.op() == Builtin::Undef)
                {
                    return Some(Rewrite::Rename(arg));
                }
            }
            let l_int: Option<u64> = lhs.into_bool().map(u64::from).or_else(|| {
                ir.get_inst(lhs)
                    .as_module(dialects.arith)
                    .and_then(|i| (i.inst == Arith::Int).then(|| ir.typed_args(&i)))
            });
            let r_int: Option<u64> = rhs.into_bool().map(u64::from).or_else(|| {
                ir.get_inst(rhs)
                    .as_module(dialects.arith)
                    .and_then(|i| (i.inst == Arith::Int).then(|| ir.typed_args(&i)))
            });
            match (l_int, r_int) {
                (Some(l), Some(r)) => {
                    let folded = (binop.fold)(l, r);
                    return if is_i1(ty) {
                        Some(Rewrite::Rename(if folded != 0 {
                            Ref::TRUE
                        } else {
                            Ref::FALSE
                        }))
                    } else {
                        dialects
                            .arith
                            .Int(folded, inst.ty())
                            .into_visit(ir, env, block)
                    };
                }
                (Some(_), None) => {
                    if binop.commutative {
                        // if only the lhs is a constant and the op is commutative, swap the operands
                        return (inst.function, (rhs, lhs), arith.ty()).into_visit(ir, env, block);
                    }
                }
                (None, Some(b)) => {
                    // op(op(a, 1), 2) => op(a, fold(1, 2)) (reassociate and fold constants)
                    if binop.associative
                        && let Some(l_inst) = ir.get_inst(lhs).as_module(dialects.arith)
                        && l_inst.op() == arith.op()
                        && let (x, y) = ir.typed_args::<(Ref, Ref), _>(&l_inst)
                        && let Some(int_inst) = ir.get_inst(y).as_module(dialects.arith)
                        && int_inst.op() == Arith::Int
                    {
                        let a: u64 = ir.typed_args(&int_inst);
                        let folded = ir.add_before(
                            env,
                            r,
                            dialects.arith.Int((binop.fold)(a, b), inst.ty()),
                        );
                        return (inst.function, (x, folded), inst.ty()).into_visit(ir, env, block);
                    }
                }
                (None, None) => {}
            }
        }
    }
    None
}

fn exact_log2(x: u64) -> Option<u32> {
    x.is_power_of_two().then(|| x.ilog2())
}

fn is_unsigned_int(ty: Type) -> bool {
    let Type::Primitive(p) = ty else {
        return false;
    };
    Primitive::try_from(p).unwrap().is_unsigned_int()
}

fn is_int(ty: Type) -> bool {
    let Type::Primitive(p) = ty else {
        return false;
    };
    Primitive::try_from(p).unwrap().is_int()
}

fn is_i1(ty: Type) -> bool {
    let Type::Primitive(p) = ty else {
        return false;
    };
    Primitive::try_from(p).unwrap() == Primitive::I1
}

rewrite::visitor! {
    Peephole,
    Rewrite,
    ir, types, inst, block, env, _dialects,
    ctx: PeepholeCtx;

    use builtin: crate::Builtin;
    use arith: Arith;
    use tuple: Tuple;
    use mem: Mem;
    use cf: Cf;

    patterns:
        (%r = arith.Add (builtin.Undef) __) => builtin.Undef(ir.get_ref_ty(r));
        (%r = arith.Add __ (builtin.Undef)) => builtin.Undef(ir.get_ref_ty(r));
        (%r = arith.Sub (builtin.Undef) __) => builtin.Undef(ir.get_ref_ty(r));
        (%r = arith.Sub __ (builtin.Undef)) => builtin.Undef(ir.get_ref_ty(r));

        (arith.Add (arith.Sub a x) y) if x == y => a;
        (arith.Sub(arith.Add a x) y) if x == y => a;

        // Mul(x, 2^n) -> x << n
        (%r = arith.Mul x (arith.Int(#c))) if is_int(types[ir.get_ref_ty(r)]) => {
            let ty = ir.get_ref_ty(r);
            let base = exact_log2(c)?;
            let base = ir.add_before(env, r, arith.Int(base.into(), ty));
            arith.Shl(x, base, ty)
        };

        // Div(x, 2^n) -> x >> n
        (%r = arith.Div x (arith.Int(#c))) if is_unsigned_int(types[ir.get_ref_ty(r)]) => {
            let ty = ir.get_ref_ty(r);
            let base = exact_log2(c)?;
            let base = ir.add_before(env, r, arith.Int(base.into(), ty));
            arith.Shr(x, base, ty)
        };

        (arith.Not (arith.Not a)) => a;
        (%r = arith.Not (arith.Int 0)) => arith.Int(1, ir.get_ref_ty(r));
        (%r = arith.Not (arith.Int 1)) => arith.Int(0, ir.get_ref_ty(r));

        (%r = arith.Not (%cmp = arith.LT a b)) if ctx.single_use(cmp) => {
            arith.GE(a, b, ir.get_ref_ty(r))
        };
        (%r = arith.Not (%cmp = arith.LE a b)) if ctx.single_use(cmp) => {
            arith.GT(a, b, ir.get_ref_ty(r))
        };
        (%r = arith.Not (%cmp = arith.GT a b)) if ctx.single_use(cmp) => {
            arith.LE(a, b, ir.get_ref_ty(r))
        };
        (%r = arith.Not (%cmp = arith.GE a b)) if ctx.single_use(cmp) => {
            arith.LT(a, b, ir.get_ref_ty(r))
        };
        (%r = arith.Not (%cmp = arith.Eq a b)) if ctx.single_use(cmp) => {
            arith.NE(a, b, ir.get_ref_ty(r))
        };
        (%r = arith.Not (%cmp = arith.NE a b)) if ctx.single_use(cmp) => {
            arith.Eq(a, b, ir.get_ref_ty(r))
        };

        // TODO: also load other types than I1
        (%r = mem.Load (mem.Global (global g))) if env[g].readonly && is_i1(types[ir.get_ref_ty(r)]) => {
            let v = &env[g].value;
            if v.is_empty() {
                builtin.Undef(ir.get_ref_ty(r)).into_visit(ir, env, block)?
            } else {
                let r = if v[0] != 0 { Ref::TRUE } else { Ref::FALSE };
                Rewrite::Rename(r)
            }
        };

        (cf.Branch cond (@t t_args) (@f f_args)) => {
            let (taken, taken_args) = if cond == Ref::TRUE {
                (t, t_args)
            } else if cond == Ref::FALSE {
                (f, f_args)
            } else {
                return None;
            };
            let taken_args = taken_args.to_vec();
            cf.Goto(BlockTarget(taken, Cow::Owned(taken_args)))
        };
        (tuple.MemberValue t (#index)) if t.is_ref() => {
            inserted_tuple_member(ir, tuple, t, index as _)?
        };
}

fn inserted_tuple_member(
    ir: &IrModify,
    tuple: ModuleOf<Tuple>,
    mut t: Ref,
    index: u32,
) -> Option<Ref> {
    while let Some(inst) = ir.get_inst(t).as_module(tuple)
        && inst.op() == Tuple::InsertMember
    {
        let (prev_t, insert_index, inserted_member): (Ref, u32, Ref) = ir.typed_args(&inst);
        if insert_index == index {
            return Some(inserted_member);
        }
        t = prev_t;
    }
    None
}
