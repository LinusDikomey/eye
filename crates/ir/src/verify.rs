use dmap::DHashSet;

use crate::{
    Argument, BlockGraph, BlockTarget, Environment, Function, FunctionIr, ModuleId, Primitive, Ref,
    Type, TypeId, Types,
    dialect::{Mem, Tuple},
    mc::UnknownRegister,
};

pub fn module(env: &Environment, module: ModuleId) {
    for id in env[module].function_ids() {
        function(env, &env[module][id]);
    }
}

pub fn function(env: &Environment, function: &Function) {
    if let Some(body) = function.ir() {
        function_body(env, body, &function.types, &function.name);
    }
}

pub fn function_body(env: &Environment, ir: &FunctionIr, types: &Types, function_name: &str) {
    let _enter = tracing::span!(
        target: "verify", tracing::Level::INFO, "Verifying function",
        function=function_name,
    )
    .entered();
    let block_graph = BlockGraph::calculate(ir, env);
    let mut accounted_refs = DHashSet::default();
    let mem = env.get_dialect_module_if_present::<Mem>();
    let tuple = env.get_dialect_module_if_present::<Tuple>();
    for block in ir.block_ids() {
        let info = &ir.blocks[block.idx()];
        if info.arg_count == 0 {
            assert_eq!(
                info.args_idx + 1,
                info.body_idx,
                "Body index of block {block} is desynchronized or args insert point is missing"
            );
            accounted_refs.insert(Ref(info.args_idx));
        } else {
            assert_eq!(
                info.args_idx + info.arg_count,
                info.body_idx,
                "Body index of block {block} is desynchronized"
            );
        }
        for r in ir.get_block_args(block).iter() {
            accounted_refs.insert(r);
        }
        let ref_digits = if ir.insts.len() < 2 {
            1
        } else {
            (ir.insts.len() - 1).ilog10() + 1
        };
        for (r, inst) in ir.get_block(block) {
            let _enter = tracing::span!(
                target: "verify", tracing::Level::INFO, "Verifying instruction",
                "{}",
                inst.display::<UnknownRegister>(r, env, types, ir, ref_digits),
            )
            .entered();
            accounted_refs.insert(r);
            for arg in ir.args_iter(inst, env) {
                match arg {
                    Argument::Ref(arg) => {
                        if let Some(i) = arg.into_ref() {
                            let arg_block = ir.find_block_for_position(i);
                            assert!(
                                block_graph.block_dominates(block, arg_block),
                                "Instruction at {r} uses {arg} from a non-dominating block"
                            );
                        }
                    }
                    Argument::BlockId(target) => {
                        assert!(target.0 <= ir.block_count());
                    }
                    Argument::BlockTarget(BlockTarget(target, args)) => {
                        assert!(target.0 <= ir.block_count());
                        for arg in args.iter() {
                            if let Some(i) = arg.into_ref() {
                                let arg_block = ir.find_block_for_position(i);
                                assert!(
                                    block_graph.block_dominates(block, arg_block),
                                    "Instruction at {r} uses {arg} from a non-dominating block"
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            let assert_ptr_ty = |ty: TypeId, name: &str| {
                let ty = types[ty];
                assert!(
                    matches!(ty, Type::Primitive(p) if Primitive::Ptr == p.try_into().unwrap()),
                    "{name} needs to be a pointer type, got {ty:?} at"
                );
            };
            if let Some(inst) = mem.and_then(|mem| inst.as_module(mem)) {
                match inst.inst {
                    Mem::Decl => assert_ptr_ty(inst.ty, "Decl result"),
                    Mem::Load => {
                        let ptr = ir.typed_args(&inst);
                        assert_ptr_ty(ir.get_ref_ty(ptr), "Load target");
                    }
                    Mem::Store => {
                        let (ptr, _value): (Ref, Ref) = ir.typed_args(&inst);
                        assert_ptr_ty(ir.get_ref_ty(ptr), "Store target");
                    }
                    _ => {}
                }
            } else if let Some(inst) = tuple.and_then(|tuple| inst.as_module(tuple)) {
                match inst.inst {
                    Tuple::MemberValue => {
                        let (tuple, index): (Ref, u32) = ir.typed_args(&inst);
                        let tuple_ty = types[ir.get_ref_ty(tuple)];
                        let Type::Tuple(elems) = tuple_ty else {
                            panic!("Tuple type expected for argument of tuple.MemberValue");
                        };
                        assert!(
                            index < elems.count,
                            "Tuple index out of range for tuple length"
                        );
                        assert!(
                            types.are_equal(elems.nth(index), inst.ty),
                            "result type doesn't match"
                        );
                    }
                    Tuple::InsertMember => {
                        let (tuple, index, value): (Ref, u32, Ref) = ir.typed_args(&inst);
                        let tuple_ty = types[ir.get_ref_ty(tuple)];
                        let Type::Tuple(elems) = tuple_ty else {
                            panic!("Tuple type expected for argument of tuple.MemberValue");
                        };
                        assert!(
                            index < elems.count,
                            "Tuple index out of range for tuple length"
                        );
                        assert!(
                            types.are_equal(elems.nth(index), ir.get_ref_ty(value)),
                            "Type of inserted item doesn't match"
                        );
                    }
                }
            }
        }
    }
    for r in ir.refs() {
        assert!(
            accounted_refs.contains(&r),
            "Instruction {} is not part of any block",
            ir.get_inst(r)
                .display::<UnknownRegister>(r, env, types, ir, 0)
        );
    }
}
