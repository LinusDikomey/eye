use compiler::{check::Hooks, compiler::BodyOrTypes, hir::Hir, typing::TypeTable};
use error::span::{IdentPath, TSpan};
use parser::ast::{
    self, Ast, BaseImpl, Definition, Expr, ExprId, FunctionId, Keyword, Method, ScopeId,
    UnresolvedType,
};

#[derive(Debug)]
pub struct Found {
    pub ty: FoundType,
    pub span: TSpan,
    pub scope: ScopeId,
    pub context: ScopeContext,
}

#[derive(Debug, Clone, Copy)]
pub enum ScopeContext {
    TopLevel,
    Function(ast::FunctionId),
    DefExpr(ast::DefExprId),
}
impl ScopeContext {
    pub fn check<H: Hooks>(
        &self,
        compiler: &compiler::Compiler,
        module: ast::ModuleId,
        scope: ScopeId,
        hooks: &mut H,
    ) -> Option<(Hir, compiler::compiler::Generics)> {
        match self {
            ScopeContext::TopLevel => None,
            &ScopeContext::Function(function_id) => {
                let checked =
                    compiler::check::function(compiler, module, function_id, false, hooks);
                let BodyOrTypes::Body(hir) = checked.body_or_types else {
                    return None;
                };
                let signature = compiler.get_signature(module, function_id);
                Some((hir, signature.generics.clone()))
            }
            &ScopeContext::DefExpr(id) => {
                let ast = compiler.get_module_ast(module);
                let (expr, ty) = &ast[id];
                let mut types = TypeTable::new();
                let expected = types.from_annotation(ty, compiler, module, scope);
                let expected = types.add(expected);
                let hir = compiler::hir::HIRBuilder::new(types);
                let hir = compiler::check::check(
                    compiler,
                    ast,
                    module,
                    &compiler::compiler::Generics::EMPTY,
                    scope,
                    hir,
                    [],
                    *expr,
                    expected,
                    "",
                    compiler::compiler::LocalScopeParent::None,
                    false,
                    hooks,
                );
                let def = compiler::eval::def_expr(
                    compiler,
                    module,
                    scope,
                    ast,
                    *expr,
                    "",
                    TSpan::EMPTY,
                    ty,
                );
                Some((hir, compiler::compiler::Generics::EMPTY))
            }
        }
    }
}

#[derive(Debug)]
pub enum FoundType {
    None,
    Error,
    Ident,
    Literal,
    EnumLiteral,
    Primitive(ast::Primitive),
    Path(IdentPath),
    TypePlaceholder,
    Underscore,
    RootModule,
    Member,
    Parameter,
    Keyword,
    Generic,
    Definition,
    CallParameterLabel,
}

pub fn find(ast: &Ast, offset: u32) -> Found {
    let scope = ast.top_level_scope_id();
    find_at_offset_scope(ast, offset, scope, ScopeContext::TopLevel).unwrap_or(Found {
        ty: FoundType::None,
        span: ast[scope].span,
        scope,
        context: ScopeContext::TopLevel,
    })
}

fn find_at_offset_scope(
    ast: &Ast,
    offset: u32,
    scope_id: ScopeId,
    context: ScopeContext,
) -> Option<Found> {
    tracing::debug!(offset = offset, "Looking in scope {scope_id:?}");
    let scope = &ast[scope_id];
    if !scope.span.contains(offset) {
        return None;
    }
    scope.definitions.values().find_map(|def| match def {
        &Definition::Expr { id, name_span, .. } => {
            if name_span.contains(offset) {
                return Some(Found {
                    ty: FoundType::Definition,
                    span: name_span,
                    scope: scope_id,
                    context,
                });
            }
            let expr = ast[id].0;
            find_at_offset_expr(ast, offset, scope_id, ScopeContext::DefExpr(id), expr)
        }
        &Definition::Use { path: p, .. } => path(offset, scope_id, context, p),
        &Definition::Global(global_id) => {
            let global = &ast[global_id];
            if global.name_span.contains(offset) {
                return Some(Found {
                    ty: FoundType::Definition,
                    span: global.name_span,
                    scope: scope_id,
                    context,
                });
            }
            find_at_offset_ty(offset, scope_id, context, &global.ty)
                .or_else(|| find_at_offset_expr(ast, offset, scope_id, context, global.val))
        }
        Definition::Module(_) | Definition::Generic(_) => None,
    })
}

fn find_at_offset_expr(
    ast: &Ast,
    offset: u32,
    scope: ScopeId,
    context: ScopeContext,
    expr: ExprId,
) -> Option<Found> {
    let rec = |expr: ExprId| find_at_offset_expr(ast, offset, scope, context, expr);
    let span = ast[expr].span(ast);
    if !span.contains(offset) {
        return None;
    }
    let nothing = Found {
        ty: FoundType::None,
        span,
        scope,
        context,
    };
    let keyword = |span: TSpan| {
        span.contains(offset).then_some(Found {
            ty: FoundType::Keyword,
            span,
            scope,
            context,
        })
    };
    let found = |ty, span| {
        Some(Found {
            ty,
            span,
            scope,
            context,
        })
    };
    let found = match &ast[expr] {
        Expr::Error(_) => found(FoundType::Error, span),
        &Expr::Block { items, scope, .. } => {
            if let Some(found) = find_at_offset_scope(ast, offset, scope, context) {
                return Some(found);
            }
            for item in items {
                if let Some(found) = find_at_offset_expr(ast, offset, scope, context, item) {
                    return Some(found);
                }
            }
            Some(Found {
                ty: FoundType::None,
                span: ast[scope].span,
                scope,
                context,
            })
        }
        &Expr::Nested { inner, .. } => rec(inner),
        Expr::IntLiteral { .. } | Expr::FloatLiteral { .. } | Expr::StringLiteral { .. } => {
            found(FoundType::Literal, span)
        }
        Expr::Array { elements, .. } | Expr::Tuple { elements, .. } => {
            elements.into_iter().find_map(rec)
        }
        &Expr::EnumLiteral {
            ident, args, span, ..
        } => {
            if ident.contains(offset) {
                found(FoundType::EnumLiteral, span)
            } else {
                args.into_iter().find_map(rec)
            }
        }
        &Expr::Function { id } => function(ast, offset, context, id),
        &Expr::Primitive { primitive, .. } => found(FoundType::Primitive(primitive), span),
        // TODO: find in trait/type definitions
        &Expr::TypeDeclaration { id } => {
            let def = &ast[id];
            let scope = def.scope;
            if let Some(found) = generics(offset, scope, context, &def.generics) {
                return Some(found);
            }
            (match &def.content {
                ast::TypeContent::Struct { members } => {
                    keyword(TSpan::new(span.start, span.start + Keyword::Struct.len())).or_else(
                        || {
                            members.iter().find_map(|member| {
                                find_at_offset_ty(offset, scope, context, &member.ty)
                            })
                        },
                    )
                }
                ast::TypeContent::Enum { variants } => {
                    keyword(TSpan::new(span.start, span.start + Keyword::Enum.len())).or_else(
                        || {
                            variants.iter().find_map(|variant| {
                                variant
                                    .args
                                    .iter()
                                    .find_map(|arg| find_at_offset_ty(offset, scope, context, arg))
                            })
                        },
                    )
                }
            })
            .or_else(|| {
                def.methods
                    .iter()
                    .find_map(|(_, m)| method(ast, offset, scope, context, m))
            })
            .or_else(|| {
                def.impls.iter().find_map(|impl_| {
                    path(offset, scope, context, impl_.implemented_trait)
                        .or_else(|| base_impl(ast, offset, scope, context, &impl_.base))
                })
            })
        }
        Expr::Trait { .. } => None,
        Expr::Ident { .. } => found(FoundType::Ident, span),
        Expr::DeclareWithVal {
            pat,
            annotated_ty,
            val,
            ..
        } => find_at_offset_expr(ast, offset, scope, context, *pat)
            .or_else(|| find_at_offset_ty(offset, scope, context, annotated_ty))
            .or_else(|| rec(*val)),
        Expr::Hole { .. } => found(FoundType::Underscore, span),
        Expr::UnOp { inner, .. } => rec(*inner),
        &Expr::BinOp { l, r, .. } => rec(l).or_else(|| rec(r)),
        Expr::As { value, ty, .. } => {
            rec(*value).or_else(|| find_at_offset_ty(offset, scope, context, ty))
        } // TODO: as keyword
        Expr::Root { .. } => found(FoundType::RootModule, span),
        &Expr::MemberAccess { left, name, .. } => {
            if name.contains(offset) {
                found(FoundType::Member, name)
            } else {
                rec(left)
            }
        }
        &Expr::Index { expr, idx, .. } => rec(expr).or_else(|| rec(idx)),
        &Expr::TupleIdx { left, .. } => rec(left),
        &Expr::ReturnUnit { .. } => keyword(span),
        &Expr::Return { val, start, .. } => keyword(TSpan::new(start, start + Keyword::Ret.len()))
            .or_else(|| rec(val))
            .or_else(|| keyword(TSpan::new(start, start + Keyword::Ret.len()))),
        &Expr::If {
            start, cond, then, ..
        } => keyword(TSpan::new(start, start + Keyword::If.len()))
            .or_else(|| rec(cond))
            .or_else(|| rec(then)),

        &Expr::IfElse {
            start,
            cond,
            then,
            else_,
            ..
        } => keyword(TSpan::new(start, start + Keyword::If.len()))
            .or_else(|| rec(cond))
            .or_else(|| rec(then))
            // TODO: else keyword
            .or_else(|| rec(else_)),
        &Expr::IfPat {
            pat,
            value,
            then,
            start,
            ..
        } => keyword(TSpan::new(start, start + Keyword::If.len()))
            .or_else(|| find_at_offset_expr(ast, offset, scope, context, pat))
            .or_else(|| rec(value))
            .or_else(|| rec(then)),
        &Expr::IfPatElse {
            start,
            pat,
            value,
            then,
            else_,
            ..
        } => keyword(TSpan::new(start, start + Keyword::If.len()))
            .or_else(|| find_at_offset_expr(ast, offset, scope, context, pat))
            .or_else(|| rec(value))
            .or_else(|| rec(then))
            // TODO: else keyword
            .or_else(|| rec(else_)),
        &Expr::Match {
            span,
            val,
            branches,
            ..
        } => keyword(TSpan::new(span.start, span.start + Keyword::Match.len()))
            .or_else(|| rec(val))
            .or_else(|| {
                branches.into_iter().find_map(|(pat, val)| {
                    find_at_offset_expr(ast, offset, scope, context, pat).or_else(|| rec(val))
                })
            }),
        &Expr::While {
            start, cond, body, ..
        } => keyword(TSpan::new(start, start + Keyword::While.len()))
            .or_else(|| rec(cond))
            .or_else(|| rec(body)),
        &Expr::WhilePat {
            start,
            pat,
            val,
            body,
            ..
        } => keyword(TSpan::new(start, start + Keyword::While.len()))
            .or_else(|| find_at_offset_expr(ast, offset, scope, context, pat))
            .or_else(|| rec(val))
            .or_else(|| rec(body)),
        &Expr::For {
            start,
            pat,
            iter,
            body,
            ..
        } => keyword(TSpan::new(start, start + Keyword::While.len()))
            .or_else(|| find_at_offset_expr(ast, offset, scope, context, pat))
            .or_else(|| rec(iter))
            .or_else(|| rec(body)),
        &Expr::FunctionCall(call_id) => {
            let call = &ast[call_id];
            rec(call.called_expr)
                .or_else(|| call.args.into_iter().find_map(&rec))
                .or_else(|| {
                    call.named_args.iter().find_map(|&(name, val)| {
                        if name.contains(offset) {
                            return found(FoundType::CallParameterLabel, name);
                        }
                        rec(val)
                    })
                })
        }
        &Expr::Asm {
            asm_str_span, args, ..
        } => {
            if asm_str_span.contains(offset) {
                return found(FoundType::Literal, asm_str_span);
            }
            args.into_iter().find_map(rec)
        }
        Expr::Break { .. } | Expr::Continue { .. } => keyword(span),
    };
    Some(found.unwrap_or(nothing))
}

fn generics(
    offset: u32,
    scope: ScopeId,
    context: ScopeContext,
    generics: &ast::Generics,
) -> Option<Found> {
    generics.types.iter().find_map(|generic_def| {
        generic_def
            .name
            .contains(offset)
            .then_some(Found {
                ty: FoundType::Generic,
                span: generic_def.name,
                scope,
                context,
            })
            .or_else(|| {
                generic_def.bounds.iter().find_map(|bound| {
                    path(offset, scope, context, bound.path).or_else(|| {
                        bound
                            .generics
                            .iter()
                            .find_map(|ty| find_at_offset_ty(offset, scope, context, ty))
                    })
                })
            })
    })
}

fn path(offset: u32, scope: ScopeId, context: ScopeContext, path: IdentPath) -> Option<Found> {
    // use inclusive contains here so completions at the end of a path work
    path.span().contains_inclusive(offset).then_some(Found {
        ty: FoundType::Path(path),
        span: path.span(),
        scope,
        context,
    })
}

fn find_at_offset_ty(
    offset: u32,
    scope: ScopeId,
    context: ScopeContext,
    ty: &UnresolvedType,
) -> Option<Found> {
    let rec = |ty| find_at_offset_ty(offset, scope, context, ty);
    let span = ty.span();
    if !span.contains(offset) {
        return None;
    }
    Some(match ty {
        &UnresolvedType::Primitive { ty, .. } => Found {
            ty: FoundType::Primitive(ty),
            span,
            scope,
            context,
        },
        UnresolvedType::Unresolved(ident_path, generics) => {
            if ident_path.span().contains(offset) {
                Found {
                    ty: FoundType::Path(*ident_path),
                    span,
                    scope,
                    context,
                }
            } else {
                return generics
                    .as_ref()
                    .and_then(|generics| generics.0.iter().find_map(rec));
            }
        }
        UnresolvedType::Pointer(pointee) => {
            return rec(&pointee.0);
        }
        UnresolvedType::Array(b) => return find_at_offset_ty(offset, scope, context, &b.0),
        UnresolvedType::Tuple(unresolved_types, _) => {
            return unresolved_types.iter().find_map(rec);
        }
        UnresolvedType::Function(func) => {
            return rec(&func.params).or_else(|| rec(&func.return_ty));
        }
        UnresolvedType::Infer(_) => Found {
            ty: FoundType::TypePlaceholder,
            span,
            scope,
            context,
        },
    })
}

fn base_impl(
    ast: &Ast,
    offset: u32,
    scope: ScopeId,
    context: ScopeContext,
    base: &BaseImpl,
) -> Option<Found> {
    generics(offset, scope, context, &base.generics)
        .or_else(|| {
            base.trait_generics
                .iter()
                .find_map(|ty| find_at_offset_ty(offset, scope, context, ty))
        })
        .or_else(|| {
            base.functions
                .iter()
                .find_map(|m| method(ast, offset, scope, context, m))
        })
}

fn method(
    ast: &Ast,
    offset: u32,
    scope: ScopeId,
    context: ScopeContext,
    method: &Method<()>,
) -> Option<Found> {
    // TODO: name of method
    if method.name.contains(offset) {
        return Some(Found {
            ty: FoundType::Definition,
            span: method.name,
            scope,
            context,
        });
    }
    function(
        ast,
        offset,
        ScopeContext::Function(method.function),
        method.function,
    )
}

fn function(ast: &Ast, offset: u32, context: ScopeContext, id: FunctionId) -> Option<Found> {
    let function = &ast[id];
    let scope = function.scope;
    let span = ast[scope].span;
    if !span.contains(offset) {
        return None;
    }
    let keyword_span = TSpan::new(span.start, span.start + Keyword::Fn.len());
    if keyword_span.contains(offset) {
        return Some(Found {
            ty: FoundType::Keyword,
            span: keyword_span,
            scope,
            context,
        });
    }
    if let Some(found) = generics(offset, scope, context, &function.generics) {
        return Some(found);
    }
    for (name, ty) in &function.params {
        if name.contains(offset) {
            return Some(Found {
                ty: FoundType::Parameter,
                span: *name,
                scope: function.scope,
                context,
            });
        }
        if let Some(found) = find_at_offset_ty(offset, function.scope, context, ty) {
            return Some(found);
        }
    }

    if let Some(found) = find_at_offset_ty(offset, function.scope, context, &function.return_type) {
        return Some(found);
    }
    Some(
        function
            .body
            .and_then(|body| find_at_offset_expr(ast, offset, scope, context, body))
            .unwrap_or(Found {
                ty: FoundType::None,
                span: ast[scope].span,
                scope,
                context,
            }),
    )
}
