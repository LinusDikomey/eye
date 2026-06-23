use error::span::{IdentPath, TSpan};
use parser::ast::{
    self, Ast, BaseImpl, Definition, Expr, ExprId, FunctionId, Generics, Keyword, Method, ScopeId,
    UnresolvedType,
};

#[derive(Debug)]
pub struct Found {
    pub ty: FoundType,
    pub span: TSpan,
    pub scope: ScopeId,
}

#[derive(Debug, Clone, Copy)]
pub enum ScopeContext {
    TopLevel,
    Function(ast::FunctionId),
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
    ParameterName,
    Keyword,
    Generic,
    Definition,
}

pub fn find(ast: &Ast, offset: u32) -> Found {
    let scope = ast.top_level_scope_id();
    find_at_offset_scope(ast, offset, scope).unwrap_or(Found {
        ty: FoundType::None,
        span: ast[scope].span,
        scope,
    })
}

fn find_at_offset_scope(ast: &Ast, offset: u32, scope_id: ScopeId) -> Option<Found> {
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
                });
            }
            let expr = ast[id].0;
            find_at_offset_expr(ast, offset, scope_id, expr)
        }
        &Definition::Use { path: p, .. } => path(offset, scope_id, p),
        &Definition::Global(global_id) => {
            let global = &ast[global_id];
            if global.name_span.contains(offset) {
                return Some(Found {
                    ty: FoundType::Definition,
                    span: global.name_span,
                    scope: scope_id,
                });
            }
            find_at_offset_ty(offset, scope_id, &global.ty)
                .or_else(|| find_at_offset_expr(ast, offset, scope_id, global.val))
        }
        Definition::Module(_) | Definition::Generic(_) => None,
    })
}

fn find_at_offset_expr(ast: &Ast, offset: u32, scope: ScopeId, expr: ExprId) -> Option<Found> {
    let rec = |expr: ExprId| find_at_offset_expr(ast, offset, scope, expr);
    let span = ast[expr].span(ast);
    if !span.contains(offset) {
        return None;
    }
    let nothing = Found {
        ty: FoundType::None,
        span,
        scope,
    };
    let keyword = |span: TSpan| {
        span.contains(offset).then_some(Found {
            ty: FoundType::Keyword,
            span,
            scope,
        })
    };
    let found = match &ast[expr] {
        Expr::Error(_) => Some(Found {
            ty: FoundType::Error,
            span,
            scope,
        }),
        &Expr::Block { items, scope, .. } => {
            if let Some(found) = find_at_offset_scope(ast, offset, scope) {
                return Some(found);
            }
            for item in items {
                if let Some(found) = find_at_offset_expr(ast, offset, scope, item) {
                    return Some(found);
                }
            }
            Some(Found {
                ty: FoundType::None,
                span: ast[scope].span,
                scope,
            })
        }
        &Expr::Nested { inner, .. } => rec(inner),
        Expr::IntLiteral { .. } | Expr::FloatLiteral { .. } | Expr::StringLiteral { .. } => {
            Some(Found {
                ty: FoundType::Literal,
                span,
                scope,
            })
        }
        Expr::Array { elements, .. } | Expr::Tuple { elements, .. } => {
            elements.into_iter().find_map(rec)
        }
        &Expr::EnumLiteral {
            ident, args, span, ..
        } => {
            if ident.contains(offset) {
                Some(Found {
                    ty: FoundType::EnumLiteral,
                    span,
                    scope,
                })
            } else {
                args.into_iter().find_map(rec)
            }
        }
        &Expr::Function { id } => function(ast, offset, id),
        &Expr::Primitive { primitive, .. } => Some(Found {
            ty: FoundType::Primitive(primitive),
            span,
            scope,
        }),
        // TODO: find in trait/type definitions
        &Expr::TypeDeclaration { id } => {
            let def = &ast[id];
            let scope = def.scope;
            if let Some(found) = generics(offset, scope, &def.generics) {
                return Some(found);
            }
            (match &def.content {
                ast::TypeContent::Struct { members } => {
                    keyword(TSpan::new(span.start, span.start + Keyword::Struct.len())).or_else(
                        || {
                            members
                                .iter()
                                .find_map(|member| find_at_offset_ty(offset, scope, &member.ty))
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
                                    .find_map(|arg| find_at_offset_ty(offset, scope, arg))
                            })
                        },
                    )
                }
            })
            .or_else(|| def.methods.iter().find_map(|(_, m)| method(ast, offset, m)))
            .or_else(|| {
                def.impls.iter().find_map(|impl_| {
                    path(offset, scope, impl_.implemented_trait)
                        .or_else(|| base_impl(ast, offset, scope, &impl_.base))
                })
            })
        }
        Expr::Trait { .. } => None,
        Expr::Ident { .. } => Some(Found {
            ty: FoundType::Ident,
            scope,
            span,
        }),
        Expr::DeclareWithVal {
            pat,
            annotated_ty,
            val,
            ..
        } => find_at_offset_expr(ast, offset, scope, *pat)
            .or_else(|| find_at_offset_ty(offset, scope, annotated_ty))
            .or_else(|| rec(*val)),
        Expr::Hole { .. } => Some(Found {
            ty: FoundType::Underscore,
            span,
            scope,
        }),
        Expr::UnOp { inner, .. } => rec(*inner),
        &Expr::BinOp { l, r, .. } => rec(l).or_else(|| rec(r)),
        Expr::As { value, ty, .. } => rec(*value).or_else(|| find_at_offset_ty(offset, scope, ty)), // TODO: as keyword
        Expr::Root { .. } => Some(Found {
            ty: FoundType::RootModule,
            span,
            scope,
        }),
        &Expr::MemberAccess { left, name, .. } => {
            if name.contains(offset) {
                Some(Found {
                    ty: FoundType::Member,
                    span: name,
                    scope,
                })
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
            .or_else(|| find_at_offset_expr(ast, offset, scope, pat))
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
            .or_else(|| find_at_offset_expr(ast, offset, scope, pat))
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
                    find_at_offset_expr(ast, offset, scope, pat).or_else(|| rec(val))
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
            .or_else(|| find_at_offset_expr(ast, offset, scope, pat))
            .or_else(|| rec(val))
            .or_else(|| rec(body)),
        &Expr::For {
            start,
            pat,
            iter,
            body,
            ..
        } => keyword(TSpan::new(start, start + Keyword::While.len()))
            .or_else(|| find_at_offset_expr(ast, offset, scope, pat))
            .or_else(|| rec(iter))
            .or_else(|| rec(body)),
        &Expr::FunctionCall(call_id) => {
            let call = &ast[call_id];
            rec(call.called_expr)
                .or_else(|| call.args.into_iter().find_map(&rec))
                .or_else(|| {
                    call.named_args.iter().find_map(|&(name, val)| {
                        if name.contains(offset) {
                            return Some(Found {
                                ty: FoundType::ParameterName,
                                span: name,
                                scope,
                            });
                        }
                        rec(val)
                    })
                })
        }
        &Expr::Asm {
            asm_str_span, args, ..
        } => {
            if asm_str_span.contains(offset) {
                return Some(Found {
                    ty: FoundType::Literal,
                    span: asm_str_span,
                    scope,
                });
            }
            args.into_iter().find_map(rec)
        }
        Expr::Break { .. } | Expr::Continue { .. } => keyword(span),
    };
    Some(found.unwrap_or(nothing))
}

fn generics(offset: u32, scope: ScopeId, generics: &Generics) -> Option<Found> {
    generics.types.iter().find_map(|generic_def| {
        generic_def
            .name
            .contains(offset)
            .then_some(Found {
                ty: FoundType::Generic,
                span: generic_def.name,
                scope,
            })
            .or_else(|| {
                generic_def.bounds.iter().find_map(|bound| {
                    path(offset, scope, bound.path).or_else(|| {
                        bound
                            .generics
                            .iter()
                            .find_map(|ty| find_at_offset_ty(offset, scope, ty))
                    })
                })
            })
    })
}

fn path(offset: u32, scope: ScopeId, path: IdentPath) -> Option<Found> {
    // use inclusive contains here so completions at the end of a path work
    path.span().contains_inclusive(offset).then_some(Found {
        ty: FoundType::Path(path),
        span: path.span(),
        scope,
    })
}

fn find_at_offset_ty(offset: u32, scope: ScopeId, ty: &UnresolvedType) -> Option<Found> {
    let rec = |ty| find_at_offset_ty(offset, scope, ty);
    let span = ty.span();
    if !span.contains(offset) {
        return None;
    }
    Some(match ty {
        &UnresolvedType::Primitive { ty, .. } => Found {
            ty: FoundType::Primitive(ty),
            span,
            scope,
        },
        UnresolvedType::Unresolved(ident_path, generics) => {
            if ident_path.span().contains(offset) {
                Found {
                    ty: FoundType::Path(*ident_path),
                    span,
                    scope,
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
        UnresolvedType::Array(b) => return find_at_offset_ty(offset, scope, &b.0),
        UnresolvedType::Tuple(unresolved_types, _) => {
            return unresolved_types.iter().find_map(rec);
        }
        UnresolvedType::Function {
            span_and_return_type,
            params,
        } => return rec(&span_and_return_type.1).or_else(|| params.iter().find_map(rec)),
        UnresolvedType::Infer(_) => Found {
            ty: FoundType::TypePlaceholder,
            span,
            scope,
        },
    })
}

fn base_impl(ast: &Ast, offset: u32, scope: ScopeId, base: &BaseImpl) -> Option<Found> {
    generics(offset, scope, &base.generics)
        .or_else(|| {
            base.trait_generics
                .iter()
                .find_map(|ty| find_at_offset_ty(offset, scope, ty))
        })
        .or_else(|| base.functions.iter().find_map(|m| method(ast, offset, m)))
}

fn method(ast: &Ast, offset: u32, method: &Method<()>) -> Option<Found> {
    // TODO: name of method
    function(ast, offset, method.function)
}

fn function(ast: &Ast, offset: u32, id: FunctionId) -> Option<Found> {
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
        });
    }
    if let Some(found) = generics(offset, scope, &function.generics) {
        return Some(found);
    }
    for (name, ty) in &function.params {
        if name.contains(offset) {
            return Some(Found {
                ty: FoundType::ParameterName,
                span: *name,
                scope: function.scope,
            });
        }
        if let Some(found) = find_at_offset_ty(offset, function.scope, ty) {
            return Some(found);
        }
    }
    Some(
        function
            .body
            .and_then(|body| find_at_offset_expr(ast, offset, scope, body))
            .unwrap_or(Found {
                ty: FoundType::None,
                span: ast[scope].span,
                scope,
            }),
    )
}
