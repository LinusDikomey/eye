pub mod const_value;
mod entry_point;
mod intrinsics;
pub mod types;

pub use entry_point::entry_point;

use std::borrow::Cow;
use std::num::NonZero;

use ir::{BlockId, BlockTarget, Environment, Ref};
use parser::ast::{self, ModuleId};

use crate::check::traits::{self, Candidates};
use crate::compiler::{Dialects, FunctionToGenerate, Generics, Instance, Instances, builtins};
use crate::hir::{CastType, LValue, LValueId, Node, Pattern, PatternId, Var};
use crate::types::{BaseType, TypeFull};
use crate::typing::{LocalTypeId, LocalTypeIds, OrdinalType};
use crate::{Compiler, Type};
use crate::{
    compiler::CheckedFunction,
    hir::{Hir, NodeId},
};

type Builder<'a> = ir::builder::Builder<&'a mut Environment>;

pub fn declare_function(
    compiler: &Compiler,
    ir: &ir::Environment,
    checked: &CheckedFunction,
    module: ModuleId,
    generics: &[Type],
) -> ir::Function {
    let name = compiler.mangle_name(checked, module, generics);
    let mut types = ir::Types::new();
    // TODO: figure out what to do when params/return_type are Invalid or never types. We can no
    // longer generate a valid signature
    let params = types::get_multiple(
        compiler,
        ir,
        &mut types,
        checked[checked.params].iter().copied(),
        Instance {
            types: generics,
            outer: None,
        },
    )
    .unwrap_or_else(|| types.add_multiple((0..checked.params.count).map(|_| ir::Type::UNIT)));

    let return_type = types::get(
        compiler,
        ir,
        &mut types,
        checked[checked.return_type],
        Instance {
            types: generics,
            outer: None,
        },
    )
    .unwrap_or(ir::Type::UNIT);
    let return_type = types.add(return_type);

    ir::Function::declare(name, types, params.iter(), checked.varargs, return_type)
}

type Result<T> = std::result::Result<T, NoReturn>;
struct NoReturn;

macro_rules! crash_point {
    ($ctx: expr) => {{
        tracing::warn!(target: "irgen", file = file!(), line = line!(), "Building crash point");
        build_crash_point($ctx);
        return Err(NoReturn);
    }};
}

pub fn lower_hir(
    compiler: &Compiler,
    dialects: &Dialects,
    instances: &mut Instances,
    mut builder: Builder,
    hir: &Hir,
    to_generate: &mut Vec<FunctionToGenerate>,
    generics: &[Type],
    params: ir::Refs,
    return_ty: ir::TypeId,
) -> (ir::FunctionIr, ir::Types) {
    let unit_ty = builder.types.add(ir::Type::UNIT);
    let ptr_ty = builder.types.add(ir::Primitive::Ptr);
    let i1_ty = builder.types.add(ir::Primitive::I1);

    let vars = hir
        .vars
        .iter()
        .map(|&var| {
            let Some(ir_ty) = types::get(
                compiler,
                builder.env,
                &mut builder.types,
                hir[var.ty()],
                Instance {
                    types: generics,
                    outer: None,
                },
            ) else {
                build_crash_point_inner(
                    compiler,
                    &mut builder,
                    dialects,
                    instances,
                    to_generate,
                    ptr_ty,
                    return_ty,
                );
                return Err(NoReturn);
            };
            let ty;
            let var_ref;
            match var {
                Var::Local(_) => {
                    ty = builder.types.add(ir_ty);
                    var_ref = builder.append(dialects.mem.Decl(ty, ptr_ty));
                }
                Var::Capture {
                    outer: _,
                    ty: _,
                    capture_idx,
                } => {
                    ty = builder.types.add(ir_ty);
                    var_ref = builder.append(dialects.mem.Decl(ty, ptr_ty));
                    // move capture
                    let captures_param = params.nth(0);
                    let value =
                        builder.append(dialects.tuple.MemberValue(captures_param, capture_idx, ty));
                    builder.append(dialects.mem.Store(var_ref, value));
                }
                // this variable should never be used in the body
                Var::CapturesParam(_) => return Ok((Ref::UNIT, unit_ty)),
            };
            Ok((var_ref, ty))
        })
        .collect();
    let mut vars: Vec<_> = match vars {
        Ok(vars) => vars,
        Err(NoReturn) => return builder.finish_body(),
    };
    debug_assert_eq!(params.count(), hir.params.len() as _);
    for (param, &var) in params.iter().zip(&hir.params) {
        if matches!(hir.vars[var.idx()], Var::CapturesParam(_)) {
            continue;
        }
        builder.append(dialects.mem.Store(vars[var.idx()].0, param));
    }
    let mut ctx = Ctx {
        compiler,
        dialects,
        instances,
        to_generate,
        hir,
        generics,
        builder,
        vars: &mut vars,
        control_flow_stack: Vec::new(),
        return_ty,
        ptr_ty,
        i1_ty,
    };

    let val = lower(&mut ctx, hir.root_id());
    if let Ok(val) = val {
        ctx.builder.append(ctx.dialects.cf.Ret(val));
    }
    ctx.builder.finish_body()
}

struct Ctx<'a> {
    compiler: &'a Compiler,
    dialects: &'a Dialects,
    instances: &'a mut Instances,
    to_generate: &'a mut Vec<FunctionToGenerate>,
    hir: &'a Hir,
    generics: &'a [Type],
    builder: Builder<'a>,
    vars: &'a [(Ref, ir::TypeId)],
    control_flow_stack: Vec<ControlFlowEntry>,
    return_ty: ir::TypeId,
    ptr_ty: ir::TypeId,
    i1_ty: ir::TypeId,
}
impl Ctx<'_> {
    fn get_ir_id(
        &mut self,
        module: ModuleId,
        id: ast::FunctionId,
        generics: Box<[Type]>,
    ) -> Option<ir::FunctionId> {
        Self::get_ir_id_inner(
            self.compiler,
            self.builder.env,
            self.dialects,
            self.instances,
            self.to_generate,
            module,
            id,
            generics,
        )
    }

    fn get_ir_id_inner(
        compiler: &Compiler,
        env: &mut ir::Environment,
        dialects: &Dialects,
        instances: &mut Instances,
        to_generate: &mut Vec<FunctionToGenerate>,
        module: ModuleId,
        id: ast::FunctionId,
        generics: Box<[Type]>,
    ) -> Option<ir::FunctionId> {
        // TODO: this check is probably not sufficient for error handling
        // check that none of the types is invalid, we never wan't to generate an instance for an
        // invalid type. The caller should build a crash point in that case.
        for &ty in &generics {
            if ty == Type::Invalid {
                return None;
            }
        }
        Some(
            instances.get_or_create_function(module, id, generics, |generics| {
                let checked = compiler.get_hir(module, id);
                let func = declare_function(compiler, env, checked, module, generics);
                let ir_id = env.add_function(dialects.main, func);
                to_generate.push(FunctionToGenerate {
                    ir_id,
                    module,
                    ast_function_id: id,
                    generics: generics.into(),
                });
                ir_id
            }),
        )
    }

    fn get_ir_global(&mut self, module: ModuleId, id: ast::GlobalId) -> Option<ir::GlobalId> {
        self.instances.get_or_create_global(module, id, || {
            let parsed = self.compiler.get_parsed_module(module);
            let name = String::from(&*parsed.ast[id].name);
            let (value, ty) = self.compiler.get_checked_global(module, id);
            let layout = self.compiler.resolved_layout(*ty, Instance::EMPTY).ok()?;
            let mut storage = vec![0; layout.size as usize].into_boxed_slice();
            if const_value::translate(value, *ty, self.compiler, &mut storage).is_err() {
                // FIXME: this tries to translate invalid globals again every time they are
                // requested since we never store to the instances. Maybe create the notion of
                // invalid globals in ir? Or get layout of the value and return zeroes.
                return None;
            };
            let global_id = self.builder.env.add_global(
                self.dialects.main,
                name,
                NonZero::new(layout.alignment).expect("alignment should not be zero"),
                storage,
                false,
            );
            Some(global_id)
        })
    }

    fn get_type(&mut self, ty: Type) -> Result<ir::Type> {
        types::get(
            self.compiler,
            self.builder.env,
            &mut self.builder.types,
            ty,
            Instance {
                types: self.generics,
                outer: None,
            },
        )
        .ok_or_else(|| {
            build_crash_point(self);
            NoReturn
        })
    }

    fn get_hir_type(&mut self, ty: LocalTypeId) -> Result<ir::Type> {
        self.get_type(self.hir[ty])
    }

    fn get_multiple_types(&mut self, ids: LocalTypeIds) -> Result<ir::TypeIds> {
        types::get_multiple(
            self.compiler,
            self.builder.env,
            &mut self.builder.types,
            self.hir[ids].iter().copied(),
            Instance {
                types: self.generics,
                outer: None,
            },
        )
        .ok_or_else(|| {
            build_crash_point(self);
            NoReturn
        })
    }

    fn terminate_noreturn(&mut self) -> Result<std::convert::Infallible> {
        let value = self.builder.append_undef(self.return_ty);
        self.builder.append(self.dialects.cf.Ret(value));
        Err(NoReturn)
    }

    fn get_instantiated_ty(&mut self, ty: LocalTypeId) -> Type {
        self.compiler.types.instantiate(self.hir[ty], self.generics)
    }
}

#[derive(Debug, Clone, Copy)]
struct ControlFlowEntry {
    loop_begin: BlockId,
    loop_after: BlockId,
}

enum ValueOrPlace {
    Value(Ref),
    Place { ptr: Ref, value_ty: ir::TypeId },
}

fn lower(ctx: &mut Ctx, node: NodeId) -> Result<Ref> {
    Ok(match lower_expr(ctx, node)? {
        ValueOrPlace::Value(r) => r,
        ValueOrPlace::Place { ptr, value_ty } => {
            ctx.builder.append(ctx.dialects.mem.Load(ptr, value_ty))
        }
    })
}

fn lower_expr(ctx: &mut Ctx, node: NodeId) -> Result<ValueOrPlace> {
    let Dialects {
        arith,
        tuple,
        mem,
        cf,
        main,
    } = ctx.dialects;
    let value = match &ctx.hir[node] {
        Node::Invalid => crash_point!(ctx),
        Node::Block(items) => {
            for item in items.iter() {
                lower(ctx, item)?;
            }
            Ref::UNIT
        }
        Node::Unit => Ref::UNIT,
        &Node::IntLiteral { val, ty } => {
            let ty = ctx.hir[ty];
            if !ty.is_int() {
                crash_point!(ctx)
            }
            let ty = ctx.get_type(ty)?;
            let ty = ctx.builder.types.add(ty);
            if let Ok(small) = val.try_into() {
                ctx.builder.append(arith.Int(small, ty))
            } else {
                todo!("large ints")
                //ctx.builder.build_large_int(val, ty)
            }
        }
        &Node::FloatLiteral { val, ty } => {
            let ty = ctx.hir[ty];
            if !ty.is_float() {
                crash_point!(ctx)
            }
            let ty = ctx.get_type(ty)?;
            let ty = ctx.builder.types.add(ty);
            ctx.builder.append(arith.Float(val, ty))
        }
        Node::BoolLiteral(true) => Ref::TRUE,
        Node::BoolLiteral(false) => Ref::FALSE,
        &Node::ArrayLiteral { elems, array_ty } => {
            let TypeFull::Instance(BaseType::Array, &[element, _n]) =
                ctx.compiler.types.lookup(ctx.hir[array_ty])
            else {
                panic!("non-array literal type");
            };
            let elem_ir_ty = ctx.get_type(element)?;
            let elem_ir_ty = ctx.builder.types.add(elem_ir_ty);
            let array_ir_ty = ctx
                .builder
                .types
                .add(ir::Type::Array(elem_ir_ty, elems.count));
            let array_var = ctx.builder.append(mem.Decl(array_ir_ty, ctx.ptr_ty));
            for (elem, i) in elems.iter().zip(0..) {
                let val = lower(ctx, elem)?;
                let u64_ty = ctx.builder.types.add(ir::Primitive::U64);
                let index = ctx.builder.append(arith.Int(i, u64_ty));
                let member_ptr = ctx
                    .builder
                    .append(mem.ArrayIndex(array_var, elem_ir_ty, index, ctx.ptr_ty));
                ctx.builder.append(mem.Store(member_ptr, val));
            }
            return Ok(ValueOrPlace::Place {
                ptr: array_var,
                value_ty: array_ir_ty,
            });
        }
        &Node::TupleLiteral { elems, elem_types } => {
            debug_assert_eq!(elems.count, elem_types.count);
            if elems.count == 0 {
                return Ok(ValueOrPlace::Value(Ref::UNIT));
            }
            let elem_types = ctx.get_multiple_types(elem_types)?;
            let tuple_ty = ctx.builder.types.add(ir::Type::Tuple(elem_types));
            let mut tuple_val = ctx.builder.append_undef(tuple_ty);
            for (elem, i) in elems.iter().zip(0..) {
                let val = lower(ctx, elem)?;
                tuple_val = ctx
                    .builder
                    .append(tuple.InsertMember(tuple_val, i, val, tuple_ty));
            }
            tuple_val
        }
        &Node::EnumLiteral {
            elems,
            elem_types,
            enum_ty,
        } => {
            debug_assert_eq!(elems.count, elem_types.count);
            let ty = ctx.get_hir_type(enum_ty)?;
            let enum_ty = ctx.builder.types.add(ty);
            let elem_types = ctx.get_multiple_types(elem_types)?;
            let elem_tuple = ctx.builder.types.add(ir::Type::Tuple(elem_types));
            let var = ctx.builder.append(mem.Decl(enum_ty, ctx.ptr_ty));
            for (elem, i) in elems.iter().zip(0..) {
                let elem_ptr = ctx
                    .builder
                    .append(mem.MemberPtr(var, elem_tuple, i, ctx.ptr_ty));
                let val = lower(ctx, elem)?;
                ctx.builder.append(mem.Store(elem_ptr, val));
            }
            // maybe do this differently, could do it like llvm: insertvalue
            ctx.builder.append(mem.Load(var, enum_ty))
        }
        Node::StringLiteral(str) => {
            let (s, _value_ty) = lower_string_literal(ctx, str);
            s
        }
        &Node::InferredEnumOrdinal(variant) => {
            let variant = &ctx.hir[variant];
            let ty = ctx.hir[variant.args.iter().next().unwrap()];
            match ty {
                Type::Unit => Ref::UNIT,
                _ => {
                    let ty = ctx.get_type(ty)?;
                    let ty = ctx.builder.types.add(ty);
                    ctx.builder.append(arith.Int(variant.ordinal, ty))
                }
            }
        }

        Node::Declare { pattern: _ } => todo!("lower declarations without values"),
        &Node::DeclareWithVal { pattern, val } => {
            let val = lower(ctx, val)?;
            lower_pattern(ctx, pattern, val, None)?;
            Ref::UNIT
        }
        Node::Variable(id) => {
            return Ok(ValueOrPlace::Place {
                ptr: ctx.vars[id.idx()].0,
                value_ty: ctx.vars[id.idx()].1,
            });
        }
        &Node::Global(module, id, ty) => {
            let Some(id) = ctx.get_ir_global(module, id) else {
                crash_point!(ctx)
            };
            let global = ctx.builder.append(mem.Global(id, ctx.ptr_ty));
            let ty = ctx.get_hir_type(ty)?;
            let ty = ctx.builder.types.add(ty);
            return Ok(ValueOrPlace::Place {
                ptr: global,
                value_ty: ty,
            });
        }
        &Node::Assign(lval, val, assign_ty, ty) => {
            let lval = lower_lval(ctx, lval)?;
            let val = lower(ctx, val)?;
            use crate::hir::Arithmetic;
            use parser::ast::AssignType;

            let arithmetic = match assign_ty {
                AssignType::Assign => {
                    ctx.builder.append(mem.Store(lval, val));
                    return Ok(ValueOrPlace::Value(Ref::UNIT));
                }
                AssignType::AddAssign => Arithmetic::Add,
                AssignType::SubAssign => Arithmetic::Sub,
                AssignType::MulAssign => Arithmetic::Mul,
                AssignType::DivAssign => Arithmetic::Div,
                AssignType::ModAssign => Arithmetic::Mod,
            };

            let ty = ctx.get_hir_type(ty)?;
            let ir_ty = ctx.builder.types.add(ty);
            let loaded = ctx.builder.append(mem.Load(lval, ir_ty));
            let result = build_arithmetic(ctx, loaded, val, arithmetic, ir_ty);
            ctx.builder.append(mem.Store(lval, result));
            Ref::UNIT
        }

        &Node::Const { id, ty } => {
            // TODO: don't create a new global constant on each usage
            // PERF: cloning const value
            let (const_val, global_ty) = ctx.compiler.const_values[id.idx()].clone();
            debug_assert_eq!(ctx.hir[ty], global_ty, "Global used as wrong type");
            let ir_ty = ctx.get_hir_type(ty)?;
            let layout = ir::type_layout(ir_ty, &ctx.builder.types, ctx.builder.env.primitives());
            let mut storage = vec![0; layout.size as usize].into_boxed_slice();
            if const_value::translate(&const_val, global_ty, ctx.compiler, &mut storage).is_err() {
                crash_point!(ctx);
            }
            let global = ctx.builder.env.add_global(
                *main,
                format!("const_{}", id.0),
                layout.align,
                storage,
                true,
            );
            let ir_ty = ctx.builder.types.add(ir_ty);
            let global = ctx.builder.append(mem.Global(global, ctx.ptr_ty));
            ctx.builder.append(mem.Load(global, ir_ty))
        }

        &Node::Negate(value, ty) => {
            let value = lower(ctx, value)?;
            let ty = ctx.get_hir_type(ty)?;
            let ty = ctx.builder.types.add(ty);
            ctx.builder.append(arith.Neg(value, ty))
        }
        &Node::Not(value) => {
            let value = lower(ctx, value)?;
            ctx.builder.append(arith.Not(value, ctx.i1_ty))
        }
        &Node::AddressOf { value, value_ty: _ } => lower_lval(ctx, value)?,
        &Node::Deref { value, deref_ty } => {
            let value = lower(ctx, value)?;
            let ty = ctx.get_hir_type(deref_ty)?;
            let value_ty = ctx.builder.types.add(ty);
            //let ptr = ctx.builder.build_load(value, ty);
            return Ok(ValueOrPlace::Place {
                ptr: value,
                value_ty,
            });
        }
        &Node::Promote { value, variable } => {
            let val = lower(ctx, value)?;
            let var = ctx.vars[variable.idx()].0;
            ctx.builder.append(mem.Store(var, val));
            var
        }

        &Node::Cast(id) => {
            let cast = &ctx.hir[id];
            let val = lower(ctx, cast.val)?;
            // TODO: separate into multiple more specific cast instructions in ir
            match &cast.cast_ty {
                CastType::Invalid => crash_point!(ctx),
                CastType::Noop => val,
                &CastType::Int { from: _, to } => {
                    let to_ty = ctx.builder.types.add(types::get_primitive(to.into()));
                    ctx.builder.append(arith.CastInt(val, to_ty))
                }
                &CastType::Float { from: _, to } => {
                    let to_ty = ctx.builder.types.add(types::get_primitive(to.into()));
                    ctx.builder.append(arith.CastFloat(val, to_ty))
                }
                &CastType::FloatToInt { from: _, to } => {
                    let to_ty = ctx.builder.types.add(types::get_primitive(to.into()));
                    ctx.builder.append(arith.CastFloatToInt(val, to_ty))
                }
                &CastType::IntToFloat { from: _, to } => {
                    let to_ty = ctx.builder.types.add(types::get_primitive(to.into()));
                    ctx.builder.append(arith.CastIntToFloat(val, to_ty))
                }
                CastType::IntToPtr { from: _ } => ctx.builder.append(mem.IntToPtr(val, ctx.ptr_ty)),
                &CastType::PtrToInt { to } => {
                    let ty = ctx.builder.types.add(types::get_primitive(to.into()));
                    ctx.builder.append(mem.PtrToInt(val, ty))
                }
                CastType::EnumToInt { .. } => todo!("cast enums to integers, might be removed"),
            }
        }
        &Node::Comparison {
            l,
            r,
            cmp,
            compared: _,
        } => {
            let l = lower(ctx, l)?;
            let r = lower(ctx, r)?;
            use crate::hir::Comparison;
            match cmp {
                Comparison::LT => ctx.builder.append(arith.LT(l, r, ctx.i1_ty)),
                Comparison::GT => ctx.builder.append(arith.GT(l, r, ctx.i1_ty)),
                Comparison::LE => ctx.builder.append(arith.LE(l, r, ctx.i1_ty)),
                Comparison::GE => ctx.builder.append(arith.GE(l, r, ctx.i1_ty)),
            }
        }
        &Node::Logic { l, r, logic } => {
            let l = lower(ctx, l)?;
            let rhs = ctx.builder.create_block();
            let res = ctx.builder.create_block();
            match logic {
                crate::hir::Logic::And => ctx.builder.append(cf.Branch(
                    l,
                    BlockTarget::new(rhs),
                    BlockTarget(res, Cow::Borrowed(&[Ref::FALSE])),
                )),
                crate::hir::Logic::Or => ctx.builder.append(cf.Branch(
                    l,
                    BlockTarget(res, Cow::Borrowed(&[Ref::TRUE])),
                    BlockTarget::new(rhs),
                )),
            };
            ctx.builder.begin_block(rhs, []);
            if let Ok(r) = lower(ctx, r) {
                ctx.builder
                    .append(cf.Goto(BlockTarget(res, Cow::Borrowed(&[r]))));
            }
            let args = ctx.builder.begin_block(res, [ctx.i1_ty]);
            args.nth(0)
        }
        &Node::Arithmetic(l, r, op, ty) => {
            let l = lower(ctx, l)?;
            let r = lower(ctx, r)?;
            let ty = ctx.get_hir_type(ty).map(|ty| ctx.builder.types.add(ty))?;
            build_arithmetic(ctx, l, r, op, ty)
        }

        &Node::Element {
            tuple_value,
            index,
            elem_types,
        } => {
            let value = lower_expr(ctx, tuple_value)?;
            let value = match value {
                ValueOrPlace::Value(val) => {
                    let elem_ty = ctx.get_hir_type(elem_types.nth(index).unwrap())?;
                    let elem_ty = ctx.builder.types.add(elem_ty);
                    let value = ctx.builder.append(tuple.MemberValue(val, index, elem_ty));
                    ValueOrPlace::Value(value)
                }
                ValueOrPlace::Place { ptr, value_ty: _ } => {
                    let elem_types = ctx.get_multiple_types(elem_types)?;
                    let elem_tuple = ctx.builder.types.add(ir::Type::Tuple(elem_types));
                    let member_ptr = ctx
                        .builder
                        .append(mem.MemberPtr(ptr, elem_tuple, index, ctx.ptr_ty));
                    ValueOrPlace::Place {
                        ptr: member_ptr,
                        value_ty: elem_types.nth(index),
                    }
                }
            };
            return Ok(value);
        }
        &Node::ArrayIndex {
            array,
            index,
            elem_ty,
        } => {
            let array = match lower_expr(ctx, array)? {
                ValueOrPlace::Value(_) => todo!("array indexing without place"),
                ValueOrPlace::Place { ptr, .. } => ptr,
            };
            let index = lower(ctx, index)?;
            let elem_ir_ty = ctx.get_hir_type(elem_ty)?;
            let elem_ir_ty = ctx.builder.types.add(elem_ir_ty);
            return Ok(ValueOrPlace::Place {
                ptr: ctx
                    .builder
                    .append(mem.ArrayIndex(array, elem_ir_ty, index, ctx.ptr_ty)),
                value_ty: elem_ir_ty,
            });
        }

        &Node::Return(val) => {
            let val = lower(ctx, val)?;
            ctx.builder.append(cf.Ret(val));
            return Err(NoReturn);
        }
        &Node::IfElse {
            cond,
            then,
            else_,
            resulting_ty,
        } => {
            let cond = lower(ctx, cond)?;
            let then_block = ctx.builder.create_block();
            let else_block = ctx.builder.create_block();
            ctx.builder.append(cf.Branch(
                cond,
                BlockTarget::new(then_block),
                BlockTarget::new(else_block),
            ));
            ctx.builder.begin_block(then_block, []);
            lower_if_else_branches(ctx, then, else_, else_block, resulting_ty)?
        }
        &Node::IfPatElse {
            pat,
            val,
            then,
            else_,
            resulting_ty,
        } => {
            let val = lower(ctx, val)?;
            let else_block = ctx.builder.create_block();
            lower_pattern(ctx, pat, val, Some(else_block))?;
            lower_if_else_branches(ctx, then, else_, else_block, resulting_ty)?
        }
        &Node::Match {
            value,
            branch_index,
            pattern_index,
            branch_count,
            resulting_ty,
        } => {
            let value = lower(ctx, value)?;
            let mut after_block = None;
            let mut next_block;
            let mut result_value = None;
            if branch_count == 0 {
                // matching on an uninhabited type so we need to terminate the block
                let undef = ctx.builder.append_undef(ctx.return_ty);
                ctx.builder.append(cf.Ret(undef));
                return Err(NoReturn);
            }
            for i in 0..branch_count {
                let is_last = i + 1 == branch_count;
                let pattern = PatternId(pattern_index + i);
                let branch = NodeId(branch_index + i);
                next_block = (!is_last).then(|| ctx.builder.create_block());
                let pattern_noreturn = lower_pattern(ctx, pattern, value, next_block).is_err();
                if pattern_noreturn {
                    // if any other than the first pattern is noreturn, the match itself might
                    // still return because a pattern before might have matched
                    if i == 0 {
                        return Err(NoReturn);
                    }
                    break;
                }
                let val = lower(ctx, branch);
                if let Ok(val) = val {
                    let after = *after_block.get_or_insert_with(|| ctx.builder.create_block());
                    ctx.builder
                        .append(cf.Goto(BlockTarget(after, Cow::Borrowed(&[val]))));
                }
                if let Some(next) = next_block {
                    ctx.builder.begin_block(next, []);
                } else {
                    // after could still be none if all branches are noreturn, we don't have to
                    // create an after block at all in this case
                    if let Some(after) = after_block {
                        let ty = ctx.get_hir_type(resulting_ty)?;
                        let ty = ctx.builder.types.add(ty);
                        let args = ctx.builder.begin_block(after, [ty]);
                        result_value = Some(args.nth(0));
                    }
                }
            }
            result_value.ok_or(NoReturn)?
        }
        &Node::While { cond, body } => {
            let cond_block = ctx.builder.create_block();
            ctx.builder.append(cf.Goto(BlockTarget::new(cond_block)));
            ctx.builder.begin_block(cond_block, []);
            let cond = lower(ctx, cond)?;
            let body_block = ctx.builder.create_block();
            let after_block = ctx.builder.create_block();
            ctx.control_flow_stack.push(ControlFlowEntry {
                loop_begin: cond_block,
                loop_after: after_block,
            });
            ctx.builder.append(cf.Branch(
                cond,
                BlockTarget::new(body_block),
                BlockTarget::new(after_block),
            ));
            ctx.builder.begin_block(body_block, []);
            if lower(ctx, body).is_ok() {
                ctx.builder.append(cf.Goto(BlockTarget::new(cond_block)));
            }
            ctx.builder.begin_block(after_block, []);
            Ref::UNIT
        }
        &Node::WhilePat { pat, val, body } => {
            let loop_start = ctx.builder.create_block();
            let after = ctx.builder.create_block();
            ctx.builder.append(cf.Goto(BlockTarget::new(loop_start)));
            ctx.builder.begin_block(loop_start, []);
            let val = lower(ctx, val)?;
            lower_pattern(ctx, pat, val, Some(after))?;
            ctx.control_flow_stack.push(ControlFlowEntry {
                loop_begin: loop_start,
                loop_after: after,
            });
            let body_value = lower(ctx, body);
            ctx.control_flow_stack.pop();
            if body_value.is_ok() {
                ctx.builder.append(cf.Goto(BlockTarget::new(loop_start)));
            }
            ctx.builder.begin_block(after, []);
            Ref::UNIT
        }
        &Node::Call {
            function,
            args,
            arg_types,
            return_ty,
            noreturn,
        } => {
            let return_ty = ctx.get_hir_type(return_ty)?;
            let res = if let Node::FunctionItem {
                function: (module, id),
                generics: call_generics,
                ty: _,
            } = ctx.hir[function]
            {
                if (module, id) == builtins::get_intrinsic(ctx.compiler) {
                    let arg_refs = args
                        .iter()
                        .skip(1)
                        .map(|arg| lower(ctx, arg))
                        .collect::<Result<Vec<_>>>()?;
                    let Node::StringLiteral(intrinsic) = &ctx.hir[args.iter().next().unwrap()]
                    else {
                        panic!("expected string literal passed to intrinsic call");
                    };
                    return intrinsics::call_intrinsic(ctx, intrinsic, &arg_refs);
                }
                // PERF: make it possible to write refs directly into the ir to avoid collecting here
                let arg_refs = args
                    .iter()
                    .map(|arg| lower(ctx, arg))
                    .collect::<Result<Vec<_>>>()?;
                let call_generics = call_generics
                    .iter()
                    .map(|generic| {
                        ctx.compiler
                            .types
                            .instantiate(ctx.hir[generic], ctx.generics)
                    })
                    .collect();
                let Some(func) = ctx.get_ir_id(module, id, call_generics) else {
                    crash_point!(ctx)
                };
                let return_ty = ctx.builder.types.add(return_ty);
                ctx.builder.append((func, arg_refs, return_ty))
            } else {
                debug_assert_eq!(args.count, arg_types.count);
                let func = lower(ctx, function)?;
                let call_ptr = ir::FunctionId {
                    module: mem.id(),
                    function: ir::dialect::Mem::CallPtr.id(),
                };
                // PERF: reuse allocation in the future?
                let mut call_ptr_args = Vec::with_capacity(args.count as usize + 1);
                call_ptr_args.push(func);
                for arg in args.iter() {
                    call_ptr_args.push(lower(ctx, arg)?);
                }
                let return_ty = ctx.builder.types.add(return_ty);
                ctx.builder.append((call_ptr, call_ptr_args, return_ty))
            };
            if noreturn {
                let ret = ctx.builder.append_undef(ctx.return_ty);
                ctx.builder.append(cf.Ret(ret));
                return Err(NoReturn);
            }
            res
        }
        &Node::TraitCall {
            trait_id,
            trait_generics,
            method_index,
            self_ty,
            args,
            return_ty,
            noreturn,
        } => {
            let self_ty = ctx.get_instantiated_ty(self_ty);
            let trait_instance: Box<[Type]> = trait_generics
                .iter()
                .map(|ty| ctx.compiler.types.instantiate(ctx.hir[ty], ctx.generics))
                .collect();

            // HACK(fn-trait): special-cased calling convention only for function trait
            let fn_trait_call = |skip_self: bool, ctx: &mut Ctx, func| -> Result<ValueOrPlace> {
                debug_assert_eq!(
                    trait_instance.len(),
                    2,
                    "Fn trait should have 2 generic parameters"
                );
                let args_ty = trait_instance[0];
                let TypeFull::Instance(BaseType::Tuple, arg_types) =
                    ctx.compiler.types.lookup(args_ty)
                else {
                    panic!(
                        "Fn trait should be called with tuples. TODO: manual impls can't \
                                enforce this requirement right now"
                    );
                };
                let mut arg_refs = Vec::with_capacity((!skip_self) as usize + arg_types.len());
                debug_assert_eq!(args.count, 2, "Fn trait call should have 2 arguments");
                if !skip_self {
                    arg_refs.push(lower(ctx, args.nth(0).unwrap())?);
                }
                let args_arg = lower(ctx, args.nth(1).unwrap())?;
                for (&ty, i) in arg_types.iter().zip(0..) {
                    let ty = ctx.get_type(ty)?;
                    let ty = ctx.builder.types.add(ty);
                    let arg = ctx.builder.append(tuple.MemberValue(args_arg, i, ty));
                    arg_refs.push(arg);
                }
                let return_ty = ctx.get_hir_type(return_ty)?;
                let return_ty = ctx.builder.types.add(return_ty);
                let res = ctx.builder.append((func, arg_refs, return_ty));
                if noreturn {
                    ctx.terminate_noreturn()?;
                }
                Ok(ValueOrPlace::Value(res))
            };

            if let TypeFull::FunctionItem { function, generics } =
                ctx.compiler.types.lookup(self_ty)
                && trait_id == builtins::get_fn_trait(ctx.compiler)
            {
                let Some(func) = ctx.get_ir_id(function.0, function.1, generics.into()) else {
                    crash_point!(ctx)
                };

                return fn_trait_call(true, ctx, func);
            }

            let TypeFull::Instance(self_base, _) = ctx.compiler.types.lookup(self_ty) else {
                unreachable!("instantiated TraitCall self type should be an instance")
            };
            match ctx.compiler.get_impl_candidates(
                trait_id,
                self_ty,
                Some(self_base),
                trait_instance.iter().copied(),
                traits::match_instance,
            ) {
                Candidates::Unique(selected) => {
                    let function = selected.functions[method_index as usize];

                    // TODO: handle impl/method generics
                    let Some(func) =
                        ctx.get_ir_id(selected.module, function, selected.impl_generics)
                    else {
                        crash_point!(ctx);
                    };

                    if trait_id == builtins::get_fn_trait(ctx.compiler) {
                        return fn_trait_call(false, ctx, func);
                    } else {
                        let mut arg_refs = Vec::with_capacity(args.count as usize);
                        for arg in args.iter() {
                            let arg = lower(ctx, arg)?;
                            arg_refs.push(arg);
                        }
                        let return_ty = ctx.get_hir_type(return_ty)?;
                        let return_ty = ctx.builder.types.add(return_ty);
                        let res = ctx.builder.append((func, arg_refs, return_ty));
                        if noreturn {
                            ctx.terminate_noreturn()?;
                        }
                        res
                    }
                }
                candidates => {
                    // FIXME: this trace could crash since Generics::EMPTY is passed which is not always correct
                    tracing::debug!(
                        target: "irgen",
                        "Failed to select a trait instance for {trait_id:?} with {}. Candidates: {candidates:?}",
                        ctx.compiler.types.display(self_ty, &Generics::EMPTY),
                    );
                    crash_point!(ctx);
                }
            }
        }
        &Node::TypeProperty(ty, property) => {
            let layout = ir::type_layout(
                ctx.get_hir_type(ty)?,
                &ctx.builder.types,
                ctx.builder.env.primitives(),
            );
            use crate::hir::TypeProperty;
            let value = match property {
                TypeProperty::Size => layout.size,
                TypeProperty::Align => layout.align.get(),
                TypeProperty::Stride => layout.stride(),
            };
            let ty = ctx.builder.types.add(ir::Primitive::I64);
            ctx.builder.append(arith.Int(value, ty))
        }
        &Node::FunctionItem {
            function,
            generics,
            ty,
        } => {
            match ctx.compiler.types.lookup(ctx.hir[ty]) {
                TypeFull::Instance(BaseType::Invalid, _) => crash_point!(ctx),
                TypeFull::Instance(BaseType::Function, _) => {
                    // type was coerced to function pointer, generate a pointer to the function
                    let generics = generics
                        .iter()
                        .map(|generic| {
                            ctx.compiler
                                .types
                                .instantiate(ctx.hir[generic], ctx.generics)
                        })
                        .collect();
                    let Some(id) = ctx.get_ir_id(function.0, function.1, generics) else {
                        crash_point!(ctx)
                    };
                    ctx.builder.append(mem.FunctionPtr(id, ctx.ptr_ty))
                } // function items have unit value
                TypeFull::FunctionItem { .. } => Ref::UNIT,
                _ => unreachable!(),
            }
        }
        &Node::Capture(i) => {
            let (captures, captures_ty) = ctx.vars[ctx.hir.params[0].idx()];
            let ir::Type::Tuple(capture_types) = ctx.builder.types[captures_ty] else {
                unreachable!()
            };

            return Ok(ValueOrPlace::Place {
                ptr: ctx
                    .builder
                    .append(mem.MemberPtr(captures, captures_ty, i.0, ctx.ptr_ty)),
                value_ty: capture_types.nth(i.0),
            });
        }
        &Node::Break(n) | &Node::Continue(n) => {
            let entry = ctx.control_flow_stack
                [ctx.control_flow_stack.len() - usize::try_from(n).unwrap() - 1];
            let target = if matches!(ctx.hir[node], Node::Break(_)) {
                entry.loop_after
            } else {
                entry.loop_begin
            };
            ctx.builder.append(cf.Goto(BlockTarget::new(target)));
            return Err(NoReturn);
        }
    };
    Ok(ValueOrPlace::Value(value))
}

fn lower_lval(ctx: &mut Ctx, lval: LValueId) -> Result<Ref> {
    let Dialects { mem, .. } = ctx.dialects;
    Ok(match ctx.hir[lval] {
        LValue::Invalid => crash_point!(ctx),
        LValue::Variable(id) => ctx.vars[id.idx()].0,
        LValue::Global(module, id) => {
            let Some(id) = ctx.get_ir_global(module, id) else {
                crash_point!(ctx)
            };
            ctx.builder.append(mem.Global(id, ctx.ptr_ty))
        }
        LValue::Deref(pointer) => lower(ctx, pointer)?,
        LValue::Member {
            tuple,
            index,
            elem_types,
        } => {
            let ptr = lower_lval(ctx, tuple)?;
            let types = ctx.get_multiple_types(elem_types)?;
            let ty = ctx.builder.types.add(ir::Type::Tuple(types));
            ctx.builder
                .append(mem.MemberPtr(ptr, ty, index, ctx.ptr_ty))
        }
        LValue::ArrayIndex {
            array,
            index,
            elem_ty: element_type,
        } => {
            let ptr = lower_lval(ctx, array)?;
            let idx = lower(ctx, index)?;
            let ty = ctx.get_hir_type(element_type)?;
            let ty = ctx.builder.types.add(ty);
            ctx.builder.append(mem.ArrayIndex(ptr, ty, idx, ctx.ptr_ty))
        }
    })
}

/// Lower a pattern, jumping to the on_mismatch block if the pattern doesn't match.
/// When on_mismatch is set to BlockIndex::MISSING, all patterns are assumed to always match
fn lower_pattern(
    ctx: &mut Ctx,
    pattern: PatternId,
    value: Ref,
    on_mismatch: Option<BlockId>,
) -> Result<()> {
    let Dialects {
        arith,
        tuple,
        mem,
        cf,
        ..
    } = ctx.dialects;
    let branch_bool = |ctx: &mut Ctx, cond: Ref| {
        let Some(on_mismatch) = on_mismatch else {
            return;
        };
        let on_match = ctx.builder.create_block();
        ctx.builder.append(cf.Branch(
            cond,
            BlockTarget::new(on_match),
            BlockTarget::new(on_mismatch),
        ));
        ctx.builder.begin_block(on_match, []);
    };
    match &ctx.hir[pattern] {
        Pattern::Invalid => crash_point!(ctx),
        Pattern::Variable(id) => {
            ctx.builder.append(mem.Store(ctx.vars[id.idx()].0, value));
        }
        Pattern::Ignore => {}
        &Pattern::Tuple {
            member_count,
            patterns,
            types,
        } => {
            for i in 0..member_count {
                let member_pat = PatternId(patterns + i);
                let member_ty = ctx.get_hir_type(LocalTypeId(types + i))?;
                let member_ty = ctx.builder.types.add(member_ty);
                let member_value = ctx.builder.append(tuple.MemberValue(value, i, member_ty));
                lower_pattern(ctx, member_pat, member_value, on_mismatch)?;
            }
        }
        &Pattern::Int(sign, val, ty) => {
            let ir_ty = ctx.get_hir_type(ty)?;
            debug_assert!(ctx.hir[ty].is_int());
            let ty = ctx.builder.types.add(ir_ty);
            let pattern_value = int_pat(&mut ctx.builder, ctx.dialects, sign, val, ty);
            let cond = ctx
                .builder
                .append(arith.Eq(value, pattern_value, ctx.i1_ty));
            branch_bool(ctx, cond);
        }
        &Pattern::Bool(b) => {
            let Some(on_mismatch) = on_mismatch else {
                return Ok(());
            };
            let on_match = ctx.builder.create_block();
            let (on_true, on_false) = if b {
                (on_match, on_mismatch)
            } else {
                (on_mismatch, on_match)
            };
            ctx.builder.append(cf.Branch(
                value,
                BlockTarget::new(on_true),
                BlockTarget::new(on_false),
            ));
            ctx.builder.begin_block(on_match, []);
        }
        Pattern::String(s) => {
            let str_eq = builtins::get_str_eq(ctx.compiler);
            // can unwrap here because we don't pass invalid generics
            let str_eq = ctx.get_ir_id(str_eq.0, str_eq.1, Box::new([])).unwrap();
            let (expected, _str_ty) = lower_string_literal(ctx, s);
            let matches = ctx.builder.append((str_eq, (value, expected), ctx.i1_ty));
            branch_bool(ctx, matches);
        }
        &Pattern::Range {
            min_max: (min, max),
            ty,
            min_max_signs: (min_sign, max_sign),
            inclusive,
        } => {
            let ir_ty = ctx.get_hir_type(ty)?;
            debug_assert!(ctx.hir[ty].is_int());
            let ty = ctx.builder.types.add(ir_ty);
            let min = int_pat(&mut ctx.builder, ctx.dialects, min_sign, min, ty);
            let max = int_pat(&mut ctx.builder, ctx.dialects, max_sign, max, ty);
            let left = ctx.builder.append(arith.GE(value, min, ty));
            branch_bool(ctx, left);
            let right = if inclusive {
                ctx.builder.append(arith.LE(value, max, ty))
            } else {
                ctx.builder.append(arith.LT(value, max, ty))
            };
            branch_bool(ctx, right);
        }
        &Pattern::EnumVariant {
            ordinal,
            types,
            args,
        } => {
            let ir_types = ctx.get_multiple_types(LocalTypeIds {
                idx: types,
                count: args.count + 1,
            })?;
            let tuple_ty = ir::Type::Tuple(ir_types);
            // HACK: right now the value is always stored when checking with an enum variant. In
            // Rust, all patterns get passed a pointer to check their value, which would probably
            // be better. Pointer is needed right now to compare ordinal and variants
            // (implicitly casting the types on load)
            let tuple_ty = ctx.builder.types.add(tuple_ty);
            let var = ctx.builder.append(mem.Decl(tuple_ty, ctx.ptr_ty));
            ctx.builder.append(mem.Store(var, value));
            let ordinal_ty = LocalTypeId(types);
            let arg_types = LocalTypeIds {
                idx: types + 1,
                count: args.count,
            };
            match ctx.hir[ordinal_ty] {
                Type::Invalid => crash_point!(ctx),
                Type::Unit => {}
                ordinal_ty => {
                    debug_assert!(ordinal_ty.is_int());
                    let ordinal_ty = ctx.get_type(ordinal_ty)?;
                    let ordinal_ty = ctx.builder.types.add(ordinal_ty);
                    let actual_ordinal = ctx.builder.append(mem.Load(var, ordinal_ty));
                    let ordinal_value = match ordinal {
                        OrdinalType::Known(val) => val,
                        OrdinalType::Inferred(variant) => ctx.hir[variant].ordinal,
                    };
                    let expected = ctx.builder.append(arith.Int(ordinal_value, ordinal_ty));
                    let ordinal_matches =
                        ctx.builder
                            .append(arith.Eq(actual_ordinal, expected, ctx.i1_ty));
                    branch_bool(ctx, ordinal_matches);
                }
            }
            if args.count == 0 {
                return Ok(());
            }
            let ir_types_tuple = ctx.builder.types.add(ir::Type::Tuple(ir_types));
            for ((arg, i), ty) in args.iter().zip(1..).zip(arg_types.iter()) {
                let ty = ctx.get_hir_type(ty)?;
                let ty = ctx.builder.types.add(ty);
                let arg_ptr = ctx
                    .builder
                    .append(mem.MemberPtr(var, ir_types_tuple, i, ctx.ptr_ty));
                let arg_value = ctx.builder.append(mem.Load(arg_ptr, ty));
                lower_pattern(ctx, arg, arg_value, on_mismatch)?;
            }
        }
    }
    Ok(())
}

fn int_pat(
    builder: &mut Builder,
    dialects: &Dialects,
    sign: bool,
    val: u128,
    ty: ir::TypeId,
) -> Ref {
    let arith = dialects.arith;
    let mut pattern_value = if let Ok(small) = val.try_into() {
        builder.append(arith.Int(small, ty))
    } else {
        todo!("large ints")
        //builder.build_large_int(val, ty)
    };
    if sign {
        pattern_value = builder.append(arith.Neg(pattern_value, ty));
    }
    pattern_value
}

fn lower_string_literal(ctx: &mut Ctx, s: &str) -> (Ref, ir::TypeId) {
    lower_string_literal_inner(&mut ctx.builder, ctx.dialects, ctx.ptr_ty, s)
}

fn lower_string_literal_inner(
    builder: &mut Builder,
    dialects: &Dialects,
    ptr_ty: ir::TypeId,
    s: &str,
) -> (Ref, ir::TypeId) {
    let Dialects {
        arith,
        tuple,
        mem,
        main,
        ..
    } = dialects;
    // TODO: cache string ir type to prevent generating it multiple times
    let elems = builder
        .types
        .add_multiple([ir::Primitive::Ptr, ir::Primitive::U64].map(Into::into));
    let str_ty = builder.types.add(ir::Type::Tuple(elems));
    let idx = builder.env[*main].global_ids().len();
    let name = format!("str{idx}");
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    let str_global = builder.env.add_global(
        *main,
        name,
        NonZero::new(1).unwrap(),
        bytes.into_boxed_slice(),
        true,
    );
    let str_ptr = builder.append(mem.Global(str_global, ptr_ty));
    let r = builder.append_undef(str_ty);
    let r = builder.append(tuple.InsertMember(r, 0, str_ptr, str_ty));
    let u64_ty = builder.types.add(ir::Primitive::U64);
    let str_len = builder.append(arith.Int(s.len() as u64, u64_ty));
    let r = builder.append(tuple.InsertMember(r, 1, str_len, str_ty));
    (r, str_ty)
}

fn build_crash_point(ctx: &mut Ctx) {
    build_crash_point_inner(
        ctx.compiler,
        &mut ctx.builder,
        ctx.dialects,
        ctx.instances,
        ctx.to_generate,
        ctx.ptr_ty,
        ctx.return_ty,
    );
}

fn build_crash_point_inner(
    compiler: &Compiler,
    builder: &mut Builder,
    dialects: &Dialects,
    instances: &mut Instances,
    to_generate: &mut Vec<FunctionToGenerate>,
    ptr_ty: ir::TypeId,
    return_ty: ir::TypeId,
) {
    let msg = "program reached a compile error at runtime";
    let (msg, _str_ty) = lower_string_literal_inner(builder, dialects, ptr_ty, msg);
    let (panic_mod, panic_function) = builtins::get_panic(compiler);
    let panic_function = Ctx::get_ir_id_inner(
        compiler,
        builder.env,
        dialects,
        instances,
        to_generate,
        panic_mod,
        panic_function,
        Box::new([]),
    )
    .expect("panic function is invalid so can't generate crash point");
    let unit = builder.types.add(ir::Type::UNIT);
    builder.append((panic_function, (msg), unit));
    let value = builder.append_undef(return_ty);
    builder.append(dialects.cf.Ret(value));
}

fn build_arithmetic(
    ctx: &mut Ctx,
    l: Ref,
    r: Ref,
    op: crate::hir::Arithmetic,
    ty: ir::TypeId,
) -> Ref {
    use crate::hir::Arithmetic;
    assert!(
        matches!(ctx.builder.types[ty], ir::Type::Primitive(p)
            if ir::Primitive::try_from(p).unwrap().is_int()
            || ir::Primitive::try_from(p).unwrap().is_float()),
        "Invalid type {:?} for arithmetic op. Will be handled properly with traits",
        ctx.builder.types[ty]
    );
    let arith = ctx.dialects.arith;
    match op {
        Arithmetic::Add => ctx.builder.append(arith.Add(l, r, ty)),
        Arithmetic::Sub => ctx.builder.append(arith.Sub(l, r, ty)),
        Arithmetic::Mul => ctx.builder.append(arith.Mul(l, r, ty)),
        Arithmetic::Div => ctx.builder.append(arith.Div(l, r, ty)),
        Arithmetic::Mod => ctx.builder.append(arith.Rem(l, r, ty)),
    }
}

fn lower_if_else_branches(
    ctx: &mut Ctx,
    then: NodeId,
    else_: NodeId,
    else_block: ir::BlockId,
    resulting_ty: LocalTypeId,
) -> Result<Ref> {
    let Dialects { cf, .. } = ctx.dialects;
    // after_block is a closure that creates the block lazily and returns it
    let else_is_trival = matches!(ctx.hir[else_], Node::Unit);
    let mut after_block = {
        let mut after_block = else_is_trival.then_some(else_block);
        move |ctx: &mut Ctx| {
            after_block.unwrap_or_else(|| {
                let block = ctx.builder.create_block();
                after_block = Some(block);
                block
            })
        }
    };
    let mut check_branch = |ctx: &mut Ctx, value: NodeId| -> Option<ir::BlockId> {
        lower(ctx, value).ok().map(|val| {
            let block = ctx.builder.current_block().unwrap();
            let after_block = after_block(ctx);
            ctx.builder
                .append(cf.Goto(BlockTarget(after_block, Cow::Borrowed(&[val]))));
            block
        })
    };
    let then_val = check_branch(ctx, then);
    ctx.builder.begin_block(else_block, []);
    if else_is_trival {
        return Ok(Ref::UNIT);
    }
    let else_val = check_branch(ctx, else_);
    match (then_val, else_val) {
        (None, None) => Err(NoReturn),
        _ => {
            let after_block = after_block(ctx);
            let ty = ctx.get_hir_type(resulting_ty)?;
            let types = ctx.builder.types.add_multiple([ty]);
            let args = ctx.builder.begin_block(after_block, types.iter());
            Ok(args.nth(0))
        }
    }
}
