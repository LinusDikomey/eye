use std::cell::OnceCell;

use crate::{
    Argument, BlockId, BlockInfo, Environment, Function, FunctionId, INLINE_ARGS, Instruction,
    ModuleId, Parameter, Ref, Refs, TypeId, Types,
    argument::IntoArgs,
    builtins::{self, Builtin},
    flags::InstFlags,
};

pub trait HasEnvironment {
    fn env(&self) -> &Environment;
}
impl HasEnvironment for &Environment {
    fn env(&self) -> &Environment {
        self
    }
}
impl HasEnvironment for &mut Environment {
    fn env(&self) -> &Environment {
        self
    }
}

pub struct Builder<Env> {
    pub env: Env,
    pub types: Types,
    insts: Vec<Instruction>,
    extra: Vec<u32>,
    blocks: Vec<BlockInfo>,
    current_block: Option<BlockId>,
}
impl<Env: HasEnvironment> Builder<Env> {
    pub fn new(env: Env) -> Self {
        Self::with_types(env, Types::new())
    }

    pub fn current_block(&self) -> Option<BlockId> {
        self.current_block
    }

    pub fn with_types(env: Env, types: Types) -> Self {
        Self {
            env,
            types,
            insts: Vec::new(),
            extra: Vec::new(),
            blocks: Vec::new(),
            current_block: None,
        }
    }

    pub fn begin_function(env: Env, id: FunctionId) -> (Self, Refs) {
        let func = &env.env()[id];
        let arg_count = func.params.len() as u32;
        let mut insts: Vec<_> = func
            .params
            .iter()
            .map(|p| {
                let &Parameter::RefOf(ty) = p else {
                    panic!("Function parameter isn't a typed value, can't create body")
                };
                Instruction {
                    function: crate::FunctionId {
                        module: crate::ModuleId::BUILTINS,
                        function: Builtin::BlockArg.id(),
                    },
                    args: [0, 0],
                    ty,
                }
            })
            .collect();
        if arg_count == 0 {
            insts.push(Instruction::NOTHING);
        }
        let types = func.types.clone();
        let builder = Self {
            env,
            types,
            insts,
            extra: Vec::new(),
            blocks: vec![BlockInfo {
                arg_count,
                args_idx: 0,
                body_idx: arg_count.max(1),
                len: 0,
                preds: Vec::new(),
                succs: Vec::new(),
            }],
            current_block: Some(BlockId::ENTRY),
        };
        let params = Refs {
            idx: 0,
            count: arg_count,
        };
        (builder, params)
    }

    #[track_caller]
    #[inline]
    pub fn append<'r>(
        &mut self,
        (function, args, ty): (FunctionId, impl IntoArgs<'r>, TypeId),
    ) -> Ref {
        let Some(current_block) = self.current_block else {
            panic!("tried to append to block while no block was active");
        };
        let def = &self.env.env()[function];
        let terminator = def.flags.terminator();
        let args = write_args(
            &mut self.extra,
            current_block,
            &mut self.blocks,
            &def.params,
            def.varargs,
            args,
        );
        let r = Ref(self.insts.len() as _);
        self.insts.push(Instruction { function, args, ty });
        self.blocks[current_block.0 as usize].len += 1;
        if terminator {
            self.current_block = None;
        }
        r
    }

    pub fn append_undef(&mut self, ty: TypeId) -> Ref {
        let id = FunctionId {
            module: ModuleId::BUILTINS,
            function: Builtin::Undef.id(),
        };
        self.append((id, (), ty))
    }

    pub fn create_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len() as _);
        self.blocks.push(BlockInfo {
            arg_count: 0,
            args_idx: 0,
            body_idx: 0,
            len: 0,
            preds: Vec::new(),
            succs: Vec::new(),
        });
        id
    }

    #[inline]
    pub fn begin_block(&mut self, id: BlockId, args: impl IntoIterator<Item = TypeId>) -> Refs {
        debug_assert!(
            self.current_block.is_none(),
            "there was already an active block"
        );
        let block = &mut self.blocks[id.0 as usize];
        debug_assert!(
            block.len == 0 && block.arg_count == 0 && block.body_idx == 0,
            "can't begin a block twice"
        );
        block.args_idx = self.insts.len() as _;
        self.insts
            .extend(args.into_iter().map(builtins::block_arg_inst));
        block.arg_count = self.insts.len() as u32 - block.args_idx;
        block.body_idx = if block.arg_count == 0 {
            self.insts.push(Instruction::NOTHING);
            block.args_idx + 1
        } else {
            block.args_idx + block.arg_count
        };
        self.current_block = Some(id);
        Refs {
            idx: block.args_idx,
            count: block.arg_count,
        }
    }

    pub fn create_and_begin_block(
        &mut self,
        args: impl IntoIterator<Item = TypeId>,
    ) -> (BlockId, Refs) {
        debug_assert!(
            self.current_block.is_none(),
            "there was already an active block"
        );
        let idx = self.insts.len() as u32;
        self.insts.extend(args.into_iter().map(|ty| Instruction {
            function: crate::FunctionId {
                module: crate::ModuleId::BUILTINS,
                function: Builtin::BlockArg.id(),
            },
            args: [0, 0],
            ty,
        }));
        let id = BlockId(self.blocks.len() as _);
        let arg_count = self.insts.len() as u32 - idx;
        let args = Refs {
            idx,
            count: arg_count,
        };
        let body_idx = if arg_count == 0 {
            // arg insert point
            self.insts.push(Instruction::NOTHING);
            idx + 1
        } else {
            idx + arg_count
        };
        self.blocks.push(BlockInfo {
            arg_count,
            args_idx: idx,
            body_idx,
            len: 0,
            preds: Vec::new(),
            succs: Vec::new(),
        });
        self.current_block = Some(id);
        (id, args)
    }

    pub fn finish(self, name: impl Into<Box<str>>, return_type: TypeId) -> Function {
        let ir = crate::FunctionIr {
            blocks: self.blocks,
            insts: self.insts,
            extra: self.extra,
            mc_regs: Vec::new(),
        };
        let entry = &ir.blocks[BlockId::ENTRY.idx()];
        let params = (entry.args_idx..entry.args_idx + entry.arg_count)
            .map(|i| {
                let ty = ir.insts[i as usize].ty;
                Parameter::RefOf(ty)
            })
            .collect();
        Function {
            name: name.into(),
            types: self.types,
            params,
            varargs: None,
            flags: InstFlags::default(),
            return_type: Some(return_type),
            ir: OnceCell::from(ir),
        }
    }

    pub fn get_ref_ty(&self, arg: Ref) -> TypeId {
        self.insts[arg.idx()].ty
    }

    pub fn finish_body(self) -> (crate::FunctionIr, Types) {
        let ir = crate::FunctionIr {
            blocks: self.blocks,
            insts: self.insts,
            extra: self.extra,
            mc_regs: Vec::new(),
        };
        (ir, self.types)
    }
}

#[track_caller]
fn encode_arg(
    extra: &mut Vec<u32>,
    block: BlockId,
    blocks: &mut [BlockInfo],
    arg: Argument<'_>,
    param: Parameter,
) -> impl Iterator<Item = u32> + use<> {
    let (a, b) = match (arg, param) {
        (Argument::Ref(r), crate::Parameter::Ref | crate::Parameter::RefOf(_)) => (r.0, None),
        (Argument::BlockId(target), crate::Parameter::BlockId) => {
            blocks[target.idx()].preds.push(block);
            blocks[block.idx()].succs.push(target);
            (target.0, None)
        }
        (Argument::BlockId(target), crate::Parameter::BlockTarget) => {
            blocks[target.idx()].preds.push(block);
            blocks[block.idx()].succs.push(target);
            (target.0, Some(0))
        }
        (Argument::BlockTarget(target), crate::Parameter::BlockTarget) => {
            blocks[target.0.idx()].preds.push(block);
            blocks[block.idx()].succs.push(target.0);
            let idx = extra.len() as u32;
            // TODO: check block has the correct number of arguments
            // (currently can't because it is set to 0 before start)
            extra.extend(target.1.iter().map(|&r| r.0));
            (target.0.0, Some(idx))
        }
        (Argument::Int(i), crate::Parameter::Int) => (i as u32, Some((i >> 32) as u32)),
        (Argument::Int(i), crate::Parameter::Int32) => {
            (i.try_into().expect("Int value too large for Int32"), None)
        }
        (Argument::Float(n), crate::Parameter::Float) => {
            let i = n.to_bits();
            (i as u32, Some((i >> 32) as u32))
        }
        (Argument::TypeId(id), crate::Parameter::TypeId) => (id.0, None),
        (Argument::FunctionId(id), crate::Parameter::FunctionId) => {
            (id.module.0, Some(id.function.0))
        }
        (Argument::GlobalId(id), crate::Parameter::GlobalId) => (id.module.0, Some(id.idx)),
        (Argument::MCReg(r), crate::Parameter::MCReg(_)) => (r.0, None),
        _ => panic!("argument was of unexpected kind, expected {param:?}"),
    };
    std::iter::once(a).chain(b)
}

#[track_caller]
pub(crate) fn write_args<'a>(
    extra: &mut Vec<u32>,
    block: BlockId,
    blocks: &mut [BlockInfo],
    params: &[Parameter],
    varargs: Option<Parameter>,
    args: impl IntoArgs<'a>,
) -> [u32; INLINE_ARGS] {
    let mut args = args.into_args();
    if (varargs.is_none() && args.len() != params.len())
        || (varargs.is_some() && args.len() < params.len())
    {
        panic!(
            "invalid parameter count, expected {} but found {}",
            params.len(),
            args.len()
        );
    }
    let mut arg_slots = (&mut args)
        .take(params.len())
        .zip(params)
        .flat_map(|(arg, param)| encode_arg(extra, block, blocks, arg, *param));
    let count: usize = params.iter().map(|p| p.slot_count()).sum();
    if count <= INLINE_ARGS && varargs.is_none() {
        [
            arg_slots.next().unwrap_or_default(),
            arg_slots.next().unwrap_or_default(),
        ]
    } else {
        // PERF: extra collect could be avoided by encoding step by step after reserving slots
        let arg_slots = arg_slots.collect::<Vec<_>>();
        let idx = extra.len() as u32;
        extra.extend(arg_slots);
        let mut i = extra.len();
        /*
        for val in arg_slots {
            extra[i] = val;
            i += 1;
        }
        */
        if let Some(vararg_param) = varargs {
            let vararg_count = args.len();
            extra.extend(std::iter::repeat_n(0, vararg_count));
            for arg in args {
                for val in encode_arg(extra, block, blocks, arg, vararg_param) {
                    extra[i] = val;
                    i += 1;
                }
            }
            [idx, vararg_count.try_into().unwrap()]
        } else {
            [idx, 0]
        }
    }
}
