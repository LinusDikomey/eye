//! Displays an entire ast roughly how it would look in the source code. This is only be used for
//! debugging and NOT for pretty printing. It is not guaranteed to be accurate.

use core::fmt;

use crate::ast::{Ast, Definition, Expr, ExprId, ScopeId, TypeContent, UnresolvedType};

impl fmt::Display for Ast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            ScopeDisplay {
                ast: self,
                scope: self.top_level_scope,
                indent: 0,
            }
        )
    }
}

struct ScopeDisplay<'a> {
    ast: &'a Ast,
    scope: ScopeId,
    indent: usize,
}
impl<'a> fmt::Display for ScopeDisplay<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, def) in &self.ast[self.scope].definitions {
            match def {
                Definition::Expr { id, .. } => write!(
                    f,
                    "{name} :: {}",
                    ExprDisplay {
                        ast: self.ast,
                        indent: self.indent,
                        expr: self.ast[*id].0,
                    }
                )?,
                Definition::Use { path, .. } => write!(f, "use {}", &self.ast[path.span()])?,
                &Definition::Global(id) => {
                    let global = &self.ast[id];
                    write!(
                        f,
                        "{name}: {} = {}",
                        TypeDisplay {
                            src: self.ast.src(),
                            ty: &global.ty
                        },
                        ExprDisplay {
                            ast: self.ast,
                            expr: global.val,
                            indent: self.indent + 1
                        }
                    )?;
                }
                Definition::Module(_) => {}
                Definition::Generic(_) => {}
            }
            writeln!(f, "\n")?;
        }
        Ok(())
    }
}

struct ExprDisplay<'a> {
    ast: &'a Ast,
    expr: ExprId,
    indent: usize,
}
impl<'a> fmt::Display for ExprDisplay<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let expr = |id| ExprDisplay {
            ast: self.ast,
            expr: id,
            indent: self.indent,
        };
        let ty = |ty| TypeDisplay {
            src: self.ast.src(),
            ty,
        };
        let indent = |f: &mut std::fmt::Formatter<'_>| {
            for _ in 0..self.indent {
                write!(f, "  ")?;
            }
            Ok(())
        };
        match &self.ast[self.expr] {
            Expr::Error(_) => write!(f, "<error>"),
            &Expr::Block { scope, items, .. } => {
                writeln!(
                    f,
                    "{{\n{}",
                    ScopeDisplay {
                        ast: self.ast,
                        scope,
                        indent: self.indent + 1,
                    }
                )?;
                for item in items {
                    indent(f)?;
                    writeln!(
                        f,
                        "  {}",
                        ExprDisplay {
                            ast: self.ast,
                            expr: item,
                            indent: self.indent + 1
                        }
                    )?;
                }
                write!(f, "}}")
            }
            &Expr::Nested { inner, .. } => write!(f, "({})", expr(inner)),

            &Expr::IntLiteral { span, .. }
            | &Expr::FloatLiteral { span, .. }
            | &Expr::StringLiteral { span, .. } => write!(f, "{}", &self.ast[span]),
            &Expr::Array { elements, .. } => {
                write!(f, "[")?;
                for (i, elem) in elements.into_iter().enumerate() {
                    if i != 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", expr(elem))?;
                }
                write!(f, "]")
            }
            Expr::Tuple {
                elements,
                named_elements,
                ..
            } => {
                write!(f, "[")?;
                let mut first = true;
                for elem in *elements {
                    if first {
                        first = false;
                    } else {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", expr(elem))?;
                }
                for (name, elem) in named_elements {
                    if first {
                        first = false;
                    } else {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", &self.ast[*name], expr(*elem))?;
                }
                write!(f, "]")
            }
            &Expr::EnumLiteral { ident, args, .. } => {
                write!(f, ".{}(", &self.ast[ident])?;
                let mut first = true;
                for arg in args {
                    if first {
                        first = false
                    } else {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", expr(arg))?;
                }
                write!(f, ")")
            }
            &Expr::Function { id } => {
                write!(f, "fn(")?;
                let func = &self.ast[id];
                let mut first = true;
                for (name, param_ty) in &func.params {
                    if first {
                        first = false
                    } else {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} {}", &self.ast[*name], ty(param_ty))?;
                }
                for named in &func.named_params {
                    if first {
                        first = false
                    } else {
                        write!(f, ", ")?;
                    }
                    write!(
                        f,
                        "{} {} = {}",
                        &self.ast[named.name],
                        ty(&named.ty),
                        expr(named.default_value)
                    )?;
                }
                write!(f, ") -> {}", ty(&func.return_type))?;
                if let Some(body) = func.body {
                    write!(f, ": {}", expr(body))
                } else {
                    write!(f, " extern")
                }
            }
            &Expr::Primitive { primitive, .. } => write!(f, "{primitive}"),
            &Expr::Type { id } => {
                let def = &self.ast[id];
                match &def.content {
                    TypeContent::Struct { .. } => Ok(()),
                    TypeContent::Enum { .. } => Ok(()),
                }
            }
            Expr::Trait { .. } => Ok(()),
            &Expr::Ident { span, .. } => write!(f, "{}", &self.ast[span]),
            Expr::DeclareWithVal {
                pat,
                annotated_ty,
                val,
                ..
            } => {
                write!(f, "{}: {} = {}", expr(*pat), ty(annotated_ty), expr(*val))
            }
            Expr::Hole { .. } => write!(f, "_"),
            &Expr::UnOp { op, inner, .. } => {
                if op.postfix() {
                    write!(f, "{}{op}", expr(inner))
                } else {
                    write!(f, "{op}{}", expr(inner))
                }
            }
            &Expr::BinOp { l, op, r, .. } => {
                write!(f, "{} {op} {}", expr(l), expr(r))
            }
            Expr::As {
                value, ty: as_ty, ..
            } => write!(f, "{} as {}", expr(*value), ty(as_ty)),
            Expr::Root { .. } => write!(f, "root"),
            &Expr::MemberAccess { left, name, .. } => {
                write!(f, "{}.{}", expr(left), &self.ast[name])
            }
            &Expr::Index { expr: l, idx, .. } => write!(f, "{}[{}]", expr(l), expr(idx)),
            &Expr::TupleIdx { left, idx, .. } => write!(f, "{}.{idx}", expr(left)),
            Expr::ReturnUnit { .. } => write!(f, "ret"),
            &Expr::Return { val, .. } => write!(f, "ret {}", expr(val)),
            &Expr::If { cond, then, .. } => write!(f, "if {}: {}", expr(cond), expr(then)),
            &Expr::IfElse {
                cond, then, else_, ..
            } => write!(f, "if {}: {} else {}", expr(cond), expr(then), expr(else_)),
            &Expr::IfPat {
                pat, value, then, ..
            } => write!(f, "if {} := {}: {}", expr(pat), expr(value), expr(then)),
            &Expr::IfPatElse {
                pat,
                value,
                then,
                else_,
                ..
            } => write!(
                f,
                "if {} := {}: {} else {}",
                expr(pat),
                expr(value),
                expr(then),
                expr(else_)
            ),
            &Expr::Match { val, branches, .. } => {
                writeln!(f, "match {} {{", expr(val))?;
                for (pat, branch) in branches {
                    indent(f)?;
                    writeln!(
                        f,
                        "  {}: {}",
                        ExprDisplay {
                            ast: self.ast,
                            expr: pat,
                            indent: self.indent + 1,
                        },
                        ExprDisplay {
                            ast: self.ast,
                            expr: branch,
                            indent: self.indent + 2
                        }
                    )?;
                }
                indent(f)?;
                write!(f, "}}")
            }
            &Expr::While { cond, body, .. } => write!(f, "while {}: {}", expr(cond), expr(body)),
            &Expr::WhilePat { pat, val, body, .. } => {
                write!(f, "while {} := {}: {}", expr(pat), expr(val), expr(body))
            }
            &Expr::For {
                pat, iter, body, ..
            } => write!(f, "for {} in {}: {}", expr(pat), expr(iter), expr(body)),
            &Expr::FunctionCall(id) => {
                let call = &self.ast[id];
                write!(f, "{}(", expr(call.called_expr))?;
                let mut first = true;
                for arg in call.args {
                    if first {
                        first = false;
                    } else {
                        write!(f, ", ")?;
                    };
                    write!(f, "{}", expr(arg))?;
                }
                for (name, arg) in &call.named_args {
                    if first {
                        first = false;
                    } else {
                        write!(f, ", ")?;
                    };
                    write!(f, "{}: {}", &self.ast[*name], expr(*arg))?;
                }
                write!(f, ")")
            }
            Expr::Asm { .. } => Ok(()), // TODO: display asm
            Expr::Break { .. } => write!(f, "break"),
            Expr::Continue { .. } => write!(f, "continue"),
        }
    }
}

struct TypeDisplay<'a> {
    src: &'a str,
    ty: &'a UnresolvedType,
}
impl<'a> fmt::Display for TypeDisplay<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        self.ty.to_string(&mut s, self.src);
        write!(f, "{s}")
    }
}
