use compiler::{
    Compiler, Def, ModuleSpan, Type,
    check::traits,
    compiler::{BodyOrTypes, LocalScope, ResolvedTypeContent, Signature, VarId},
    hir::HIRBuilder,
    types::{BaseType, TypeFull},
    typing::{LocalTypeId, TypeInfo},
};
use parser::ast::{self, Ast, Expr, ExprId, FunctionId, ModuleId, ScopeId, TraitId};

use crate::{
    lsp::{Lsp, find_in_ast::ScopeContext},
    types::request::{
        CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionParams,
        MarkupContent, MarkupKind,
    },
};

impl Lsp {
    pub fn complete(&mut self, complete: CompletionParams) -> Vec<CompletionItem> {
        tracing::info!("Handling completion");
        let Some((module, offset, found)) = self.find_document_position(&complete.position) else {
            tracing::info!("Document not found: {:?}", complete.position);
            return Vec::new();
        };
        let ast = self.compiler.get_module_ast(module);
        tracing::info!("AST at completion time:\n{}", ast);
        let mut completions = Vec::new();

        let context = self.find_context_for_scope(module, found.scope);
        match context {
            ScopeContext::TopLevel => completions.push(CompletionItem {
                label: format!("debug_completion_toplevel {found:?}"),
                kind: None,
                detail: None,
                labelDetails: None,
                documentation: None,
                preselect: false,
            }),
            ScopeContext::Function(function_id) => {
                let ast = self.compiler.get_module_ast(module);
                let mut variables = Vec::new();
                let mut hooks = CompletionHooks {
                    variables: &mut variables,
                    target_offset: offset,
                    target_scope: found.scope,
                    completion_context: CompletionContext::Scope(found.scope),
                    ast,
                    done: false,
                    completing_member_access: None,
                };
                // TODO: currently this doesn't properly handle closures!
                let checked =
                    compiler::check::function(&self.compiler, module, function_id, &mut hooks);
                if let BodyOrTypes::Body(hir) = &checked.body_or_types {
                    let signature = self.compiler.get_signature(module, function_id);
                    match hooks.completion_context {
                        CompletionContext::Scope(id) => {
                            const_completions(
                                &mut completions,
                                &self.compiler,
                                module,
                                id,
                                false,
                                None,
                            );
                        }
                        CompletionContext::MemberAccess {
                            object,
                            expected_ty,
                        } => {
                            // member access completion
                            debug_assert!(hooks.variables.is_empty());
                            let expected = hir[expected_ty];
                            match object {
                                MemberObject::Value(left_ty) => {
                                    let left_ty = hir[left_ty];
                                    value_member_access_completion(
                                        &mut completions,
                                        &self.compiler,
                                        signature,
                                        left_ty,
                                        expected,
                                    );
                                }
                                MemberObject::Module(id) => const_completions(
                                    &mut completions,
                                    &self.compiler,
                                    id,
                                    self.compiler.get_parsed_module(id).ast.top_level_scope_id(),
                                    true,
                                    Some(expected),
                                ),
                                // TODO: completion on types and traits
                                MemberObject::BaseType(_) => {}
                                MemberObject::Type(_) => {}
                                MemberObject::Trait(_, _) => {}
                            }
                        }
                    }
                    for (name, variable) in variables {
                        let ty = hir[hir.vars[variable.idx()].ty()];
                        let ty = self.compiler.types.display(ty, &signature.generics);
                        completions.push(CompletionItem {
                                label: name,
                                kind: Some(CompletionItemKind::Variable),
                                detail: None,
                                labelDetails: Some(CompletionItemLabelDetails {
                                    description: Some(format!(": {ty} VARIABLE")),
                                    detail: None,
                                }),
                                documentation: Some(MarkupContent {
                                    kind: MarkupKind::Markdown,
                                    value: "Documentation for completion will go here\n\nCode block test:\n```eye\nexample :: fn(x i32) {}\n```".to_string(),
                                }),
                                preselect: false,
                            });
                    }
                }
            }
        }

        tracing::info!("Returning {} completions", completions.len());
        completions
    }
}

enum CompletionContext {
    Scope(ScopeId),
    MemberAccess {
        /// object that we are getting a member of
        object: MemberObject,
        /// type that the member should have
        expected_ty: LocalTypeId,
    },
}

enum MemberObject {
    Value(LocalTypeId),
    Module(ModuleId),
    BaseType(BaseType),
    Type(LocalTypeId),
    Trait(ModuleId, TraitId),
}

struct CompletionHooks<'a> {
    variables: &'a mut Vec<(String, VarId)>,
    /// the static scope the completion was requested in
    target_scope: ScopeId,
    /// where non-local completions will finally be gathered from
    completion_context: CompletionContext,
    target_offset: u32,
    ast: &'a Ast,
    done: bool,
    /// stores the left expr in case a MemberAccess should be completed. Saves the left expr and
    /// expected type of the member access
    completing_member_access: Option<(ExprId, LocalTypeId)>,
}
impl<'a> compiler::check::Hooks for CompletionHooks<'a> {
    fn on_check_expr(
        &mut self,
        expr: ExprId,
        _hir: &mut HIRBuilder,
        scope: &mut compiler::compiler::LocalScope,
        expected: LocalTypeId,
        _return_ty: LocalTypeId,
        _noreturn: &mut bool,
    ) {
        if self.done {
            return;
        }
        if let Some((left, member_expected)) = &mut self.completing_member_access {
            if expr == *left {
                self.completion_context = CompletionContext::MemberAccess {
                    object: MemberObject::Value(expected),
                    expected_ty: *member_expected,
                };
                self.completing_member_access = None;
                self.done = true;
            }
            return;
        }
        if let Expr::MemberAccess { left, name, .. } = self.ast[expr]
            && (name.start.saturating_sub(1)..name.end).contains(&self.target_offset)
        {
            // now start looking for the left expr being checked
            self.completing_member_access = Some((left, expected));
            return;
        }
        if self.ast[expr].span(self.ast).start < self.target_offset {
            return;
        }
        self.done = true;
        self.complete_in_scope(scope);
    }

    fn on_exit_scope(&mut self, scope: &mut compiler::compiler::LocalScope, hir: &mut HIRBuilder) {
        if let CompletionContext::MemberAccess {
            object: MemberObject::Value(ty),
            expected_ty,
        } = self.completion_context
        {
            // Now that we are done checking a member access, see if the TypeInfo was some kind of
            // item and not a normal type. This information will be lost after finishing the types
            // so we check here and not after the function typecheck completes
            let new_object = match hir.types[ty] {
                TypeInfo::ModuleItem(id) => MemberObject::Module(id),
                TypeInfo::BaseTypeItem(id) => MemberObject::BaseType(id),
                TypeInfo::TypeItem(id) => MemberObject::Type(id),
                TypeInfo::TraitItem { module, id } => MemberObject::Trait(module, id),
                _ => return,
            };
            self.completion_context = CompletionContext::MemberAccess {
                object: new_object,
                expected_ty,
            };
        }
        if !self.done && scope.static_scope.is_some_and(|s| s == self.target_scope) {
            self.complete_in_scope(scope);
        }
    }
}
impl<'a> CompletionHooks<'a> {
    fn complete_in_scope(&mut self, scope: &mut LocalScope) {
        let mut current = &*scope;
        loop {
            self.variables.extend(
                current
                    .variables
                    .iter()
                    .map(|(name, var)| (name.clone().into_string(), *var)),
            );
            match &current.parent {
                compiler::compiler::LocalScopeParent::Some(parent) => {
                    current = parent;
                }
                // TODO: handle closed over scopes for completions
                compiler::compiler::LocalScopeParent::ClosedOver { .. }
                | compiler::compiler::LocalScopeParent::None => break,
            }
        }
    }
}

fn const_completions(
    completions: &mut Vec<CompletionItem>,
    compiler: &Compiler,
    module: ModuleId,
    scope: ScopeId,
    external: bool,
    expected: Option<Type>,
) {
    let mut current = (module, scope);
    let mut visit_prelude = !external;
    loop {
        let ast = compiler.get_module_ast(current.0);
        let scope = &ast[current.1];
        for (name, def) in &scope.definitions {
            if external && matches!(def, ast::Definition::Use { .. }) {
                // don't show uses when completing for an external module
                continue;
            }
            let def = compiler.resolve_in_scope(current.0, current.1, name, ModuleSpan::MISSING);
            let kind = match def {
                Def::Invalid => CompletionItemKind::Constant,
                Def::Function(module, id) => {
                    completions.push(function_completion(compiler, name, module, id, expected));
                    continue;
                }
                Def::BaseType(base) => base_kind(compiler, base),
                Def::Type(ty) => match compiler.types.lookup(ty) {
                    TypeFull::Instance(base, _) => base_kind(compiler, base),
                    TypeFull::Tuple { .. } => CompletionItemKind::Struct,
                    TypeFull::FunctionItem { .. } => CompletionItemKind::Function,
                    TypeFull::Generic { .. } => CompletionItemKind::TypeParameter,
                    TypeFull::Const(_) => CompletionItemKind::Constant,
                },
                Def::Trait(_, _) => CompletionItemKind::Interface,
                Def::ConstValue(_) => CompletionItemKind::Constant,
                Def::Module(_) => CompletionItemKind::Module,
                Def::Global(_, _) => CompletionItemKind::Variable,
            };
            completions.push(CompletionItem {
                label: name.to_owned(),
                kind: Some(kind),
                detail: None,
                labelDetails: None,
                documentation: None,
                preselect: false,
            });
        }

        match scope.parent {
            Some(parent) => current.1 = parent,
            None => {
                if visit_prelude {
                    let prelude = compiler::compiler::builtins::get_prelude(compiler);
                    current = (
                        prelude,
                        compiler.get_module_ast(prelude).top_level_scope_id(),
                    );
                    visit_prelude = false;
                } else {
                    break;
                }
            }
        }
    }
}

fn base_kind(compiler: &Compiler, base: compiler::types::BaseType) -> CompletionItemKind {
    match compiler.get_base_type_def(base).def {
        ResolvedTypeContent::Builtin(_) | ResolvedTypeContent::Struct(_) => {
            CompletionItemKind::Struct
        }
        ResolvedTypeContent::Enum(_) => CompletionItemKind::Enum,
    }
}

fn value_member_access_completion(
    completions: &mut Vec<CompletionItem>,
    compiler: &Compiler,
    signature: &Signature,
    left: Type,
    expected: Type,
) {
    let mut on_member = |name: &str, ty: Type| {
        let type_display = compiler.types.display(ty, &signature.generics);
        completions.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::Field),
            detail: Some(format!("{type_display}")),
            labelDetails: Some(CompletionItemLabelDetails {
                detail: Some(format!(": {type_display}")),
                description: None,
            }),
            documentation: None,
            preselect: ty == expected,
        });
    };

    match compiler.types.lookup(left) {
        TypeFull::Instance(base, ty_generics) => {
            let def = compiler.get_base_type_def(base);
            match &def.def {
                ResolvedTypeContent::Builtin(_) | ResolvedTypeContent::Enum(_) => {}
                ResolvedTypeContent::Struct(struct_def) => {
                    for (name, ty, _default) in &struct_def.named_fields {
                        let ty = compiler.types.instantiate(*ty, ty_generics);
                        on_member(name, ty);
                    }
                }
            }
            for (name, function) in &def.methods {
                completions.push(function_completion(
                    compiler,
                    name,
                    def.module,
                    *function,
                    Some(expected),
                ));
            }
        }
        TypeFull::Tuple {
            members,
            named_members,
        } => {
            for (ty, i) in members.iter().zip(0..) {
                on_member(&format!("{i}"), *ty);
            }
            for (name, ty) in named_members {
                on_member(name, *ty);
            }
        }
        _ => {}
    }
}

fn function_completion(
    compiler: &Compiler,
    name: &str,
    module: ModuleId,
    function: FunctionId,
    expected: Option<Type>,
) -> CompletionItem {
    let signature = compiler.get_signature(module, function);
    let preselect = expected.is_some_and(|expected| {
        // Check if the return type of the function matches the expected type, considering any
        // possible generics instantiation.
        // This function is in `traits` right now but should be moved out and probably named better
        traits::match_instance(
            expected,
            signature.return_type,
            &compiler.types,
            &mut vec![None; signature.generics.count() as usize],
        )
    });
    CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::Function),
        detail: None,
        labelDetails: None,
        documentation: Some(MarkupContent::markdown(format!(
            "```eye\n{name} :: {}\n```",
            display_signature(compiler, signature)
        ))),
        preselect,
    }
}

fn display_signature(compiler: &Compiler, signature: &Signature) -> String {
    use std::fmt::Write;

    let mut s = "fn(".to_owned();
    let mut first = true;
    for (name, ty) in &signature.params {
        if first {
            first = false;
        } else {
            s.push_str(", ");
        }

        write!(
            s,
            "{name} {}",
            compiler.types.display(*ty, &signature.generics)
        )
        .unwrap();
    }
    for (name, ty, _default) in &signature.named_params {
        if first {
            first = false;
        } else {
            s.push_str(", ");
        }

        write!(
            s,
            "{name} {} = ...",
            compiler.types.display(*ty, &signature.generics)
        )
        .unwrap();
    }
    write!(
        s,
        ") -> {}",
        compiler
            .types
            .display(signature.return_type, &signature.generics)
    )
    .unwrap();
    s
}
