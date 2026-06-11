use compiler::{
    Def, ModuleSpan,
    compiler::{BodyOrTypes, LocalScope, VarId},
    hir::HIRBuilder,
    typing::LocalTypeId,
};
use parser::ast::{Ast, ScopeId};

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
        let mut completions = Vec::new();

        let mut current = (module, found.scope);
        let mut in_prelude = false;
        loop {
            let ast = self.compiler.get_module_ast(current.0);
            let scope = &ast[current.1];
            for name in scope.definitions.keys() {
                let def =
                    self.compiler
                        .resolve_in_scope(current.0, current.1, name, ModuleSpan::MISSING);
                let kind = match def {
                    Def::Invalid => CompletionItemKind::Constant,
                    Def::Function(_, _) => CompletionItemKind::Function,
                    Def::BaseType(_) => CompletionItemKind::TypeParameter,
                    Def::Type(_) => CompletionItemKind::Struct, // there is no Type Kind unfortunately
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
                });
            }

            match scope.parent {
                Some(parent) => current.1 = parent,
                None => {
                    if !in_prelude {
                        let prelude = compiler::compiler::builtins::get_prelude(&self.compiler);
                        current = (
                            prelude,
                            self.compiler.get_module_ast(prelude).top_level_scope_id(),
                        );
                        in_prelude = true;
                    } else {
                        break;
                    }
                }
            }
        }
        let context = self.find_context_for_scope(module, found.scope);
        match context {
            ScopeContext::TopLevel => completions.push(CompletionItem {
                label: format!("completion_toplevel {found:?}"),
                kind: None,
                detail: None,
                labelDetails: None,
                documentation: None,
            }),
            ScopeContext::Function(function_id) => {
                let ast = self.compiler.get_module_ast(module);
                let mut variables = Vec::new();
                let mut hooks = CompletionHooks {
                    variables: &mut variables,
                    target_offset: offset,
                    target_scope: found.scope,
                    ast,
                    done: false,
                };
                // TODO: currently this doesn't properly handle closures!
                let checked =
                    compiler::check::function(&self.compiler, module, function_id, &mut hooks);
                // TODO: remove this completion that should help with debugging
                if !hooks.done {
                    completions.push(CompletionItem {
                        label: "debug_completion_not_done".into(),
                        kind: None,
                        detail: None,
                        labelDetails: None,
                        documentation: None,
                    });
                }
                if let BodyOrTypes::Body(hir) = &checked.body_or_types {
                    let signature = self.compiler.get_signature(module, function_id);
                    for (name, variable) in variables {
                        let ty = hir[hir.vars[variable.idx()].ty()];
                        let ty = self.compiler.types.display(ty, &signature.generics);
                        completions.push(CompletionItem {
                            label: name,
                            kind: Some(CompletionItemKind::Variable),
                            detail: None,
                            labelDetails: Some(CompletionItemLabelDetails {
                                description: Some(format!(": {ty}")),
                                detail: None,
                            }),
                            documentation: Some(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: "Documentation for completion will go here\n\nCode block test:\n```eye\nexample :: fn(x i32) {}\n```".to_string(),
                            }),
                        });
                    }
                }
            }
        }

        tracing::info!("Returning {} completions", completions.len());
        completions
    }
}

struct CompletionHooks<'a> {
    variables: &'a mut Vec<(String, VarId)>,
    target_scope: ScopeId,
    target_offset: u32,
    ast: &'a Ast,
    done: bool,
}
impl<'a> compiler::check::Hooks for CompletionHooks<'a> {
    fn on_check_expr(
        &mut self,
        expr: parser::ast::ExprId,
        _hir: &mut HIRBuilder,
        scope: &mut compiler::compiler::LocalScope,
        _expected: LocalTypeId,
        _return_ty: LocalTypeId,
        _noreturn: &mut bool,
    ) {
        if self.done || self.ast[expr].span(self.ast).start < self.target_offset {
            return;
        }
        self.complete(scope);
    }

    fn on_exit_scope(&mut self, scope: &mut compiler::compiler::LocalScope) {
        if !self.done && scope.static_scope.is_some_and(|s| s == self.target_scope) {
            self.complete(scope);
        }
    }
}
impl<'a> CompletionHooks<'a> {
    fn complete(&mut self, scope: &mut LocalScope) {
        self.done = true;
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
