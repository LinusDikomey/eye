#![allow(unused)] // TODO: this is unfinished, finish it then remove this #[allow]
use error::span::IdentPath;

use crate::ast::{
    Ast, Definition, Expr, ExprId, ExprIds, FunctionId, ScopeId, TraitId, TypeId, UnresolvedType,
};

impl Ast<()> {
    fn eq_scope(&self, other: &Self, a: ScopeId, b: ScopeId) -> bool {
        let a_defs = &self[a].definitions;
        let b_defs = &self[b].definitions;
        if a_defs.len() != b_defs.len() {
            return false;
        }
        for (name, a_def) in a_defs {
            let Some(b_def) = b_defs.get(name) else {
                return false;
            };
            if !self.eq_definition(other, a_def, b_def) {
                return false;
            }
        }
        true
    }

    fn eq_definition(&self, other: &Self, a: &Definition, b: &Definition) -> bool {
        match *a {
            Definition::Expr { id: a, .. } => {
                let &Definition::Expr { id: b, .. } = b else {
                    return false;
                };
                let ((a, a_ty), (b, b_ty)) = (&self[a], &other[b]);
                self.eq_ty(other, a_ty, b_ty) && self.eq_expr(other, *a, *b)
            }
            Definition::Use { path: a, .. } => {
                let &Definition::Use { path: b, .. } = b else {
                    return false;
                };
                self.eq_path(other, a, b)
            }
            Definition::Global(a) => {
                let &Definition::Global(b) = b else {
                    return false;
                };
                let a = &self[a];
                let b = &other[b];
                a.name == b.name
                    && self.eq_ty(other, &a.ty, &b.ty)
                    && self.eq_expr(other, a.val, b.val)
            }
            Definition::Module(a) => {
                let &Definition::Module(b) = b else {
                    return false;
                };
                a == b
            }
            Definition::Generic(a) => {
                let &Definition::Generic(b) = b else {
                    return false;
                };
                a == b
            }
        }
    }

    fn eq_ty(&self, other: &Self, a: &UnresolvedType, b: &UnresolvedType) -> bool {
        match a {
            UnresolvedType::Primitive { ty: a, .. } => {
                let UnresolvedType::Primitive { ty: b, .. } = b else {
                    return false;
                };
                a == b
            }
            UnresolvedType::Unresolved(a, a_generics) => {
                let UnresolvedType::Unresolved(b, ref b_generics) = *b else {
                    return false;
                };
                self.eq_path(other, *a, b)
                    && match (a_generics, b_generics) {
                        (Some(a), Some(b)) => {
                            a.0.len() == b.0.len()
                                && a.0
                                    .iter()
                                    .zip(b.0.iter())
                                    .all(|(a, b)| self.eq_ty(other, a, b))
                        }
                        (None, None) => true,
                        _ => false,
                    }
            }
            UnresolvedType::Pointer(a) => {
                let UnresolvedType::Pointer(b) = b else {
                    return false;
                };
                self.eq_ty(other, &a.0, &b.0)
            }
            UnresolvedType::Array(a) => {
                let UnresolvedType::Array(b) = b else {
                    return false;
                };
                self.eq_ty(other, &a.0, &b.0) && a.1 == b.1
            }
            UnresolvedType::Tuple(a, _) => {
                let UnresolvedType::Tuple(b, _) = b else {
                    return false;
                };
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| self.eq_ty(other, a, b))
            }
            UnresolvedType::Function {
                span_and_return_type: a_span_return,
                params: a_params,
            } => {
                let UnresolvedType::Function {
                    span_and_return_type: b_span_return,
                    params: b_params,
                } = b
                else {
                    return false;
                };
                self.eq_ty(other, &a_span_return.1, &b_span_return.1)
                    && a_params.len() == b_params.len()
                    && a_params
                        .iter()
                        .zip(b_params.iter())
                        .all(|(a, b)| self.eq_ty(other, a, b))
            }
            UnresolvedType::Infer(_) => matches!(b, UnresolvedType::Infer(_)),
        }
    }

    fn eq_path(&self, other: &Self, a: IdentPath, b: IdentPath) -> bool {
        let (a_root, mut a_segments, a_end) = a.segments(self.src());
        let (b_root, mut b_segments, b_end) = b.segments(other.src());
        a_root.is_some() == b_root.is_some()
            && match (a_end, b_end) {
                (Some((a, _)), Some((b, _))) => a == b,
                (None, None) => true,
                _ => false,
            }
            && loop {
                match (a_segments.next(), b_segments.next()) {
                    (Some((a, _)), Some((b, _))) if a == b => {}
                    (None, None) => break true,
                    _ => break false,
                }
            }
    }

    fn eq_expr(&self, other: &Ast, a: ExprId, b: ExprId) -> bool {
        let a = &self[a];
        let b = &other[b];
        match a {
            Expr::Error(_) => matches!(b, Expr::Error(_)),
            Expr::Block {
                scope: a_scope,
                items: a,
                ..
            } => {
                let Expr::Block {
                    scope: b_scope,
                    items: b,
                    ..
                } = b
                else {
                    return false;
                };
                self.eq_scope(other, *a_scope, *b_scope)
                    && a.len() == b.len()
                    && a.into_iter()
                        .zip(*b)
                        .all(|(a, b)| self.eq_expr(other, a, b))
            }
            &Expr::Nested { inner: a, .. } => {
                matches!(*b, Expr::Nested { inner: b, .. } if self.eq_expr(other, a, b))
            }
            Expr::IntLiteral { span: a, .. } => {
                matches!(b, Expr::IntLiteral { span: b, .. } if self.src()[a.range()] == other.src()[b.range()])
            }
            Expr::FloatLiteral { span: a, .. } => {
                matches!(b, Expr::FloatLiteral { span: b, .. } if self.src()[a.range()] == other.src()[b.range()])
            }
            Expr::StringLiteral { span: a, .. } => {
                matches!(b, Expr::StringLiteral { span: b, .. } if self.src()[a.range()] == other.src()[b.range()])
            }
            Expr::Array { elements: a, .. } => {
                matches!(b, Expr::Array { elements: b, .. } if a.count == b.count && a.into_iter().zip(*b).all(|(a, b)| self.eq_expr(other, a, b)))
            }
            Expr::Tuple { elements: a, .. } => {
                matches!(b, Expr::Tuple {elements: b, .. } if a.count == b.count && a.into_iter().zip(*b).all(|(a, b)| self.eq_expr(other, a, b)))
            }
            &Expr::EnumLiteral {
                ident: a_ident,
                args: a_args,
                ..
            } => matches!(b, &Expr::EnumLiteral { ident: b_ident, args: b_args, ..}
                if self.src()[a_ident.range()] == other.src()[b_ident.range()]
                && self.eq_exprs(other, a_args, b_args)
            ),
            &Expr::Function { id: a } => {
                matches!(b, &Expr::Function { id: b } if self.eq_function(other, a, b))
            }
            Expr::Primitive { primitive: a, .. } => {
                matches!(b, Expr::Primitive { primitive: b, .. } if a == b)
            }
            &Expr::TypeDeclaration { id: a } => {
                matches!(b, &Expr::TypeDeclaration { id: b } if self.eq_type_def(other, a, b))
            }
            &Expr::Trait { id: a } => {
                matches!(b, &Expr::Trait { id: b } if self.eq_trait_def(other, a, b))
            }
            Expr::Ident { span: a, .. } => {
                matches!(b, &Expr::Ident { span: b, .. } if self.src[a.range()] == other.src[b.range()])
            }
            Expr::DeclareWithVal {
                pat,
                t_colon_and_equals_or_colon_equals,
                annotated_ty,
                val,
            } => todo!(),
            Expr::Hole { loc, t } => todo!(),
            Expr::UnOp {
                start_or_end,
                t_op,
                op,
                inner,
            } => todo!(),
            Expr::BinOp { t_op, op, l, r } => todo!(),
            Expr::As { value, t_as, ty } => todo!(),
            Expr::Root { start, t } => todo!(),
            Expr::MemberAccess { left, t_dot, name } => todo!(),
            Expr::Index {
                expr,
                t_lbracket,
                idx,
                t_rbracket,
                end,
            } => todo!(),
            Expr::TupleIdx {
                left,
                t_dot,
                t_int,
                idx,
                end,
            } => todo!(),
            Expr::ReturnUnit { start, t } => todo!(),
            Expr::Return { start, t_ret, val } => todo!(),
            Expr::If {
                start,
                t_if,
                cond,
                t_colon,
                then,
            } => todo!(),
            Expr::IfElse {
                start,
                t_if,
                cond,
                t_colon,
                then,
                t_else,
                else_,
            } => todo!(),
            Expr::IfPat {
                start,
                t_if,
                t_colon_eq,
                pat,
                t_colon,
                value,
                then,
            } => todo!(),
            Expr::IfPatElse {
                start,
                t_if,
                pat,
                t_colon_eq,
                value,
                t_colon,
                then,
                t_else,
                else_,
            } => todo!(),
            Expr::Match {
                t_match,
                span,
                val,
                t_lbrace,
                branches,
                t_rbrace,
            } => todo!(),
            Expr::While {
                start,
                t_while,
                cond,
                t_colon,
                body,
            } => todo!(),
            Expr::WhilePat {
                start,
                t_while,
                pat,
                t_colon_eq,
                val,
                t_colon,
                body,
            } => todo!(),
            Expr::For {
                start,
                t_for,
                pat,
                t_in,
                iter,
                t_colon,
                body,
            } => todo!(),
            Expr::FunctionCall(call_id) => todo!(),
            Expr::Asm {
                t_asm,
                t_lparen,
                t_string_literal,
                asm_str_span,
                args,
                t_rparen,
                span,
            } => todo!(),
            Expr::Break { start, t } => todo!(),
            Expr::Continue { start, t } => todo!(),
        }
    }

    fn eq_exprs(&self, other: &Ast, a: ExprIds, b: ExprIds) -> bool {
        a.count == b.count && a.into_iter().zip(b).all(|(a, b)| self.eq_expr(other, a, b))
    }

    fn eq_function(&self, other: &Ast, a: FunctionId, b: FunctionId) -> bool {
        todo!()
    }

    fn eq_type_def(&self, other: &Ast, a: TypeId, b: TypeId) -> bool {
        todo!()
    }

    fn eq_trait_def(&self, other: &Ast, a: TraitId, b: TraitId) -> bool {
        todo!()
    }
}
