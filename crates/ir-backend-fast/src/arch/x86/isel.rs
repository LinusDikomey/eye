use std::convert::Infallible;

use ir::{
    BlockGraph, BlockId, Environment, FunctionId, FunctionIr, IntoArgs, MCReg, ModuleId, ModuleOf,
    Primitive, Ref, Type, TypeId, Types,
    dialect::Arith,
    mc::{Abi, BackendState, IselCtx, Mc},
    modify::IrModify,
    rewrite::{ReverseRewriteOrder, Rewrite},
    slots::Slots,
};

use crate::arch::x86::{Reg, X86, isa::Size};

pub fn codegen(
    env: &Environment,
    body: &ir::FunctionIr,
    types: &ir::Types,
    isel: &mut InstructionSelector,
    main_module: ModuleId,
    abi: &'static dyn Abi<X86>,
    state: &mut BackendState,
    function_name: &str,
) -> (FunctionIr, ir::Types) {
    let _enter = tracing::span!(
        target: "isel",
        tracing::Level::INFO,
        "function",
        function = function_name,
    )
    .entered();
    let mut body = body.clone();

    let mut regs = Slots::with_default(&body, types, MCReg::from_virt(0));
    for r in body.refs() {
        _ = regs.visit_primitive_slots_mut::<Infallible, _>(
            r,
            types[body.get_ref_ty(r)],
            types,
            |regs, p| {
                use Primitive as P;
                match p {
                    P::I1 | P::I8 | P::U8 => regs[0] = body.new_reg(ir::mc::RegClass::GP8),
                    P::I16 | P::U16 => regs[0] = body.new_reg(ir::mc::RegClass::GP16),
                    P::I32 | P::U32 => regs[0] = body.new_reg(ir::mc::RegClass::GP32),
                    P::I64 | P::U64 | P::Ptr => regs[0] = body.new_reg(ir::mc::RegClass::GP64),
                    P::F32 => regs[0] = body.new_reg(ir::mc::RegClass::F32),
                    P::F64 => regs[0] = body.new_reg(ir::mc::RegClass::F64),
                    _ => todo!(),
                };
                Ok(())
            },
        );
    }
    let mut types = types.clone();
    let block_graph = BlockGraph::calculate(&body, env);
    let unit = types.add(Type::Tuple(ir::TypeIds::EMPTY));

    let mut ir = IrModify::new(body);
    let args = ir.get_block_args(BlockId::ENTRY);
    abi.implement_params(args, &mut ir, env, isel.mc, &types, &regs, unit);
    let mut ctx = IselCtx::new(
        main_module,
        env,
        &ir,
        regs,
        isel.mc,
        unit,
        abi,
        state,
        &block_graph,
    );

    ir::rewrite::rewrite_in_place(
        &mut ir,
        &types,
        env,
        &mut ctx,
        isel,
        ReverseRewriteOrder::new(&block_graph),
    );

    (ir.finish_and_compress(env), types)
}

fn primitive_of_ref(r: Ref, ir: &IrModify, types: &Types) -> Primitive {
    let Type::Primitive(p) = types[ir.get_ref_ty(r)] else {
        unreachable!()
    };
    p.try_into().expect("Invalid primitive encountered")
}

fn int_size_of_ref(r: Ref, ir: &IrModify, types: &Types) -> Size {
    arith_class(r, ir, types).1
}

enum ArithClass {
    Signed,
    Unsigned,
    Float,
}

fn arith_class(r: Ref, ir: &IrModify, types: &Types) -> (ArithClass, Size) {
    match primitive_of_ref(r, ir, types) {
        Primitive::I1 | Primitive::I8 => (ArithClass::Signed, Size::S8),
        Primitive::I16 => (ArithClass::Signed, Size::S16),
        Primitive::I32 => (ArithClass::Signed, Size::S32),
        Primitive::I64 => (ArithClass::Signed, Size::S64),
        Primitive::I128 => (ArithClass::Signed, Size::S128),
        Primitive::U8 => (ArithClass::Unsigned, Size::S8),
        Primitive::U16 => (ArithClass::Unsigned, Size::S16),
        Primitive::U32 => (ArithClass::Unsigned, Size::S32),
        Primitive::U64 => (ArithClass::Unsigned, Size::S64),
        Primitive::U128 => (ArithClass::Unsigned, Size::S128),
        Primitive::F32 => (ArithClass::Float, Size::S32),
        Primitive::F64 => (ArithClass::Float, Size::S64),
        Primitive::Ptr => unreachable!("unsupported type Ptr for arithmetic"),
    }
}

// TODO: this really sucks, need a good struct for ir visitors that passes everything used in ir visitor
// and probably another struct for calling cmp_branch
#[allow(clippy::too_many_arguments)]
#[must_use]
fn cmp_branch<
    A1: IntoArgs<'static>,
    F1: Fn(BlockId) -> (FunctionId, A1, TypeId),
    A2: IntoArgs<'static>,
    F2: Fn(BlockId) -> (FunctionId, A2, TypeId),
>(
    ctx: &mut IselCtx<X86>,
    ir: &mut IrModify,
    types: &Types,
    env: &Environment,
    dialects: &InstructionSelector,
    block: BlockId,
    cmp_r: Ref,
    r: Ref,
    a: Ref,
    b: Ref,
    b1: BlockId,
    b1_args: Vec<Ref>,
    b2: BlockId,
    b2_args: Vec<Ref>,
    cond_jmp: F1,
    inverse_cond_jmp: F2,
) -> (
    FunctionId,
    impl use<A1, F1, A2, F2> + IntoArgs<'static>,
    TypeId,
) {
    cmp(ctx, ir, types, env, dialects, cmp_r, a, b);
    branch(
        ctx,
        ir,
        env,
        dialects,
        block,
        r,
        b1,
        b1_args,
        b2,
        b2_args,
        cond_jmp,
        inverse_cond_jmp,
    )
}

fn branch<
    A1: IntoArgs<'static>,
    F1: Fn(BlockId) -> (FunctionId, A1, TypeId),
    A2: IntoArgs<'static>,
    F2: Fn(BlockId) -> (FunctionId, A2, TypeId),
>(
    ctx: &mut IselCtx<X86>,
    ir: &mut IrModify,
    env: &Environment,
    dialects: &InstructionSelector,
    block: BlockId,
    r: Ref,
    b1: BlockId,
    b1_args: Vec<Ref>,
    b2: BlockId,
    b2_args: Vec<Ref>,
    cond_jmp: F1,
    inverse_cond_jmp: F2,
) -> (
    FunctionId,
    impl use<A1, F1, A2, F2> + IntoArgs<'static>,
    TypeId,
) {
    let InstructionSelector { x86, mc, .. } = *dialects;
    let next_block = ctx.next_block(block);
    if next_block.is_some_and(|next| next == b1) {
        // if b1 is the next block, we want to invert the condition to only emit one jump
        create_args_copy(
            ctx,
            env,
            r,
            dialects.mc,
            dialects.x86,
            ir,
            b2,
            &b2_args.to_vec(),
        );
        ir.add_before(env, r, inverse_cond_jmp(b2));
        create_args_copy(ctx, env, r, mc, x86, ir, b1, &b1_args.to_vec());
        // the jmp is still emitted (to maintain correct successor info)
        // but will be remvoed during emit
        x86.jmp(b1)
    } else {
        create_args_copy(ctx, env, r, mc, x86, ir, b1, &b1_args.to_vec());
        ir.add_before(env, r, cond_jmp(b1));
        create_args_copy(ctx, env, r, mc, x86, ir, b2, &b2_args.to_vec());
        x86.jmp(b2)
    }
}

fn create_args_copy(
    ctx: &mut IselCtx<X86>,
    env: &Environment,
    before: Ref,
    mc: ModuleOf<Mc>,
    x86: ModuleOf<X86>,
    ir: &mut IrModify,
    target: BlockId,
    args: &[Ref],
) {
    ctx.create_args_copy(env, before, mc, ir, target, args, |ir, env, reg, b| {
        ir.add_before(env, before, x86.mov_ri8(reg, b as u32));
    });
}

ir::visitor! {
    InstructionSelector,
    Rewrite,
    ir, types, inst, block, env, dialects,
    ctx: IselCtx<'_, X86>;

    use builtin: ir::Builtin;
    use arith: ir::dialect::Arith;
    // use tuple: ir::dialect::Tuple;
    use mem: ir::dialect::Mem;
    use cf: ir::dialect::Cf;

    use x86: X86;
    use mc: ir::mc::Mc;

    patterns:
    (builtin.Undef) => {
        // don't need to do anything, registers are already allocated
    };
    (%r = arith.Int (#x)) => {
        let regs = ctx.regs.get(r);
        match int_size_of_ref(r, ir, types) {
            Size::S8 => {
                ir.replace(env, r, x86.mov_ri8(regs[0], x as u8 as u32));
            }
            Size::S16 => {
                ir.replace(env, r, x86.mov_ri16(regs[0], x as u16 as u32));
            }
            Size::S32 => {
                ir.replace(env, r, x86.mov_ri32(regs[0], x as u32));
            }
            Size::S64 => {
                if x > u32::MAX as u64 {
                    todo!()
                }
                ir.replace(env, r, x86.mov_ri64(regs[0], x.try_into().unwrap()));
            }
            Size::S128 => todo!("128 bit ints"),
        }
    };
    (%r = arith.Float (float _f)) => todo!("floats") as ();
    (%r = arith.Neg x) => {
        let x = ctx.regs.get(x);
        let out = ctx.regs.get(r);
        match int_size_of_ref(r, ir, types) {
            Size::S8 => {
                let (&[out], &[x]) = (out, x) else { unreachable!() };
                ctx.copy(env, r, mc, ir, &[out, x]);
                ir.replace(env, r, x86.neg_r8(out));
            }
            Size::S16 => {
                let (&[out], &[x]) = (out, x) else { unreachable!() };
                ctx.copy(env, r, mc, ir, &[out, x]);
                ir.replace(env, r, x86.neg_r16(out));
            }
            Size::S32 => {
                let (&[out], &[x]) = (out, x) else { unreachable!() };
                ctx.copy(env, r, mc, ir, &[out, x]);
                ir.replace(env, r, x86.neg_r32(x));
            }
            Size::S64 => {
                let (&[out], &[x]) = (out, x) else { unreachable!() };
                ctx.copy(env, r, mc, ir, &[out, x]);
                ir.replace(env, r, x86.neg_r64(x));
            }
            Size::S128 => todo!(),
        }
    };
    (%r = arith.Not x) => {
        let out = ctx.regs.get_one(r);
        ctx.copy(env, r, mc, ir, &[out, ctx.regs.get_one(x)]);
        x86.xor_ri8(out, 1)
    };
    (%r = arith.Add a b) => int_bin_op(ctx, ir, types, env, dialects, r, a, b, IntBinOp {
        i8: [X86::add_rr8, X86::add_ri8],
        i16: [X86::add_rr16, X86::add_ri16],
        i32: [X86::add_rr32, X86::add_ri32],
        i64: [X86::add_rr64, X86::add_ri64],
    });
    (%r = arith.Sub a b) => int_bin_op(ctx, ir, types, env, dialects, r, a, b, IntBinOp {
        i8: [X86::sub_rr8, X86::sub_ri8],
        i16: [X86::sub_rr16, X86::sub_ri16],
        i32: [X86::sub_rr32, X86::sub_ri32],
        i64: [X86::sub_rr64, X86::sub_ri64],
    });
    (%r = arith.Mul a (arith.Int (#x))) => {
        let primitive = primitive_of_ref(r, ir, types);
        match primitive {
            Primitive::I1 => todo!(),
            Primitive::I8 => {
                let a = ctx.regs.get_one(a);
                let out = ctx.regs.get_one(r);
                ctx.copy(env, r, mc, ir, &[out, a]);
                // TODO: figure out how to truncate
                ir.add_before(env, r, x86.imul_ri16(out, x as _));
                ir.replace_with(env, r, Ref::UNIT);
            }
            Primitive::I16 => {
                let a = ctx.regs.get_one(a);
                let out = ctx.regs.get_one(r);
                ctx.copy(env, r, mc, ir, &[out, a]);
                ir.replace(env, r, x86.imul_ri16(out, x as _));
            }
            Primitive::I32 => todo!(),
            Primitive::I64 => todo!(),
            Primitive::I128 => todo!(),
            Primitive::U8 => todo!(),
            Primitive::U16 => todo!(),
            Primitive::U32 => todo!(),
            Primitive::U64 => todo!(),
            Primitive::U128 => todo!(),
            Primitive::F32 => todo!(),
            Primitive::F64 => todo!(),
            Primitive::Ptr => todo!(),
        }
    };
    (%r = arith.Div a b) => todo!("div") as ();
    (%r = arith.Rem a b) => todo!("rem") as ();
    (%r = arith.Or a b) => {
        x86.or_rr8(ctx.regs.get_one(a), ctx.regs.get_one(b))
    };
    (%r = arith.And a b) => {
        x86.and_rr8(ctx.regs.get_one(a), ctx.regs.get_one(b))
    };
    (%r = arith.Eq a b) => cmp_op(ctx, ir, types, env, dialects, r, a, b, X86::sete);
    (%r = arith.NE a b) => cmp_op(ctx, ir, types, env, dialects, r, a, b, X86::setne);
    (%r = arith.LT a b) if ctx.use_count(r) > 0 => cmp_op(ctx, ir, types, env, dialects, r, a, b, CmpOp {
        signed: X86::setl,
        unsigned: X86::setc,
    });
    (%r = arith.GT a b) => cmp_op(ctx, ir, types, env, dialects, r, a, b, CmpOp {
        signed: X86::setg,
        unsigned: X86::seta,
    });
    (%r = arith.LE a b) => cmp_op(ctx, ir, types, env, dialects, r, a, b, CmpOp {
        signed: X86::setle,
        unsigned: X86::setbe,
    });
    (%r = arith.GE a b) => cmp_op(ctx, ir, types, env, dialects, r, a, b, CmpOp {
        signed: X86::setge,
        unsigned: X86::setnc,
    });
    (%r = arith.Xor a b) => int_bin_op(ctx, ir, types, env, dialects, r, a, b, IntBinOp {
        i8: [X86::xor_rr8, X86::xor_ri8],
        i16: [X86::xor_rr16, X86::xor_ri16],
        i32: [X86::xor_rr32, X86::xor_ri32],
        i64: [X86::xor_rr64, X86::xor_ri64],
    });
    (%r = arith.Shl a b) => todo!("shl") as ();
    (%r = arith.Shr a b) => todo!("shr") as ();
    (%r = arith.Rol a b) => todo!("rol") as ();
    (%r = arith.Ror a b) => todo!("ror") as ();
    (%r = arith.CastInt x) => {
        // TODO: support large ints
        let src = ctx.regs.get_one(x);
        let dst = ctx.regs.get_one(r);
        match (arith_class(x, ir, types), arith_class(r, ir, types)) {
            ((ArithClass::Float, _), _) | (_, (ArithClass::Float, _)) => unreachable!(),

            // truncate or keep the same size by just doing a mov in the larger size. Upper bits
            // will be just be ignored by further operations.
            ((_, Size::S8 | Size::S16 | Size::S32 | Size::S64), (_, Size::S8)) => ir.replace(env, r, x86.mov_rr8(dst, src)),
            ((_, Size::S16 | Size::S32 | Size::S64), (_, Size::S16)) => ir.replace(env, r, x86.mov_rr16(dst, src)),
            ((_, Size::S32 | Size::S64), (_, Size::S32)) => ir.replace(env, r, x86.mov_rr32(dst, src)),
            ((_, Size::S64), (_, Size::S64)) => ir.replace(env, r, x86.mov_rr64(dst, src)),

            // zero extension
            ((ArithClass::Unsigned, Size::S8), (_, Size::S16 | Size::S32 | Size::S64)) => ir.replace(env, r, x86.movzx32_rr8(dst, src)),
            ((ArithClass::Unsigned, Size::S16), (_, Size::S32 | Size::S64)) => ir.replace(env, r, x86.movzx32_rr16(dst, src)),
            ((ArithClass::Unsigned, Size::S32), (_, Size::S64)) => ir.replace(env, r, x86.mov_rr32(dst, src)),

            // sign extension
            ((ArithClass::Signed, Size::S8 | Size::S16), (_, Size::S16 | Size::S32)) => ir.replace(env, r, x86.movsx32_rr8(dst, src)),
            ((ArithClass::Signed, Size::S8), (_, Size::S64)) => ir.replace(env, r, x86.movsx64_rr8(dst, src)),
            ((ArithClass::Signed, Size::S16), (_, Size::S64)) => ir.replace(env, r, x86.movsx64_rr16(dst, src)),
            ((ArithClass::Signed, Size::S32), (_, Size::S64)) => ir.replace(env, r, x86.movsx64_rr32(dst, src)),

            _ => unreachable!(),
        }
    };
    (%r = arith.CastFloat x) => todo!("CastFloat") as ();
    (%r = arith.CastIntToFloat x) => todo!("CastIntToFloat") as ();
    (%r = arith.CastFloatToInt x) => todo!("CastFloatToInt") as ();
    (%r = cf.Ret value) => {
        ctx.abi.implement_return(value, ir, env, mc, x86, types, &ctx.regs, r, ctx.unit);
    };
    (%r = cf.Goto (@b b_args)) => {
        let b_args = b_args.to_vec();
        create_args_copy(ctx, env, r, mc, x86, ir, b, &b_args);
        x86.jmp(b)
    };
    (%r = cf.Branch (%cmp = arith.Eq a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        let b1_args = b1_args.to_vec();
        let b2_args = b2_args.to_vec();
        ctx.remove_use(cmp, ir, env);
        cmp_branch(ctx, ir, types, env, dialects, block, cmp, r, a, b, b1, b1_args, b2, b2_args,
            |b| x86.je(b),
            |b| x86.jne(b),
        )
    };
    (%r = cf.Branch (%cmp = arith.LT a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        let b1_args = b1_args.to_vec();
        let b2_args = b2_args.to_vec();
        ctx.remove_use(cmp, ir, env);
        cmp_branch(ctx, ir, types, env, dialects, block, cmp, r, a, b, b1, b1_args, b2, b2_args,
            |b| x86.jl(b),
            |b| x86.jge(b),
        )
    };
    (%r = cf.Branch (%cmp = arith.GT a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        let b1_args = b1_args.to_vec();
        let b2_args = b2_args.to_vec();
        ctx.remove_use(cmp, ir, env);
        cmp_branch(ctx, ir, types, env, dialects, block, cmp, r, a, b, b1, b1_args, b2, b2_args,
            |b| x86.jg(b),
            |b| x86.jle(b),
        )
    };
    (%r = cf.Branch (%cmp = arith.LE a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        let b1_args = b1_args.to_vec();
        let b2_args = b2_args.to_vec();
        ctx.remove_use(cmp, ir, env);
        cmp_branch(ctx, ir, types, env, dialects, block, cmp, r, a, b, b1, b1_args, b2, b2_args,
            |b| x86.jle(b),
            |b| x86.jg(b),
        )
    };
    (%r = cf.Branch (%cmp = arith.GE a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        let b1_args = b1_args.to_vec();
        let b2_args = b2_args.to_vec();
        ctx.remove_use(cmp, ir, env);
        cmp_branch(ctx, ir, types, env, dialects, block, cmp, r, a, b, b1, b1_args, b2, b2_args,
            |b| x86.jge(b),
            |b| x86.jl(b),
        )
    };
    (%r = cf.Branch cond (@b1 b1_args) (@b2 b2_args)) => {
        let cond = ctx.regs.get_one(cond);
        let b1_args = b1_args.to_vec();
        let b2_args = b2_args.to_vec();
        ir.add_before(env, r, x86.test_rr8(cond, cond));
        branch(ctx, ir, env, dialects, block, r, b1, b1_args, b2, b2_args, |b| x86.jne(b), |b| x86.je(b))
    };
    (%r = mem.Decl (type ty)) => {
        let layout = ir::type_layout(types[ty], types, env.primitives());
        let offset = ctx.alloc_stack(layout);
        let out = ctx.regs.get_one(r);
        x86.lea_rm64(out, MCReg::from_phys(Reg::rbp), (-(offset as i32)) as u32)
    };
    (%r = mem.Load ptr) => {
        let ptr = ctx.regs.get_one(ptr);
        match types[ir.get_ref_ty(r)] {
            Type::Primitive(primitive_id) => match Primitive::try_from(primitive_id).unwrap() {
                Primitive::I1 | Primitive::I8 | Primitive::U8 => {
                    ir.replace(env, r, x86.mov_rm8(ctx.regs.get_one(r), ptr, 0));
                }
                Primitive::I16 | Primitive::U16 => {
                    ir.replace(env, r, x86.mov_rm16(ctx.regs.get_one(r), ptr, 0));
                }
                Primitive::I32 | Primitive::U32 => {
                    ir.replace(env, r, x86.mov_rm32(ctx.regs.get_one(r), ptr, 0));
                }
                Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                    ir.replace(env, r, x86.mov_rm64(ctx.regs.get_one(r), ptr, 0));
                }
                Primitive::F32 | Primitive::F64 => todo!("load floats"),
                Primitive::I128 | Primitive::U128 => todo!("load 128-bit integers"),
            }
            Type::Array(_, _) | Type::Tuple(_) => todo!("load aggregrates"),
        }
    };
    (%r = mem.Store ptr value) => {
        let ptr = ctx.regs.get_one(ptr);
        match types[ir.get_ref_ty(value)] {
            Type::Primitive(primitive_id) => match Primitive::try_from(primitive_id).unwrap() {
                Primitive::I1 | Primitive::I8 | Primitive::U8 => {
                    ir.replace(env, r, x86.mov_mr8(ptr, 0, ctx.regs.get_one(value)));
                }
                Primitive::I16 | Primitive::U16 => {
                    ir.replace(env, r, x86.mov_mr16(ptr, 0, ctx.regs.get_one(value)));
                }
                Primitive::I32 | Primitive::U32 => {
                    ir.replace(env, r, x86.mov_mr32(ptr, 0, ctx.regs.get_one(value)));
                }
                Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                    ir.replace(env, r, x86.mov_mr64(ptr, 0, ctx.regs.get_one(value)));
                }
                Primitive::F32 | Primitive::F64 => todo!("store floats"),
                Primitive::I128 | Primitive::U128 => todo!("store 128-bit integers"),
            }
            Type::Array(_, _) | Type::Tuple(_) => todo!("store aggregrates"),
        }
    };
    (%r = _) => {
        if inst.module() == ctx.main_module {
            let abi = ctx.abi;
            abi.implement_call(r, ir, env, mc, x86, types, &ctx.regs, ctx.unit);
        } else if inst.module() != dialects.x86.id() {
            todo!("unhandled instruction at {r}: {}", env.get_inst_name(ir.get_inst(r)));
        }
    };
}

struct IntBinOp {
    i8: [X86; 2],
    i16: [X86; 2],
    i32: [X86; 2],
    i64: [X86; 2],
}
fn int_bin_op(
    ctx: &mut IselCtx<X86>,
    ir: &mut IrModify,
    types: &Types,
    env: &Environment,
    dialects: &InstructionSelector,
    r: Ref,
    a: Ref,
    b: Ref,
    ops: IntBinOp,
) {
    let InstructionSelector { mc, x86, .. } = *dialects;
    {
        let primitive = primitive_of_ref(r, ir, types);
        let [op_rr, op_ri] = match primitive {
            Primitive::I1 => todo!(),
            Primitive::I8 | Primitive::U8 => ops.i8,
            Primitive::I16 | Primitive::U16 => ops.i16,
            Primitive::I32 | Primitive::U32 => ops.i32,
            Primitive::I64 | Primitive::U64 => ops.i64,
            Primitive::I128 | Primitive::U128 => todo!("128-bit add"),
            Primitive::F32 => todo!(),
            Primitive::F64 => todo!(),
            Primitive::Ptr => unreachable!(),
        };
        // encode_args
        let out = ctx.regs.get_one(r);
        let a = ctx.regs.get_one(a);
        ctx.copy(env, r, mc, ir, &[out, a]);
        if let Some(c) = ir
            .get_inst(b)
            .as_module(dialects.arith)
            .and_then(|inst| (inst.op() == Arith::Int).then(|| ir.typed_args::<u32, _>(&inst)))
        {
            ctx.remove_use(b, ir, env);
            ir.replace(
                env,
                r,
                (FunctionId::new(x86.id(), op_ri.id()), (out, c), ctx.unit),
            );
        } else {
            let b = ctx.regs.get_one(b);
            ir.replace(
                env,
                r,
                (FunctionId::new(x86.id(), op_rr.id()), (out, b), ctx.unit),
            );
        }
    }
}

struct CmpOp {
    signed: X86,
    unsigned: X86,
}
impl From<X86> for CmpOp {
    fn from(value: X86) -> Self {
        Self {
            signed: value,
            unsigned: value,
        }
    }
}

fn cmp(
    ctx: &mut IselCtx<X86>,
    ir: &mut IrModify,
    types: &Types,
    env: &Environment,
    dialects: &InstructionSelector,
    before: Ref,
    a: Ref,
    b: Ref,
) {
    let (_, size) = arith_class(a, ir, types);
    let cmp_op = match size {
        Size::S8 => X86::cmp_rr8,
        Size::S16 => X86::cmp_rr16,
        Size::S32 => X86::cmp_rr32,
        Size::S64 => X86::cmp_rr64,
        Size::S128 => todo!(),
    };
    let a = ctx.regs.get_one(a);
    let b = ctx.regs.get_one(b);
    ir.add_before(
        env,
        before,
        (
            FunctionId::new(dialects.x86.id(), cmp_op.id()),
            (a, b),
            TypeId::UNIT,
        ),
    );
}

fn cmp_op(
    ctx: &mut IselCtx<X86>,
    ir: &mut IrModify,
    types: &Types,
    env: &Environment,
    dialects: &InstructionSelector,
    r: Ref,
    a: Ref,
    b: Ref,
    ops: impl Into<CmpOp>,
) {
    let ops = ops.into();
    let (class, _) = arith_class(a, ir, types);
    cmp(ctx, ir, types, env, dialects, r, a, b);
    let after_cmp_op = match class {
        ArithClass::Signed => ops.signed,
        ArithClass::Unsigned => ops.unsigned,
        ArithClass::Float => todo!("floats"),
    };
    let out = ctx.regs.get_one(r);
    ir.replace(
        env,
        r,
        (
            FunctionId::new(dialects.x86.id(), after_cmp_op.id()),
            out,
            TypeId::UNIT,
        ),
    );
}
