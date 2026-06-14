use error::span::{IdentPath, TSpan};
use parser::ast::{self, Ast, Definition, Expr, ExprId, ScopeId, UnresolvedType};

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
    #[allow(unused)] // TODO: remove unused
    Primitive(ast::Primitive),
    #[allow(unused)] // TODO: remove unused
    Path(IdentPath),
    TypePlaceholder,
    Underscore,
    RootModule,
    Member,
    ParameterName,
    Keyword,
    Generic,
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
    for def in scope.definitions.values() {
        match def {
            &Definition::Expr { id, .. } => {
                let expr = ast[id].0;
                if let Some(found) = find_at_offset_expr(ast, offset, scope_id, expr) {
                    return Some(found);
                };
            }
            &Definition::Use { path, .. } => {
                if path.span().contains(offset) {
                    return Some(Found {
                        ty: FoundType::Path(path),
                        span: path.span(),
                        scope: scope_id,
                    });
                }
            }
            &Definition::Global(global_id) => {
                let expr = ast[global_id].val;
                find_at_offset_expr(ast, offset, scope_id, expr);
            }
            Definition::Module(_) | Definition::Generic(_) => {}
        }
    }
    None
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
        &Expr::Function { id } => {
            let function = &ast[id];
            let scope = function.scope;
            for generic_def in &function.generics.types {
                if generic_def.name.contains(offset) {
                    return Some(Found {
                        ty: FoundType::Generic,
                        span: generic_def.name,
                        scope,
                    });
                }
                for bound in &generic_def.bounds {
                    if bound.path.span().contains(offset) {
                        return Some(Found {
                            ty: FoundType::Path(bound.path),
                            span,
                            scope,
                        });
                    }
                    if let Some(found) = bound
                        .generics
                        .iter()
                        .find_map(|ty| find_at_offset_ty(offset, scope, ty))
                    {
                        return Some(found);
                    }
                }
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
        &Expr::Primitive { primitive, .. } => Some(Found {
            ty: FoundType::Primitive(primitive),
            span,
            scope,
        }),
        // TODO: find in trait/type definitions
        &Expr::Type { .. } => None,
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
        Expr::As { value, ty, .. } => rec(*value).or_else(|| find_at_offset_ty(offset, scope, ty)),
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
        &Expr::ReturnUnit { .. } => None,
        &Expr::Return { val, .. } => rec(val),
        &Expr::If { cond, then, .. } => rec(cond).or_else(|| rec(then)),
        &Expr::IfElse {
            cond, then, else_, ..
        } => rec(cond).or_else(|| rec(then)).or_else(|| rec(else_)),
        &Expr::IfPat {
            pat, value, then, ..
        } => find_at_offset_expr(ast, offset, scope, pat)
            .or_else(|| rec(value))
            .or_else(|| rec(then)),
        &Expr::IfPatElse {
            pat,
            value,
            then,
            else_,
            ..
        } => find_at_offset_expr(ast, offset, scope, pat)
            .or_else(|| rec(value))
            .or_else(|| rec(then))
            .or_else(|| rec(else_)),
        &Expr::Match { val, branches, .. } => rec(val).or_else(|| {
            branches.into_iter().find_map(|(pat, val)| {
                find_at_offset_expr(ast, offset, scope, pat).or_else(|| rec(val))
            })
        }),
        &Expr::While { cond, body, .. } => rec(cond).or_else(|| rec(body)),
        &Expr::WhilePat { pat, val, body, .. } => find_at_offset_expr(ast, offset, scope, pat)
            .or_else(|| rec(val))
            .or_else(|| rec(body)),
        &Expr::For {
            pat, iter, body, ..
        } => find_at_offset_expr(ast, offset, scope, pat)
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
        Expr::Break { .. } | Expr::Continue { .. } => Some(Found {
            ty: FoundType::Keyword,
            span,
            scope,
        }),
    };
    Some(found.unwrap_or(nothing))
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
