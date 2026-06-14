mod complete;
mod hover;

use std::path::Path;

use compiler::{
    Compiler, Def, ModuleSpan,
    check::ProjectErrors,
    compiler::{LocalItem, LocalScope},
    hir::HIRBuilder,
    types::{BaseType, TypeFull},
    typing::LocalTypeId,
};
use error::span::TSpan;
use parser::ast::{self, Ast, Expr, ExprId, ModuleId};
use serde_json::Value;

use crate::{
    ResponseError,
    lsp::{
        Lsp,
        find_in_ast::{FoundType, ScopeContext},
    },
    types::{
        Location, Range, TextDocumentContentChangeEvent, TextEdit,
        notification::{
            DidChangeTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
        },
        request::{
            CompletionParams, DefinitionParams, DocumentFormattingParams, HoverParams, Request,
        },
    },
};

impl Lsp {
    pub fn handle_notification(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(), ResponseError> {
        match method {
            "textDocument/didOpen" => self.on_notification(Self::did_open, params),
            "textDocument/didChange" => self.on_notification(Self::did_change, params),
            "textDocument/didSave" => self.on_notification(Self::did_save, params),
            _ => {
                tracing::info!("Unhandled notification: {method} {params}");
                Ok(())
            }
        }
    }

    pub fn handle_request(&mut self, method: &str, params: Value) -> Result<Value, ResponseError> {
        match method {
            HoverParams::METHOD => self.on_request(Self::hover, params),
            CompletionParams::METHOD => self.on_request(Self::complete, params),
            DefinitionParams::METHOD => self.on_request(Self::definition, params),
            DocumentFormattingParams::METHOD => self.on_request(Self::formatting, params),
            _ => {
                tracing::info!("Unhandled request {method} {params}");
                Err(ResponseError {
                    code: crate::ERROR_FAILED,
                    message: "unsupported request".to_owned(),
                    data: Value::Null,
                })
            }
        }
    }

    pub fn did_open(&mut self, open: DidOpenTextDocumentParams) {
        if let Some((project, module)) = self.find_project_of_uri(&open.textDocument.uri) {
            tracing::info!(
                "Opened file {} is part of existing project {} at {}",
                open.textDocument.uri.path().display(),
                self.compiler.get_project(project).name,
                self.compiler
                    .get_project(project)
                    .root
                    .as_ref()
                    .unwrap()
                    .display(),
            );
            self.update_module(
                project,
                module,
                open.textDocument.text.into_boxed_str(),
                open.textDocument.version,
            );
            return;
        }
        let path = open.textDocument.uri.path();
        let project_path = find_project_path(path);
        let name = if project_path.is_file() {
            project_path.file_stem()
        } else {
            project_path.file_name()
        }
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
        tracing::info!(
            "Opened file {} is part of new project {name} at {}",
            path.display(),
            project_path.display()
        );
        match self.compiler.add_project(
            name.to_owned(),
            project_path.to_path_buf(),
            self.std.into_iter().collect(),
        ) {
            Ok(id) => {
                self.projects.push(id);
                let Some(module) = self.module_by_path(id, open.textDocument.uri.path()) else {
                    return;
                };
                self.update_module(
                    id,
                    module,
                    open.textDocument.text.into_boxed_str(),
                    open.textDocument.version,
                );
            }
            Err(err) => tracing::error!("Failed to add new project: {err:?}"),
        }
    }

    pub fn did_change(&mut self, change_params: DidChangeTextDocumentParams) {
        let Some((project, module_id)) =
            self.find_project_of_uri(&change_params.textDocument.identifier.uri)
        else {
            tracing::warn!(
                "Could not find file for change at {:?}",
                change_params.textDocument.identifier
            );
            return;
        };

        let module = &mut self.compiler.modules[module_id.idx()];
        // assumes the module was parsed already, which should be the case after did_open
        let parsed = module.parsed.get_mut().unwrap();
        let mut src = parsed.ast.src().to_owned();
        for change in change_params.contentChanges {
            match change {
                TextDocumentContentChangeEvent::TextDocumentContentChangePartial {
                    range,
                    text,
                } => {
                    let span = range.to_span(&src);
                    src.replace_range(span.range(), &text);
                }
                TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument {
                    ..
                } => {
                    // requested partial changes
                    unreachable!()
                }
            }
        }
        tracing::debug!(target: "did_change", "{:?} {project:?}:{module_id:?}:\n{src}", change_params.textDocument.identifier.uri);
        self.update_module(
            project,
            module_id,
            src.into_boxed_str(),
            change_params.textDocument.version,
        );
    }

    pub fn did_save(&mut self, save: DidSaveTextDocumentParams) {
        let Some((project, _)) = self.find_project_of_uri(&save.textDocument.uri) else {
            tracing::warn!(
                "Got save notification for file that is not part of any project: {:?}",
                save.textDocument.uri.path().display()
            );
            return;
        };

        self.invalidate_project_and_recheck(project, ProjectErrors::new());
    }

    pub fn definition(&mut self, params: DefinitionParams) -> Option<Location> {
        fn function(
            compiler: &Compiler,
            module: ModuleId,
            id: ast::FunctionId,
        ) -> (ModuleId, TSpan) {
            let ast = compiler.get_module_ast(module);
            let func = &ast[id];
            if !func.associated_name.is_empty() {
                return (module, func.associated_name);
            }
            let span = ast[func.scope].span;
            (module, span)
        }
        fn base_type(compiler: &Compiler, id: BaseType) -> Option<(ModuleId, TSpan)> {
            let def = compiler.types.get_base(id);
            if def.module == ModuleId::MISSING {
                // builtin type with no definition
                return None;
            }
            let ast = compiler.get_module_ast(def.module);
            let name_span = compiler.types.get_base(id).name_span;
            let span = if name_span == TSpan::MISSING {
                ast[def.id].span(ast.scopes())
            } else {
                name_span
            };
            Some((def.module, span))
        }

        fn goto_type_definition(
            compiler: &Compiler,
            ty: compiler::Type,
        ) -> Option<(ModuleId, TSpan)> {
            match compiler.types.lookup(ty) {
                TypeFull::Instance(id, _) => base_type(compiler, id),
                TypeFull::FunctionItem {
                    function: (module, id),
                    generics: _,
                } => Some(function(compiler, module, id)),
                TypeFull::Tuple { .. } => None,
                TypeFull::Generic(_) => None,
                TypeFull::Const(_) => None,
            }
        }

        let (module, _, found) = self.find_document_position(&params.position)?;
        let context = self.find_context_for_scope(module, found.scope);
        let (module, span) = 'find_span: {
            match found.ty {
                FoundType::Ident | FoundType::Member => {
                    let ast = self.compiler.get_module_ast(module);
                    let def = if let ScopeContext::Function(id) = context {
                        let mut hooks = FindHooks::new(found.span, ast, &self.compiler, found.ty);
                        let checked =
                            compiler::check::function(&self.compiler, module, id, &mut hooks);
                        tracing::info!(
                            "Found for definition handler: {:?} {:?}",
                            hooks.local_item,
                            hooks.ty
                        );
                        match hooks.local_item {
                            Some(LocalItem::Def(def)) => def,
                            Some(LocalItem::Var(_)) => return None, // TODO: goto variable definition
                            Some(LocalItem::Invalid) | None => {
                                let ty = hooks.ty?;
                                let ty = checked[ty];
                                break 'find_span goto_type_definition(&self.compiler, ty)?;
                            }
                        }
                    } else {
                        if matches!(found.ty, FoundType::Member) {
                            // member not findable outside of function for now
                            return None;
                        }
                        let name = &ast.src()[found.span.range()];
                        self.compiler.resolve_in_scope(
                            module,
                            found.scope,
                            name,
                            ModuleSpan::MISSING,
                        )
                    };
                    match def {
                        Def::Invalid => return None,
                        Def::Function(module, id) => function(&self.compiler, module, id),
                        Def::Module(module) => (module, TSpan::EMPTY),
                        Def::Global(module, global) => {
                            let ast = self.compiler.get_module_ast(module);
                            (module, ast[ast[global].scope].span)
                        }
                        Def::Trait(module, trait_id) => {
                            let ast = self.compiler.get_module_ast(module);
                            (module, ast[ast[trait_id].scope].span)
                        }
                        Def::BaseType(id) => base_type(&self.compiler, id)?,
                        Def::Type(ty) => goto_type_definition(&self.compiler, ty)?,
                        Def::ConstValue(_) => return None,
                    }
                }
                _ => return None,
            }
        };
        let ast = self.compiler.get_module_ast(module);
        let range = Range::from_span(span, ast.src());
        Some(Location {
            uri: self.uri_from_module(module),
            range,
        })
    }

    pub fn formatting(&mut self, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        let Ok(src) = std::fs::read_to_string(params.textDocument.uri.path()) else {
            return None;
        };
        let len = src.len().try_into().ok()?;
        let range = Range::from_span(TSpan::new(0, len), &src);
        let (formatted, errors) = format::format(src.into_boxed_str());
        if errors.error_count() > 0 {
            return None;
        }
        Some(vec![TextEdit {
            range,
            newText: formatted,
        }])
    }
}

fn find_project_path(file: &Path) -> &Path {
    let Some(mut dir) = file.parent() else {
        return file;
    };
    loop {
        if dir.join("main.eye").exists() {
            return dir;
        } else if dir.join("mod.eye").exists() {
            let Some(parent) = dir.parent() else {
                return file;
            };
            dir = parent;
        } else {
            return file;
        }
    }
}

struct FindHooks<'a> {
    span: TSpan,
    ast: &'a Ast,
    compiler: &'a Compiler,
    find: FoundType,
    local_item: Option<LocalItem>,
    ty: Option<LocalTypeId>,
}
impl<'a> FindHooks<'a> {
    pub fn new(span: TSpan, ast: &'a Ast, compiler: &'a Compiler, find: FoundType) -> Self {
        Self {
            span,
            ast,
            compiler,
            find,
            local_item: None,
            ty: None,
        }
    }

    fn handle_found_expr(
        &mut self,
        _expr: ExprId,
        hir: &mut HIRBuilder,
        scope: &mut LocalScope,
        ty: LocalTypeId,
        is_pattern: bool,
    ) {
        let name = &self.ast.src()[self.span.range()];
        self.ty = Some(ty);
        if matches!(self.find, FoundType::Ident) && !is_pattern {
            // TODO: this should probably not emit errors
            let item = scope.resolve(name, self.span, self.compiler, &mut hir.vars);
            self.local_item = Some(item);
        }
    }
}
impl<'a> compiler::check::Hooks for FindHooks<'a> {
    fn on_check_expr(
        &mut self,
        expr: parser::ast::ExprId,
        hir: &mut HIRBuilder,
        scope: &mut compiler::compiler::LocalScope,
        ty: compiler::typing::LocalTypeId,
        _return_ty: compiler::typing::LocalTypeId,
        _noreturn: &mut bool,
    ) {
        // TODO: for members, check the left type immediately after checking (similar to what
        // happens in the complete handler) to find method items etc.
        if matches!(self.find, FoundType::Member)
            && let Expr::MemberAccess { name, .. } = self.ast[expr]
            && name == self.span
        {
        } else if self.ast[expr].span(self.ast) != self.span {
            return;
        }
        self.handle_found_expr(expr, hir, scope, ty, false);
    }

    fn on_checked_lvalue(
        &mut self,
        expr: parser::ast::ExprId,
        hir: &mut HIRBuilder,
        scope: &mut compiler::compiler::LocalScope,
        ty: LocalTypeId,
    ) {
        if self.ast[expr].span(self.ast) != self.span {
            return;
        }
        self.handle_found_expr(expr, hir, scope, ty, false);
    }

    fn on_check_pattern(
        &mut self,
        expr: ExprId,
        hir: &mut HIRBuilder,
        scope: &mut LocalScope,
        ty: LocalTypeId,
    ) {
        if self.ast[expr].span(self.ast) != self.span {
            return;
        }
        self.handle_found_expr(expr, hir, scope, ty, true);
    }
}
