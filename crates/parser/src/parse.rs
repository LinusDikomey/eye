use dmap::DHashMap;

use std::collections::hash_map::Entry;

use crate::ast::{
    self, Attribute, Attributes, Definition, EnumVariantDefinition, Expr, ExprId, ExprIdPairs,
    ExprIds, Function, GenericDef, Generics, Global, Impl, InherentImpl, Item, ItemValue, Method,
    StructMember, TraitDefinition, TreeToken, UnOp, UnresolvedType,
};

use crate::unexpected;
use crate::{Delimit, ParseResult, Parser, ast::ScopeId};

use error::{
    CompileError, Error,
    span::{IdentPath, TSpan},
};
use token::{self, ExpectedTokens, Keyword, Operator, Token, TokenType};

macro_rules! step_or_unexpected {
    (
        $parser: ident,
        $tok: ident,
        $($tok_ty: ident $($kw: ident $(($kw_val: ident))? )? => $res: expr),*
    ) => {{
        let $tok = $parser.toks.step();
        match $tok.ty {
            $(
                TokenType::$tok_ty $((Keyword::$kw $(($kw_val))* ))* => $res,
            )*
            _ => return Err(unexpected(
                $tok,
                ExpectedTokens::AnyOf(Box::new([ $(TokenType::$tok_ty $((Keyword::$kw))*),* ])),
            ))
        }
    }};
}

fn t<T: TreeToken>(t: Token) -> T {
    T::t(t)
}

trait TokenExt {
    fn span(&self) -> TSpan;
}
impl TokenExt for Token {
    fn span(&self) -> TSpan {
        TSpan::new(self.start, self.end)
    }
}

impl<T: TreeToken> Parser<'_, T> {
    pub fn parse_module(&mut self, module_defs: DHashMap<String, Definition<T>>) -> ScopeId {
        self.parse_items_until(0, None, module_defs, None, Self::on_module_level_expr)
            .0
    }

    /// Parses a delimited list. The `item` function parses an item and is supposed to handle the
    /// result itself.
    /// `delim` is a delimiter. Trailing delimiters are allowed, but optional here.
    /// `end` is the ending token. It will be returned from this function once parsed.
    fn parse_delimited<F>(
        &mut self,
        open: Token,
        delim: TokenType,
        close: TokenType,
        item: F,
    ) -> ParseResult<Token>
    where
        F: FnMut(&mut Parser<T>) -> Result<Delimit, CompileError>,
    {
        self.parse_delimited_inner(open, delim, close, item)
            .map(|(closing, _trailing)| closing)
    }

    /// Similar to parse_delimited but also returns the trailing delimiter if present
    fn parse_delimited_inner<F>(
        &mut self,
        open: Token,
        delim: TokenType,
        close: TokenType,
        mut item: F,
    ) -> ParseResult<(Token, Option<Token>)>
    where
        F: FnMut(&mut Parser<T>) -> Result<Delimit, CompileError>,
    {
        let mut trailing_delim = None;
        let expected = || ExpectedTokens::AnyOf(Box::new([delim, close]));
        loop {
            if let Some(end_tok) = self.toks.step_if(close) {
                return Ok((end_tok, trailing_delim));
            } else if let Some(eof) = self.toks.step_if(TokenType::Eof) {
                return Err(unexpected(eof, expected()));
            }
            trailing_delim = None;
            match item(self) {
                Ok(Delimit::Yes) => match self.toks.step() {
                    tok if tok.ty == delim => {
                        trailing_delim = Some(tok);
                        continue;
                    }
                    tok if tok.ty == close => return Ok((tok, trailing_delim)),
                    tok => {
                        return Err(unexpected(
                            tok,
                            ExpectedTokens::AnyOf(Box::new([delim, close])),
                        ));
                    }
                },
                Ok(Delimit::No) => trailing_delim = None,
                Ok(Delimit::Optional) => trailing_delim = self.toks.step_if(delim),
                Ok(Delimit::OptionalIfNewLine) => {
                    trailing_delim = self.toks.step_if(delim);
                    if trailing_delim.is_none() {
                        let after = self.toks.peek();
                        if after.ty != TokenType::Eof && !after.new_line && after.ty != close {
                            self.recover_in_delimited(open, close);
                            return Err(unexpected(after, expected()));
                        }
                    }
                }
                Err(err) => {
                    self.recover_in_delimited(open, close);
                    return Err(err);
                }
            }
        }
    }

    fn recover_in_delimited(&mut self, open: Token, close: TokenType) {
        let mut open_count: u32 = 0;
        // try to find the close of the current enclosed delimited
        loop {
            match self.toks.step().ty {
                ty if ty == open.ty => open_count += 1,
                ty if ty == close => {
                    if let Some(x) = open_count.checked_sub(1) {
                        open_count = x
                    } else {
                        break;
                    }
                }
                TokenType::Eof => break,
                _ => {}
            }
        }
    }

    /// parses items until end token or eof if end is None
    /// returns the end position
    fn parse_items_until(
        &mut self,
        start: u32,
        parent: Option<ScopeId>,
        definitions: DHashMap<String, Definition<T>>,
        start_end: Option<(Token, TokenType)>,
        mut on_expr: impl FnMut(&mut Self, &mut ast::Scope<T>, ScopeId, Expr<T>, TSpan),
    ) -> (ScopeId, Token) {
        let scope_id = self.ast.scope(ast::Scope::missing());
        let mut scope = ast::Scope {
            parent,
            definitions,
            span: TSpan::MISSING, // span will be updated at the end of this function
            has_errors: false,
        };

        let end_tok = loop {
            if let Some((_, end)) = start_end {
                if let Some(end_tok) = self.toks.step_if(end) {
                    break end_tok;
                } else if let Some(eof) = self.toks.step_if(TokenType::Eof) {
                    self.toks
                        .errors
                        .emit_err(unexpected(eof, ExpectedTokens::Specific(end)));
                    break eof;
                }
            } else if let Some(eof) = self.toks.step_if(TokenType::Eof) {
                break eof;
            }

            let item = match self.parse_item(scope_id) {
                Ok(item) => item,
                Err(err) => {
                    scope.has_errors = true;
                    self.toks.errors.emit_err(err);
                    continue;
                }
            };
            match item.value {
                ItemValue::Definition {
                    t_name,
                    name,
                    name_span,
                    t_colon_colon,
                    annotated_ty,
                    value,
                } => {
                    // try to assign names of definitions
                    // this is a bit hacky but works for now
                    match *self.ast.get_expr(value) {
                        Expr::Function { id } => self.ast.assign_function_name(id, name_span),
                        Expr::Trait { id } => self.ast.assign_trait_name(id, name_span),
                        _ => {}
                    }
                    match scope.definitions.entry(name) {
                        Entry::Occupied(_) => {
                            scope.has_errors = true;
                            self.toks.errors.emit_err(
                                Error::DuplicateDefinition.at(name_span.start, name_span.end),
                            );
                        }
                        Entry::Vacant(vacant_entry) => {
                            vacant_entry.insert(Definition::Expr {
                                t_name,
                                name_span,
                                t_colon_colon,
                                id: self.ast.def_expr(value, annotated_ty),
                            });
                        }
                    }
                }
                ItemValue::Use { t_use, path } => {
                    let (_, _, name) = path.segments(self.toks.src);
                    let name_span = name.map_or_else(|| path.span(), |(_, span)| span);
                    let name = name.map_or("root", |(name, _)| name).to_owned();
                    match scope.definitions.entry(name) {
                        Entry::Occupied(_) => {
                            scope.has_errors = true;
                            self.toks
                                .errors
                                .emit_err(Error::DuplicateDefinition.at_span(name_span))
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(Definition::Use {
                                t_use,
                                path,
                                id: self.ast.next_use_id(),
                            });
                        }
                    }
                }
                ItemValue::Expr(r) => {
                    let span = TSpan {
                        start,
                        end: self.toks.pos(),
                    };
                    on_expr(self, &mut scope, scope_id, r, span);
                }
            };
        };
        scope.span = TSpan::new(start, end_tok.end);
        *self.ast.get_scope_mut(scope_id) = scope;
        (scope_id, end_tok)
    }

    fn on_module_level_expr(
        &mut self,
        scope: &mut ast::Scope<T>,
        scope_id: ScopeId,
        expr: Expr<T>,
        _: TSpan,
    ) {
        let expr = self.ast.expr(expr);
        match self.ast.get_expr(expr) {
            Expr::DeclareWithVal {
                pat,
                annotated_ty,
                val,
                t_colon_and_equals_or_colon_equals,
            } => {
                let start = self.ast.get_expr(expr).span_builder(self.ast).start;
                let end = self.ast.get_expr(*val).span_builder(self.ast).end;
                match self.ast.get_expr(*pat) {
                    Expr::Ident { span, t } => {
                        let name = self.toks.src[span.range()].to_owned();
                        let id = self.ast.global(Global {
                            name: name.clone().into_boxed_str(),
                            name_span: *span,
                            t_name: t.clone(),
                            scope: scope_id,
                            t_colon_and_equals_or_colon_equals: t_colon_and_equals_or_colon_equals
                                .clone(),
                            ty: annotated_ty.clone(),
                            val: *val,
                            span: TSpan::new(start, end),
                        });
                        scope.definitions.insert(name, Definition::Global(id));
                    }
                    expr => {
                        let span = expr.span_builder(self.ast);
                        self.ast.get_scope_mut(scope_id).has_errors = true;
                        self.toks
                            .errors
                            .emit_err(Error::InvalidGlobalVarPattern.at_span(span));
                    }
                }
            }
            _ => self.toks.errors.emit_err(CompileError {
                err: Error::InvalidTopLevelBlockItem,
                span: self.ast.get_expr(expr).span_builder(self.ast),
            }),
        };
    }

    fn parse_block_or_stmt(&mut self, scope: ScopeId) -> ParseResult<(T::Opt<T>, Expr<T>)> {
        step_or_unexpected! {self, tok,
            LBrace => Ok((T::opt_none(), self.parse_block_from_lbrace(tok, scope))),
            Colon => {
                self.parse_stmt(scope).map(|expr| (T::opt(t(tok)), expr))
            }
        }
    }

    fn parse_block_from_lbrace(&mut self, lbrace: Token, parent: ScopeId) -> Expr<T> {
        debug_assert_eq!(lbrace.ty, TokenType::LBrace);
        let mut items = Vec::new();

        let (scope, rbrace) = self.parse_items_until(
            lbrace.start,
            Some(parent),
            dmap::new(),
            Some((lbrace, TokenType::RBrace)),
            |_, _, _, expr, _| items.push(expr),
        );

        let items = self.ast.exprs(items);
        Expr::Block {
            t_open: T::t(lbrace),
            scope,
            items,
            t_close: T::t(rbrace),
        }
    }

    fn parse_item(&mut self, scope: ScopeId) -> Result<Item<T>, CompileError> {
        let attributes = self.parse_attributes(scope)?;
        let value = match self.toks.peek().ty {
            // use statement
            TokenType::Keyword(Keyword::Use) => {
                let t_use = self.toks.step_assert(TokenType::Keyword(Keyword::Use));
                let path = self.parse_path()?;
                ItemValue::Use {
                    t_use: t(t_use),
                    path,
                }
            }
            // either a variable or constant definition or just an identifier
            TokenType::Ident => {
                let ident = self.toks.step_assert(TokenType::Ident);
                let ident_span = ident.span();
                let name = ident.get_val(self.toks.src);
                match self.toks.peek().ty {
                    // constant definition
                    TokenType::DoubleColon => {
                        let double_colon = self.toks.step_assert(TokenType::DoubleColon);
                        let value = self.parse_expr(scope).unwrap_or_else(|err| {
                            let span = err.span;
                            self.toks.errors.emit_err(err);
                            Expr::Error(span)
                        });
                        let value = self.ast.expr(value);
                        ItemValue::Definition {
                            t_name: t(ident),
                            t_colon_colon: T::either_a(t(double_colon)),
                            name: name.to_owned(),
                            name_span: ident_span,
                            annotated_ty: UnresolvedType::Infer(double_colon.span()),
                            value,
                        }
                    }
                    // Variable or constant declaration with explicit type
                    TokenType::Colon => {
                        let colon = self.toks.step_assert(TokenType::Colon);
                        let annotated_ty = self.parse_type()?;
                        let token = self
                            .toks
                            .step_expect([TokenType::Equals, TokenType::Colon])?;
                        match token.ty {
                            TokenType::Equals => {
                                let equals = token;
                                // typed variable with initial value
                                let pat = self.ast.expr(Expr::Ident {
                                    span: ident_span,
                                    t: T::t(ident),
                                });
                                let val = self.parse_expr(scope)?;
                                let val = self.ast.expr(val);
                                ItemValue::Expr(Expr::DeclareWithVal {
                                    pat,
                                    annotated_ty,
                                    t_colon_and_equals_or_colon_equals: T::either_a((
                                        t(colon),
                                        t(equals),
                                    )),
                                    val,
                                })
                            }
                            TokenType::Colon => {
                                let second_colon = token;
                                // typed constant
                                let value = self.parse_expr(scope)?;
                                ItemValue::Definition {
                                    t_name: t(ident),
                                    name: name.to_owned(),
                                    name_span: ident_span,
                                    t_colon_colon: T::either_b((t(colon), t(second_colon))),
                                    annotated_ty,
                                    value: self.ast.expr(value),
                                }
                            }
                            _ => unreachable!(), // handled by step_expect
                        }
                    }
                    // Variable declaration with inferred type
                    TokenType::Declare => {
                        let decl = self.toks.step_assert(TokenType::Declare);
                        let pat = self.ast.expr(Expr::Ident {
                            span: ident_span,
                            t: t(ident),
                        });
                        let val = self.parse_expr(scope)?;

                        ItemValue::Expr(Expr::DeclareWithVal {
                            pat,
                            t_colon_and_equals_or_colon_equals: T::either_b(t(decl)),
                            annotated_ty: UnresolvedType::Infer(decl.span()),
                            val: self.ast.expr(val),
                        })
                    }
                    _ => {
                        let var = Expr::Ident {
                            span: ident_span,
                            t: t(ident),
                        };
                        let expr = self.parse_stmt_starting_with(var, scope)?;
                        ItemValue::Expr(expr)
                    }
                }
            }
            _ => ItemValue::Expr(self.parse_stmt(scope)?),
        };
        Ok(Item { attributes, value })
    }

    fn parse_attributes(&mut self, scope: ScopeId) -> ParseResult<Box<[Attribute<T>]>> {
        let mut attributes = Vec::new();
        while let Some(at) = self.toks.step_if(TokenType::At) {
            let path = self.parse_path()?;
            let mut args = Vec::new();
            let peeked = self.toks.peek();
            let (end, t_parens) = if peeked.ty == TokenType::LParen && !peeked.new_line {
                let lparen = self.toks.step_assert(TokenType::LParen);
                let rparen =
                    self.parse_delimited(lparen, TokenType::Comma, TokenType::RParen, |p| {
                        args.push(p.parse_expr(scope)?);
                        Ok(Delimit::OptionalIfNewLine)
                    })?;
                (rparen.end, T::opt((t(lparen), t(rparen))))
            } else {
                (path.span().end, T::opt_none())
            };
            attributes.push(Attribute {
                t_at: t(at),
                path,
                t_parens,
                args: self.ast.exprs(args),
                span: TSpan::new(at.start, end),
            });
        }
        Ok(attributes.into_boxed_slice())
    }

    fn parse_function_def(
        &mut self,
        attributes: Attributes<T>,
        fn_tok: Token,
        scope: ScopeId,
        associated_name: TSpan,
        inherited_generics: Vec<GenericDef<T>>,
    ) -> ParseResult<Function<T>> {
        let mut func = self.parse_function_header(
            attributes,
            fn_tok,
            scope,
            associated_name,
            inherited_generics,
        )?;
        self.parse_function_body(&mut func)?;
        Ok(func)
    }

    fn struct_definition(
        &mut self,
        struct_tok: Token,
        scope: ScopeId,
    ) -> ParseResult<ast::TypeDef<T>> {
        debug_assert_eq!(struct_tok.ty, TokenType::Keyword(Keyword::Struct));
        let generics = self.parse_optional_generics(Vec::new())?;

        let scope = {
            let scope =
                ast::Scope::from_generics(scope, self.toks.src, &generics.types, TSpan::MISSING);
            self.ast.scope(scope)
        };

        let lbrace = self.toks.step_expect(TokenType::LBrace)?;
        let mut members = Vec::new();
        let mut methods = dmap::new();
        let mut impls = Vec::new();

        let rbrace = self.parse_delimited(lbrace, TokenType::Comma, TokenType::RBrace, |p| {
            let attributes = p.parse_attributes(scope)?;
            if let Some(impl_tok) = p.toks.step_if(TokenType::Keyword(Keyword::Impl)) {
                impls.push(p.parse_inherent_impl(attributes, impl_tok, scope, &generics.types)?);
                return Ok(Delimit::No);
            }
            let ident = p.toks.step_expect(TokenType::Ident)?;
            if let Some(colon_colon) = p.toks.step_if(TokenType::DoubleColon) {
                let attributes = p.parse_attributes(scope)?;
                let fn_tok = p.toks.step_expect(TokenType::Keyword(Keyword::Fn))?;
                let method = p.parse_function_def(
                    attributes,
                    fn_tok,
                    scope,
                    ident.span(),
                    generics.types.to_vec(),
                )?;
                let func_id = p.ast.function(method);
                let name = ident.get_val(p.toks.src).to_owned();
                let method = Method {
                    name: ident.span(),
                    t_colon_colon: t(colon_colon),
                    function: func_id,
                };
                if let Some(_existing) = methods.insert(name, method) {
                    return Err(CompileError::new(Error::DuplicateDefinition, ident.span()));
                }
                Ok(Delimit::No)
            } else {
                let name = ident.span();
                let ty = p.parse_type()?;
                members.push(StructMember {
                    attributes,
                    name,
                    ty,
                });
                Ok(Delimit::OptionalIfNewLine)
            }
        })?;
        self.ast.get_scope_mut(scope).span = TSpan::new(struct_tok.start, rbrace.end);
        Ok(ast::TypeDef {
            t_introducer: t(struct_tok),
            generics,
            t_lbrace: t(lbrace),
            scope,
            methods,
            impls: impls.into_boxed_slice(),
            content: ast::TypeContent::Struct { members },
            t_rbrace: t(rbrace),
        })
    }

    fn enum_definition(&mut self, enum_tok: Token, scope: ScopeId) -> ParseResult<ast::TypeDef<T>> {
        debug_assert_eq!(enum_tok.ty, TokenType::Keyword(Keyword::Enum));
        let generics = self.parse_optional_generics(Vec::new())?;

        let scope = self.ast.scope(ast::Scope::from_generics(
            scope,
            self.toks.src,
            &generics.types,
            TSpan::MISSING,
        ));

        let mut variants = Vec::new();
        let mut methods = dmap::new();
        let mut impls = Vec::new();

        let lbrace = self.toks.step_expect(TokenType::LBrace)?;
        let rbrace = self.parse_delimited(lbrace, TokenType::Comma, TokenType::RBrace, |p| {
            let attributes = p.parse_attributes(scope)?;
            if let Some(impl_tok) = p.toks.step_if(TokenType::Keyword(Keyword::Impl)) {
                impls.push(p.parse_inherent_impl(attributes, impl_tok, scope, &generics.types)?);
                return Ok(Delimit::No);
            }
            let ident = p.toks.step_expect(TokenType::Ident)?;
            match p.toks.peek().ty {
                TokenType::LParen => {
                    let lparen = p.toks.step_assert(TokenType::LParen);

                    let name_span = ident.span();
                    let mut args = vec![];
                    let rparen =
                        p.parse_delimited(lparen, TokenType::Comma, TokenType::RParen, |p| {
                            p.parse_type().map(|ty| {
                                args.push(ty);
                                Delimit::OptionalIfNewLine
                            })
                        })?;
                    variants.push(EnumVariantDefinition {
                        name_span,
                        t_parens: T::opt((t(lparen), t(rparen))),
                        args: args.into_boxed_slice(),
                        end: rparen.end,
                    });
                    Ok(Delimit::OptionalIfNewLine)
                }
                TokenType::DoubleColon => {
                    let t_colon_colon = t(p.toks.step_assert(TokenType::DoubleColon));
                    let attributes = p.parse_attributes(scope)?;
                    let fn_tok = p.toks.step_expect(TokenType::Keyword(Keyword::Fn))?;
                    let method = p.parse_function_def(
                        attributes,
                        fn_tok,
                        scope,
                        ident.span(),
                        generics.types.to_vec(),
                    )?;
                    let func_id = p.ast.function(method);
                    let name = ident.get_val(p.toks.src).to_owned();
                    let method = Method {
                        name: ident.span(),
                        t_colon_colon,
                        function: func_id,
                    };
                    if let Some(_existing) = methods.insert(name, method) {
                        return Err(CompileError::new(Error::DuplicateDefinition, ident.span()));
                    }
                    Ok(Delimit::No)
                }
                _ => {
                    // enum variant without arguments
                    let name_span = ident.span();
                    variants.push(EnumVariantDefinition {
                        name_span,
                        t_parens: T::opt_none(),
                        args: Box::new([]),
                        end: name_span.end,
                    });
                    Ok(Delimit::OptionalIfNewLine)
                }
            }
        })?;

        self.ast.get_scope_mut(scope).span = TSpan::new(enum_tok.start, rbrace.end);

        Ok(ast::TypeDef {
            t_introducer: t(enum_tok),
            generics,
            t_lbrace: t(lbrace),
            scope,
            methods,
            impls: impls.into_boxed_slice(),
            content: ast::TypeContent::Enum {
                variants: variants.into_boxed_slice(),
            },
            t_rbrace: t(rbrace),
        })
    }

    /// Will just return a function with the body always set to `None`
    fn parse_function_header(
        &mut self,
        attributes: Attributes<T>,
        fn_tok: Token,
        scope: ScopeId,
        associated_name: TSpan,
        inherited_generics: Vec<GenericDef<T>>,
    ) -> ParseResult<Function<T>> {
        debug_assert_eq!(fn_tok.ty, TokenType::Keyword(Keyword::Fn));
        let start = fn_tok.start;
        let mut end = fn_tok.end;
        let generics = self.parse_optional_generics(inherited_generics)?;

        let mut params = Vec::new();
        let mut named_params = Vec::new();
        let mut varargs = false;
        let mut t_varargs = T::opt_none();

        let t_parens = if let Some(lparen) = self.toks.step_if(TokenType::LParen) {
            let rparen =
                self.parse_delimited(lparen, TokenType::Comma, TokenType::RParen, |p| {
                    if varargs {
                        // can't be an RParen since parse_delimited would have returned otherwise
                        let err = p.toks.step_expect(TokenType::RParen).unwrap_err();
                        return Err(err);
                    }
                    if let Some(tok) = p.toks.step_if(TokenType::TripleDot) {
                        varargs = true;
                        t_varargs = T::opt(t(tok));
                    } else {
                        let name_tok = p.toks.step_expect(TokenType::Ident)?;
                        let name_span = TSpan::new(name_tok.start, name_tok.end);
                        let equals = p.toks.step_if(TokenType::Equals);
                        let ty = if let Some(equals) = equals {
                            UnresolvedType::Infer(equals.span())
                        } else if !matches!(p.toks.peek().ty, TokenType::RParen | TokenType::Comma)
                        {
                            p.parse_type()?
                        } else {
                            UnresolvedType::Infer(name_span)
                        };
                        if let Some(t_eq) = equals.or_else(|| p.toks.step_if(TokenType::Equals)) {
                            let default_value = p.parse_expr(scope)?;
                            // TODO: should check no duplicate params exist here or in check_signature
                            named_params.push(ast::NamedParam {
                                name: name_span,
                                ty,
                                t_eq: T::t(t_eq),
                                default_value: p.ast.expr(default_value),
                            });
                        } else {
                            params.push((name_span, ty));
                        }
                    }
                    Ok(Delimit::OptionalIfNewLine)
                })?;
            end = rparen.end;
            T::opt((t(lparen), t(rparen)))
        } else {
            T::opt_none()
        };
        let (t_arrow, return_type) = if let Some(arrow) = self.toks.step_if(TokenType::Arrow) {
            let ty = self.parse_type()?;
            end = ty.span().end;
            (T::opt(t(arrow)), ty)
        } else {
            (T::opt_none(), UnresolvedType::Infer(TSpan::new(end, end)))
        };

        let span = TSpan::new(start, end);
        let function_scope = self.ast.scope(ast::Scope::from_generics(
            scope,
            self.toks.src,
            &generics.types,
            span,
        ));

        Ok(Function {
            attributes,
            t_fn: t(fn_tok),
            generics,
            t_parens,
            params: params.into_boxed_slice(),
            named_params: named_params.into_boxed_slice(),
            varargs,
            t_varargs,
            t_arrow,
            return_type,
            t_colon: T::opt_none(),
            t_extern: T::opt_none(),
            body: None,
            scope: function_scope,
            signature_span: TSpan::new(start, end),
            associated_name,
        })
    }

    fn parse_function_body(&mut self, func: &mut Function<T>) -> ParseResult<()> {
        let body = step_or_unexpected! {self, tok,
            LBrace => {
                let block = self.parse_block_from_lbrace(tok, func.scope);
                Some(self.ast.expr(block))
            },
            Colon => {
                let val = self.parse_expr(func.scope)?;
                func.t_colon = T::opt(t(tok));
                Some(self.ast.expr(val))
            },
            Keyword Extern => {
                func.t_extern = T::opt(t(tok));
                None
            }
        };
        if let Some(body) = body {
            func.body = Some(body);
            let new_end = self.ast.get_expr(body).span_builder(self.ast).end;
            self.ast.get_scope_mut(func.scope).span.end = new_end;
        }
        Ok(())
    }

    fn parse_trait_def(
        &mut self,
        trait_tok: Token,
        parent_scope: ScopeId,
        associated_name: TSpan,
    ) -> ParseResult<TraitDefinition<T>> {
        debug_assert_eq!(trait_tok.ty, TokenType::Keyword(Keyword::Trait));
        let self_generic = GenericDef {
            name: TSpan::MISSING,
            t_colon: T::opt_none(),
            bounds: Box::new([]),
        };
        let generics = self.parse_optional_generics(vec![self_generic])?;
        let scope =
            ast::Scope::from_generics(parent_scope, self.toks.src, &generics.types, TSpan::MISSING);
        let scope = self.ast.scope(scope);
        let lbrace = self.toks.step_expect(TokenType::LBrace)?;
        let mut functions = Vec::new();
        let rbrace = loop {
            step_or_unexpected! {self, tok,
                RBrace => break tok,
                Ident => {
                    let name_span = tok.span();
                    let t_colon_colon = t(self.toks.step_expect(TokenType::DoubleColon)?);
                    let attributes = self.parse_attributes(scope)?;
                    let fn_tok = self.toks.step_expect(TokenType::Keyword(Keyword::Fn))?;
                    let mut func = self.parse_function_header(
                        attributes,
                        fn_tok,
                        scope,
                        name_span,
                        generics.types.to_vec(),
                    )?;
                    if matches!(self.toks.peek().ty, TokenType::Colon | TokenType::LBrace) {
                        self.parse_function_body(&mut func)?;
                    }
                    let function = self.ast.function(func);
                    functions.push(Method { name: name_span, t_colon_colon, function });
                }
            }
        };
        let mut impls = Vec::new();
        let t_attached_impls =
            if let Some(t_for) = self.toks.step_if(TokenType::Keyword(Keyword::For)) {
                let lbrace = self.toks.step_expect(TokenType::LBrace)?;
                let rbrace = loop {
                    let token = self.toks.step();
                    match token.ty {
                        TokenType::RBrace => break token,
                        TokenType::Keyword(Keyword::Impl) => {
                            impls.push(self.parse_attached_impl(token, parent_scope).inspect_err(
                                |_| self.recover_in_delimited(lbrace, TokenType::RBrace),
                            )?);
                        }
                        _ => {
                            self.recover_in_delimited(lbrace, TokenType::RBrace);
                            return Err(unexpected(
                                token,
                                ExpectedTokens::AnyOf(Box::new([
                                    TokenType::RBrace,
                                    TokenType::Keyword(Keyword::Impl),
                                ])),
                            ));
                        }
                    }
                };
                T::opt((t(t_for), t(lbrace), t(rbrace)))
            } else {
                T::opt_none()
            };
        let span = TSpan::new(trait_tok.start, rbrace.end);
        self.ast.get_scope_mut(scope).span = span;

        Ok(TraitDefinition {
            t_trait: t(trait_tok),
            generics,
            scope,
            t_lbrace: t(lbrace),
            functions,
            t_rbrace: t(rbrace),
            t_attached_impls,
            impls: impls.into_boxed_slice(),
            associated_name,
        })
    }

    fn parse_attached_impl(
        &mut self,
        impl_tok: Token,
        parent_scope: ScopeId,
    ) -> ParseResult<Impl<T>> {
        let start = impl_tok.start;
        let generics = self.parse_optional_generics(Vec::new())?;
        let underscore = self.toks.step_expect(TokenType::Underscore)?;
        let (trait_generics, t_brackets, trait_generics_span) =
            self.parse_generics_instance(TSpan::new(underscore.end, underscore.end))?;
        let t_for = t(self.toks.step_expect(TokenType::Keyword(Keyword::For))?);
        let impl_scope = self.ast.scope(ast::Scope::from_generics(
            parent_scope,
            self.toks.src,
            &generics.types,
            TSpan::MISSING,
        ));
        let implemented_type = self.parse_type()?;
        let lbrace = self.toks.step_expect(TokenType::LBrace)?;
        let (functions, rbrace) =
            self.parse_trait_impl_body(lbrace, impl_scope, &generics.types)?;
        self.ast.get_scope_mut(impl_scope).span = TSpan::new(start, rbrace.end);
        Ok(ast::Impl {
            t_impl: t(impl_tok),
            t_underscore: t(underscore),
            t_for,
            implemented_type,
            base: ast::BaseImpl {
                scope: impl_scope,
                generics,
                t_brackets,
                trait_generics,
                trait_generics_span,
                t_lbrace: t(lbrace),
                functions,
                t_rbrace: t(rbrace),
            },
        })
    }

    fn parse_trait_impl_body(
        &mut self,
        lbrace: Token,
        scope: ScopeId,
        impl_generics: &[GenericDef<T>],
    ) -> ParseResult<(Box<[Method<T>]>, Token)> {
        debug_assert_eq!(lbrace.ty, TokenType::LBrace);
        let mut functions = Vec::new();
        let rbrace = self.parse_delimited(lbrace, TokenType::Comma, TokenType::RBrace, |p| {
            let name = p.toks.step_expect(TokenType::Ident)?;
            let name_span = name.span();
            let t_colon_colon = t(p.toks.step_expect(TokenType::DoubleColon)?);
            let attributes = p.parse_attributes(scope)?;
            let fn_tok = p.toks.step_expect(TokenType::Keyword(Keyword::Fn))?;
            // TODO: figure out inherited generics here
            let func =
                p.parse_function_def(attributes, fn_tok, scope, name_span, impl_generics.to_vec())?;
            let func_id = p.ast.function(func);
            functions.push(Method {
                name: name_span,
                t_colon_colon,
                function: func_id,
            });
            Ok(Delimit::OptionalIfNewLine)
        })?;
        Ok((functions.into_boxed_slice(), rbrace))
    }

    fn parse_inherent_impl(
        &mut self,
        attributes: Attributes<T>,
        t_impl: Token,
        outer_scope: ScopeId,
        type_generics: &[GenericDef<T>],
    ) -> ParseResult<InherentImpl<T>> {
        let impl_generics = self.parse_optional_generics(type_generics.to_vec())?;
        let scope = self.ast.scope(ast::Scope::from_generics(
            outer_scope,
            self.toks.src,
            &impl_generics.types,
            TSpan::MISSING,
        ));
        let implemented_trait = self.parse_path()?;
        let pos = implemented_trait.span().end;
        let (trait_generics, t_brackets, generics_span) =
            self.parse_generics_instance(TSpan::new(pos, pos))?;
        let lbrace = self.toks.step_expect(TokenType::LBrace)?;
        let (functions, rbrace) =
            self.parse_trait_impl_body(lbrace, scope, &impl_generics.types)?;
        self.ast.get_scope_mut(scope).span = TSpan::new(t_impl.start, rbrace.end);
        Ok(InherentImpl {
            attributes,
            t_impl: t(t_impl),
            implemented_trait,
            base: ast::BaseImpl {
                scope,
                generics: impl_generics,
                t_brackets,
                trait_generics,
                trait_generics_span: generics_span,
                t_lbrace: t(lbrace),
                functions,
                t_rbrace: t(rbrace),
            },
        })
    }

    fn parse_stmt(&mut self, scope: ScopeId) -> ParseResult<Expr<T>> {
        let expr = self.parse_expr(scope)?;
        self.stmt_postfix(expr, scope)
    }

    fn parse_stmt_starting_with(&mut self, expr: Expr<T>, scope: ScopeId) -> ParseResult<Expr<T>> {
        let lhs = self.parse_factor_postfix(expr, true, scope)?;
        let expr = self.parse_bin_op_rhs(0, lhs, scope)?;
        self.stmt_postfix(expr, scope)
    }

    fn stmt_postfix(&mut self, expr: Expr<T>, scope: ScopeId) -> ParseResult<Expr<T>> {
        let expr = if let Some(colon) = self.toks.step_if(TokenType::Colon) {
            let annotated_ty = self.parse_type()?;
            let equals = self.toks.step_expect(TokenType::Equals)?;
            let val = self.parse_expr(scope)?;
            Expr::DeclareWithVal {
                pat: self.ast.expr(expr),
                t_colon_and_equals_or_colon_equals: T::either_a((t(colon), t(equals))),
                annotated_ty,
                val: self.ast.expr(val),
            }
        } else if let Some(declare) = self.toks.step_if(TokenType::Declare) {
            let val = self.parse_expr(scope)?;
            Expr::DeclareWithVal {
                pat: self.ast.expr(expr),
                t_colon_and_equals_or_colon_equals: T::either_b(t(declare)),
                annotated_ty: UnresolvedType::Infer(declare.span()),
                val: self.ast.expr(val),
            }
        } else if let Some(eq) = self.toks.step_if(TokenType::Equals) {
            let val = self.parse_expr(scope)?;
            Expr::BinOp {
                t_op: t(eq),
                op: Operator::Assignment(token::AssignType::Assign),
                l: self.ast.expr(expr),
                r: self.ast.expr(val),
            }
        } else {
            return Ok(expr);
        };
        Ok(expr)
    }

    fn parse_expr(&mut self, scope: ScopeId) -> ParseResult<Expr<T>> {
        let lhs = self.parse_factor(true, scope)?;
        self.parse_bin_op_rhs(0, lhs, scope)
    }

    fn parse_factor(&mut self, include_as: bool, scope: ScopeId) -> ParseResult<Expr<T>> {
        let attributes = self.parse_attributes(scope)?;
        let first = self.toks.step();
        let attributes_span = if attributes.is_empty() {
            TSpan::EMPTY
        } else {
            TSpan::new(
                attributes.first().unwrap().span.start,
                attributes.last().unwrap().span.end,
            )
        };
        let mut has_unused_attributes = !attributes.is_empty();
        let expr = match first.ty {
            TokenType::Keyword(Keyword::Fn) => {
                let func =
                    self.parse_function_def(attributes, first, scope, TSpan::EMPTY, Vec::new())?;
                has_unused_attributes = false;
                Expr::Function {
                    id: self.ast.function(func),
                }
            }
            TokenType::Keyword(Keyword::Struct) => {
                let def = self.struct_definition(first, scope)?;
                let id = self.ast.type_def(def);
                Expr::TypeDeclaration { id }
            }
            TokenType::Keyword(Keyword::Enum) => {
                let def = self.enum_definition(first, scope)?;
                let id = self.ast.type_def(def);
                Expr::TypeDeclaration { id }
            }
            TokenType::Keyword(Keyword::Trait) => {
                let trait_def = self.parse_trait_def(first, scope, TSpan::MISSING)?;
                let id = self.ast.trait_def(trait_def);
                Expr::Trait { id }
            }
            TokenType::LBrace => {
                let block = self.parse_block_from_lbrace(first, scope);
                return self.parse_factor_postfix(block, true, scope);
            }
            TokenType::LBracket => {
                let mut elems = Vec::new();
                let rbracket =
                    self.parse_delimited(first, TokenType::Comma, TokenType::RBracket, |p| {
                        elems.push(p.parse_expr(scope)?);
                        Ok(Delimit::OptionalIfNewLine)
                    })?;
                Expr::Array {
                    span: TSpan::new(first.start, rbracket.end),
                    t_lbracket: t(first),
                    elements: self.ast.exprs(elems),
                    t_rbracket: t(rbracket),
                }
            }
            TokenType::LParen => {
                let (elements, named_elements, rparen, has_trailing_comma) =
                    self.parse_tuple(first, scope)?;
                if let Some(inner) = elements.into_single()
                    && named_elements.is_empty()
                    && !has_trailing_comma
                {
                    // this is just a grouping expression, not a tuple
                    Expr::Nested {
                        span: TSpan::new(first.start, rparen.end),
                        t_lparen: t(first),
                        inner,
                        t_rparen: t(rparen),
                    }
                } else {
                    Expr::Tuple {
                        span: TSpan::new(first.start, rparen.end),
                        t_lparen: t(first),
                        elements,
                        named_elements,
                        t_rparen: t(rparen),
                    }
                }
            }
            TokenType::Minus => {
                let val = self.parse_factor(false, scope)?;
                Expr::UnOp {
                    start_or_end: first.start,
                    t_op: t(first),
                    op: UnOp::Neg,
                    inner: self.ast.expr(val),
                }
            }
            TokenType::Bang => {
                let val = self.parse_factor(false, scope)?;
                Expr::UnOp {
                    start_or_end: first.start,
                    t_op: t(first),
                    op: UnOp::Not,
                    inner: self.ast.expr(val),
                }
            }
            TokenType::Ampersand => {
                let val = self.parse_factor(false, scope)?;
                Expr::UnOp {
                    start_or_end: first.start,
                    t_op: t(first),
                    op: UnOp::Ref,
                    inner: self.ast.expr(val),
                }
            }
            TokenType::Keyword(Keyword::Ret) => {
                if !self.toks.peek().new_line {
                    let val = self.parse_expr(scope)?;
                    let val = self.ast.expr(val);
                    Expr::Return {
                        start: first.start,
                        t_ret: t(first),
                        val,
                    }
                } else {
                    Expr::ReturnUnit {
                        start: first.start,
                        t: t(first),
                    }
                }
            }
            TokenType::IntLiteral => Expr::IntLiteral {
                span: first.span(),
                t: t(first),
            },
            TokenType::FloatLiteral => Expr::FloatLiteral {
                span: first.span(),
                t: t(first),
            },
            TokenType::StringLiteral => Expr::StringLiteral {
                span: TSpan::new(first.start, first.end),
                t: t(first),
            },
            TokenType::Ident => Expr::Ident {
                span: first.span(),
                t: t(first),
            },
            TokenType::Underscore => Expr::Hole {
                loc: first.start,
                t: t(first),
            },
            TokenType::Keyword(Keyword::If) => {
                let t_if = t(first);
                let cond = self.parse_expr(scope)?;
                let cond = self.ast.expr(cond);
                let pat_value = self
                    .toks
                    .step_if(TokenType::Declare)
                    .map(|colon_eq| {
                        let pat_value = self.parse_expr(scope)?;
                        Ok((t(colon_eq), self.ast.expr(pat_value)))
                    })
                    .transpose()?;
                let (t_colon, then) = self.parse_block_or_stmt(scope)?;
                let then = self.ast.expr(then);

                let else_ =
                    if let Some(else_) = self.toks.step_if(TokenType::Keyword(Keyword::Else)) {
                        let t_else = t(else_);
                        let else_ = self.parse_stmt(scope)?;
                        Some((t_else, self.ast.expr(else_)))
                    } else {
                        None
                    };

                let start = first.start;
                match (pat_value, else_) {
                    (None, None) => Expr::If {
                        t_if,
                        start,
                        cond,
                        t_colon,
                        then,
                    },
                    (None, Some((t_else, else_))) => Expr::IfElse {
                        t_if,
                        start,
                        cond,
                        t_colon,
                        then,
                        t_else,
                        else_,
                    },
                    (Some((t_colon_eq, value)), None) => Expr::IfPat {
                        t_if,
                        start,
                        pat: cond,
                        t_colon_eq,
                        value,
                        t_colon,
                        then,
                    },
                    (Some((t_colon_eq, value)), Some((t_else, else_))) => Expr::IfPatElse {
                        t_if,
                        start,
                        pat: cond,
                        t_colon_eq,
                        value,
                        t_colon,
                        then,
                        t_else,
                        else_,
                    },
                }
            }
            TokenType::Keyword(Keyword::Match) => {
                let val = self.parse_expr(scope)?;
                let val = self.ast.expr(val);
                let lbrace = self.toks.step_expect(TokenType::LBrace)?;

                let mut branches = Vec::new();
                let rbrace =
                    self.parse_delimited(lbrace, TokenType::Comma, TokenType::RBrace, |p| {
                        let pat = p.parse_expr(scope)?;
                        let (branch, delimit) = step_or_unexpected! {p, tok,
                            Colon => {
                                (p.parse_expr(scope)?, Delimit::Yes)
                            },
                            LBrace => {
                                (p.parse_block_from_lbrace(tok, scope), Delimit::Optional)
                            }
                        };
                        branches.extend([pat, branch]);
                        Ok(delimit)
                    })?;
                debug_assert_eq!(branches.len() % 2, 0);
                let branch_count = (branches.len() / 2) as u32;
                let branches = self.ast.exprs(branches).idx;
                Expr::Match {
                    t_match: t(first),
                    span: TSpan::new(lbrace.start, rbrace.end),
                    val,
                    t_lbrace: t(lbrace),
                    branches: ExprIdPairs {
                        idx: branches,
                        pair_count: branch_count,
                    },
                    t_rbrace: t(rbrace),
                }
            }
            TokenType::Keyword(Keyword::While) => self.parse_while_from_cond(first, scope)?,
            TokenType::Keyword(Keyword::For) => {
                //self.parse_
                let pat = self.parse_expr(scope)?;
                let t_in = self.toks.step_expect(TokenType::Keyword(Keyword::In))?;
                let iter = self.parse_expr(scope)?;
                let (t_colon, body) = self.parse_block_or_stmt(scope)?;
                Expr::For {
                    start: first.start,
                    t_for: t(first),
                    pat: self.ast.expr(pat),
                    t_in: t(t_in),
                    iter: self.ast.expr(iter),
                    t_colon,
                    body: self.ast.expr(body),
                }
            }
            TokenType::Keyword(Keyword::Primitive(primitive)) => Expr::Primitive {
                primitive,
                start: first.start,
                t: t(first),
            },
            TokenType::Keyword(Keyword::Root) => Expr::Root {
                start: first.start,
                t: t(first),
            },
            TokenType::Dot => {
                let ident = self.toks.step_expect(TokenType::Ident)?;
                let (args, parens) = self.parse_enum_args(scope)?;
                let args = self.ast.exprs(args);
                let span = TSpan::new(
                    first.start,
                    parens.map_or(ident.end, |(_, rparen)| rparen.end),
                );
                Expr::EnumLiteral {
                    span,
                    t_dot: t(first),
                    ident: ident.span(),
                    t_ident: t(ident),
                    t_parens: parens.map_or(T::opt_none(), |(l, r)| T::opt((t(l), t(r)))),
                    args,
                }
            }
            TokenType::Keyword(Keyword::Asm) => {
                let lparen = self.toks.step_expect(TokenType::LParen)?;
                let string_lit = self.toks.step_expect(TokenType::StringLiteral)?;
                let asm_str_span = string_lit.span();
                let mut args = Vec::new();
                let rparen = if let Some(rparen) = self.toks.step_if(TokenType::RParen) {
                    rparen
                } else if self.toks.step_if(TokenType::Comma).is_some() || self.toks.peek().new_line
                {
                    self.parse_delimited(lparen, TokenType::Comma, TokenType::RParen, |p| {
                        args.push(p.parse_expr(scope)?);
                        Ok(Delimit::OptionalIfNewLine)
                    })?
                } else {
                    self.recover_in_delimited(lparen, TokenType::RParen);
                    return Err(unexpected(
                        self.toks.step(),
                        ExpectedTokens::AnyOf(Box::new([TokenType::Comma, TokenType::RParen])),
                    ));
                };
                Expr::Asm {
                    t_asm: t(first),
                    t_lparen: t(lparen),
                    t_string_literal: t(string_lit),
                    span: TSpan::new(first.start, rparen.end),
                    asm_str_span,
                    t_rparen: t(rparen),
                    args: self.ast.exprs(args),
                }
            }
            TokenType::Keyword(Keyword::Break) => Expr::Break {
                start: first.start,
                t: t(first),
            },
            TokenType::Keyword(Keyword::Continue) => Expr::Continue {
                start: first.start,
                t: t(first),
            },
            _ => {
                return Err(unexpected(first, ExpectedTokens::Expr));
            }
        };
        if has_unused_attributes {
            return Err(CompileError {
                err: Error::UnusedAttributes,
                span: attributes_span,
            });
        }
        self.parse_factor_postfix(expr, include_as, scope)
    }

    fn parse_factor_postfix(
        &mut self,
        mut expr: Expr<T>,
        include_as: bool,
        scope: ScopeId,
    ) -> ParseResult<Expr<T>> {
        loop {
            let tok = self.toks.peek();
            expr = match tok.ty {
                TokenType::LParen if !tok.new_line => {
                    let lparen = self.toks.step_assert(TokenType::LParen);

                    // function call
                    let called_expr = self.ast.expr(expr);

                    let (args, named_args, rparen, _) = self.parse_tuple(lparen, scope)?;
                    let call = self.ast.call(ast::Call {
                        called_expr,
                        t_lparen: t(lparen),
                        open_paren_start: lparen.start,
                        args,
                        named_args,
                        t_rparen: t(rparen),
                        end: rparen.end,
                    });
                    Expr::FunctionCall(call)
                }
                TokenType::LBracket if !tok.new_line => {
                    let lbracket = self.toks.step_assert(TokenType::LBracket);

                    // array index
                    let idx = self.parse_expr(scope)?;
                    let idx = self.ast.expr(idx);

                    let rbracket = self.toks.step_expect(TokenType::RBracket)?;
                    let end = rbracket.end;

                    Expr::Index {
                        expr: self.ast.expr(expr),
                        t_lbracket: t(lbracket),
                        idx,
                        t_rbracket: t(rbracket),
                        end,
                    }
                }
                TokenType::Dot => {
                    let dot = self.toks.step_assert(TokenType::Dot);
                    let tok = self.toks.peek();
                    match tok.ty {
                        TokenType::Ident => {
                            self.toks.step_assert(TokenType::Ident);
                            Expr::MemberAccess {
                                left: self.ast.expr(expr),
                                t_dot: t(dot),
                                name: tok.span(),
                            }
                        }
                        TokenType::IntLiteral => {
                            self.toks.step_assert(TokenType::IntLiteral);
                            let idx = self.toks.src[tok.span().range()].parse().unwrap();
                            Expr::TupleIdx {
                                left: self.ast.expr(expr),
                                t_dot: t(dot),
                                t_int: t(tok),
                                idx,
                                end: tok.end,
                            }
                        }
                        _ => {
                            // still create a MemberAccess expr with an empty name. This is helpful
                            // for diagnostics and lsp completions
                            self.toks.errors.emit_err(unexpected(
                                tok,
                                ExpectedTokens::AnyOf(
                                    [TokenType::Ident, TokenType::IntLiteral].into(),
                                ),
                            ));
                            Expr::MemberAccess {
                                left: self.ast.expr(expr),
                                t_dot: t(dot),
                                name: TSpan::new(dot.end + 1, dot.end + 1),
                            }
                        }
                    }
                }
                TokenType::Caret => {
                    let caret = self.toks.step_assert(TokenType::Caret);
                    Expr::UnOp {
                        start_or_end: caret.end,
                        t_op: t(caret),
                        op: UnOp::Deref,
                        inner: self.ast.expr(expr),
                    }
                }
                TokenType::Keyword(Keyword::As) if include_as => {
                    let t_as = self.toks.step_assert(TokenType::Keyword(Keyword::As));
                    let target_ty = self.parse_type()?;
                    Expr::As {
                        value: self.ast.expr(expr),
                        t_as: t(t_as),
                        ty: target_ty,
                    }
                }
                _ => break Ok(expr),
            };
        }
    }

    /// parse a tuple and return (elements, named_elements, rparen, has_trailing_comma)
    fn parse_tuple(
        &mut self,
        lparen: Token,
        scope: ScopeId,
    ) -> ParseResult<(ExprIds, Box<[(TSpan, ExprId)]>, Token, bool)> {
        let mut elems = Vec::new();
        let mut named_elems = Vec::new();
        let (rparen, trailing_comma) =
            self.parse_delimited_inner(lparen, TokenType::Comma, TokenType::RParen, |p| {
                let expr = p.parse_expr(scope)?;
                if p.toks.step_if(TokenType::Colon).is_some() {
                    let value = p.parse_expr(scope)?;
                    let Expr::Ident {
                        span: name_span, ..
                    } = expr
                    else {
                        p.toks
                            .errors
                            .emit_err(Error::NameExpected.at_span(expr.span_builder(p.ast)));
                        return Ok(Delimit::OptionalIfNewLine);
                    };
                    named_elems.push((name_span, p.ast.expr(value)));
                } else {
                    elems.push(expr);
                }
                Ok(Delimit::OptionalIfNewLine)
            })?;
        let args = self.ast.exprs(elems);
        Ok((
            args,
            named_elems.into_boxed_slice(),
            rparen,
            trailing_comma.is_some(),
        ))
    }

    fn parse_bin_op_rhs(
        &mut self,
        expr_prec: u8,
        mut lhs: Expr<T>,
        scope: ScopeId,
    ) -> ParseResult<Expr<T>> {
        while let Some(op) = Option::<Operator>::from(self.toks.peek().ty) {
            let op_prec = op.precedence();
            if op_prec < expr_prec {
                break;
            }
            let op_token = self.toks.step(); // op
            let mut rhs = self.parse_factor(true, scope)?;

            // If BinOp binds less tightly with RHS than the operator after RHS, let
            // the pending operator take RHS as its LHS.
            if let Some(next_op) = Option::<Operator>::from(self.toks.peek().ty)
                && op_prec < next_op.precedence()
            {
                rhs = self.parse_bin_op_rhs(op.precedence() + 1, rhs, scope)?;
            }
            lhs = Expr::BinOp {
                t_op: t(op_token),
                op,
                l: self.ast.expr(lhs),
                r: self.ast.expr(rhs),
            };
        }
        Ok(lhs)
    }

    /// Starts after the while keyword has already been parsed
    fn parse_while_from_cond(&mut self, while_tok: Token, scope: ScopeId) -> ParseResult<Expr<T>> {
        let t_while = t(while_tok);
        debug_assert_eq!(while_tok.ty, TokenType::Keyword(Keyword::While));
        let cond = self.parse_expr(scope)?;
        let cond = self.ast.expr(cond);
        if let Some(colon_eq) = self.toks.step_if(TokenType::Declare) {
            let val = self.parse_expr(scope)?;
            let val = self.ast.expr(val);
            let (t_colon, body) = self.parse_block_or_stmt(scope)?;
            let body = self.ast.expr(body);
            Ok(Expr::WhilePat {
                t_while,
                start: while_tok.start,
                pat: cond,
                t_colon_eq: t(colon_eq),
                val,
                t_colon,
                body,
            })
        } else {
            let (t_colon, body) = self.parse_block_or_stmt(scope)?;
            Ok(Expr::While {
                t_while,
                start: while_tok.start,
                cond,
                t_colon,
                body: self.ast.expr(body),
            })
        }
    }

    fn parse_path(&mut self) -> ParseResult<IdentPath> {
        let first = self
            .toks
            .step_expect([TokenType::Keyword(Keyword::Root), TokenType::Ident])?;
        let (s, e) = (first.start, first.end);
        self.parse_rest_of_path(s, e)
    }

    fn parse_rest_of_path(&mut self, start: u32, mut end: u32) -> ParseResult<IdentPath> {
        while self.toks.step_if(TokenType::Dot).is_some() {
            end = self.toks.step_expect(TokenType::Ident)?.end;
        }
        Ok(IdentPath::new(TSpan::new(start, end)))
    }

    fn parse_type(&mut self) -> ParseResult<UnresolvedType> {
        let tok = self.toks.step();
        match tok.ty {
            TokenType::Keyword(Keyword::Root) => Ok(UnresolvedType::Unresolved(
                self.parse_rest_of_path(tok.start, tok.end)?,
                self.parse_optional_generic_instance()?
                    .map(|(_, types, _, span)| (types, span)),
            )),
            TokenType::Ident => Ok(UnresolvedType::Unresolved(
                self.parse_rest_of_path(tok.start, tok.end)?,
                self.parse_optional_generic_instance()?
                    .map(|(_, types, _, span)| (types, span)),
            )),
            TokenType::Keyword(Keyword::Primitive(primitive)) => Ok(UnresolvedType::Primitive {
                ty: primitive,
                span_start: tok.start,
            }),
            TokenType::LParen => {
                let mut tuple_types = Vec::new();
                let rparen_end = self
                    .parse_delimited(tok, TokenType::Comma, TokenType::RParen, |p| {
                        tuple_types.push(p.parse_type()?);
                        Ok(Delimit::OptionalIfNewLine)
                    })?
                    .end;
                Ok(UnresolvedType::Tuple(
                    tuple_types,
                    TSpan::new(tok.start, rparen_end),
                ))
            }
            TokenType::LBracket => {
                let elem_ty = self.parse_type()?;
                self.toks.step_expect(TokenType::Semicolon)?;
                let count = if self.toks.step_if(TokenType::Underscore).is_some() {
                    None
                } else {
                    let int_span = self.toks.step_expect(TokenType::IntLiteral)?.span();
                    match self.toks.src[int_span.range()]
                        .parse::<u64>()
                        .expect("Int Literal Error")
                        .try_into()
                    {
                        Ok(count) => Some(count),
                        Err(_) => {
                            return Err(CompileError::new(Error::ArrayTooLarge, int_span));
                        }
                    }
                };
                let end = self.toks.step_expect(TokenType::RBracket)?.end;
                Ok(UnresolvedType::Array(Box::new((
                    elem_ty,
                    count,
                    TSpan::new(tok.start, end),
                ))))
            }
            TokenType::Star => Ok(UnresolvedType::Pointer(Box::new((
                self.parse_type()?,
                tok.start,
            )))),
            TokenType::Keyword(Keyword::Fn) => {
                let start = tok.start;
                let mut params = Vec::new();
                let end = if let Some(lparen) = self.toks.step_if(TokenType::LParen) {
                    self.parse_delimited(lparen, TokenType::Comma, TokenType::RParen, |p| {
                        params.push(p.parse_type()?);
                        Ok(Delimit::OptionalIfNewLine)
                    })?
                    .end
                } else {
                    tok.end
                };
                let return_type = if self.toks.step_if(TokenType::Arrow).is_some() {
                    self.parse_type()?
                } else {
                    UnresolvedType::Tuple(Vec::new(), TSpan::new(end, end))
                };
                Ok(UnresolvedType::Function {
                    span_and_return_type: Box::new((TSpan::new(start, end), return_type)),
                    params: params.into_boxed_slice(),
                })
            }
            TokenType::Underscore => Ok(UnresolvedType::Infer(tok.span())),
            _ => Err(unexpected(tok, ExpectedTokens::Type)),
        }
    }

    fn parse_optional_generics(
        &mut self,
        inherited: Vec<GenericDef<T>>,
    ) -> ParseResult<Generics<T>> {
        let Some(lbracket) = self.toks.step_if(TokenType::LBracket) else {
            return Ok(Generics {
                t_brackets: T::opt_none(),
                types: inherited.into_boxed_slice(),
            });
        };
        let mut generics = inherited;
        let rbracket =
            self.parse_delimited(lbracket, TokenType::Comma, TokenType::RBracket, |p| {
                let name = p.toks.step_expect(TokenType::Ident)?;
                let name_span = name.span();
                let mut requirements = Vec::new();
                let t_colon = if let Some(colon) = p.toks.step_if(TokenType::Colon) {
                    loop {
                        let path = p.parse_path()?;
                        let pos = path.span().end;
                        let (generics, brackets, generics_span) =
                            p.parse_generics_instance(TSpan::new(pos, pos))?;
                        requirements.push(ast::TraitBound {
                            path,
                            generics,
                            brackets,
                            generics_span,
                        });
                        if p.toks.step_if(TokenType::Plus).is_none() {
                            break;
                        }
                    }
                    T::opt(t(colon))
                } else {
                    T::opt_none()
                };
                generics.push(GenericDef {
                    name: name_span,
                    t_colon,
                    bounds: requirements.into_boxed_slice(),
                });
                Ok(Delimit::OptionalIfNewLine)
            })?;
        let generic_count: Result<u8, _> = generics.len().try_into();
        if generic_count.is_err() {
            let start = generics[0].span().start;
            let end = generics.last().unwrap().span().end;
            return Err(Error::TooManyGenerics(generics.len()).at_span(TSpan::new(start, end)));
        };
        Ok(Generics {
            t_brackets: T::opt((t(lbracket), t(rbracket))),
            types: generics.into_boxed_slice(),
        })
    }

    fn parse_generics_instance(
        &mut self,
        default_span: TSpan,
    ) -> ParseResult<(Box<[UnresolvedType]>, T::Opt<(T, T)>, TSpan)> {
        Ok(self.parse_optional_generic_instance()?.map_or(
            (Box::new([]), T::opt_none(), default_span),
            |(lbracket, types, rbracket, span)| (types, T::opt((lbracket, rbracket)), span),
        ))
    }

    fn parse_optional_generic_instance(
        &mut self,
    ) -> ParseResult<Option<(T, Box<[UnresolvedType]>, T, TSpan)>> {
        self.toks
            .step_if(TokenType::LBracket)
            .map(|lbracket| {
                let mut types = Vec::new();
                let rbracket =
                    self.parse_delimited(lbracket, TokenType::Comma, TokenType::RBracket, |p| {
                        types.push(p.parse_type()?);
                        Ok(Delimit::OptionalIfNewLine)
                    })?;
                Ok((
                    t(lbracket),
                    types.into_boxed_slice(),
                    t(rbracket),
                    TSpan::new(lbracket.start, rbracket.end),
                ))
            })
            .transpose()
    }

    /// parses optional arguments of an enum and returns them and the end position if arguments are
    /// present.
    fn parse_enum_args(
        &mut self,
        scope: ScopeId,
    ) -> ParseResult<(Vec<Expr<T>>, Option<(Token, Token)>)> {
        let mut args = vec![];
        let lparen = self.toks.peek();
        let rparen = (lparen.ty == TokenType::LParen && !lparen.new_line)
            .then(|| {
                let lparen = self.toks.step_assert(TokenType::LParen);
                self.parse_delimited(lparen, TokenType::Comma, TokenType::RParen, |p| {
                    args.push(p.parse_expr(scope)?);
                    Ok(Delimit::OptionalIfNewLine)
                })
            })
            .transpose()?;
        Ok((args, rparen.map(|rparen| (lparen, rparen))))
    }
}
