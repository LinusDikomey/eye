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

use crate::arch::x86::{Reg, X86};

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

enum IntSize {
    I8,
    I16,
    I32,
    I64,
    I128,
}

fn int_size_of_ref(r: Ref, ir: &IrModify, types: &Types) -> IntSize {
    match primitive_of_ref(r, ir, types) {
        Primitive::I8 | Primitive::U8 => IntSize::I8,
        Primitive::I16 | Primitive::U16 => IntSize::I16,
        Primitive::I32 | Primitive::U32 => IntSize::I32,
        Primitive::I64 | Primitive::U64 => IntSize::I64,
        Primitive::I128 | Primitive::U128 => IntSize::I128,
        Primitive::I1 | Primitive::F32 | Primitive::F64 | Primitive::Ptr => {
            unreachable!()
        }
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
    env: &Environment,
    x86: ModuleOf<X86>,
    mc: ModuleOf<Mc>,
    block: BlockId,
    cmp: Ref,
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
    let next_block = ctx.next_block(block);
    match (ctx.regs.get(a), ctx.regs.get(b)) {
        (&[a], &[b]) => {
            ir.replace(env, cmp, x86.cmp_rr32(a, b, ctx.unit));
        }
        _ => todo!("large int comparisons"),
    }
    if next_block.is_some_and(|next| next == b1) {
        // if b1 is the next block, we want to invert the condition to only emit one jump
        ctx.create_args_copy(env, r, mc, ir, b2, &b2_args.to_vec());
        ir.add_before(env, r, inverse_cond_jmp(b2));
        ctx.create_args_copy(env, r, mc, ir, b1, &b1_args.to_vec());
        // the jmp is still emitted (to maintain correct successor info)
        // but will be remvoed during emit
        x86.jmp(b1, ctx.unit)
    } else {
        ctx.create_args_copy(env, r, mc, ir, b1, &b1_args.to_vec());
        ir.add_before(env, r, cond_jmp(b1));
        ctx.create_args_copy(env, r, mc, ir, b2, &b2_args.to_vec());
        x86.jmp(b2, ctx.unit)
    }
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
            IntSize::I8 => {
                ir.replace(env, r, x86.mov_ri8(regs[0], x as u8 as u32, ctx.unit));
            }
            IntSize::I16 => {
                ir.replace(env, r, x86.mov_ri16(regs[0], x as u16 as u32, ctx.unit));
            }
            IntSize::I32 => {
                ir.replace(env, r, x86.mov_ri32(regs[0], x as u32, ctx.unit));
            }
            IntSize::I64 => {
                if x > u32::MAX as u64 {
                    todo!()
                }
                ir.replace(env, r, x86.mov_ri64(regs[0], x.try_into().unwrap(), ctx.unit));
            }
            IntSize::I128 => todo!("128 bit ints"),
        }
    };
    (%r = arith.Neg x) => {
        let x = ctx.regs.get(x);
        let out = ctx.regs.get(r);
        match int_size_of_ref(r, ir, types) {
            IntSize::I8 => {
                let (&[out], &[x]) = (out, x) else { unreachable!() };
                ctx.copy(env, r, mc, ir, &[out, x]);
                ir.replace(env, r, x86.neg_r8(out, ctx.unit));
            }
            IntSize::I16 => {
                let (&[out], &[x]) = (out, x) else { unreachable!() };
                ctx.copy(env, r, mc, ir, &[out, x]);
                ir.replace(env, r, x86.neg_r16(out, ctx.unit));
            }
            IntSize::I32 => {
                let (&[out], &[x]) = (out, x) else { unreachable!() };
                ctx.copy(env, r, mc, ir, &[out, x]);
                ir.replace(env, r, x86.neg_r32(x, ctx.unit));
            }
            IntSize::I64 => {
                let (&[out], &[x]) = (out, x) else { unreachable!() };
                ctx.copy(env, r, mc, ir, &[out, x]);
                ir.replace(env, r, x86.neg_r64(x, ctx.unit));
            }
            IntSize::I128 => todo!(),
        }
    };
    (%r = cf.Ret value) => {
        ctx.abi.implement_return(value, ir, env, mc, x86, types, &ctx.regs, r, ctx.unit);
    };
    (%r = cf.Goto (@b b_args)) => {
        let b_args = b_args.to_vec();
        ctx.create_args_copy(env, r, mc, ir, b, &b_args);
        x86.jmp(b, ctx.unit)
    };
    (%r = cf.Branch (%cmp = arith.Eq a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        cmp_branch(ctx, ir, env, x86, mc, block, cmp, r, a, b, b1, b1_args.to_vec(), b2, b2_args.to_vec(),
            |b| x86.je(b),
            |b| x86.jne(b),
        )
    };
    (%r = cf.Branch (%cmp = arith.LT a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        cmp_branch(ctx, ir, env, x86, mc, block, cmp, r, a, b, b1, b1_args.to_vec(), b2, b2_args.to_vec(),
            |b| x86.jl(b),
            |b| x86.jge(b),
        )
    };
    (%r = cf.Branch (%cmp = arith.GT a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        cmp_branch(ctx, ir, env, x86, mc, block, cmp, r, a, b, b1, b1_args.to_vec(), b2, b2_args.to_vec(),
            |b| x86.jg(b),
            |b| x86.jle(b),
        )
    };
    (%r = cf.Branch (%cmp = arith.LE a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        cmp_branch(ctx, ir, env, x86, mc, block, cmp, r, a, b, b1, b1_args.to_vec(), b2, b2_args.to_vec(),
            |b| x86.jle(b),
            |b| x86.jg(b),
        )
    };
    (%r = cf.Branch (%cmp = arith.GE a b) (@b1 b1_args) (@b2 b2_args)) if ctx.single_use(cmp) => {
        // PERF: cloning the args here
        cmp_branch(ctx, ir, env, x86, mc, block, cmp, r, a, b, b1, b1_args.to_vec(), b2, b2_args.to_vec(),
            |b| x86.jge(b),
            |b| x86.jl(b),
        )
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
                ir.add_before(env, r, x86.imul_ri16(out, x as _, ctx.unit));
                ir.replace_with(env, r, Ref::UNIT);
            }
            Primitive::I16 => {
                let a = ctx.regs.get_one(a);
                let out = ctx.regs.get_one(r);
                ctx.copy(env, r, mc, ir, &[out, a]);
                ir.replace(env, r, x86.imul_ri16(out, x as _, ctx.unit));
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
    (%r = mem.Decl (type ty)) => {
        let layout = ir::type_layout(types[ty], types, env.primitives());
        let offset = ctx.alloc_stack(layout);
        let out = ctx.regs.get_one(r);
        x86.lea_rm64(out, MCReg::from_phys(Reg::rbp), (-(offset as i32)) as u32, ctx.unit)
    };
    (%r = mem.Load ptr) => {
        let ptr = ctx.regs.get_one(ptr);
        match types[ir.get_ref_ty(r)] {
            Type::Primitive(primitive_id) => match Primitive::try_from(primitive_id).unwrap() {
                Primitive::I1 | Primitive::I8 | Primitive::U8 => {
                    ir.replace(env, r, x86.mov_rm8(ctx.regs.get_one(r), ptr, 0, ctx.unit));
                }
                Primitive::I16 | Primitive::U16 => {
                    ir.replace(env, r, x86.mov_rm16(ctx.regs.get_one(r), ptr, 0, ctx.unit));
                }
                Primitive::I32 | Primitive::U32 => {
                    ir.replace(env, r, x86.mov_rm32(ctx.regs.get_one(r), ptr, 0, ctx.unit));
                }
                Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                    ir.replace(env, r, x86.mov_rm64(ctx.regs.get_one(r), ptr, 0, ctx.unit));
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
                    ir.replace(env, r, x86.mov_mr8(ptr, 0, ctx.regs.get_one(value), ctx.unit));
                }
                Primitive::I16 | Primitive::U16 => {
                    ir.replace(env, r, x86.mov_mr16(ptr, 0, ctx.regs.get_one(value), ctx.unit));
                }
                Primitive::I32 | Primitive::U32 => {
                    ir.replace(env, r, x86.mov_mr32(ptr, 0, ctx.regs.get_one(value), ctx.unit));
                }
                Primitive::I64 | Primitive::U64 | Primitive::Ptr => {
                    ir.replace(env, r, x86.mov_mr64(ptr, 0, ctx.regs.get_one(value), ctx.unit));
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
