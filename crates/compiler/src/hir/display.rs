use std::fmt;

use color_format::{cwrite, cwriteln};
use parser::ast::{AssignType, Primitive};

use crate::{
    Compiler,
    compiler::Generics,
    hir::{
        Arithmetic, CastType, Comparison, Hir, LValue, LValueId, Logic, Node, NodeId, Pattern,
        PatternId, PatternIds, TypeProperty,
    },
    typing::{LocalTypeId, LocalTypeIds},
};

pub struct HirDisplay<'a> {
    pub node: NodeId,
    pub hir: &'a Hir,
    pub compiler: &'a Compiler,
    pub generics: &'a Generics,
    pub indent_count: usize,
}
impl<'a> fmt::Display for HirDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let &Self {
            node,
            hir,
            compiler,
            generics,
            indent_count,
        } = self;
        fn indent_n(f: &mut fmt::Formatter<'_>, n: usize) -> fmt::Result {
            for _ in 0..n {
                write!(f, "  ")?;
            }
            Ok(())
        }
        let indent = |f| indent_n(f, indent_count);
        let display = |node| hir.display(node, compiler, generics, indent_count);
        let display_indented = |node| hir.display(node, compiler, generics, indent_count + 1);
        let display_pattern = |node| hir.display_pattern(node, compiler, generics);
        let display_lvalue = |node| hir.display_lvalue(node, compiler, generics, indent_count);
        let display_ty = |ty| compiler.types.display(ty, generics);
        match &hir[node] {
            Node::Invalid => {
                cwrite!(f, "(#b<invalid>)")?;
            }
            Node::Block(statements) => {
                cwriteln!(f, "(")?;
                for id in statements.iter() {
                    indent_n(f, indent_count + 1)?;
                    cwriteln!(f, "{}", display_indented(id))?;
                }
                indent(f)?;
                cwrite!(f, ")")?;
            }
            Node::Unit => cwrite!(f, "(#b<unit>)")?,
            &Node::IntLiteral { val, ty } => {
                cwrite!(f, "(#b<int> #y<{val}>): {}", display_ty(hir[ty]))?;
            }
            &Node::FloatLiteral { val, ty } => {
                cwrite!(f, "(#b<float> #y<{val}>): {}", display_ty(hir[ty]))?;
            }
            Node::BoolLiteral(b) => {
                cwrite!(f, "#y<{b}>")?;
            }
            &Node::ArrayLiteral { elems, array_ty } => {
                cwrite!(f, "(#b<array> ")?;
                for (i, elem) in elems.iter().enumerate() {
                    if i != 0 {
                        cwrite!(f, " ")?;
                    }
                    cwrite!(f, "{}", display(elem))?;
                }
                cwrite!(f, "): {}", display_ty(hir[array_ty]))?;
            }
            Node::TupleLiteral { elems, elem_types } => {
                cwrite!(f, "(#b<tuple> ")?;
                for (i, (elem, ty)) in elems.iter().zip(elem_types.iter()).enumerate() {
                    if i != 0 {
                        cwrite!(f, " ")?;
                    }
                    cwrite!(f, "{}: {}", display(elem), display_ty(hir[ty]))?;
                }
                cwrite!(f, ")")?;
            }
            &Node::EnumLiteral {
                elems,
                elem_types,
                enum_ty,
            } => {
                cwrite!(f, "(#b<enum-literal>")?;
                for (i, (elem, ty)) in elems.iter().zip(elem_types.iter()).enumerate() {
                    if i != 0 {
                        cwrite!(f, " ")?;
                    }
                    cwrite!(f, "{}: {}", display(elem), display_ty(hir[ty]))?;
                }
                cwrite!(f, "): {}", display_ty(hir[enum_ty]))?;
            }
            Node::StringLiteral(s) => {
                cwrite!(f, "(#b<string> {s:?})")?;
            }
            Node::InferredEnumOrdinal(id) => {
                cwrite!(f, "(#b<enum-ordinal> {}{})", "#", id.0)?;
            }
            &Node::DeclareWithVal { pattern, val } => {
                cwrite!(
                    f,
                    "(#b<decl> {} {})",
                    display_pattern(pattern),
                    display(val),
                )?;
            }
            Node::Variable(id) => cwrite!(f, "#g<${}>", id.0)?,
            &Node::Global(module, id, ty) => cwrite!(
                f,
                "(#b<global> {}:{}): {}",
                module.idx(),
                id.idx(),
                display_ty(hir[ty])
            )?,
            &Node::Assign(lval, val, assign_ty, ty) => {
                let op = match assign_ty {
                    AssignType::Assign => "",
                    AssignType::AddAssign => "-add",
                    AssignType::SubAssign => "-sub",
                    AssignType::MulAssign => "-mul",
                    AssignType::DivAssign => "-div",
                    AssignType::ModAssign => "-mod",
                };
                cwrite!(
                    f,
                    "(#b<assign{op}> {}: {} {})",
                    display_lvalue(lval),
                    display_ty(hir[ty]),
                    display(val),
                )?;
            }
            &Node::Const { id, ty } => {
                cwrite!(f, "(#b<const> #y<{}>): {}", id.0, display_ty(hir[ty]))?
            }
            &Node::Negate(id, ty) => {
                cwrite!(f, "(#b<neg> {}): {}", display(id), display_ty(hir[ty]),)?;
            }
            &Node::Not(id) => cwrite!(f, "(#b<not> {})", display(id))?,
            &Node::AddressOf { value, value_ty } => cwrite!(
                f,
                "(#b<addr> {}: {})",
                display_lvalue(value),
                display_ty(hir[value_ty])
            )?,
            &Node::Deref { value, deref_ty } => cwrite!(
                f,
                "(#b<deref> {}): {}",
                display(value),
                display_ty(hir[deref_ty]),
            )?,
            &Node::Promote { value, variable } => cwrite!(
                f,
                "(#b<promote> {} #m<into> #g<${}>)",
                display(value),
                variable.0,
            )?,
            &Node::Cast(id) => {
                let cast = &hir[id];
                cwrite!(f, "(#b<cast> ")?;
                match cast.cast_ty {
                    CastType::Invalid => cwrite!(f, "#m<invalid>")?,
                    CastType::Noop => cwrite!(f, "#m<noop>")?,
                    CastType::Int { from, to } => cwrite!(
                        f,
                        "(#m<int> #r<{}> #r<{}>)",
                        <&str>::from(Primitive::from(from)),
                        <&str>::from(Primitive::from(to)),
                    )?,
                    CastType::Float { from, to } => cwrite!(
                        f,
                        "(#m<float> #r<{}> #r<{}>)",
                        <&str>::from(Primitive::from(from)),
                        <&str>::from(Primitive::from(to)),
                    )?,
                    CastType::IntToFloat { from, to } => cwrite!(
                        f,
                        "(#m<int-float> #r<{}> #r<{}>)",
                        <&str>::from(Primitive::from(from)),
                        <&str>::from(Primitive::from(to)),
                    )?,
                    CastType::FloatToInt { from, to } => cwrite!(
                        f,
                        "(#m<float-int> #r<{}> #r<{}>)",
                        <&str>::from(Primitive::from(from)),
                        <&str>::from(Primitive::from(to)),
                    )?,
                    CastType::IntToPtr { from } => cwrite!(
                        f,
                        "(#m<int-ptr> #r<{}>)",
                        <&str>::from(Primitive::from(from))
                    )?,
                    CastType::PtrToInt { to } => {
                        cwrite!(f, "(#m<ptr-int> #r<{}>)", <&str>::from(Primitive::from(to)))?
                    }
                    CastType::EnumToInt { from, to } => cwrite!(
                        f,
                        "(#b<enum-int> {} #r<{}>",
                        display_ty(hir[from]),
                        <&str>::from(Primitive::from(to))
                    )?,
                }
                cwrite!(f, " {})", display(cast.val))?;
            }
            &Node::Comparison {
                l,
                r,
                cmp,
                compared: _,
            } => {
                let cmp = match cmp {
                    Comparison::LT => "<",
                    Comparison::GT => ">",
                    Comparison::LE => "<=",
                    Comparison::GE => ">=",
                };
                cwrite!(f, "(#b<{cmp}> {} {})", display(l), display(r),)?
            }
            &Node::Logic { l, r, logic } => {
                let logic = match logic {
                    Logic::And => "and",
                    Logic::Or => "or",
                };
                cwrite!(f, "(#b<{logic}> {} {})", display(l), display(r),)?
            }
            &Node::Arithmetic(l, r, op, ty) => {
                let op = match op {
                    Arithmetic::Add => "+",
                    Arithmetic::Sub => "-",
                    Arithmetic::Mul => "*",
                    Arithmetic::Div => "/",
                    Arithmetic::Mod => "%",
                };
                cwrite!(
                    f,
                    "(#b<{op}> {} {}): {}",
                    display(l),
                    display(r),
                    display_ty(hir[ty]),
                )?;
            }
            &Node::Member {
                tuple_value,
                index,
                tuple_ty: tuple_type,
            } => {
                cwrite!(
                    f,
                    "(#b<element> #y<{index}> {}: {})",
                    display(tuple_value),
                    display_ty(hir[tuple_type])
                )?;
            }
            &Node::ArrayIndex {
                array,
                index,
                elem_ty,
            } => {
                cwrite!(
                    f,
                    "(#b<index> {} {}): {}",
                    display(array),
                    display(index),
                    display_ty(hir[elem_ty]),
                )?;
            }
            &Node::Return(val) => cwrite!(f, "(#b<return> {})", display(val),)?,
            &Node::IfElse {
                cond,
                then,
                else_,
                resulting_ty,
            } => {
                cwriteln!(f, "(if {}", display(cond))?;
                for branch in [then, else_] {
                    indent_n(f, indent_count + 1)?;
                    cwriteln!(f, "{}", display_indented(branch))?;
                }
                indent(f)?;
                cwrite!(f, "): {}", display_ty(hir[resulting_ty]))?;
            }
            &Node::IfPatElse {
                pat,
                val,
                then,
                else_,
                resulting_ty,
            } => {
                cwriteln!(f, "(#b<if-pat> {} {}", display_pattern(pat), display(val),)?;
                for branch in [then, else_] {
                    indent_n(f, indent_count + 1)?;
                    cwriteln!(f, "{}", display_indented(branch))?;
                }
                indent(f)?;
                cwrite!(f, "): {}", display_ty(hir[resulting_ty]))?;
            }
            &Node::Match {
                value,
                branch_index,
                pattern_index,
                branch_count,
                resulting_ty,
            } => {
                cwrite!(f, "(#b<match> {}", display(value),)?;
                for i in 0..branch_count {
                    cwriteln!(f)?;
                    indent_n(f, indent_count + 1)?;
                    let pattern = PatternId(pattern_index + i);
                    let branch = NodeId(branch_index + i);
                    cwrite!(
                        f,
                        "{} {}",
                        display_pattern(pattern),
                        display_indented(branch)
                    )?;
                }
                cwriteln!(f)?;
                indent(f)?;
                cwrite!(f, "): {}", display_ty(hir[resulting_ty]))?;
            }
            &Node::While { cond, body } => {
                cwrite!(f, "(#b<while> {} {})", display(cond), display(body),)?
            }
            &Node::WhilePat { pat, val, body } => cwrite!(
                f,
                "(#b<while> {} {} {})",
                display_pattern(pat),
                display(val),
                display(body),
            )?,
            &Node::Call {
                function,
                args,
                arg_types,
                return_ty,
                noreturn: _,
            } => {
                cwrite!(f, "(#b<call> {}", display(function))?;
                for (arg, ty) in args.iter().zip(arg_types.iter()) {
                    cwrite!(f, " {}: {}", display(arg), display_ty(hir[ty]),)?;
                }
                cwrite!(f, "): {}", display_ty(hir[return_ty]))?;
            }
            &Node::TraitCall {
                trait_id,
                trait_generics,
                method_index,
                self_ty,
                args,
                return_ty,
                noreturn: _,
            } => {
                cwrite!(
                    f,
                    "(#b<trait-call> #r<{}>",
                    compiler.get_trait_name(trait_id.0, trait_id.1)
                )?;
                if trait_generics.count > 0 {
                    cwrite!(f, "[")?;
                    for (i, generic) in trait_generics.iter().enumerate() {
                        if i != 0 {
                            cwrite!(f, ", ")?;
                        }
                        cwrite!(f, "{}", display_ty(hir[generic]))?;
                    }
                    cwrite!(f, "]")?;
                }
                cwrite!(f, " #m<as> {}", display_ty(hir[self_ty]))?;
                cwrite!(f, ".#y<{method_index}>")?;
                for arg in args.iter() {
                    cwrite!(f, " {}", display(arg))?;
                }
                cwrite!(f, "): {}", display_ty(hir[return_ty]))?;
            }
            &Node::TypeProperty(ty, property) => {
                let prop = match property {
                    TypeProperty::Size => "size",
                    TypeProperty::Align => "align",
                    TypeProperty::Stride => "stride",
                };
                cwrite!(f, "(#b<type_prop> #c<{prop}> #r<{}>)", display_ty(hir[ty]))?;
            }
            &Node::FunctionItem {
                function,
                generics,
                ty,
            } => {
                let name = compiler.get_function_name(function.0, function.1);
                cwrite!(f, "(#b<{name}>[")?;
                for (i, generic) in generics.iter().enumerate() {
                    if i != 0 {
                        cwrite!(f, " ")?;
                    }
                    cwrite!(f, "{}", display_ty(hir[generic]))?;
                }
                cwrite!(f, "]): {}", display_ty(hir[ty]))?;
            }
            &Node::Capture(id) => cwrite!(f, "(#b<capture> #y<{}>)", id.0)?,
            &Node::Break(n) => cwrite!(f, "(#b<break> #y<{n}>)")?,
            &Node::Continue(n) => cwrite!(f, "(#b<continue> #y<{n}>)")?,
            &Node::Type { value, value_ty } => {
                cwrite!(
                    f,
                    "(#b<type> {}): {}",
                    display_ty(value),
                    display_ty(hir[value_ty])
                )?;
            }
        }
        Ok(())
    }
}

pub struct PatternDisplay<'a> {
    pub pattern: PatternId,
    pub hir: &'a Hir,
    pub compiler: &'a Compiler,
    pub generics: &'a Generics,
}
impl<'a> fmt::Display for PatternDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let &Self {
            pattern,
            hir,
            compiler,
            generics,
        } = self;
        let display_ty = |ty| compiler.types.display(ty, generics);
        match &hir[pattern] {
            Pattern::Invalid => cwrite!(f, "#m<invalid>"),
            Pattern::Variable(id) => {
                let var_ty = hir.vars[id.idx()].ty();
                cwrite!(f, "#g<${}>: {}", id.0, display_ty(hir[var_ty]))
            }
            Pattern::Ignore => cwrite!(f, "_"),
            &Pattern::Tuple {
                member_count,
                patterns,
                types: type_ids,
            } => {
                cwrite!(f, "(")?;
                let pats = PatternIds {
                    index: patterns,
                    count: member_count,
                };
                let type_ids = LocalTypeIds {
                    idx: type_ids,
                    count: member_count,
                };
                for (i, pat) in pats.iter().enumerate() {
                    if i != 0 {
                        cwrite!(f, " ")?;
                    }
                    cwrite!(f, "{}", hir.display_pattern(pat, compiler, generics))?;
                }
                cwrite!(f, "): (")?;
                for (i, ty) in type_ids.iter().enumerate() {
                    if i != 0 {
                        cwrite!(f, " ")?;
                    }
                    cwrite!(f, "{}", display_ty(hir[ty]))?;
                }
                cwrite!(f, ")")
            }
            &Pattern::Int(signed, unsigned_val, ty) => cwrite!(
                f,
                "(#b<int> #y<{}{}>): {}",
                if signed { "-" } else { "" },
                unsigned_val,
                display_ty(hir[ty]),
            ),

            Pattern::Bool(b) => cwrite!(f, "#m<{b}>"),
            Pattern::String(s) => cwrite!(f, "(#m<string> #y<{s:?}>)"),
            &Pattern::Range {
                min_max,
                ty: _,
                min_max_signs,
                inclusive,
            } => {
                cwrite!(
                    f,
                    "(#b<range> {}{} {}{}#m<{}>)",
                    if min_max_signs.0 { "-" } else { "" },
                    min_max.0,
                    if min_max_signs.1 { "-" } else { "" },
                    min_max.1,
                    if inclusive { " inclusive" } else { "" }
                )
            }
            &Pattern::EnumVariant {
                ordinal,
                types: enum_types,
                args,
            } => {
                cwrite!(
                    f,
                    "(#b<enum-variant> #y<{ordinal:?}>: {}",
                    display_ty(hir[LocalTypeId(enum_types)]),
                )?;
                for (arg, ty) in args.iter().zip(
                    LocalTypeIds {
                        idx: enum_types + 1,
                        count: args.count,
                    }
                    .iter(),
                ) {
                    cwrite!(
                        f,
                        " {}: {}",
                        hir.display_pattern(arg, compiler, generics),
                        display_ty(hir[ty]),
                    )?;
                }
                cwrite!(f, ")")
            }
        }
    }
}

pub struct LValueDisplay<'a> {
    pub lval: LValueId,
    pub hir: &'a Hir,
    pub compiler: &'a Compiler,
    pub generics: &'a Generics,
    pub indent_count: usize,
}
impl<'a> fmt::Display for LValueDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            lval,
            hir,
            compiler,
            generics,
            indent_count,
        } = self;
        let indent_count = *indent_count;
        let display = |node| hir.display(node, compiler, generics, indent_count);
        let display_lvalue = |node| hir.display_lvalue(node, compiler, generics, indent_count);
        let display_ty = |ty| compiler.types.display(ty, generics);
        match hir[*lval] {
            LValue::Invalid => cwrite!(f, "(#b<invalid>)"),
            LValue::Variable(id) => cwrite!(f, "(#b<var> #y<{}>)", id.0),
            LValue::Global(module, id) => cwrite!(f, "(#b<global> {} {})", module.idx(), id.idx()),
            LValue::Deref(val) => {
                cwrite!(f, "(deref {})", display(val))
            }
            LValue::Member {
                tuple_value: tuple,
                index,
                tuple_ty,
            } => cwrite!(
                f,
                "(#b<member> #y<{index}> {}: {})",
                display_lvalue(tuple),
                display_ty(hir[tuple_ty]),
            ),

            LValue::ArrayIndex {
                array,
                index,
                elem_ty: element_type,
            } => cwrite!(
                f,
                "(#b<index> {} {}): {}",
                display_lvalue(array),
                display(index),
                display_ty(hir[element_type]),
            ),
        }
    }
}
