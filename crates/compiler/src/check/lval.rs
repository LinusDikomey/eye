use error::{Error, span::TSpan};

use crate::{
    check::Hooks,
    compiler::{Def, LocalItem, LocalScope, ModuleSpan, ResolvedTypeContent},
    hir::{LValue, Node},
    types::{BaseType, TypeFull},
    typing::{Bounds, LocalTypeId, LocalTypeIds, TypeInfo},
};
use parser::ast::{Ast, Expr, ExprId, UnOp};

use super::Ctx;

pub fn check<H: Hooks>(
    ctx: &mut Ctx<H>,
    expr: ExprId,
    scope: &mut LocalScope,
    return_ty: LocalTypeId,
    noreturn: &mut bool,
) -> (LValue, LocalTypeId) {
    let (value, ty) = check_inner(ctx, expr, scope, return_ty, noreturn);
    ctx.hooks.on_checked_lvalue(expr, &mut ctx.hir, scope, ty);
    (value, ty)
}

pub fn check_inner<H: Hooks>(
    ctx: &mut Ctx<H>,
    expr: ExprId,
    scope: &mut LocalScope,
    return_ty: LocalTypeId,
    noreturn: &mut bool,
) -> (LValue, LocalTypeId) {
    match ctx.ast[expr] {
        Expr::Ident { span, .. } => {
            match scope.resolve(
                &ctx.ast.src()[span.range()],
                span,
                ctx.compiler,
                &mut ctx.hir.vars,
            ) {
                LocalItem::Invalid | LocalItem::Def(Def::Invalid) => {
                    (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID))
                }
                LocalItem::Var(id) => {
                    let var_ty = ctx.hir.get_var(id).ty();
                    (LValue::Variable(id), var_ty)
                }
                LocalItem::Def(def) => def_lvalue(ctx, expr, def),
            }
        }
        Expr::UnOp {
            op: UnOp::Deref,
            inner,
            ..
        } => {
            let pointee = ctx.hir.types.add_unknown();
            let pointer = ctx
                .hir
                .types
                .add(TypeInfo::Instance(BaseType::Pointer, pointee.into()));
            let node = ctx.check(inner, scope, pointer, return_ty, noreturn);
            (LValue::Deref(ctx.hir.add(node)), pointee)
        }
        Expr::MemberAccess {
            left,
            name: name_span,
            ..
        } => {
            let left_ty = ctx.hir.types.add_unknown();
            let left_val = ctx.check(left, scope, left_ty, return_ty, noreturn);
            let name = &ctx.ast.src()[name_span.range()];
            if let TypeInfo::ModuleItem(id) = ctx.hir.types[left_ty] {
                let def = ctx.compiler.resolve_in_module(
                    id,
                    name,
                    ModuleSpan {
                        module: ctx.module,
                        span: name_span,
                    },
                );
                return def_lvalue(ctx, expr, def);
            }
            let Some((dereffed_left_ty, pointer_count)) =
                auto_deref(ctx, left_ty, |ast| ast[left].span(ast))
            else {
                return (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID));
            };
            if let Some((base, generics)) = dereffed_left_ty.into_base(ctx.compiler) {
                let ty = ctx.compiler.get_base_type_def(base);
                // TODO differentiate between nonexistant member and 'can't assign to
                // member' in the case of methods, enum variants etc.
                if let ResolvedTypeContent::Struct(struct_) = &ty.def {
                    let indexed_field = struct_.get_indexed_field(ctx, generics, name);
                    let Some((index, field_ty)) = indexed_field else {
                        ctx.emit(Error::NonexistantMember(None).at_span(name_span));
                        return (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID));
                    };

                    let left_lval =
                        dereffed_to_lvalue(ctx, left_val, left_ty, pointer_count, |ast| {
                            ast[left].span(ast)
                        });
                    return (
                        LValue::Member {
                            tuple_value: ctx.hir.add_lvalue(left_lval),
                            index,
                            // TODO: is it safe to add a new type var here?
                            tuple_ty: ctx.hir.types.add(dereffed_left_ty),
                        },
                        ctx.hir.types.add_info_or_idx(field_ty),
                    );
                }
            } else if let Some(tuple) = dereffed_left_ty.into_tuple(ctx.compiler) {
                let Some(((_, member_ty), index)) = tuple
                    .named_members(&ctx.hir.types)
                    .zip(tuple.positional_count()..)
                    .find(|((member_name, _), _)| *member_name == name)
                else {
                    ctx.emit(Error::NonexistantMember(None).at_span(name_span));
                    return (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID));
                };
                let left_lval = dereffed_to_lvalue(ctx, left_val, left_ty, pointer_count, |ast| {
                    ast[left].span(ast)
                });
                return (
                    LValue::Member {
                        tuple_value: ctx.hir.add_lvalue(left_lval),
                        index,
                        tuple_ty: ctx.hir.types.add(dereffed_left_ty),
                    },
                    ctx.hir.types.add_info_or_idx(member_ty),
                );
            }
            // TODO(error): better error why the type doesn't have named members
            ctx.emit(Error::NonexistantMember(None).at_span(name_span));
            (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID))
        }
        Expr::TupleIdx { left, idx, .. } => {
            let left_ty = ctx.hir.types.add_unknown();
            let left_val = ctx.check(left, scope, left_ty, return_ty, noreturn);
            let Some((dereffed_ty, pointer_count)) =
                auto_deref(ctx, left_ty, |ast| ast[left].span(ast))
            else {
                return (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID));
            };
            if let Some(tuple) = dereffed_ty.into_tuple(ctx.compiler) {
                let Some(elem_ty) = tuple.members().nth(idx) else {
                    ctx.emit(Error::NonexistantMember(None).at_span(ctx.span(expr)));
                    return (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID));
                };
                let lval = dereffed_to_lvalue(ctx, left_val, left_ty, pointer_count, |ast| {
                    ast[left].span(ast)
                });
                (
                    LValue::Member {
                        tuple_value: ctx.hir.add_lvalue(lval),
                        index: idx,
                        tuple_ty: ctx.hir.types.add(dereffed_ty),
                    },
                    ctx.hir.types.add_info_or_idx(elem_ty),
                )
            } else {
                // TODO(error): use span of the idx, not of the whole expr
                // TODO(error): better error why the type doesn't have tuple members
                ctx.emit(Error::NonexistantMember(None).at_span(ctx.span(expr)));
                (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID))
            }
        }
        Expr::Index { expr, idx, .. } => {
            let array_generics = ctx
                .hir
                .types
                .add_multiple([TypeInfo::Unknown(Bounds::EMPTY), TypeInfo::UnknownConst]);
            let element_type = array_generics.nth(0).unwrap();
            let (array, array_ty) = check(ctx, expr, scope, return_ty, noreturn);
            ctx.specify(
                array_ty,
                TypeInfo::Instance(BaseType::Array, array_generics),
                |ast| ast[expr].span(ast),
            );
            let index_ty = ctx.hir.types.add(TypeInfo::Integer);
            let index = ctx.check(idx, scope, index_ty, return_ty, noreturn);
            (
                LValue::ArrayIndex {
                    array: ctx.hir.add_lvalue(array),
                    index: ctx.hir.add(index),
                    elem_ty: element_type,
                },
                element_type,
            )
        }
        _ => {
            ctx.emit(Error::CantAssignTo.at_span(ctx.span(expr)));
            (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID))
        }
    }
}

fn auto_deref<H: Hooks>(
    ctx: &mut Ctx<'_, H>,
    ty: LocalTypeId,
    span: impl Fn(&Ast) -> TSpan,
) -> Option<(TypeInfo, u32)> {
    let mut pointer_count = 0;
    let mut current_ty = ty;
    loop {
        match ctx.hir.types[current_ty] {
            TypeInfo::Instance(BaseType::Invalid, _) => return None,
            TypeInfo::Instance(BaseType::Pointer, pointee) => {
                current_ty = pointee.nth(0).unwrap();
                pointer_count += 1;
            }
            ty @ TypeInfo::Known(known_ty) => {
                if let TypeFull::Instance(BaseType::Pointer, &[pointee]) =
                    ctx.compiler.types.lookup(known_ty)
                {
                    current_ty = ctx.hir.types.add(TypeInfo::Known(pointee));
                    pointer_count += 1;
                } else {
                    return Some((ty, pointer_count));
                }
            }
            TypeInfo::Unknown(bounds) => {
                let needed_bound = bounds.iter().next().map(|bound| {
                    let id = ctx.hir.types.get_bound(bound).trait_id;
                    ctx.compiler.get_trait_name(id.0, id.1).to_owned()
                });
                ctx.emit(Error::TypeMustBeKnownHere { needed_bound }.at_span(span(ctx.ast)));
                return None;
            }
            ty => return Some((ty, pointer_count)),
        }
    }
}
fn dereffed_to_lvalue<H: Hooks>(
    ctx: &mut Ctx<H>,
    node: Node,
    ty: LocalTypeId,
    mut pointer_count: u32,
    span: impl Fn(&Ast) -> TSpan,
) -> LValue {
    if pointer_count == 0 {
        if let Some(lval) = LValue::try_from_node(&node, &mut ctx.hir) {
            lval
        } else {
            ctx.emit(Error::CantAssignTo.at_span(span(ctx.ast)));
            LValue::Invalid
        }
    } else {
        let mut current_val = node;
        let mut current_ty = ty;
        // perform additional derefs and keep the last one as an LValue
        while pointer_count > 1 {
            let TypeInfo::Instance(BaseType::Pointer, pointee) = ctx.hir.types[current_ty] else {
                unreachable!()
            };
            let pointee = pointee.nth(0).unwrap();
            current_val = Node::Deref {
                value: ctx.hir.add(current_val),
                deref_ty: pointee,
            };
            current_ty = pointee;
            pointer_count -= 1;
        }
        LValue::Deref(ctx.hir.add(current_val))
    }
}

fn def_lvalue<H: Hooks>(ctx: &mut Ctx<H>, expr: ExprId, def: Def) -> (LValue, LocalTypeId) {
    match def {
        Def::Global(module, id) => {
            // PERF: cloning type
            let global_ty = ctx.compiler.get_checked_global(module, id).1;
            let ty = ctx.from_type_instance(global_ty, LocalTypeIds::EMPTY);
            let ty = ctx.hir.types.add_info_or_idx(ty);
            (LValue::Global(module, id), ty)
        }
        Def::Invalid => (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID)),
        _ => {
            ctx.emit(Error::CantAssignTo.at_span(ctx.span(expr)));
            (LValue::Invalid, ctx.hir.types.add(TypeInfo::INVALID))
        }
    }
}
