use error::span::TSpan;
use parser::ast::{
    Attribute, BaseImpl, Definition, Expr, ExprId, ExprIds, Generics, IdentPath, InherentImpl,
    ScopeId, Token, UnresolvedType,
};

use crate::{
    ALLOWED_NEWLINES_SCOPE, Cst,
    dom::{Cond, Node, R},
};

pub fn module(cst: &Cst) -> Node {
    let mut nodes = Vec::new();
    let mut converter = Converter {
        cst,
        pos: 0,
        allowed_newlines_before_next: Some(ALLOWED_NEWLINES_SCOPE),
    };
    converter.scope_contents(&mut nodes, cst.top_level_scope_id());
    converter.emit_whitespace(&mut nodes, &cst.src()[converter.pos as usize..], 0);
    Node::Group(nodes, R::Width(0))
}

struct Converter<'a> {
    cst: &'a Cst,
    pos: u32,
    allowed_newlines_before_next: Option<u32>,
}

impl<'a> Converter<'a> {
    pub fn keep_user_newlines(&mut self) {
        self.allowed_newlines_before_next = Some(crate::ALLOWED_NEWLINES);
    }

    pub fn keep_user_newlines_scope(&mut self) {
        self.allowed_newlines_before_next = Some(crate::ALLOWED_NEWLINES_SCOPE);
    }

    fn tok(&mut self, nodes: &mut Vec<Node>, token: Token) {
        self.tok_with_char(nodes, TSpan::new(token.start, token.end), None);
    }

    fn tok_s(&mut self, nodes: &mut Vec<Node>, token: Token) {
        self.tok_with_char(nodes, TSpan::new(token.start, token.end), Some(' '));
    }

    fn tok_span(&mut self, nodes: &mut Vec<Node>, span: TSpan) {
        self.tok_with_char(nodes, span, None);
    }

    fn tok_with_char(&mut self, nodes: &mut Vec<Node>, span: TSpan, c: Option<char>) {
        debug_assert!(span.start <= span.end);
        self.whitespace_before(nodes, span.start);
        let mut text = self.cst.src()[span.start as usize..span.end as usize].to_owned();
        if let Some(c) = c {
            text.push(c);
        }
        nodes.push(Node::Text(text.into()));
        self.pos = span.end;
    }

    fn whitespace_before(&mut self, nodes: &mut Vec<Node>, before: u32) {
        let ws = &self.cst.src()[self.pos as usize..before as usize];
        self.pos = before;
        debug_assert!(self.pos <= before);
        let max_newlines = self
            .allowed_newlines_before_next
            .take()
            .map_or(0, |x| x + 1);
        self.emit_whitespace(nodes, ws, max_newlines);
    }

    #[must_use]
    fn ty(&mut self, ty: &UnresolvedType) -> Node {
        // TODO: types currently don't preserve comments, they will probably get merged into the
        // expr grammar anyways
        let mut s = String::new();
        ty.to_string(&mut s, self.cst.src());
        Node::Text(s.into_boxed_str())
    }

    fn scope_contents(&mut self, nodes: &mut Vec<Node>, scope: ScopeId) {
        let scope = &self.cst[scope];
        let mut defs: Vec<_> = scope.definitions.values().collect();
        // sort the items by their order of appearence so it is preserved
        defs.sort_by_key(|&def| self.def_start(def));
        for item in defs {
            self.def(nodes, item);
        }
    }

    fn def_start(&self, def: &Definition<Token>) -> u32 {
        match def {
            Definition::Expr { t_name, .. } => t_name.start,
            Definition::Use { t_use, .. } => t_use.start,
            &Definition::Global(global_id) => self.cst[global_id].t_name.start,
            Definition::Module(_) | Definition::Generic(_) => 0,
        }
    }

    fn def(&mut self, nodes: &mut Vec<Node>, item: &Definition<Token>) {
        self.keep_user_newlines_scope();
        match item {
            &Definition::Expr {
                t_name,
                t_colon_colon,
                id,
            } => {
                let (expr, ty) = &self.cst[id];
                self.tok_s(nodes, t_name);
                match t_colon_colon {
                    parser::ast::Either::A(t) => {
                        self.tok_s(nodes, t);
                    }
                    parser::ast::Either::B((a, b)) => {
                        self.tok_s(nodes, a);
                        nodes.push(self.ty(ty));
                        nodes.push(" ".into());
                        self.tok_s(nodes, b);
                    }
                }
                self.expr(nodes, *expr);
            }
            &Definition::Use { t_use, path, id: _ } => {
                self.tok_s(nodes, t_use);
                self.path(nodes, path);
            }
            &Definition::Global(global_id) => {
                let global = &self.cst[global_id];
                match global.t_colon_and_equals_or_colon_equals {
                    parser::ast::Either::A((colon, equals)) => {
                        self.tok(nodes, global.t_name);
                        self.tok_s(nodes, colon);
                        nodes.push(self.ty(&global.ty));
                        self.tok_s(nodes, equals);
                    }
                    parser::ast::Either::B(colon_equals) => {
                        self.tok_s(nodes, global.t_name);
                        self.tok_s(nodes, colon_equals);
                    }
                }
                self.expr(nodes, global.val);
            }
            Definition::Module(_) | Definition::Generic(_) => {}
        }
        nodes.push(Node::Text("\n".into()));
    }

    fn path(&mut self, nodes: &mut Vec<Node>, path: IdentPath) {
        let s = &self.cst.src()[path.span().range()];
        // TODO: format paths
        nodes.push(Node::Text(s.into()));
    }

    fn expr(&mut self, nodes: &mut Vec<Node>, id: ExprId) {
        #[allow(unused_variables)] // FIXME: remove when all branches implemented
        match &self.cst[id] {
            Expr::Error(span) => {
                nodes.push(Node::Text(self.cst.src()[span.range()].into()));
            }
            &Expr::Block {
                t_open,
                scope,
                items,
                t_close,
            } => {
                let mut block_nodes = Vec::new();
                self.tok(&mut block_nodes, t_open);
                // split the contents of the block into two groups: definitions and statements
                // this is done since we want to sort all the definitions at the start but still
                // have to iterate in order of appearance to handle whitespace properly
                let mut def_group = Vec::new();
                let mut statement_group = Vec::new();
                let scope = &self.cst[scope];
                enum DefOrStatement<'a> {
                    Def(&'a str, &'a Definition<Token>),
                    Expr(ExprId),
                }
                let mut items: Vec<_> = scope
                    .definitions
                    .iter()
                    .map(|(name, def)| DefOrStatement::Def(name, def))
                    .chain(items.into_iter().map(DefOrStatement::Expr))
                    .collect();
                items.sort_by_key(|item| match item {
                    DefOrStatement::Def(_, def) => self.def_start(def),
                    &DefOrStatement::Expr(id) => self.cst[id].start(self.cst),
                });
                let mut first_expr = true;
                for item in items {
                    match item {
                        DefOrStatement::Def(name, def) => {
                            self.def(&mut def_group, def);
                        }
                        DefOrStatement::Expr(id) => {
                            if !first_expr || statement_group.is_empty() {
                                statement_group.push(Node::Text("\n".into()));
                            }
                            self.keep_user_newlines();
                            first_expr = false;
                            self.expr(&mut statement_group, id);
                        }
                    }
                }
                let mut group = def_group;
                if !group.is_empty() && !statement_group.is_empty() {
                    group.push(Node::Text("\n\n".into()));
                }
                group.append(&mut statement_group);

                let empty = group.is_empty();
                if !empty {
                    group.push(Node::Text("\n".into()));
                }
                self.close_group(&mut block_nodes, group, t_close);
                nodes.push(Node::Group(block_nodes, R::Width(0)));
            }
            &Expr::Nested {
                span,
                t_lparen,
                inner,
                t_rparen,
            } => {
                self.tok(nodes, t_lparen);
                self.expr(nodes, inner);
                self.tok(nodes, t_rparen);
            }
            Expr::IntLiteral { t, .. }
            | Expr::FloatLiteral { t, .. }
            | Expr::StringLiteral { t, .. }
            | Expr::Ident { t, .. }
            | Expr::Primitive { t, .. }
            | Expr::Hole { t, .. }
            | Expr::Root { t, .. }
            | Expr::ReturnUnit { t, .. }
            | Expr::Break { t, .. }
            | Expr::Continue { t, .. } => self.tok(nodes, *t),
            &Expr::Array {
                t_lbracket,
                elements,
                t_rbracket,
                ..
            } => {
                self.tok(nodes, t_lbracket);
                let mut group = Vec::new();
                let mut first = true;
                group.push(Cond::Broken.then("\n"));
                for elem in elements {
                    // TODO: not handling the comma as a token here so comments may get lost
                    if !first {
                        group.push(Cond::Flat.then(", "));
                        group.push(Cond::Broken.then("\n"));
                    }
                    first = false;
                    self.expr(&mut group, elem);
                }
                group.push(Cond::Broken.then("\n"));
                self.close_group(nodes, group, t_rbracket);
            }
            Expr::Tuple {
                span,
                t_lparen,
                elements,
                named_elements,
                t_rparen,
            } => {
                self.args_with_extra(nodes, *t_lparen, *t_rparen, *elements, |s, nodes, first| {
                    if elements.len() == 1 && named_elements.is_empty() {
                        nodes.push(Node::Text(",".into()));
                        nodes.push(Cond::Broken.then("\n"));
                    } else {
                        for &(name, arg) in named_elements {
                            if !*first {
                                nodes.push(Cond::Flat.then(", "));
                                nodes.push(Cond::Broken.then("\n"));
                            }
                            *first = false;
                            s.tok_span(nodes, name);
                            nodes.push(": ".into());
                            s.expr(nodes, arg);
                        }
                    }
                });
            }
            &Expr::EnumLiteral {
                span,
                t_dot,
                ident,
                t_ident,
                t_parens,
                args,
            } => {
                self.tok(nodes, t_dot);
                self.tok(nodes, t_ident);
                if let Some((l, r)) = t_parens {
                    self.args(nodes, l, r, args);
                }
            }
            &Expr::Function { id } => self.function(nodes, id),
            Expr::DeclareWithVal {
                pat,
                t_colon_and_equals_or_colon_equals,
                annotated_ty,
                val,
            } => {
                self.expr(nodes, *pat);
                match *t_colon_and_equals_or_colon_equals {
                    parser::ast::Either::A((colon, equals)) => {
                        self.tok_s(nodes, colon);
                        nodes.push(self.ty(annotated_ty));
                        nodes.push(" ".into());
                        self.tok_s(nodes, equals);
                    }
                    parser::ast::Either::B(colon_equals) => {
                        nodes.push(Node::Text(" ".into()));
                        self.tok_s(nodes, colon_equals);
                    }
                }
                self.expr(nodes, *val);
            }
            &Expr::BinOp { t_op, op: _, l, r } => {
                self.expr(nodes, l);
                nodes.push(Node::Text(" ".into()));
                self.tok_s(nodes, t_op);
                self.expr(nodes, r);
            }
            &Expr::Return { start, t_ret, val } => {
                self.tok_s(nodes, t_ret);
                self.expr(nodes, val);
            }
            &Expr::Type { id } => {
                let def = &self.cst[id];
                self.tok(nodes, def.t_introducer);
                self.generics(nodes, &def.generics);
                nodes.push(" ".into());
                self.tok(nodes, def.t_lbrace);
                nodes.push("\n".into());
                let mut group = Vec::new();
                match &def.content {
                    parser::ast::TypeContent::Struct { members } => {
                        for member in members {
                            self.keep_user_newlines();
                            self.attributes(&mut group, &member.attributes);
                            self.tok_with_char(&mut group, member.name, Some(' '));
                            group.push(self.ty(&member.ty));
                            group.push("\n".into());
                        }
                    }
                    parser::ast::TypeContent::Enum { variants } => {
                        for variant in variants {
                            self.keep_user_newlines();
                            self.tok_span(&mut group, variant.name_span);
                            self.generics_instance(&mut group, variant.t_parens, &variant.args);
                            group.push("\n".into());
                        }
                    }
                }
                let mut methods: Vec<_> = def.methods.values().collect();
                methods.sort_by_key(|m| m.name.start);
                for method in methods {
                    self.method(&mut group, method);
                }

                for InherentImpl {
                    attributes,
                    t_impl,
                    implemented_trait,
                    base,
                } in &def.impls
                {
                    self.keep_user_newlines();
                    self.attributes(&mut group, attributes);
                    self.tok(&mut group, *t_impl);
                    self.generics(&mut group, &base.generics);
                    group.push(" ".into());
                    self.path(&mut group, *implemented_trait);
                    self.generics_instance(&mut group, base.t_brackets, &base.trait_generics);
                    group.push(" ".into());
                    self.impl_body(&mut group, base);
                    group.push("\n".into());
                }
                self.close_group(nodes, group, def.t_rbrace);
            }
            &Expr::Trait { id } => {
                let def = &self.cst[id];
                self.tok(nodes, def.t_trait);
                self.generics(nodes, &def.generics);
                nodes.push(" ".into());
                self.tok(nodes, def.t_lbrace);
                if !def.functions.is_empty() {
                    nodes.push("\n".into());
                }
                let mut group = Vec::new();
                for method in &def.functions {
                    self.method(&mut group, method);
                }
                self.close_group(nodes, group, def.t_rbrace);
                if let Some((t_for, lbrace, rbrace)) = def.t_attached_impls {
                    nodes.push(" ".into());
                    self.tok_s(nodes, t_for);
                    self.tok(nodes, lbrace);
                    let mut group = Vec::new();
                    if !def.impls.is_empty() {
                        group.push("\n".into());
                    }
                    for impl_ in &def.impls {
                        self.impl_(&mut group, impl_);
                    }
                    self.whitespace_before(&mut group, rbrace.start);
                    nodes.push(Node::Indent(Box::new(Node::Group(group, R::Width(0)))));
                    nodes.push("\n".into());
                    self.tok(nodes, rbrace);
                }
            }
            &Expr::UnOp {
                t_op, op, inner, ..
            } => {
                if op.postfix() {
                    self.expr(nodes, inner);
                    self.tok(nodes, t_op);
                } else {
                    self.tok(nodes, t_op);
                    self.expr(nodes, inner);
                }
            }
            Expr::As { value, t_as, ty } => {
                self.expr(nodes, *value);
                nodes.push(" ".into());
                self.tok_s(nodes, *t_as);
                nodes.push(self.ty(ty));
            }
            &Expr::MemberAccess { left, t_dot, name } => {
                self.expr(nodes, left);
                self.tok(nodes, t_dot);
                self.tok_span(nodes, name);
            }
            &Expr::Index {
                expr,
                t_lbracket,
                idx,
                t_rbracket,
                ..
            } => {
                self.expr(nodes, expr);
                self.tok(nodes, t_lbracket);
                self.expr(nodes, idx);
                self.tok(nodes, t_rbracket);
            }
            &Expr::TupleIdx {
                left, t_dot, t_int, ..
            } => {
                self.expr(nodes, left);
                self.tok(nodes, t_dot);
                self.tok(nodes, t_int);
            }
            &Expr::If {
                t_if,
                cond,
                t_colon,
                then,
                ..
            } => self.if_else(nodes, t_if, cond, None, t_colon, then, None),
            &Expr::IfElse {
                t_if,
                cond,
                t_colon,
                then,
                t_else,
                else_,
                ..
            } => self.if_else(
                nodes,
                t_if,
                cond,
                None,
                t_colon,
                then,
                Some((t_else, else_)),
            ),
            &Expr::IfPat {
                t_if,
                t_colon_eq,
                pat,
                t_colon,
                value,
                then,
                ..
            } => self.if_else(
                nodes,
                t_if,
                pat,
                Some((t_colon_eq, value)),
                t_colon,
                then,
                None,
            ),
            &Expr::IfPatElse {
                t_if,
                t_colon_eq,
                pat,
                t_colon,
                value,
                then,
                t_else,
                else_,
                ..
            } => self.if_else(
                nodes,
                t_if,
                pat,
                Some((t_colon_eq, value)),
                t_colon,
                then,
                Some((t_else, else_)),
            ),
            &Expr::Match {
                t_match,
                val,
                t_lbrace,
                branches,
                t_rbrace,
                ..
            } => {
                self.tok_s(nodes, t_match);
                self.expr(nodes, val);
                nodes.push(" ".into());
                self.tok(nodes, t_lbrace);
                nodes.push("\n".into());
                let mut group = Vec::new();
                for (pat, val) in branches {
                    self.keep_user_newlines();
                    self.expr(&mut group, pat);
                    group.push(": ".into());
                    self.expr(&mut group, val);
                    group.push(",\n".into());
                }
                self.close_group(nodes, group, t_rbrace);
            }
            &Expr::While {
                t_while,
                cond,
                t_colon,
                body,
                ..
            } => {
                self.tok_s(nodes, t_while);
                self.expr(nodes, cond);
                if let Some(t) = t_colon {
                    self.tok_s(nodes, t);
                } else {
                    nodes.push(" ".into());
                }
                self.expr(nodes, body);
            }
            &Expr::WhilePat {
                t_while,
                pat,
                t_colon_eq,
                val,
                t_colon,
                body,
                ..
            } => {
                self.tok_s(nodes, t_while);
                self.expr(nodes, pat);
                nodes.push(" ".into());
                self.tok_s(nodes, t_colon_eq);
                self.expr(nodes, val);
                self.opt_tok_space(nodes, t_colon);
                self.expr(nodes, body);
            }
            &Expr::For {
                t_for,
                pat,
                t_in,
                iter,
                t_colon,
                body,
                ..
            } => {
                self.tok_s(nodes, t_for);
                self.expr(nodes, pat);
                nodes.push(" ".into());
                self.tok_s(nodes, t_in);
                self.expr(nodes, iter);
                self.opt_tok_space(nodes, t_colon);
                self.expr(nodes, body);
            }
            &Expr::FunctionCall(call_id) => {
                let call = &self.cst[call_id];
                self.expr(nodes, call.called_expr);
                self.args_with_extra(
                    nodes,
                    call.t_lparen,
                    call.t_rparen,
                    call.args,
                    |s, nodes, first| {
                        for &(name, arg) in &call.named_args {
                            if !*first {
                                nodes.push(Cond::Flat.then(", "));
                                nodes.push(Cond::Broken.then("\n"));
                            }
                            *first = false;
                            s.tok_span(nodes, name);
                            nodes.push(": ".into());
                            s.expr(nodes, arg);
                        }
                    },
                );
            }
            &Expr::Asm {
                t_asm,
                t_lparen,
                t_string_literal,
                args,
                t_rparen,
                ..
            } => {
                self.tok(nodes, t_asm);
                self.tok(nodes, t_lparen);
                let mut group = vec![Cond::Broken.then("\n")];
                self.tok(&mut group, t_string_literal);
                for arg in args {
                    group.push(Cond::Flat.then(", "));
                    group.push(Cond::Broken.then("\n"));
                    self.expr(&mut group, arg);
                }
                group.push(Cond::Broken.then("\n"));
                self.close_group(nodes, group, t_rparen);
            }
        }
    }

    fn opt_tok_space(&mut self, nodes: &mut Vec<Node>, t: Option<Token>) {
        if let Some(t) = t {
            self.tok_s(nodes, t);
        } else {
            nodes.push(" ".into());
        }
    }

    fn attributes(&mut self, nodes: &mut Vec<Node>, attributes: &[Attribute<Token>]) {
        for attribute in attributes {
            self.tok(nodes, attribute.t_at);
            self.path(nodes, attribute.path);
            if let Some((l, r)) = attribute.t_parens {
                self.args(nodes, l, r, attribute.args);
            }
        }
        if !attributes.is_empty() {
            nodes.push(" ".into());
        }
    }

    fn if_else(
        &mut self,
        nodes: &mut Vec<Node>,
        t_if: Token,
        cond: ExprId,
        pat: Option<(Token, ExprId)>,
        t_colon: Option<Token>,
        then: ExprId,
        else_: Option<(Token, ExprId)>,
    ) {
        let mut group = Vec::new();
        self.tok_s(&mut group, t_if);
        self.expr(&mut group, cond);
        if let Some((colon_colon, val)) = pat {
            group.push(" ".into());
            self.tok_s(&mut group, colon_colon);
            self.expr(&mut group, val);
        }
        if let Some(t_colon) = t_colon {
            self.tok_s(&mut group, t_colon);
        } else {
            group.push(" ".into());
        }
        self.expr(&mut group, then);
        if let Some((t_else, else_)) = else_ {
            if t_colon.is_some() {
                group.push(Cond::Flat.then(" "));
                group.push(Cond::Broken.then("\n"));
            } else {
                group.push(" ".into());
            }
            self.tok_s(&mut group, t_else);
            self.expr(&mut group, else_);
        }
        nodes.push(Node::Group(group, R::Width(0)));
    }

    fn args(&mut self, nodes: &mut Vec<Node>, l: Token, r: Token, elements: ExprIds) {
        self.args_with_extra(nodes, l, r, elements, |_, _, _| {});
    }

    fn args_with_extra<F: FnOnce(&mut Self, &mut Vec<Node>, &mut bool)>(
        &mut self,
        nodes: &mut Vec<Node>,
        l: Token,
        r: Token,
        elements: ExprIds,
        extra_end: F,
    ) {
        self.tok(nodes, l);
        let mut first = true;
        let mut group = Vec::new();
        group.push(Cond::Broken.then("\n"));
        for element in elements {
            if !first {
                // TODO: comma token
                group.push(Cond::Flat.then(", "));
                group.push(Cond::Broken.then("\n"));
            }
            self.expr(&mut group, element);
            first = false;
        }
        extra_end(self, &mut group, &mut first);
        if !first {
            group.push(Cond::Broken.then("\n"));
        }
        self.close_group(nodes, group, r);
    }

    fn emit_whitespace(&self, nodes: &mut Vec<Node>, ws: &str, max_newlines: u32) {
        let mut chars = ws.char_indices().peekable();
        enum State {
            WS,
            Single,
            Multi(u32),
        }
        let mut state = State::WS;
        let mut comment_start = 0;
        let mut newline_count = 0;
        while let Some((pos, c)) = chars.next() {
            match c {
                '#' if chars.next_if(|(_, c)| *c == '-').is_some() => match &mut state {
                    State::WS => {
                        if newline_count != 0 {
                            nodes.push(Node::Text(
                                "\n".repeat(newline_count.min(max_newlines as usize))
                                    .into_boxed_str(),
                            ));
                            newline_count = 0;
                        }
                        state = State::Multi(1);
                        comment_start = pos;
                    }
                    State::Single => {}
                    State::Multi(n) => *n += 1,
                },
                '#' if matches!(state, State::WS) => {
                    if newline_count != 0 {
                        nodes.push(Node::Text(
                            "\n".repeat(newline_count.min(max_newlines as usize))
                                .into_boxed_str(),
                        ));
                        newline_count = 0;
                    }
                    state = State::Single;
                    comment_start = pos;
                }
                '-' if chars.next_if(|(_, c)| *c == '#').is_some() => match &mut state {
                    State::WS => panic!("Invalid closing multi-line comment in whitespace"),
                    State::Single => {}
                    State::Multi(n) => {
                        *n -= 1;
                        if *n == 0 {
                            nodes.push(Node::Text(ws[comment_start..pos + 2].into()));
                            state = State::WS;
                        }
                    }
                },
                '\n' => match state {
                    State::WS => newline_count += 1,
                    State::Single => {
                        nodes.push(Node::Text(ws[comment_start..pos + 1].into()));
                        state = State::WS;
                    }
                    State::Multi(_) => {}
                },
                _ => {}
            }
        }
        if newline_count != 0 {
            nodes.push(Node::Text(
                "\n".repeat(newline_count.min(max_newlines as usize))
                    .into_boxed_str(),
            ));
        }
    }

    fn generics(&mut self, nodes: &mut Vec<Node>, generics: &Generics<Token>) {
        if let Some((l, r)) = generics.t_brackets {
            self.tok(nodes, l);
            let mut group = Vec::new();
            let mut first = true;
            // TODO: probably breaks with inhereted generics
            for generic in &generics.types {
                if generic.name == TSpan::MISSING {
                    continue;
                }
                if !first {
                    group.push(Cond::Flat.then(", "));
                }
                group.push(Cond::Broken.then("\n"));
                first = false;
                self.tok_span(&mut group, generic.name);
                if let Some(t) = generic.t_colon {
                    self.tok_s(&mut group, t);
                }
                let mut bounds_group = Vec::new();
                let mut first_bound = true;
                for bound in &generic.bounds {
                    if !first_bound {
                        bounds_group.push(Cond::Broken.then("\n+ "));
                        bounds_group.push(Cond::Flat.then(" + "));
                    }
                    first_bound = false;
                    self.path(&mut bounds_group, bound.path);
                    self.generics_instance(&mut bounds_group, bound.brackets, &bound.generics);
                }
                group.push(Node::Indent(Box::new(Node::Group(
                    bounds_group,
                    R::Width(0),
                ))));
            }
            group.push(Cond::Broken.then("\n"));
            self.close_group(nodes, group, r);
        }
    }

    fn generics_instance(
        &mut self,
        nodes: &mut Vec<Node>,
        brackets: Option<(Token, Token)>,
        generics: &[UnresolvedType],
    ) {
        let Some((l, r)) = brackets else { return };
        self.tok(nodes, l);
        let mut group = Vec::new();
        let mut first = true;
        for ty in generics {
            if !first {
                group.push(Cond::Flat.then(", "));
                group.push(Cond::Broken.then("\n"));
            }
            group.push(self.ty(ty));
            first = false;
        }
        self.close_group(nodes, group, r);
    }

    fn function(&mut self, nodes: &mut Vec<Node>, id: parser::ast::FunctionId) {
        let function = &self.cst[id];
        let mut group = Vec::new();
        self.attributes(&mut group, &function.attributes);
        self.tok(&mut group, function.t_fn);
        self.generics(&mut group, &function.generics);
        if let Some((l, r)) = function.t_parens {
            self.tok(&mut group, l);
            let mut args = Vec::new();
            let mut first = true;
            args.push(Cond::Broken.then("\n"));
            for (name_span, ty) in &function.params {
                if first {
                    first = false;
                    args.push(Cond::Broken.then("\n"));
                } else {
                    args.push(Cond::Flat.then(", "));
                }
                if matches!(ty, UnresolvedType::Infer(_)) {
                    self.tok_span(&mut args, *name_span);
                } else {
                    self.tok_span(&mut args, *name_span);
                    args.extend([" ".into(), self.ty(ty)]);
                }
                args.push(Cond::Broken.then("\n"));
            }
            for param in &function.named_params {
                if first {
                    first = false;
                    args.push(Cond::Broken.then("\n"));
                } else {
                    args.push(Cond::Flat.then(", "));
                }
                self.tok_span(&mut args, param.name);
                args.push(" ".into());
                if !matches!(param.ty, UnresolvedType::Infer(_)) {
                    args.extend([self.ty(&param.ty), " ".into()]);
                }
                self.tok_s(&mut args, param.t_eq);
                self.expr(&mut args, param.default_value);
                args.push(Cond::Broken.then("\n"));
            }
            if let Some(t) = function.t_varargs {
                if !first {
                    args.push(Cond::Flat.then(", "));
                }
                self.tok(&mut args, t);
            }
            self.whitespace_before(&mut args, r.start);
            self.close_group(&mut group, args, r);
        }
        if let Some(arrow) = function.t_arrow {
            group.push(" ".into());
            self.tok_s(&mut group, arrow);
            group.push(self.ty(&function.return_type));
        }

        if let Some(t_extern) = function.t_extern {
            group.push(" ".into());
            self.tok(&mut group, t_extern);
        }
        if let Some(t_colon) = function.t_colon {
            self.tok(&mut group, t_colon);
        }

        if let Some(body) = function.body {
            group.push(Node::Text(" ".into()));
            self.expr(&mut group, body);
        }

        nodes.push(Node::Group(group, R::Width(0)));
    }

    fn method(&mut self, nodes: &mut Vec<Node>, method: &parser::ast::Method<Token>) {
        self.keep_user_newlines();
        self.tok_with_char(nodes, method.name, Some(' '));
        self.tok_s(nodes, method.t_colon_colon);
        self.function(nodes, method.function);
        nodes.push("\n".into());
    }

    fn impl_(&mut self, nodes: &mut Vec<Node>, impl_: &parser::ast::Impl<Token>) {
        self.keep_user_newlines();
        self.tok(nodes, impl_.t_impl);
        self.generics(nodes, &impl_.base.generics);
        nodes.push(" ".into());
        self.tok(nodes, impl_.t_underscore);
        self.generics_instance(nodes, impl_.base.t_brackets, &impl_.base.trait_generics);
        nodes.push(" ".into());
        self.tok_s(nodes, impl_.t_for);
        nodes.push(self.ty(&impl_.implemented_type));
        nodes.push(" ".into());
        self.impl_body(nodes, &impl_.base);
    }

    fn impl_body(&mut self, nodes: &mut Vec<Node>, base: &BaseImpl<Token>) {
        self.tok(nodes, base.t_lbrace);
        nodes.push("\n".into());
        let mut group = Vec::new();
        for method in &base.functions {
            self.method(&mut group, method);
        }
        self.close_group(nodes, group, base.t_rbrace);
    }

    /// Ends an indented group while handling the whitespace before the closing token properly
    /// so a comment before a closing token is still indented with the group
    fn close_group(&mut self, nodes: &mut Vec<Node>, mut group: Vec<Node>, closing: Token) {
        self.whitespace_before(&mut group, closing.start);
        nodes.push(Node::Indent(Box::new(Node::Group(group, R::Width(0)))));
        self.tok(nodes, closing);
    }
}
