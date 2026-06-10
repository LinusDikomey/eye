use std::cell::RefCell;

use crate::{
    check::Hooks,
    compiler::{Generics, LocalScope, LocalScopeParent, VarId},
    hir::{HIRBuilder, Node},
    typing::{LocalTypeId, LocalTypeIds, NamedMembers, TypeInfo, TypeInfoOrIdx},
};
use indexmap::IndexMap;

use parser::ast::FunctionId;

use super::Ctx;

pub struct CheckedClosure {
    pub id: FunctionId,
    pub hir: HIRBuilder,
    pub generics: Generics,
    pub root: Node,
    pub params: Box<[(Box<str>, VarId)]>,
    pub param_types: LocalTypeIds,
    pub return_type: LocalTypeId,
}

impl<'a, H: Hooks> Ctx<'a, H> {
    pub fn closure(&mut self, id: FunctionId, closed_over: &mut LocalScope) -> (Node, TypeInfo) {
        let _enter = tracing::info_span!("closure", id = ?id, module = ?self.module).entered();
        let function = &self.ast[id];
        let body = function
            .body
            .expect("TODO: handle extern functions in expr position just like normal externs");
        let generics = self.compiler.resolve_generics(
            &function.generics.types,
            self.module,
            function.scope,
            self.ast,
        );

        let param_types = self
            .hir
            .types
            .add_multiple_unknown(1 + (function.params.len() + function.named_params.len()) as u32);
        let captures_ty = param_types.nth(0).unwrap();

        for (param_ty, r) in function
            .params
            .iter()
            .map(|(_, ty)| ty)
            .chain(function.named_params.iter().map(|param| &param.ty))
            .zip(param_types.iter().skip(1))
        {
            let info = self.hir.types.from_annotation(
                param_ty,
                self.compiler,
                self.module,
                function.scope,
            );
            self.hir.types.replace(r, TypeInfoOrIdx::TypeInfo(info));
        }

        let return_type = self.hir.types.from_annotation(
            &function.return_type,
            self.compiler,
            self.module,
            function.scope,
        );
        let return_type = self.hir.types.add(return_type);

        let param_names = function
            .params
            .iter()
            .map(|(name_span, _)| name_span)
            .chain(function.named_params.iter().map(|param| &param.name))
            .map(|name_span| {
                self.ast.src()[name_span.range()]
                    .to_owned()
                    .into_boxed_str()
            })
            .zip(param_types.iter().skip(1));

        let captures = RefCell::new(IndexMap::new());
        assert_eq!(
            function.generics.count(),
            0,
            "TODO: handle generics in closures/functions in expr position",
        );
        assert!(
            function.named_params.is_empty(),
            "TODO: handle named params in closures/functions in expr position",
        );

        let types = std::mem::take(&mut self.hir.types);
        let (closure_hir, root, captures_param, params) = {
            let mut outer_hir = std::mem::replace(&mut self.hir, HIRBuilder::new(types));
            let outer_vars = RefCell::new(std::mem::take(&mut outer_hir.vars));

            let params: Box<[(Box<str>, VarId)]> = param_names
                .map(|(name, ty)| (name, self.hir.add_var(ty)))
                .collect();
            let variables = params
                .iter()
                .map(|(name, var)| (name.clone(), *var))
                .collect();
            let mut scope = LocalScope {
                parent: LocalScopeParent::ClosedOver {
                    scope: &mut *closed_over,
                    captures: &captures,
                    outer_vars: &outer_vars,
                },
                variables,
                module: self.module,
                static_scope: Some(function.scope),
            };

            let captures_param = self.hir.add_captures_param_var(captures_ty);

            let root = self.check(body, &mut scope, return_type, return_type, &mut false);

            let mut inner_hir = std::mem::replace(&mut self.hir, outer_hir);
            self.hir.types = std::mem::take(&mut inner_hir.types);
            self.hir.vars = outer_vars.into_inner();
            (inner_hir, root, captures_param, params)
        };

        let captures = captures.into_inner();
        let capture_count = captures.len() as _;
        let capture_nodes = self.hir.add_invalid_nodes(capture_count);
        // let outside_capture_infos = self.hir.types.add_multiple_unknown(capture_count);
        let capture_types = self.hir.types.add_multiple_unknown(capture_count);
        for (((var, _), node), id) in captures
            .into_iter()
            .zip(capture_nodes.iter())
            .zip(capture_types.iter())
        {
            self.hir.modify_node(node, Node::Variable(var));
            let var = self.hir.get_var(var);
            self.hir.types.replace(id, var.ty());
        }
        self.hir.types.replace(
            captures_ty,
            TypeInfo::Tuple {
                members: capture_types,
                named_members: NamedMembers::EMPTY,
            },
        );

        let params = std::iter::once(("".into(), captures_param))
            .chain(params)
            .collect();

        let checked = CheckedClosure {
            id,
            generics,
            hir: closure_hir,
            root,
            params,
            param_types,
            return_type,
        };
        debug_assert_eq!(checked.params.len(), param_types.count as usize);
        self.checked_closures.push(checked);

        // pass the inherited generics into the closure invocation
        // TODO: chain the closure's generic here as Unknown types
        let generics_instance = self
            .hir
            .types
            .add_multiple((0..self.generics.count()).map(|i| {
                TypeInfo::Known(
                    self.compiler
                        .types
                        .intern(crate::types::TypeFull::Generic(i)),
                )
            }));
        tracing::debug!(target: "closure", "Creating closure {:?} : {id:?}", self.module);
        let params_tuple = self.hir.types.add(TypeInfo::Tuple {
            members: param_types.skip(1),
            named_members: NamedMembers::EMPTY,
        });
        (
            Node::TupleLiteral {
                elems: capture_nodes,
                elem_types: capture_types,
            },
            TypeInfo::Closure {
                function: id,
                captures: capture_types,
                generics: generics_instance,
                params: params_tuple,
                return_type,
            },
        )
    }
}
