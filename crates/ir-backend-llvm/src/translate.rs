#![allow(unsafe_op_in_unsafe_fn)]

use std::{collections::VecDeque, ffi::CString, ptr};

use ir::{
    Argument, BlockId, BlockTarget, Environment, FunctionId, GlobalId, ModuleId, Ref, Type, TypeId,
    Types, dialect::Primitive,
};
use llvm_sys::{
    LLVMIntPredicate, LLVMRealPredicate, LLVMTypeKind,
    core::{
        self, LLVMAddFunction, LLVMAddGlobal, LLVMAddIncoming, LLVMBuildAdd, LLVMBuildAlloca,
        LLVMBuildAnd, LLVMBuildBr, LLVMBuildCall2, LLVMBuildCondBr, LLVMBuildExtractValue,
        LLVMBuildFAdd, LLVMBuildFCmp, LLVMBuildFDiv, LLVMBuildFMul, LLVMBuildFRem, LLVMBuildFSub,
        LLVMBuildICmp, LLVMBuildInBoundsGEP2, LLVMBuildIntCast2, LLVMBuildLoad2, LLVMBuildMul,
        LLVMBuildNeg, LLVMBuildNot, LLVMBuildOr, LLVMBuildPhi, LLVMBuildRet, LLVMBuildRetVoid,
        LLVMBuildSDiv, LLVMBuildSRem, LLVMBuildStore, LLVMBuildSub, LLVMBuildUDiv, LLVMBuildURem,
        LLVMConstInt, LLVMConstReal, LLVMFunctionType, LLVMGetEnumAttributeKindForName,
        LLVMGetParam, LLVMGetReturnType, LLVMGetTypeKind, LLVMGetUndef, LLVMInt1TypeInContext,
        LLVMInt8TypeInContext, LLVMInt32TypeInContext, LLVMPointerTypeInContext,
        LLVMPositionBuilderAtEnd, LLVMPrintValueToString, LLVMSetInitializer,
        LLVMVoidTypeInContext,
    },
    prelude::{LLVMBuilderRef, LLVMContextRef, LLVMModuleRef, LLVMTypeRef, LLVMValueRef},
};

use crate::{Dialects, Error, FALSE, Intrinsics, NONE, llvm_bool};

pub unsafe fn add_function(
    ctx: LLVMContextRef,
    llvm_module: LLVMModuleRef,
    function: &ir::Function,
    attribs: &Attribs,
) -> Result<(LLVMValueRef, LLVMTypeRef), Error> {
    let return_ty = llvm_ty(
        ctx,
        function.types()[function
            .return_type()
            .expect("function needs explicit return type to be lowered")],
        function.types(),
    )
    .unwrap_or_else(|| LLVMVoidTypeInContext(ctx));
    let mut params: Vec<_> = function
        .params()
        .iter()
        .filter_map(|param| {
            let &ir::Parameter::RefOf(ty) = param else {
                panic!("invalid parameter kind {param:?}")
            };
            llvm_ty(ctx, function.types()[ty], function.types())
        })
        .collect();
    let varargs = function.varargs().is_some_and(|arg| {
        assert_eq!(arg, ir::Parameter::Ref, "unsupported type for varargs");
        true
    });
    let llvm_func_ty = LLVMFunctionType(
        return_ty,
        params.as_mut_ptr(),
        params.len() as _,
        llvm_bool(varargs),
    );
    let name = CString::new(function.name.to_string()).map_err(|_| Error::NulByte)?;
    let llvm_func = LLVMAddFunction(llvm_module, name.as_ptr(), llvm_func_ty);
    if function.flags().terminator() {
        let noreturn = core::LLVMCreateEnumAttribute(ctx, attribs.noreturn, 0);
        core::LLVMAddAttributeAtIndex(llvm_func, -1i32 as u32, noreturn);
    }
    Ok((llvm_func, llvm_func_ty))
}

pub fn add_global(
    ctx: LLVMContextRef,
    llvm_module: LLVMModuleRef,
    global: &ir::Global,
) -> Result<LLVMValueRef, Error> {
    let ty = unsafe {
        core::LLVMArrayType2(
            LLVMInt8TypeInContext(ctx),
            global.value.len().try_into().unwrap(),
        )
    };
    let name = CString::new(global.name.as_bytes()).map_err(|_| Error::NulByte)?;
    let llvm_global = unsafe { LLVMAddGlobal(llvm_module, ty, name.as_ptr()) };
    unsafe {
        let value = core::LLVMConstStringInContext2(
            ctx,
            global.value.as_ptr().cast(),
            global.value.len(),
            1,
        );
        LLVMSetInitializer(llvm_global, value);
        core::LLVMSetAlignment(
            llvm_global,
            global
                .align
                .get()
                .try_into()
                .expect("align too large for llvm"),
        );
        if global.readonly {
            core::LLVMSetGlobalConstant(llvm_global, 1);
        }
    }
    Ok(llvm_global)
}

pub unsafe fn function(
    env: &Environment,
    module_id: ModuleId,
    ctx: LLVMContextRef,
    dialects: &Dialects,
    llvm_module: LLVMModuleRef,
    llvm_funcs: &[(LLVMValueRef, LLVMTypeRef)],
    globals: &[LLVMValueRef],
    llvm_func: LLVMValueRef,
    builder: LLVMBuilderRef,
    function: &ir::Function,
    intrinsics: &Intrinsics,
) -> Result<(), Error> {
    if let Some(ir) = function.ir() {
        build_func(
            env,
            module_id,
            function,
            dialects,
            llvm_module,
            llvm_funcs,
            globals,
            ir,
            ctx,
            builder,
            llvm_func,
            intrinsics,
        );
    }
    Ok(())
}

pub struct Attribs {
    noreturn: u32,
}
impl Attribs {
    pub fn lookup() -> Self {
        fn l(s: &str) -> u32 {
            unsafe { LLVMGetEnumAttributeKindForName(s.as_ptr().cast(), s.len()) }
        }
        Self {
            noreturn: l("noreturn"),
        }
    }
}

/// Converts an ir type to it's corresponding llvm type. Returns `None` if the type was zero-sized
unsafe fn llvm_ty(ctx: LLVMContextRef, ty: Type, types: &Types) -> Option<LLVMTypeRef> {
    match ty {
        Type::Primitive(id) => Some(match Primitive::try_from(id).unwrap() {
            Primitive::I1 => core::LLVMInt1TypeInContext(ctx),
            Primitive::I8 | Primitive::U8 => core::LLVMInt8TypeInContext(ctx),
            Primitive::I16 | Primitive::U16 => core::LLVMInt16TypeInContext(ctx),
            Primitive::I32 | Primitive::U32 => core::LLVMInt32TypeInContext(ctx),
            Primitive::I64 | Primitive::U64 => core::LLVMInt64TypeInContext(ctx),
            Primitive::I128 | Primitive::U128 => core::LLVMInt128TypeInContext(ctx),
            Primitive::F32 => core::LLVMFloatTypeInContext(ctx),
            Primitive::F64 => core::LLVMDoubleTypeInContext(ctx),
            Primitive::Ptr => core::LLVMPointerTypeInContext(ctx, 0),
        }),
        Type::Array(elem, size) => Some(core::LLVMArrayType2(
            llvm_ty(ctx, types[elem], types)?,
            size as u64,
        )),
        Type::Tuple(elems) => {
            if elems.count() == 0 {
                return None;
            }
            let mut types = elems
                .iter()
                .filter_map(|ty| llvm_ty(ctx, types[ty], types))
                .collect::<Vec<_>>();
            if types.is_empty() {
                return None;
            }
            Some(core::LLVMStructTypeInContext(
                ctx,
                types.as_mut_ptr(),
                types.len() as _,
                FALSE,
            ))
        }
    }
}

unsafe fn build_func(
    env: &Environment,
    module: ModuleId,
    func: &ir::Function,
    dialects: &Dialects,
    llvm_module: LLVMModuleRef,
    llvm_funcs: &[(LLVMValueRef, LLVMTypeRef)],
    globals: &[LLVMValueRef],
    ir: &ir::FunctionIr,
    ctx: LLVMContextRef,
    builder: LLVMBuilderRef,
    llvm_func: LLVMValueRef,
    intrinsics: &Intrinsics,
) {
    tracing::debug!(
        target: "backend-translate",
        "Translating function {} to LLVM IR",
        func.name
    );

    let types = func.types();

    let mut instructions = vec![ptr::null_mut(); ir.inst_count() as usize];

    // create an LLVM block for each block while also creating a Phi node for each incoming block arg.
    let blocks: Vec<_> = ir
        .block_ids()
        .map(|block| {
            let llvm_block = core::LLVMAppendBasicBlockInContext(ctx, llvm_func, NONE);

            let block_args = ir.get_block_args(block);
            LLVMPositionBuilderAtEnd(builder, llvm_block);
            let mut llvm_param_idx = 0;
            for arg in block_args.iter() {
                if block == ir::BlockId::ENTRY {
                    let ty = types[ir.get_ref_ty(arg)];
                    // skip all zero-sized parameters
                    // PERF: don't have to generate an llvm_ty here could use some is_zero_sized function
                    if llvm_ty(ctx, ty, types).is_some() {
                        instructions[arg.idx()] = LLVMGetParam(llvm_func, llvm_param_idx);
                        llvm_param_idx += 1;
                    }
                } else {
                    let arg_ty = types[ir.get_ref_ty(arg)];
                    if let Some(ty) = llvm_ty(ctx, arg_ty, types) {
                        instructions[arg.idx()] = LLVMBuildPhi(builder, ty, NONE);
                    }
                }
            }
            llvm_block
        })
        .collect();

    let block_graph = ir::BlockGraph::calculate(ir, env);

    let i1 = LLVMInt1TypeInContext(ctx);

    let get_ref_and_type_ptr =
        |instructions: &[LLVMValueRef], r: Ref| -> (Option<LLVMValueRef>, Type) {
            match r {
                Ref::UNIT => (None, Type::UNIT),
                Ref::TRUE => (
                    Some(LLVMConstInt(i1, 1, 0)),
                    Type::Primitive(Primitive::I1.id()),
                ),
                Ref::FALSE => (
                    Some(LLVMConstInt(i1, 0, 0)),
                    Type::Primitive(Primitive::I1.id()),
                ),
                _ => {
                    let inst = ir.get_inst(r);

                    let r = instructions[r.idx()];
                    let ty = types[inst.ty()];
                    ((!r.is_null()).then_some(r), ty)
                }
            }
        };
    let get_ref_and_type =
        |instructions: &[LLVMValueRef], r: Ref| get_ref_and_type_ptr(instructions, r);
    let get_ref = |instructions: &[LLVMValueRef], r: Ref| match r {
        Ref::UNIT => None,
        Ref::TRUE => Some(LLVMConstInt(i1, 1, 0)),
        Ref::FALSE => Some(LLVMConstInt(i1, 0, 0)),
        _ => {
            let r = instructions[r.idx()];
            (!r.is_null()).then_some(r)
        }
    };

    let table_ty = |ty: TypeId| llvm_ty(ctx, types[ty], types);

    let mut queued_blocks = vec![false; ir.block_ids().len()];
    let mut block_queue = VecDeque::from([BlockId::ENTRY]);
    queued_blocks[BlockId::ENTRY.idx()] = true;

    for block in block_graph.postorder().iter().copied().rev() {
        let llvm_block = blocks[block.idx()];

        let mut handle_successor =
            |ir: &ir::FunctionIr, instructions: &[LLVMValueRef], successor: BlockTarget| {
                for (block_arg_ref, &r) in ir
                    .get_block_args(successor.0)
                    .iter()
                    .zip(successor.1.iter())
                {
                    if let Some(mut llvm_val) = get_ref(instructions, r) {
                        let phi = instructions[block_arg_ref.idx()];
                        if !phi.is_null() {
                            let mut block = llvm_block;
                            LLVMAddIncoming(phi, &mut llvm_val, &mut block, 1);
                        }
                    }
                }
                if !queued_blocks[successor.0.idx()] {
                    queued_blocks[successor.0.idx()] = true;
                    block_queue.push_back(successor.0);
                }
            };

        LLVMPositionBuilderAtEnd(builder, llvm_block);
        for (i, inst) in ir.get_block(block) {
            tracing::debug!(target: "backend-gen", "Generating %{i:?} = {inst:?}");
            let val: LLVMValueRef = if let Some(inst) = inst.as_module(ir::BUILTIN) {
                match inst.op() {
                    ir::Builtin::Nothing => ptr::null_mut(),
                    ir::Builtin::BlockArg => unreachable!(),
                    ir::Builtin::Undef => llvm_ty(ctx, types[inst.ty()], types)
                        .map(|ty| LLVMGetUndef(ty))
                        .unwrap_or(ptr::null_mut()),
                    ir::Builtin::Copy => unreachable!(),
                }
            } else if let Some(inst) = inst.as_module(dialects.arith) {
                use ir::dialect::Arith as I;
                let un_op = || -> Ref { ir.typed_args(&inst) };
                let bin_op = || -> (Ref, Ref) { ir.typed_args(&inst) };
                let comparison = |builder: LLVMBuilderRef,
                                  instructions: &[LLVMValueRef],
                                  (a, b): (Ref, Ref),
                                  preds: (
                    LLVMIntPredicate,
                    LLVMIntPredicate,
                    LLVMRealPredicate,
                )| {
                    let (l, ty) = get_ref_and_type(instructions, a);
                    let l = l.unwrap();
                    let r = get_ref(instructions, b).unwrap();
                    match ty {
                        t if is_unsigned_int(t) => LLVMBuildICmp(builder, preds.0, l, r, NONE),
                        t if is_signed_int(t) => LLVMBuildICmp(builder, preds.1, l, r, NONE),
                        t if is_float(t) => LLVMBuildFCmp(builder, preds.2, l, r, NONE),
                        Type::Primitive(p) if p == Primitive::Ptr.id() => {
                            LLVMBuildICmp(builder, preds.0, l, r, NONE)
                        }
                        t => panic!("invalid type for lt {t:?}"),
                    }
                };
                match inst.op() {
                    I::Int => {
                        let int: u64 = ir.typed_args(&inst);
                        LLVMConstInt(table_ty(inst.ty()).unwrap(), int, FALSE)
                    }
                    I::Float => {
                        let float: f64 = ir.typed_args(&inst);
                        LLVMConstReal(table_ty(inst.ty()).unwrap(), float)
                    }
                    I::Neg => {
                        let r = get_ref(&instructions, un_op()).unwrap();
                        match types[inst.ty()] {
                            Type::Primitive(p) => match Primitive::try_from(p).unwrap() {
                                p if p.is_int() => LLVMBuildNeg(builder, r, NONE),
                                p if p.is_float() => LLVMBuildNeg(builder, r, NONE),
                                _ => panic!("invalid primitive for neg"),
                            },
                            _ => panic!("invalid type for neg"),
                        }
                    }
                    I::Not => {
                        let l = get_ref(&instructions, un_op()).unwrap();
                        LLVMBuildNot(builder, l, NONE)
                    }
                    I::Add | I::Sub | I::Mul | I::Div | I::Rem | I::Or | I::And => {
                        let (l, r) = bin_op();
                        let l = get_ref(&instructions, l).unwrap();
                        let r = get_ref(&instructions, r).unwrap();

                        match inst.op() {
                            I::Add => match types[inst.ty()] {
                                t if is_int(t) => LLVMBuildAdd(builder, l, r, NONE),
                                t if is_float(t) => LLVMBuildFAdd(builder, l, r, NONE),
                                _ => panic!("invalid type for add"),
                            },
                            I::Sub => match types[inst.ty()] {
                                t if is_int(t) => LLVMBuildSub(builder, l, r, NONE),
                                t if is_float(t) => LLVMBuildFSub(builder, l, r, NONE),
                                _ => panic!("invalid type for sub"),
                            },
                            I::Mul => match types[inst.ty()] {
                                t if is_int(t) => LLVMBuildMul(builder, l, r, NONE),
                                t if is_float(t) => LLVMBuildFMul(builder, l, r, NONE),
                                _ => panic!("invalid type for mul"),
                            },
                            I::Div => match types[inst.ty()] {
                                t if is_unsigned_int(t) => LLVMBuildUDiv(builder, l, r, NONE),
                                t if is_signed_int(t) => LLVMBuildSDiv(builder, l, r, NONE),
                                t if is_float(t) => LLVMBuildFDiv(builder, l, r, NONE),
                                _ => panic!("invalid type for udiv"),
                            },
                            I::Rem => match types[inst.ty()] {
                                t if is_unsigned_int(t) => LLVMBuildURem(builder, l, r, NONE),
                                t if is_signed_int(t) => LLVMBuildSRem(builder, l, r, NONE),
                                t if is_float(t) => LLVMBuildFRem(builder, l, r, NONE),
                                _ => panic!("invalid type for urem"),
                            },
                            I::Or => LLVMBuildOr(builder, l, r, NONE),
                            I::And => LLVMBuildAnd(builder, l, r, NONE),
                            _ => unreachable!(),
                        }
                    }
                    I::Eq | I::NE => {
                        let is_eq = inst.op() == I::Eq;
                        let (a, b) = bin_op();
                        let (l, ty) = get_ref_and_type(&instructions, a);
                        let r = get_ref(&instructions, b);

                        if let (Some(l), Some(r)) = (l, r) {
                            match ty {
                                t if is_int(t) => {
                                    let pred = if is_eq {
                                        LLVMIntPredicate::LLVMIntEQ
                                    } else {
                                        LLVMIntPredicate::LLVMIntNE
                                    };
                                    LLVMBuildICmp(builder, pred, l, r, NONE)
                                }
                                t if is_float(t) => {
                                    let pred = if is_eq {
                                        LLVMRealPredicate::LLVMRealUEQ
                                    } else {
                                        LLVMRealPredicate::LLVMRealUNE
                                    };
                                    LLVMBuildFCmp(builder, pred, l, r, NONE)
                                }
                                Type::Primitive(p) if p == Primitive::Ptr.id() => {
                                    let pred = if is_eq {
                                        LLVMIntPredicate::LLVMIntEQ
                                    } else {
                                        LLVMIntPredicate::LLVMIntNE
                                    };
                                    LLVMBuildICmp(builder, pred, l, r, NONE)
                                }
                                t => panic!("invalid type for eq/ne: {t:?}"),
                            }
                        } else {
                            ptr::null_mut()
                        }
                    }
                    I::LT => comparison(
                        builder,
                        &instructions,
                        bin_op(),
                        (
                            LLVMIntPredicate::LLVMIntULT,
                            LLVMIntPredicate::LLVMIntSLT,
                            LLVMRealPredicate::LLVMRealOLT,
                        ),
                    ),
                    I::GT => comparison(
                        builder,
                        &instructions,
                        bin_op(),
                        (
                            LLVMIntPredicate::LLVMIntUGT,
                            LLVMIntPredicate::LLVMIntSGT,
                            LLVMRealPredicate::LLVMRealOGT,
                        ),
                    ),
                    I::LE => comparison(
                        builder,
                        &instructions,
                        bin_op(),
                        (
                            LLVMIntPredicate::LLVMIntULE,
                            LLVMIntPredicate::LLVMIntSLE,
                            LLVMRealPredicate::LLVMRealOLE,
                        ),
                    ),
                    I::GE => comparison(
                        builder,
                        &instructions,
                        bin_op(),
                        (
                            LLVMIntPredicate::LLVMIntUGE,
                            LLVMIntPredicate::LLVMIntSGE,
                            LLVMRealPredicate::LLVMRealOGE,
                        ),
                    ),
                    I::Xor => {
                        let (l, r) = bin_op();
                        core::LLVMBuildXor(
                            builder,
                            get_ref(&instructions, l).unwrap(),
                            get_ref(&instructions, r).unwrap(),
                            NONE,
                        )
                    }
                    I::Shl => {
                        let (l, r) = bin_op();
                        core::LLVMBuildShl(
                            builder,
                            get_ref(&instructions, l).unwrap(),
                            get_ref(&instructions, r).unwrap(),
                            NONE,
                        )
                    }
                    I::Shr => {
                        let (l, r) = bin_op();
                        let op = if is_unsigned_int(types[inst.ty()]) {
                            core::LLVMBuildLShr
                        } else {
                            debug_assert!(is_signed_int(types[inst.ty()]));
                            core::LLVMBuildAShr
                        };
                        op(
                            builder,
                            get_ref(&instructions, l).unwrap(),
                            get_ref(&instructions, r).unwrap(),
                            NONE,
                        )
                    }
                    I::Rol | I::Ror => {
                        let (l, r) = bin_op();
                        let ((Some(l), l_ty), Some(r)) = (
                            get_ref_and_type(&instructions, l),
                            get_ref(&instructions, r),
                        ) else {
                            panic!("invalid types for rol/ror")
                        };
                        let l_ty = llvm_ty(ctx, l_ty, types).unwrap();
                        let mut fshl_args = [l_ty];
                        let intrinsic = if inst.op() == I::Rol {
                            intrinsics.fshl
                        } else {
                            intrinsics.fshr
                        };
                        let function = core::LLVMGetIntrinsicDeclaration(
                            llvm_module,
                            intrinsic,
                            fshl_args.as_mut_ptr(),
                            fshl_args.len(),
                        );
                        let ty = core::LLVMIntrinsicGetType(
                            ctx,
                            intrinsics.fshl,
                            fshl_args.as_mut_ptr(),
                            fshl_args.len(),
                        );
                        let mut args = [l, l, r];
                        LLVMBuildCall2(
                            builder,
                            ty,
                            function,
                            args.as_mut_ptr(),
                            args.len() as u32,
                            NONE,
                        )
                    }
                    I::CastInt => {
                        let val = get_ref(&instructions, un_op());
                        // there are no zero-sized integers so unwraps are fine
                        let val = val.unwrap();
                        let to_ty = types[inst.ty()];
                        let signed = is_signed_int(to_ty);
                        let target_ty = llvm_ty(ctx, to_ty, types).unwrap();
                        LLVMBuildIntCast2(builder, val, target_ty, llvm_bool(signed), NONE)
                    }
                    I::CastFloat => {
                        let (val, from_ty) = get_ref_and_type(&instructions, un_op());
                        // there are no zero-sized floats so unwraps are fine
                        let val = val.unwrap();
                        let to_ty = types[inst.ty()];
                        let target_ty = llvm_ty(ctx, to_ty, types).unwrap();
                        let (Type::Primitive(from_ty), Type::Primitive(to_ty)) = (from_ty, to_ty)
                        else {
                            panic!()
                        };
                        let from_ty = Primitive::try_from(from_ty).unwrap();
                        let to_ty = Primitive::try_from(to_ty).unwrap();
                        match (from_ty, to_ty) {
                            (Primitive::F32, Primitive::F64) => {
                                core::LLVMBuildFPExt(builder, val, target_ty, NONE)
                            }
                            (Primitive::F64, Primitive::F32) => {
                                core::LLVMBuildFPTrunc(builder, val, target_ty, NONE)
                            }
                            _ => panic!("invalid types for CastFloat"),
                        }
                    }
                    I::CastIntToFloat => {
                        let (val, from_ty) = get_ref_and_type(&instructions, un_op());
                        // there are no zero-sized ints/floats so unwraps are fine
                        let val = val.unwrap();
                        let to_ty = types[inst.ty()];
                        let target_ty = llvm_ty(ctx, to_ty, types).unwrap();
                        if is_unsigned_int(from_ty) {
                            core::LLVMBuildUIToFP(builder, val, target_ty, NONE)
                        } else {
                            core::LLVMBuildSIToFP(builder, val, target_ty, NONE)
                        }
                    }
                    I::CastFloatToInt => {
                        // there are no zero-sized ints/floats so unwraps are fine
                        let val = get_ref(&instructions, un_op()).unwrap();
                        let to_ty = types[inst.ty()];
                        let target_ty = llvm_ty(ctx, to_ty, types).unwrap();
                        if is_unsigned_int(to_ty) {
                            core::LLVMBuildFPToUI(builder, val, target_ty, NONE)
                        } else {
                            core::LLVMBuildFPToSI(builder, val, target_ty, NONE)
                        }
                    }
                }
            } else if let Some(inst) = inst.as_module(dialects.cf) {
                use ir::dialect::Cf as I;
                match inst.op() {
                    I::Goto => {
                        let target: BlockTarget = ir.typed_args(&inst);
                        let target_block = target.0;
                        handle_successor(ir, &instructions, target);
                        LLVMBuildBr(builder, blocks[target_block.idx()])
                    }
                    I::Branch => {
                        let (cond, on_true, on_false): (Ref, BlockTarget, BlockTarget) =
                            ir.typed_args(&inst);
                        let cond = get_ref(&instructions, cond).unwrap();
                        let on_true_block = on_true.0;
                        let on_false_block = on_false.0;
                        handle_successor(ir, &instructions, on_true);
                        handle_successor(ir, &instructions, on_false);
                        LLVMBuildCondBr(
                            builder,
                            cond,
                            blocks[on_true_block.idx()],
                            blocks[on_false_block.idx()],
                        )
                    }
                    I::Ret => {
                        let value = ir.typed_args(&inst);
                        let (r, ty) = get_ref_and_type(&instructions, value);

                        if let Some(r) =
                            r.and_then(|r| llvm_ty(ctx, ty, types).is_some().then_some(r))
                        {
                            LLVMBuildRet(builder, r)
                        } else {
                            LLVMBuildRetVoid(builder)
                        }
                    }
                }
            } else if let Some(inst) = inst.as_module(dialects.mem) {
                use ir::dialect::Mem as I;
                match inst.op() {
                    I::Decl => {
                        let ty = ir.typed_args(&inst);
                        if let Some(ty) = table_ty(ty) {
                            LLVMBuildAlloca(builder, ty, NONE)
                        } else {
                            // return a null pointer if a declaration has a zero-sized type
                            core::LLVMConstPointerNull(LLVMPointerTypeInContext(ctx, 0))
                        }
                    }
                    I::Load => {
                        let r = ir.typed_args(&inst);
                        let val = get_ref(&instructions, r);
                        if let Some(pointee_ty) = llvm_ty(ctx, types[inst.ty()], types) {
                            let val = val.unwrap();
                            LLVMBuildLoad2(builder, pointee_ty, val, NONE)
                        } else {
                            ptr::null_mut()
                        }
                    }
                    I::Store => {
                        let (ptr, val) = ir.typed_args(&inst);
                        let (val, ty) = get_ref_and_type(&instructions, val);
                        if !types.is_zero_sized(ty, env.primitives()) {
                            let ptr = get_ref(&instructions, ptr).unwrap();
                            LLVMBuildStore(builder, val.unwrap(), ptr);
                        }
                        ptr::null_mut()
                    }
                    I::MemberPtr => {
                        let (ptr, tuple, idx): (Ref, TypeId, u32) = ir.typed_args(&inst);
                        let Type::Tuple(elem_types) = types[tuple] else {
                            panic!()
                        };
                        let offset = ir::offset_in_tuple(elem_types, idx, types, env.primitives());

                        let i8_ty = LLVMInt8TypeInContext(ctx);
                        if let Some(llvm_ptr) = get_ref(&instructions, ptr) {
                            let mut offset =
                                LLVMConstInt(LLVMInt32TypeInContext(ctx), offset, FALSE);
                            LLVMBuildInBoundsGEP2(builder, i8_ty, llvm_ptr, &mut offset, 1, NONE)
                        } else {
                            ptr::null_mut()
                        }
                    }
                    I::Offset => {
                        let (ptr, offset): (Ref, u32) = ir.typed_args(&inst);
                        let i8_ty = LLVMInt8TypeInContext(ctx);
                        if let Some(llvm_ptr) = get_ref(&instructions, ptr) {
                            let mut offset =
                                LLVMConstInt(LLVMInt32TypeInContext(ctx), offset.into(), FALSE);
                            LLVMBuildInBoundsGEP2(builder, i8_ty, llvm_ptr, &mut offset, 1, NONE)
                        } else {
                            ptr::null_mut()
                        }
                    }
                    I::IntToPtr => {
                        let int = ir.typed_args(&inst);
                        // no zero-sized integers so unwrapping is fine
                        let val = get_ref(&instructions, int).unwrap();
                        core::LLVMBuildIntToPtr(
                            builder,
                            val,
                            LLVMPointerTypeInContext(ctx, 0),
                            NONE,
                        )
                    }
                    I::PtrToInt => {
                        let ptr = ir.typed_args(&inst);
                        // no zero-sized integers so unwrapping is fine
                        let val = get_ref(&instructions, ptr).unwrap();
                        let ty = llvm_ty(ctx, types[inst.ty()], types).unwrap();
                        core::LLVMBuildPtrToInt(builder, val, ty, NONE)
                    }
                    I::Global => {
                        let id: GlobalId = ir.typed_args(&inst);
                        if id.module != module {
                            panic!("unsupported: can't take global in in different module")
                        }
                        globals[id.idx as usize]
                    }
                    I::ArrayIndex => {
                        let (array_ptr, elem_ty, idx): (Ref, TypeId, Ref) = ir.typed_args(&inst);
                        let ptr = get_ref(&instructions, array_ptr).unwrap();
                        let ty = llvm_ty(ctx, func[elem_ty], func.types()).unwrap();
                        let mut indices = [get_ref(&instructions, idx).unwrap()];
                        LLVMBuildInBoundsGEP2(
                            builder,
                            ty,
                            ptr,
                            indices.as_mut_ptr(),
                            indices.len() as _,
                            NONE,
                        )
                    }
                    I::FunctionPtr => {
                        let func_id: FunctionId = ir.typed_args(&inst);
                        if func_id.module != module {
                            panic!(
                                "unsupported: can't take function pointer of function in different module"
                            )
                        }
                        let (llvm_func, _) = llvm_funcs[func_id.function.idx()];
                        llvm_func
                    }
                    I::CallPtr => {
                        let mut args = ir.typed_args_iter(&inst);
                        let Some(Argument::Ref(ptr)) = args.next() else {
                            unreachable!()
                        };
                        let mut arg_refs = Vec::new();
                        let mut arg_types = Vec::new();
                        for arg in args {
                            let Argument::Ref(arg) = arg else {
                                unreachable!()
                            };
                            if let (Some(r), ty) = get_ref_and_type(&instructions, arg) {
                                arg_refs.push(r);
                                let ty = llvm_ty(ctx, ty, func.types()).unwrap();
                                arg_types.push(ty);
                            }
                        }
                        let ptr = get_ref(&instructions, ptr).unwrap();
                        let return_ty = llvm_ty(ctx, func[inst.ty()], func.types())
                            .unwrap_or_else(|| LLVMVoidTypeInContext(ctx));

                        let ty = LLVMFunctionType(
                            return_ty,
                            arg_types.as_mut_ptr(),
                            arg_types.len() as _,
                            FALSE,
                        );
                        LLVMBuildCall2(
                            builder,
                            ty,
                            ptr,
                            arg_refs.as_mut_ptr(),
                            arg_refs.len() as _,
                            NONE,
                        )
                    }
                }
            } else if let Some(inst) = inst.as_module(dialects.tuple) {
                use ir::dialect::Tuple as I;
                match inst.op() {
                    I::MemberValue => {
                        let (tuple, mut idx): (Ref, u32) = ir.typed_args(&inst);
                        if let (Some(val), tuple_ty) = get_ref_and_type(&instructions, tuple) {
                            let Type::Tuple(ids) = tuple_ty else { panic!() };
                            for ty in ids.iter().take(idx as usize) {
                                if types.is_zero_sized(types[ty], env.primitives()) {
                                    idx -= 1;
                                }
                            }
                            LLVMBuildExtractValue(builder, val, idx, NONE)
                        } else {
                            ptr::null_mut()
                        }
                    }
                    I::InsertMember => {
                        let (tuple, mut idx, member): (Ref, u32, Ref) = ir.typed_args(&inst);
                        let (tuple, tuple_ty) = get_ref_and_type(&instructions, tuple);
                        if let Some(tuple) = tuple {
                            if let Some(member) = get_ref(&instructions, member) {
                                let Type::Tuple(ids) = tuple_ty else { panic!() };
                                for ty in ids.iter().take(idx as usize) {
                                    if types.is_zero_sized(types[ty], env.primitives()) {
                                        idx -= 1;
                                    }
                                }
                                core::LLVMBuildInsertValue(builder, tuple, member, idx, NONE)
                            } else {
                                // inserting a zero-sized member is a no-op
                                tuple
                            }
                        } else {
                            ptr::null_mut()
                        }
                    }
                }
            } else if inst.module() == module {
                let (llvm_func, llvm_func_ty) = llvm_funcs[inst.function().idx()];
                let inst_func = &env[inst.module()][inst.function()];
                let args = inst.args_inner(
                    inst_func.params(),
                    inst_func.varargs(),
                    ir.blocks(),
                    ir.extra(),
                );
                let mut args: Vec<_> = args
                    .filter_map(|arg| {
                        let Argument::Ref(r) = arg else {
                            unreachable!()
                        };
                        get_ref(&instructions, r)
                    })
                    .collect();
                let is_void = LLVMGetTypeKind(LLVMGetReturnType(llvm_func_ty))
                    == LLVMTypeKind::LLVMVoidTypeKind;
                let value = LLVMBuildCall2(
                    builder,
                    llvm_func_ty,
                    llvm_func,
                    args.as_mut_ptr(),
                    args.len() as u32,
                    NONE,
                );
                // don't put the return value if the function returns void because void should
                // not be used
                if is_void { ptr::null_mut() } else { value }
            } else {
                panic!("unknown module")
            };
            if !val.is_null() {
                tracing::debug!(target: "backend-gen", "  -> {:?}", val_str(val));
            }
            instructions[i.idx()] = val;
        }
    }
}

fn is_int(t: Type) -> bool {
    match t {
        Type::Primitive(p) => Primitive::try_from(p).unwrap().is_int(),
        _ => false,
    }
}

fn is_signed_int(t: Type) -> bool {
    matches!(t, Type::Primitive(p) if Primitive::try_from(p).unwrap().is_signed_int())
}

fn is_unsigned_int(t: Type) -> bool {
    matches!(t, Type::Primitive(p) if Primitive::try_from(p).unwrap().is_unsigned_int())
}

fn is_float(t: Type) -> bool {
    match t {
        Type::Primitive(p) => Primitive::try_from(p).unwrap().is_float(),
        _ => false,
    }
}

fn val_str(val: LLVMValueRef) -> CString {
    unsafe { CString::from_raw(LLVMPrintValueToString(val)) }
}
