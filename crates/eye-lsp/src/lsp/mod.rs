use std::{
    cell::OnceCell,
    path::{Component, Path, PathBuf},
};

use compiler::{
    Compiler, Def, ModuleSpan, ProjectId, check::ProjectErrors, compiler::ParsedModule,
};
use dmap::DHashMap;
use parser::ast::{ModuleId, ScopeId};
use serde_json::Value;

use crate::{
    ResponseError,
    lsp::find_in_ast::{Found, ScopeContext},
    send_notification,
    types::{self, Diagnostic, TextDocumentPositionParams, Uri, request::Request},
};

mod find_in_ast;
mod handlers;

pub struct Lsp {
    compiler: Compiler,
    projects: Vec<ProjectId>,
    std: Option<ProjectId>,
}
impl Lsp {
    pub fn new(initialize: types::Initialize) -> Self {
        let mut compiler = Compiler::new();
        let project_path = initialize.root_uri.map_or_else(
            || initialize.root_path.as_deref().map(PathBuf::from),
            |uri| Some(uri.path().to_path_buf()),
        );
        let std = match compiler.add_project("std".to_owned(), compiler::std_path::find(), vec![]) {
            Ok(std) => {
                compiler.resolve_builtins(std);
                Some(std)
            }
            Err(err) => {
                tracing::error!("Failed to add std library: {err:?}");
                None
            }
        };
        let project = project_path.and_then(|path| {
            let name = path.components().next_back().map_or_else(
                || "<unnamed project>".to_owned(),
                |s| s.as_os_str().to_string_lossy().into_owned(),
            );
            if !path.join("main.eye").exists() {
                return None;
            }
            compiler
                .add_project(name, path, std.into_iter().collect())
                .ok()
        });
        let mut lsp = Self {
            compiler,
            projects: project.into_iter().collect(),
            std,
        };
        lsp.update_diagnostics(None);

        lsp
    }

    fn on_request<F: FnMut(&mut Self, R) -> R::Response, R: Request>(
        &mut self,
        mut handler: F,
        params: Value,
    ) -> Result<Value, ResponseError> {
        Ok(serde_json::to_value(handler(self, serde_json::from_value(params)?)).unwrap())
    }

    fn on_notification<F: FnMut(&mut Self, N), N: types::notification::Notification>(
        &mut self,
        mut handler: F,
        params: Value,
    ) -> Result<(), ResponseError> {
        handler(self, serde_json::from_value(params)?);
        Ok(())
    }

    pub fn find_project_of_uri(&self, uri: &Uri) -> Option<(ProjectId, ModuleId)> {
        // TODO: doesn't handle adding new files to projects
        let path = uri.path();
        self.projects
            .iter()
            .copied()
            .find_map(|project| Some(project).zip(self.module_by_path(project, path)))
    }

    pub fn module_by_path(&self, project: ProjectId, path: &Path) -> Option<ModuleId> {
        let diff = pathdiff::diff_paths(
            path,
            self.compiler.get_project(project).root.as_ref().unwrap(),
        )?;
        if diff.components().any(|c| c == Component::ParentDir) {
            return None;
        }
        let mut module = self.compiler.get_project(project).root_module;
        for component in diff.components() {
            match component {
                Component::Normal(s) if s == "main.eye" => return Some(module),
                Component::Normal(s) => {
                    let path = Path::new(s);
                    if path.extension().is_some_and(|ext| ext != "eye") {
                        return None;
                    }
                    let name = path.file_stem().and_then(|name| name.to_str())?;
                    let Def::Module(new_module) =
                        self.compiler
                            .resolve_in_module(module, name, ModuleSpan::MISSING)
                    else {
                        // TODO: handle new modules that are not yet present on disk here
                        return None;
                    };
                    module = new_module;
                }
                _ => return None,
            }
        }
        Some(module)
    }

    pub fn uri_from_module(&self, module: ModuleId) -> Uri {
        Uri::from_path(self.compiler.modules[module.idx()].storage.path().unwrap())
    }

    pub fn find_module_of_uri(&mut self, uri: &Uri) -> Option<ModuleId> {
        let path = uri.path();
        self.projects.iter().copied().find_map(|project_id| {
            let project = self.compiler.get_project(project_id);
            if path == project.root.as_ref().unwrap() {
                return Some(project.root_module);
            }
            let diff = pathdiff::diff_paths(path, project.root.as_ref().unwrap())?;
            if diff.components().any(|c| c == Component::ParentDir) {
                tracing::debug!("Project at {:?} has no relative path", project.root);
                return None;
            }
            let mut module = project.root_module;
            let mut components = diff.components();
            let file_name = components.next_back()?;
            let Component::Normal(file_name) = file_name else {
                tracing::debug!("not normal");
                return None;
            };
            tracing::debug!("looking for module in project");
            let final_name = file_name.to_str()?.strip_suffix(".eye")?;
            for component in components {
                let Component::Normal(name) = component else {
                    continue;
                };
                let name = name.to_str()?;
                let Def::Module(new_module) =
                    self.compiler
                        .resolve_in_module(module, name, ModuleSpan::MISSING)
                else {
                    tracing::debug!("not a module resolved");
                    return None;
                };
                module = new_module;
            }
            Some(match final_name {
                "mod" | "main" => module,
                _ => {
                    let Def::Module(module) =
                        self.compiler
                            .resolve_in_module(module, final_name, ModuleSpan::MISSING)
                    else {
                        tracing::debug!("not a module resolvedin final");
                        return None;
                    };
                    module
                }
            })
        })
    }

    pub fn find_document_position(
        &mut self,
        position: &TextDocumentPositionParams,
    ) -> Option<(ModuleId, u32, Found)> {
        let Some(module) = self.find_module_of_uri(&position.textDocument.uri) else {
            tracing::debug!("Module not found for {:?}", position.textDocument.uri);
            return None;
        };
        let ast = self.compiler.get_module_ast(module);
        let offset = position.position.to_offset(ast.src());
        Some((module, offset, find_in_ast::find(ast, offset)))
    }

    pub fn find_context_for_scope(&mut self, module: ModuleId, scope: ScopeId) -> ScopeContext {
        let ast = self.compiler.get_module_ast(module);
        let mut context_scopes = DHashMap::default();
        context_scopes.insert(ast.top_level_scope_id(), ScopeContext::TopLevel);
        for function in ast.function_ids() {
            context_scopes.insert(ast[function].scope, ScopeContext::Function(function));
        }
        let mut current = scope;
        loop {
            if let Some(context) = context_scopes.get(&current) {
                return *context;
            }
            let Some(parent) = ast[current].parent else {
                tracing::warn!("Scope didn't have a parent during context search");
                return ScopeContext::TopLevel;
            };
            current = parent;
        }
    }

    pub fn update_module(
        &mut self,
        project: ProjectId,
        module: ModuleId,
        new_text: Box<str>,
        version: i32,
    ) {
        let (definitions, child_modules) = self.compiler.module_pre_definitions(module);
        let project_errors = ProjectErrors::new();
        let mut errors = error::Errors::new();
        let new_ast = parser::parse(new_text, &mut errors, definitions);
        project_errors.add_module(module, errors);
        self.compiler.modules[module.idx()].parsed = OnceCell::from(ParsedModule {
            symbols: compiler::compiler::ModuleSymbols::empty(&new_ast),
            ast: new_ast,
            version: Some(version),
            child_modules,
        });

        self.invalidate_project_and_recheck(project, project_errors);
    }

    pub fn invalidate_project_and_recheck(
        &mut self,
        project: ProjectId,
        new_errors: ProjectErrors,
    ) {
        // find all invalidated projects
        let mut invalidated_projects = dmap::new_set();
        invalidated_projects.insert(project);

        loop {
            let mut changed = false;
            for project in self.compiler.project_ids() {
                if invalidated_projects.contains(&project) {
                    continue;
                }
                if self
                    .compiler
                    .get_project(project)
                    .dependencies
                    .iter()
                    .any(|dep| invalidated_projects.contains(dep))
                {
                    invalidated_projects.insert(project);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        for module in self.compiler.module_ids() {
            let project = self.compiler.modules[module.idx()].project;
            if invalidated_projects.contains(&project)
                && let Some(parsed) = self.compiler.modules[module.idx()].parsed.get_mut()
            {
                parsed.symbols = compiler::compiler::ModuleSymbols::empty(&parsed.ast);
            }
        }

        self.update_diagnostics(Some((project, new_errors)));
    }

    pub fn update_diagnostics(
        &mut self,
        mut new_project_errors: Option<(ProjectId, ProjectErrors)>,
    ) {
        tracing::debug!("Updating diagnostics for {} projects", self.projects.len());
        for &project in &self.projects {
            self.compiler.errors = if new_project_errors
                .as_ref()
                .is_some_and(|&(id, _)| id == project)
            {
                new_project_errors.take().unwrap().1
            } else {
                ProjectErrors::new()
            };
            self.compiler.check_complete_project(project);
            let errors = std::mem::replace(&mut self.compiler.errors, ProjectErrors::new());
            for (&module, errors) in errors.by_file.borrow().iter() {
                let parsed = self.compiler.get_parsed_module(module);
                let version = parsed.version;
                let src = parsed.ast.src();
                let mut diagnostics = Vec::new();
                let mut emit = |errors: &[error::CompileError], severity| {
                    for error in errors {
                        // PERF: more efficient position calculation
                        let start = error::calculate_position(src, error.span.start);
                        let end = error::calculate_position(src, error.span.end);
                        let mut message = error.err.conclusion().to_owned();
                        if let Some(details) = error.err.details() {
                            message.push('\n');
                            message.push_str(&details);
                        }
                        diagnostics.push(Diagnostic {
                            range: types::Range {
                                start: types::Position {
                                    line: start.line,
                                    character: start.column,
                                },
                                end: types::Position {
                                    line: end.line,
                                    character: end.column,
                                },
                            },
                            severity: Some(severity),
                            code: None,
                            codeDescription: None,
                            source: None,
                            message,
                        });
                    }
                };
                emit(&errors.errors, types::DiagnosticSeverity::Error);
                emit(&errors.warnings, types::DiagnosticSeverity::Warning);

                let params = types::notification::PublishDiagnosticsParams {
                    uri: Uri::from_path(
                        self.compiler.modules[module.idx()].storage.path().unwrap(),
                    ),
                    version,
                    diagnostics,
                };
                if !params.diagnostics.is_empty() {
                    tracing::info!("Emitting {} errors", params.diagnostics.len());
                }
                send_notification(params);
            }
        }
    }
}
