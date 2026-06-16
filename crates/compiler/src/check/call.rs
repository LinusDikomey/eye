use crate::check::Hooks;
use crate::types::{BaseType, BuiltinType};
use crate::typing::{LocalOrGlobalInstance, NamedMembers};
use crate::{InvalidTypeError, Type};
use crate::{
    compiler::{LocalScope, ResolvedStructDef, ResolvedTypeContent, builtins},
    eval::ConstValueId,
    hir::{Node, NodeIds},
    typing::{Bound, LocalTypeId, LocalTypeIds, TypeInfo, TypeInfoOrIdx},
};
use error::{Error, span::TSpan};
use parser::ast::{Ast, Call, ExprId, ExprIds};

use super::Ctx;

impl<'a, H: Hooks> Ctx<'a, H> {
    pub fn check_call(
        &mut self,
        call: &Call,
        expr: ExprId,
        scope: &mut LocalScope,
        expected: LocalTypeId,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
    ) -> Node {
        let called_ty = self.hir.types.add_unknown();
        let called_node = self.check(call.called_expr, scope, called_ty, return_ty, noreturn);
        let call_span = TSpan::new(call.open_paren_start, call.end);
        let mut instance_type_called =
            |ctx: &mut Self, base: BaseType, info: TypeInfo, instance: LocalOrGlobalInstance| {
                let resolved = ctx.compiler.get_base_type_def(base);
                match &resolved.def {
                    ResolvedTypeContent::Struct(struct_def) => {
                        ctx.specify(expected, info, |ast| ast[expr].span(ast));
                        ctx.check_struct_initializer(
                            struct_def, instance, call, scope, return_ty, noreturn,
                        )
                        .unwrap_or_else(|_| {
                            ctx.invalidate(expected);
                            Node::Invalid
                        })
                    }
                    ResolvedTypeContent::Builtin(_) | ResolvedTypeContent::Enum(_) => {
                        ctx.emit(Error::FunctionOrStructTypeExpected.at_span(ctx.span(expr)));
                        Node::Invalid
                    }
                }
            };

        let mut s = String::new();
        self.hir.types.type_to_string_inner(
            self.compiler,
            self.generics,
            self.hir.types[called_ty],
            &mut s,
        );

        let called_info = self.hir.types[called_ty];
        if called_info.is_invalid() {
            self.invalidate(expected);
            return Node::Invalid;
        } else if let Some(function_instance) =
            called_info.into_specific_base(self.compiler, BaseType::Function)
        {
            return self.function_type_call(
                function_instance,
                call,
                called_node,
                call_span,
                scope,
                expected,
                return_ty,
                noreturn,
            );
        }
        match called_info {
            TypeInfo::BaseTypeItem(base) => {
                debug_assert!(
                    !matches!(
                        self.compiler.get_base_type_def(base).def,
                        ResolvedTypeContent::Builtin(BuiltinType::Function)
                    ),
                    "function base type should not be possible to obtain"
                );
                let generic_count = self.compiler.get_base_type_generic_count(base);
                let base_generic_count = generic_count.into();
                let base_generics = self.hir.types.add_multiple_unknown(base_generic_count);
                let info = TypeInfo::Instance(base, base_generics);
                return instance_type_called(self, base, info, base_generics.into());
            }
            TypeInfo::TypeItem(ty) => {
                let info = self.hir.types[ty];
                if let TypeInfo::Unknown(bounds) = info {
                    self.emit_unknown(bounds, self.span(call.called_expr));
                    self.invalidate(expected);
                    return Node::Invalid;
                } else if let Some((base, generics)) = info.into_base(self.compiler) {
                    if base == BaseType::Invalid {
                        self.invalidate(expected);
                        return Node::Invalid;
                    }
                    return instance_type_called(self, base, info, generics);
                }
            }
            TypeInfo::FunctionItem {
                module,
                function,
                generics,
            } => {
                let signature = self.compiler.get_signature(module, function);
                let function = self.hir.add(Node::FunctionItem {
                    function: (module, function),
                    generics,
                    ty: called_ty,
                });
                match self.check_call_args(
                    scope,
                    return_ty,
                    noreturn,
                    call_span,
                    call.args,
                    &call.named_args,
                    generics.into(),
                    &signature.params,
                    &signature.named_params,
                    signature.varargs,
                    false,
                ) {
                    Ok((args, arg_types)) => {
                        if *noreturn {
                            return Node::Invalid;
                        }
                        // TODO: figure out noreturn checking, maybe after typecheck is done?
                        let return_info = self.hir.types.from_type_instance(
                            &self.compiler.types,
                            signature.return_type,
                            generics,
                        );
                        let return_var = self.hir.types.add_info_or_idx(return_info);
                        let call_noreturn = self
                            .hir
                            .types
                            .is_uninhabited(self.compiler, self.hir.types[return_var]);
                        match call_noreturn {
                            Ok(true) => *noreturn = true,
                            Ok(false) => {
                                self.unify(expected, return_var, |ast: &Ast| ast[expr].span(ast));
                            }
                            Err(InvalidTypeError) => {
                                self.invalidate(expected);
                            }
                        }

                        return Node::Call {
                            function,
                            args,
                            return_ty: return_var,
                            arg_types,
                            noreturn: *noreturn,
                        };
                    }
                    Err(err) => {
                        self.emit(err);
                        self.invalidate(expected);
                        return Node::Invalid;
                    }
                }
            }
            TypeInfo::MethodItem {
                module,
                function,
                generics,
            } => {
                let signature = self.compiler.get_signature(module, function);
                // it was already checked that the first argument fits the self parameter correctly
                let signature_params = &signature.params[1..];

                self.specify_resolved(expected, signature.return_type, generics, |ast| {
                    ast[expr].span(ast)
                });
                match self.check_call_args(
                    scope,
                    return_ty,
                    noreturn,
                    call_span,
                    call.args,
                    &call.named_args,
                    generics.into(),
                    signature_params,
                    &signature.named_params,
                    signature.varargs,
                    true,
                ) {
                    Ok((args, arg_types)) => {
                        if *noreturn {
                            return Node::Invalid;
                        }
                        self.hir
                            .modify_node(args.iter().next().unwrap(), called_node);
                        let self_type = self.hir.types.from_type_instance(
                            &self.compiler.types,
                            signature.params[0].1,
                            generics,
                        );
                        self.hir
                            .types
                            .replace(arg_types.iter().next().unwrap(), self_type);
                        // TODO: figure out noreturn checking
                        /*
                        let call_noreturn = matches!(
                            self.compiler.uninhabited(&signature.return_type, generics),
                            Ok(true)
                        );
                        if call_noreturn {
                            *noreturn = true;
                        }
                        */

                        return Node::Call {
                            function: self.hir.add(Node::FunctionItem {
                                function: (module, function),
                                generics,
                                ty: called_ty,
                            }),
                            args,
                            return_ty: expected,
                            arg_types,
                            noreturn: false, // TODO
                        };
                    }
                    Err(err) => {
                        self.emit(err);
                        self.invalidate(expected);
                        return Node::Invalid;
                    }
                }
            }
            TypeInfo::EnumVariantItem {
                enum_type,
                generics,
                ordinal: variant,
                arg_types,
            } => {
                self.specify(expected, TypeInfo::Instance(enum_type, generics), |ast| {
                    ast[expr].span(ast)
                });
                if call.args.count + 1 != arg_types.count {
                    self.emit(
                        Error::InvalidArgCount {
                            expected: arg_types.count - 1,
                            varargs: false,
                            found: arg_types.count,
                        }
                        .at_span(self.span(expr)),
                    );
                    self.invalidate(expected);
                    return Node::Invalid;
                }
                let elems = self.hir.add_invalid_nodes(arg_types.count);
                let mut elem_iter = elems.iter();
                let mut arg_type_iter = arg_types.iter();
                self.hir.modify_node(
                    elem_iter.next().unwrap(),
                    Node::IntLiteral {
                        val: variant as _,
                        ty: arg_type_iter.next().unwrap(),
                    },
                );
                for ((r, arg), ty) in elem_iter.zip(call.args).zip(arg_types.iter()) {
                    let node = self.check(arg, scope, ty, return_ty, noreturn);
                    self.hir.modify_node(r, node);
                }
                return Node::EnumLiteral {
                    elems,
                    elem_types: arg_types,
                    enum_ty: called_ty,
                };
            }
            TypeInfo::TraitMethodItem {
                module: trait_module,
                trait_id,
                method_index,
            } => {
                let Some(checked_trait) = self.compiler.get_checked_trait(trait_module, trait_id)
                else {
                    return Node::Invalid;
                };
                let signature = &checked_trait.functions[method_index as usize];
                let span = self.ast[expr].span(self.ast);
                let generics =
                    signature
                        .generics
                        .instantiate(&mut self.hir.types, &self.compiler.types, span);
                debug_assert!(
                    signature.generics.count() > checked_trait.generics.count(),
                    "the method should at least have the trait's and the self type's generics {} >= {}",
                    signature.generics.count(),
                    checked_trait.generics.count() + 1,
                );
                // generics NOT including the self type
                let trait_generics = LocalTypeIds {
                    idx: generics.idx + 1,
                    count: checked_trait.generics.count().into(),
                };
                let self_ty = generics.iter().next().unwrap();
                assert!(
                    matches!(self.hir.types[self_ty], TypeInfo::Unknown(bounds) if bounds.is_empty()),
                    "TODO: handle existing self bounds"
                );
                let self_bounds = self.hir.types.add_bounds([Bound {
                    trait_id: (trait_module, trait_id),
                    generics: trait_generics,
                    span,
                }]);
                self.hir
                    .types
                    .replace(self_ty, TypeInfo::Unknown(self_bounds));
                self.specify_resolved(expected, signature.return_type, generics, |_| span);
                match self.check_call_args(
                    scope,
                    return_ty,
                    noreturn,
                    call_span,
                    call.args,
                    &call.named_args,
                    generics.into(),
                    &signature.params,
                    &signature.named_params,
                    signature.varargs,
                    false,
                ) {
                    Ok((args, _)) => {
                        /*
                        let call_noreturn = matches!(
                            self.compiler
                                .uninhabited(&signature.return_type, generics),
                            Ok(true)
                        );
                        if call_noreturn {
                            *noreturn = true;
                        }
                        */
                        return Node::TraitCall {
                            trait_id: (trait_module, trait_id),
                            trait_generics,
                            method_index,
                            self_ty,
                            args,
                            return_ty: expected,
                            noreturn: false, // call_noreturn, // TODO
                        };
                    }
                    Err(err) => {
                        self.emit(err);
                        self.invalidate(expected);
                        self.invalidate(self_ty);
                        return Node::Invalid;
                    }
                }
            }
            _ => {}
        }

        // TODO: auto ref/deref on called value
        let arg_types = self.hir.types.add_multiple_unknown(call.args.len() as _);
        let fn_instance = self.hir.types.add_multiple_info_or_idx([
            TypeInfoOrIdx::TypeInfo(TypeInfo::Tuple {
                members: arg_types,
                named_members: NamedMembers::EMPTY,
            }),
            expected.into(),
        ]);
        assert_eq!(
            call.named_args.len(),
            0,
            "TODO: support named args in trait calls or error properly"
        );

        let fn_trait = builtins::get_fn_trait(self.compiler);
        let fn_bound = Bound {
            trait_id: fn_trait,
            generics: fn_instance,
            span: call_span,
        };
        self.specify_bound(called_ty, fn_bound, self.span(call.called_expr));

        let arg_nodes = self.hir.add_invalid_nodes(arg_types.count);
        for ((arg, ty), node_id) in call
            .args
            .into_iter()
            .zip(arg_types.iter())
            .zip(arg_nodes.iter())
        {
            let node = self.check(arg, scope, ty, return_ty, noreturn);
            self.hir.modify_node(node_id, node);
            if *noreturn {
                return Node::Invalid;
            }
        }
        let fn_args = self.hir.add_nodes([
            called_node,
            Node::TupleLiteral {
                elems: arg_nodes,
                elem_types: arg_types,
            },
        ]);
        Node::TraitCall {
            trait_id: fn_trait,
            trait_generics: fn_instance,
            method_index: 0,
            self_ty: called_ty,
            args: fn_args,
            return_ty: expected,
            noreturn: false,
        }
    }

    fn function_type_call(
        &mut self,
        function_generics: LocalOrGlobalInstance,
        call: &Call,
        called_node: Node,
        call_span: TSpan,
        scope: &mut LocalScope,
        expected: LocalTypeId,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
    ) -> Node {
        let return_type = function_generics.nth(0).unwrap();
        let params = match function_generics {
            LocalOrGlobalInstance::Local(ids) => ids.skip(1),
            LocalOrGlobalInstance::Global(items) => self
                .hir
                .types
                .add_multiple(items.iter().copied().skip(1).map(TypeInfo::Known)),
        };
        // TODO: call noreturn checking
        let call_noreturn = false;
        // let call_noreturn = matches!(
        //     self.compiler.uninhabited(
        //         &self.hir
        //             .types
        //             .to_generic_resolved(self.hir.types[return_type])
        //             .unwrap_or(TypeOld::Invalid),
        //         &[], // TODO: this will probably cause issues, need to be able to not pass in generics?
        //     ),
        //     Ok(true)
        // );
        self.specify_or_unify(expected.into(), return_type, |_| call_span);
        match self.check_call_args_inner(
            scope,
            return_ty,
            noreturn,
            call_span,
            call.args,
            &[],
            params.count,
            &[],
            params,
            false,
        ) {
            Ok((args, arg_types)) => {
                if *noreturn {
                    return Node::Invalid;
                }
                if call_noreturn {
                    *noreturn = true;
                }
                let function = self.hir.add(called_node);

                Node::Call {
                    function,
                    args,
                    return_ty: expected,
                    arg_types,
                    noreturn: false, // TODO: call_noreturn,
                }
            }
            Err(err) => {
                self.emit(err);
                self.invalidate(expected);
                Node::Invalid
            }
        }
    }

    fn call_arg_types(
        &mut self,
        arg_count: u32,
        params: &[(Box<str>, Type)],
        named_params: &[(Box<str>, Type, Option<ConstValueId>)],
        generics: LocalOrGlobalInstance,
        varargs: bool,
        extra_arg_slot: bool,
    ) -> Result<LocalTypeIds, Error> {
        // TODO: this function probably breaks with varargs and named args, properly define what combinations are allowed and how
        let invalid_arg_count = if varargs {
            arg_count < params.len() as u32
        } else {
            arg_count != params.len() as u32
        };
        if invalid_arg_count {
            let expected = params.len() as _;
            return Err(Error::InvalidArgCount {
                expected,
                varargs,
                found: arg_count,
            });
        }
        let arg_type_vars = self
            .hir
            .types
            .add_multiple_unknown(extra_arg_slot as u32 + arg_count + named_params.len() as u32);
        for (&ty, var) in params
            .iter()
            .map(|(_, ty)| ty)
            .chain(named_params.iter().map(|(_, ty, _)| ty))
            .zip(arg_type_vars.iter().skip(extra_arg_slot as usize))
        {
            let ty = self.from_type_instance(ty, generics);
            self.hir.types.replace(var, ty);
        }
        Ok(arg_type_vars)
    }

    fn check_call_args(
        &mut self,
        scope: &mut LocalScope,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
        span: TSpan,
        args: ExprIds,
        named_args: &[(TSpan, ExprId)],
        generics: LocalOrGlobalInstance,
        params: &[(Box<str>, Type)],
        named_params: &[(Box<str>, Type, Option<ConstValueId>)],
        varargs: bool,
        extra_arg_slot: bool,
    ) -> Result<(NodeIds, LocalTypeIds), error::CompileError> {
        let arg_types = self
            .call_arg_types(
                args.count,
                params,
                named_params,
                generics,
                varargs,
                extra_arg_slot,
            )
            .map_err(|err| err.at_span(span))?;
        self.check_call_args_inner(
            scope,
            return_ty,
            noreturn,
            span,
            args,
            named_args,
            params.len() as u32,
            named_params,
            arg_types,
            extra_arg_slot,
        )
    }

    fn check_call_args_inner(
        &mut self,
        scope: &mut LocalScope,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
        span: TSpan,
        args: ExprIds,
        named_args: &[(TSpan, ExprId)],
        param_count: u32,
        named_params: &[(Box<str>, Type, Option<ConstValueId>)],
        arg_types: LocalTypeIds,
        extra_arg_slot: bool,
    ) -> Result<(NodeIds, LocalTypeIds), error::CompileError> {
        let all_arg_nodes = self.hir.add_invalid_nodes(arg_types.count);
        let arg_nodes = if extra_arg_slot {
            all_arg_nodes.skip(1)
        } else {
            all_arg_nodes
        };

        let param_arg_types = arg_types.skip(extra_arg_slot as u32);

        // iterating over the signature, all extra arguments in case of vararg
        // arguments will stay unknown which is intended
        for ((arg, node_idx), ty) in args
            .into_iter()
            .zip(
                arg_nodes.iter().take(param_count as usize).chain(
                    arg_nodes
                        .iter()
                        .skip(param_count as usize + named_args.len()),
                ),
            )
            .zip(
                param_arg_types.iter().take(param_count as usize).chain(
                    param_arg_types
                        .iter()
                        .skip(param_count as usize + named_args.len()),
                ),
            )
        {
            let node = self.check(arg, scope, ty, return_ty, noreturn);
            if *noreturn {
                return Ok((all_arg_nodes, param_arg_types));
            }
            self.hir.modify_node(node_idx, node);
        }

        for &(name_span, value) in named_args {
            let name = &self.ast[name_span];
            let Some(i) = named_params
                .iter()
                .position(|(arg_name, _, _)| &**arg_name == name)
            else {
                self.emit(Error::NonexistantNamedArg.at_span(name_span));
                // still check the expr with an unknown expected type
                let ty = self.hir.types.add_unknown();
                self.check(value, scope, ty, return_ty, noreturn);
                if *noreturn {
                    return Ok((all_arg_nodes, arg_types));
                }
                continue;
            };
            let node_idx = arg_nodes.iter().nth(param_count as usize + i).unwrap();
            let ty = param_arg_types.nth(param_count + i as u32).unwrap();
            let node = self.check(value, scope, ty, return_ty, noreturn);
            if *noreturn {
                return Ok((all_arg_nodes, arg_types));
            }
            self.hir.modify_node(node_idx, node);
        }

        let mut missing_named_args = Vec::new();
        for ((name, _, default_val), (node_id, ty)) in named_params.iter().zip(
            arg_nodes
                .iter()
                .zip(param_arg_types.iter())
                .skip(param_count as usize),
        ) {
            if matches!(self.hir[node_id], Node::Invalid) {
                let Some(default_val) = *default_val else {
                    missing_named_args.push(name.clone());
                    continue;
                };
                self.hir.modify_node(
                    node_id,
                    Node::Const {
                        id: default_val,
                        ty,
                    },
                );
            }
        }
        if !missing_named_args.is_empty() {
            return Err(Error::MissingNamedArgs {
                names: missing_named_args.into_boxed_slice(),
            }
            .at_span(span));
        }
        debug_assert_eq!(all_arg_nodes.count, arg_types.count);
        Ok((all_arg_nodes, arg_types))
    }

    fn check_struct_initializer(
        &mut self,
        struct_def: &ResolvedStructDef,
        generics: LocalOrGlobalInstance,
        call: &Call,
        scope: &mut LocalScope,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
    ) -> Result<Node, ()> {
        let span = TSpan::new(call.open_paren_start, call.end);
        match self.check_call_args(
            scope,
            return_ty,
            noreturn,
            span,
            call.args,
            &call.named_args,
            generics,
            &struct_def.fields,
            &struct_def.named_fields,
            false,
            false,
        ) {
            Ok((elems, elem_types)) => Ok(Node::TupleLiteral { elems, elem_types }),
            Err(err) => {
                self.emit(err);
                Err(())
            }
        }
    }
}
