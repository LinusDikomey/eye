use std::fmt::Write;

use compiler::{
    ConstValue, Def, ModuleSpan, Type,
    compiler::{BodyOrTypes, Generics, LocalItem, VarId},
};

use crate::{
    lsp::{
        Lsp,
        find_in_ast::{FoundType, ScopeContext},
        handlers::FindHooks,
    },
    types::{
        Range,
        request::{Hover, HoverContents, HoverParams, MarkedString},
    },
};

fn text(s: impl Into<String>) -> MarkedString {
    MarkedString::String(s.into())
}

fn code(s: impl Into<String>) -> MarkedString {
    MarkedString::Code {
        language: "eye".to_owned(),
        value: s.into(),
    }
}

impl Lsp {
    pub fn hover(&mut self, hover: HoverParams) -> Hover {
        let Some((module, _offset, found)) = self.find_document_position(&hover.position) else {
            return Hover::default();
        };
        let context = self.find_context_for_scope(module, found.scope);
        let ast = self.compiler.get_module_ast(module);
        let hover = |contents| Hover {
            contents,
            range: Some(Range::from_span(found.span, ast.src())),
        };
        match found.ty {
            FoundType::None => Hover::default(),
            FoundType::Error => hover("syntax error".into()),
            FoundType::Definition => {
                let name = &ast[found.span];
                let def =
                    self.compiler
                        .resolve_in_scope(module, found.scope, name, ModuleSpan::MISSING);
                hover(self.hover_def(def, name))
            }
            FoundType::Ident | FoundType::Literal | FoundType::EnumLiteral | FoundType::Member => {
                match context {
                    ScopeContext::TopLevel => Hover::default(),
                    ScopeContext::Function(function_id) => {
                        let ast = self.compiler.get_module_ast(module);
                        let mut hooks = FindHooks::new(found.span, ast, &self.compiler, found.ty);
                        let checked = compiler::check::function(
                            &self.compiler,
                            module,
                            function_id,
                            &mut hooks,
                        );
                        let BodyOrTypes::Body(hir) = checked.body_or_types else {
                            return Hover::default();
                        };
                        let signature = self.compiler.get_signature(module, function_id);
                        let hover_var = |var: VarId| {
                            let ty = hir.vars[var.idx()].ty();
                            let val = &ast.src()[found.span.range()];
                            let ty = self
                                .compiler
                                .types
                                .display(hir[ty], &signature.generics)
                                .to_string();
                            let mut text = format!("```eye\n{val}: {ty}\n```");
                            if let compiler::hir::Var::Capture { outer, .. } = hir.vars[var.idx()] {
                                write!(text, "\n---\nCapture of #{}", outer.0).unwrap();
                            }
                            Hover {
                                contents: text.into(),
                                range: Some(Range::from_span(found.span, ast.src())),
                            }
                        };
                        let name_or_literal = &ast.src()[found.span.range()];
                        let Some(item) = hooks.local_item else {
                            let Some(ty) = hooks.ty else {
                                return hover("expr not found".into());
                            };
                            let ty = self.compiler.types.display(hir[ty], &signature.generics);
                            return hover(format!("{name_or_literal} : {ty}").into());
                        };
                        match item {
                            LocalItem::Var(var_id) => hover_var(var_id),
                            LocalItem::Invalid | LocalItem::Def(Def::Invalid) => {
                                hover("<invalid value>".into())
                            }
                            LocalItem::Def(def) => hover(self.hover_def(def, name_or_literal)),
                        }
                    }
                }
            }
            FoundType::Primitive(p) => hover(HoverContents::MarkedStrings(vec![
                text("primitive type "),
                code(p.into_str()),
            ])),
            FoundType::Path(path) => {
                let def = self.compiler.resolve_path(module, found.scope, path);
                hover(format!("Definition {def:?}").into())
            }
            // FoundType::TypePlaceholder => todo!(),
            // FoundType::Underscore => todo!(),
            FoundType::RootModule => hover(HoverContents::MarkedStrings(vec![
                text("Root module at "),
                code(
                    self.compiler
                        .module_path(self.compiler.modules[module.idx()].root),
                ),
            ])),
            // FoundType::ParameterName => todo!(),
            FoundType::Keyword => hover(HoverContents::MarkedStrings(vec![
                MarkedString::String("Keyword ".into()),
                MarkedString::Code {
                    language: "eye".into(),
                    value: ast.src()[found.span.range()].to_owned(),
                },
            ])),
            // FoundType::Generic => todo!(),
            _ => hover(format!("TODO: implement hover type {found:?}").into()),
        }
    }

    fn hover_def(&self, def: Def, name: &str) -> HoverContents {
        let hover_const_value = |value: &ConstValue, ty: Type, kind_text: &str, assign: &str| {
            let generics = Generics::EMPTY;
            let ty = self.compiler.types.display(ty, &generics);
            let value = match value {
                ConstValue::Undefined => "undefined".to_owned(),
                ConstValue::Unit => "()".to_owned(),
                ConstValue::Int(i) => i.to_string(),
                ConstValue::Float(f) => f.to_string(),
                ConstValue::Aggregate(_) => "TODO: display aggregate const values".to_owned(),
            };
            format!("{kind_text}\n```eye\n{name}: {ty} {assign} {value}\n```").into()
        };

        match def {
            Def::ConstValue(id) => {
                let (value, ty) = &self.compiler.const_values[id.idx()];
                hover_const_value(value, *ty, "constant", ":")
            }
            Def::Global(module, id) => {
                let (value, ty) = self.compiler.get_checked_global(module, id);
                hover_const_value(value, *ty, "global", "=")
            }
            Def::Module(module) => {
                let path = self.compiler.module_path(module);
                format!("module {path}").into()
            }
            Def::Function(function_module, function) => {
                let signature = self.compiler.get_signature(function_module, function);
                let mut text = format!("```eye\n{name} :: fn");
                if signature.generics.count() > 0 {
                    text.push('[');
                    for i in 0..signature.generics.count() {
                        if i != 0 {
                            text.push_str(", ");
                        }
                        // TODO: not displaying bounds here for now, should be
                        // displayed here or in where clause when it is supported
                        text.push_str(signature.generics.get_name(i));
                    }
                    text.push(']');
                }
                if signature.params.len() + signature.named_params.len() > 0 {
                    let mut first = true;
                    let mut param_delimiter = |text: &mut String| {
                        if first {
                            first = false;
                        } else {
                            text.push_str(", ");
                        }
                    };
                    text.push('(');
                    for (name, ty) in &signature.params {
                        param_delimiter(&mut text);
                        write!(
                            text,
                            "{name} {}",
                            self.compiler.types.display(*ty, &signature.generics),
                        )
                        .unwrap();
                    }
                    for (name, ty, _default) in &signature.named_params {
                        param_delimiter(&mut text);
                        write!(
                            text,
                            "{name} {} = <TODO: display default>",
                            self.compiler.types.display(*ty, &signature.generics)
                        )
                        .unwrap();
                    }
                    text.push(')');
                }
                text.into()
            }
            // TODO: handle each case separately and produce proper hover text
            def => format!("Definition {def:?}").into(),
        }
    }
}
