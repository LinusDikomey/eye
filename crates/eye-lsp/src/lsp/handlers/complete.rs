use compiler::{
    Compiler, Def, ModuleSpan, Type,
    compiler::{BodyOrTypes, LocalScope, ResolvedTypeContent, Signature, VarId},
    hir::HIRBuilder,
    types::TypeFull,
    typing::LocalTypeId,
};
use parser::ast::{Ast, Expr, ExprId, FunctionId, ModuleId, ScopeId};

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
                    completing_member_access: None,
                };
                // TODO: currently this doesn't properly handle closures!
                let checked =
                    compiler::check::function(&self.compiler, module, function_id, &mut hooks);
                if let BodyOrTypes::Body(hir) = &checked.body_or_types {
                    let signature = self.compiler.get_signature(module, function_id);
                    if let Some((_, expected, left_ty)) = hooks.completing_member_access {
                        // member access completion
                        debug_assert!(hooks.variables.is_empty());
                        let expected = hir[expected];
                        let left_ty = hir[left_ty];
                        member_access_completion(
                            &mut completions,
                            &self.compiler,
                            signature,
                            left_ty,
                            expected,
                        );
                    } else {
                        const_completions(&mut completions, &self.compiler, module, found.scope);

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
    /// stores the left expr in case a MemberAccess should be completed. Saves the lhs and expected type of both the member and the lhs
    completing_member_access: Option<(ExprId, LocalTypeId, LocalTypeId)>,
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
        if let Some((left, _, left_ty)) = &mut self.completing_member_access {
            if expr == *left {
                *left_ty = expected;
                self.done = true;
            }
            return;
        }
        if let Expr::MemberAccess { left, name, .. } = self.ast[expr]
            && (name.start.saturating_sub(1)..name.end).contains(&self.target_offset)
        {
            // now start looking for the left expr being checked
            self.completing_member_access = Some((left, expected, LocalTypeId::MISSING));
            return;
        }
        if self.ast[expr].span(self.ast).start < self.target_offset {
            return;
        }
        self.done = true;
        self.complete_in_scope(scope);
    }

    fn on_exit_scope(&mut self, scope: &mut compiler::compiler::LocalScope) {
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
) {
    let mut current = (module, scope);
    let mut in_prelude = false;
    loop {
        let ast = compiler.get_module_ast(current.0);
        let scope = &ast[current.1];
        for name in scope.definitions.keys() {
            let def = compiler.resolve_in_scope(current.0, current.1, name, ModuleSpan::MISSING);
            let kind = match def {
                Def::Invalid => CompletionItemKind::Constant,
                Def::Function(module, id) => {
                    completions.push(function_completion(compiler, name, module, id));
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
            });
        }

        match scope.parent {
            Some(parent) => current.1 = parent,
            None => {
                if !in_prelude {
                    let prelude = compiler::compiler::builtins::get_prelude(compiler);
                    current = (
                        prelude,
                        compiler.get_module_ast(prelude).top_level_scope_id(),
                    );
                    in_prelude = true;
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

fn member_access_completion(
    completions: &mut Vec<CompletionItem>,
    compiler: &Compiler,
    signature: &Signature,
    left: Type,
    _expected: Type,
) {
    let mut on_member = |name: &str, ty: Type| {
        let ty = compiler.types.display(ty, &signature.generics);
        completions.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::Field),
            detail: Some(format!("{ty}")),
            labelDetails: Some(CompletionItemLabelDetails {
                detail: Some(format!(": {ty}")),
                description: None,
            }),
            documentation: None,
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
                completions.push(function_completion(compiler, name, def.module, *function));
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
) -> CompletionItem {
    let signature = compiler.get_signature(module, function);
    CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::Function),
        detail: None,
        labelDetails: None,
        documentation: Some(MarkupContent::markdown(format!(
            "```eye\n{name} :: {}\n```",
            display_signature(compiler, signature)
        ))),
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
