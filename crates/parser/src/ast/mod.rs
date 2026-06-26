use core::fmt;
use dmap::{self, DHashMap};
use error::span::TSpan;
use std::{fmt::Debug, ops::Index};

pub use error::span::IdentPath;
pub use token::{
    AssignType, FloatLiteral, FloatType, IntLiteral, IntType, Keyword, Operator, Primitive, Token,
    TokenType,
};

mod display;
mod eq;

macro_rules! ids {
    ($($name: ident)*) => {
        $(
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
            pub struct $name(u32);
            impl $name {
                pub const fn from_inner(inner: u32) -> Self {
                    Self(inner)
                }

                pub const fn into_inner(self) -> u32 {
                    self.0
                }

                pub fn idx(self) -> usize {
                    self.0 as usize
                }
            }
        )*
    };
}

// All of these ids are local to their ast
ids! {
    ScopeId
    ExprId
    CallId
    FunctionId
    TypeId
    TraitId
    GlobalId
    IdentId
    MemberAccessId
    DefExprId
    ModuleId
    UseId
}

impl TypeId {
    pub const MISSING: Self = Self(u32::MAX);
}
impl ModuleId {
    pub const MISSING: Self = Self(u32::MAX);
}

#[derive(Debug, Clone, Copy)]
pub struct ExprIds {
    pub idx: u32,
    pub count: u32,
}
impl ExprIds {
    pub const EMPTY: Self = Self { idx: 0, count: 0 };

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn into_single(self) -> Option<ExprId> {
        (self.count == 1).then_some(ExprId(self.idx))
    }
}
impl Iterator for ExprIds {
    type Item = ExprId;

    fn next(&mut self) -> Option<Self::Item> {
        self.count.checked_sub(1).map(|count| {
            self.count = count;
            let idx = self.idx;
            self.idx += 1;
            ExprId(idx)
        })
    }
}
impl ExactSizeIterator for ExprIds {
    fn len(&self) -> usize {
        self.count as usize
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExprIdPairs {
    pub(crate) idx: u32,
    pub(crate) pair_count: u32,
}
impl ExprIdPairs {
    pub const EMPTY: Self = Self {
        idx: 0,
        pair_count: 0,
    };

    pub fn pair_count(&self) -> u32 {
        self.pair_count
    }
}
impl Iterator for ExprIdPairs {
    type Item = (ExprId, ExprId);

    fn next(&mut self) -> Option<Self::Item> {
        self.pair_count.checked_sub(1).map(|count| {
            self.pair_count = count;
            let idx = self.idx;
            self.idx += 2;
            (ExprId(idx), ExprId(idx + 1))
        })
    }
}

/// Ast for a single file
#[derive(Debug)]
pub struct Ast<T: TreeToken = ()> {
    src: Box<str>,
    top_level_scope: ScopeId,
    scopes: Box<[Scope<T>]>,
    pub exprs: Box<[Expr<T>]>,
    def_exprs: Box<[(ExprId, UnresolvedType)]>,
    calls: Box<[Call<T>]>,
    functions: Box<[Function<T>]>,
    types: Box<[TypeDef<T>]>,
    traits: Box<[TraitDefinition<T>]>,
    globals: Box<[Global<T>]>,
    use_count: u32,
}
impl<T: TreeToken> Ast<T> {
    pub fn src(&self) -> &str {
        &self.src
    }

    pub fn scope_ids(&self) -> impl Iterator<Item = ScopeId> {
        (0..self.scopes.len() as _).map(ScopeId)
    }

    pub fn scopes(&self) -> &[Scope<T>] {
        &self.scopes
    }

    pub fn function_ids(&self) -> impl Iterator<Item = FunctionId> + use<T> {
        (0..self.functions.len() as _).map(FunctionId)
    }

    pub fn type_ids(&self) -> impl Iterator<Item = TypeId> {
        (0..self.types.len() as _).map(TypeId)
    }

    pub fn global_ids(&self) -> impl Iterator<Item = GlobalId> {
        (0..self.globals.len() as _).map(GlobalId)
    }

    pub fn trait_ids(&self) -> impl Iterator<Item = TraitId> {
        (0..self.traits.len() as _).map(TraitId)
    }

    pub fn top_level_scope_id(&self) -> ScopeId {
        self.top_level_scope
    }

    pub fn top_level_scope(&self) -> &Scope<T> {
        // the last scope is guaranteed to exist and to represent the top level scope of this Ast
        &self[self.top_level_scope]
    }

    pub fn def_expr_count(&self) -> u32 {
        self.def_exprs.len() as _
    }

    pub fn function_count(&self) -> usize {
        self.functions.len()
    }

    pub fn type_count(&self) -> usize {
        self.types.len()
    }

    pub fn global_count(&self) -> usize {
        self.globals.len()
    }

    pub fn trait_count(&self) -> usize {
        self.traits.len()
    }

    pub fn use_count(&self) -> u32 {
        self.use_count
    }
}
impl<T: TreeToken> Index<TSpan> for Ast<T> {
    type Output = str;
    fn index(&self, index: TSpan) -> &Self::Output {
        &self.src[index.range()]
    }
}
impl<T: TreeToken> Index<ScopeId> for Ast<T> {
    type Output = Scope<T>;
    fn index(&self, index: ScopeId) -> &Self::Output {
        &self.scopes[index.idx()]
    }
}
impl<T: TreeToken> Index<ExprId> for Ast<T> {
    type Output = Expr<T>;
    fn index(&self, index: ExprId) -> &Self::Output {
        &self.exprs[index.idx()]
    }
}
impl<T: TreeToken> Index<DefExprId> for Ast<T> {
    type Output = (ExprId, UnresolvedType);
    fn index(&self, index: DefExprId) -> &Self::Output {
        &self.def_exprs[index.0 as usize]
    }
}
impl<T: TreeToken> Index<ExprIds> for Ast<T> {
    type Output = [Expr<T>];

    fn index(&self, index: ExprIds) -> &Self::Output {
        &self.exprs[index.idx as usize..index.idx as usize + index.count as usize]
    }
}
impl<T: TreeToken> Index<CallId> for Ast<T> {
    type Output = Call<T>;
    fn index(&self, index: CallId) -> &Self::Output {
        &self.calls[index.idx()]
    }
}
impl<T: TreeToken> Index<FunctionId> for Ast<T> {
    type Output = Function<T>;

    fn index(&self, index: FunctionId) -> &Self::Output {
        &self.functions[index.idx()]
    }
}
impl<T: TreeToken> Index<TypeId> for Ast<T> {
    type Output = TypeDef<T>;

    fn index(&self, index: TypeId) -> &Self::Output {
        &self.types[index.idx()]
    }
}
impl<T: TreeToken> Index<TraitId> for Ast<T> {
    type Output = TraitDefinition<T>;

    fn index(&self, index: TraitId) -> &Self::Output {
        &self.traits[index.idx()]
    }
}
impl<T: TreeToken> Index<GlobalId> for Ast<T> {
    type Output = Global<T>;
    fn index(&self, index: GlobalId) -> &Self::Output {
        &self.globals[index.idx()]
    }
}

pub struct AstBuilder<T: TreeToken> {
    scopes: Vec<Scope<T>>,
    exprs: Vec<Expr<T>>,
    def_exprs: Vec<(ExprId, UnresolvedType)>,
    calls: Vec<Call<T>>,
    functions: Vec<Function<T>>,
    types: Vec<TypeDef<T>>,
    traits: Vec<TraitDefinition<T>>,
    globals: Vec<Global<T>>,
    use_count: u32,
}
impl<T: TreeToken> Default for AstBuilder<T> {
    fn default() -> Self {
        Self {
            scopes: Vec::new(),
            exprs: Vec::new(),
            def_exprs: Vec::new(),
            calls: Vec::new(),
            functions: Vec::new(),
            types: Vec::new(),
            traits: Vec::new(),
            globals: Vec::new(),
            use_count: 0,
        }
    }
}
impl<T: TreeToken> AstBuilder<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scope(&mut self, scope: Scope<T>) -> ScopeId {
        let id = ScopeId(self.scopes.len() as _);
        self.scopes.push(scope);
        id
    }

    pub fn get_scope_mut(&mut self, id: ScopeId) -> &mut Scope<T> {
        &mut self.scopes[id.idx()]
    }

    pub fn expr(&mut self, expr: Expr<T>) -> ExprId {
        let id = ExprId(self.exprs.len() as _);
        self.exprs.push(expr);
        id
    }

    pub fn def_expr(&mut self, value: ExprId, ty: UnresolvedType) -> DefExprId {
        let id = DefExprId(self.def_exprs.len() as _);
        self.def_exprs.push((value, ty));
        id
    }

    pub fn exprs(&mut self, exprs: impl IntoIterator<Item = Expr<T>>) -> ExprIds {
        let idx = self.exprs.len();
        self.exprs.extend(exprs);
        let count = self.exprs.len() - idx;
        ExprIds {
            idx: idx as _,
            count: count as _,
        }
    }

    pub fn call(&mut self, call: Call<T>) -> CallId {
        let id = CallId(self.calls.len() as _);
        self.calls.push(call);
        id
    }

    pub fn function(&mut self, function: Function<T>) -> FunctionId {
        let id = FunctionId(self.functions.len() as _);
        self.functions.push(function);
        id
    }

    pub fn type_def(&mut self, type_def: TypeDef<T>) -> TypeId {
        let id = TypeId(self.types.len() as _);
        for method in type_def.methods.values() {
            self.functions[method.function.idx()].context = FunctionContext::Method(id);
        }
        for impl_ in &type_def.impls {
            for method in &impl_.base.functions {
                self.functions[method.function.idx()].context =
                    FunctionContext::InherentImpl(id, impl_.implemented_trait);
            }
        }
        self.types.push(type_def);
        id
    }

    pub fn trait_def(&mut self, trait_def: TraitDefinition<T>) -> TraitId {
        let id = TraitId(self.traits.len() as _);
        for impl_ in &trait_def.impls {
            for method in &impl_.base.functions {
                self.functions[method.function.idx()].context =
                    FunctionContext::Impl(id, impl_.implemented_type.clone());
            }
        }
        self.traits.push(trait_def);
        id
    }

    pub fn global(&mut self, global: Global<T>) -> GlobalId {
        let id = GlobalId(self.globals.len() as _);
        self.globals.push(global);
        id
    }

    pub fn get_expr(&self, expr: ExprId) -> &Expr<T> {
        &self.exprs[expr.idx()]
    }

    pub fn next_use_id(&mut self) -> UseId {
        let id = self.use_count;
        self.use_count += 1;
        UseId(id)
    }

    pub fn finish_with_top_level_scope(self, src: Box<str>, top_level_scope: ScopeId) -> Ast<T> {
        Ast {
            src,
            top_level_scope,
            scopes: self.scopes.into_boxed_slice(),
            exprs: self.exprs.into_boxed_slice(),
            def_exprs: self.def_exprs.into_boxed_slice(),
            calls: self.calls.into_boxed_slice(),
            functions: self.functions.into_boxed_slice(),
            types: self.types.into_boxed_slice(),
            traits: self.traits.into_boxed_slice(),
            globals: self.globals.into_boxed_slice(),
            use_count: self.use_count,
        }
    }

    pub(super) fn assign_function_name(&mut self, id: FunctionId, associated_name: TSpan) {
        self.functions[id.idx()].associated_name = associated_name;
    }

    pub(super) fn assign_trait_name(&mut self, id: TraitId, associated_name: TSpan) {
        self.traits[id.idx()].associated_name = associated_name;
    }
}

pub trait TreeToken: Debug + Clone {
    type Opt<T: Debug + Clone>: Debug + Clone;
    type Either<A: Debug + Clone, B: Debug + Clone>: Debug + Clone;
    fn t(token: Token) -> Self;
    fn opt<T: Debug + Clone>(t: T) -> Self::Opt<T>;
    fn opt_none<T: Debug + Clone>() -> Self::Opt<T>;
    fn either_a<A: Debug + Clone, B: Debug + Clone>(a: A) -> Self::Either<A, B>;
    fn either_b<A: Debug + Clone, B: Debug + Clone>(b: B) -> Self::Either<A, B>;
}
impl TreeToken for () {
    type Opt<T: Debug + Clone> = ();
    type Either<A: Debug + Clone, B: Debug + Clone> = ();
    fn t(_token: Token) -> Self {}
    fn opt<T: Debug + Clone>(_t: T) -> Self::Opt<T> {}
    fn opt_none<T: Debug + Clone>() -> Self::Opt<T> {}
    fn either_a<A: Debug + Clone, B: Debug + Clone>(_a: A) -> Self::Either<A, B> {}
    fn either_b<A: Debug + Clone, B: Debug + Clone>(_b: B) -> Self::Either<A, B> {}
}
impl TreeToken for Token {
    type Opt<T: Debug + Clone> = Option<T>;
    type Either<A: Debug + Clone, B: Debug + Clone> = Either<A, B>;
    fn t(token: Token) -> Self {
        token
    }

    fn opt<T: Debug + Clone>(t: T) -> Self::Opt<T> {
        Some(t)
    }

    fn opt_none<T: Debug + Clone>() -> Self::Opt<T> {
        None
    }

    fn either_a<A: Debug + Clone, B: Debug + Clone>(a: A) -> Self::Either<A, B> {
        Either::A(a)
    }

    fn either_b<A: Debug + Clone, B: Debug + Clone>(b: B) -> Self::Either<A, B> {
        Either::B(b)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Either<A, B> {
    A(A),
    B(B),
}

#[derive(Debug)]
pub struct TypeDef<T: TreeToken = ()> {
    pub t_introducer: T,
    pub generics: Generics<T>,
    pub t_lbrace: T,
    pub scope: ScopeId,
    pub content: TypeContent<T>,
    pub methods: DHashMap<String, Method<T>>,
    pub impls: Box<[InherentImpl<T>]>,
    pub t_rbrace: T,
}
impl<T: TreeToken> TypeDef<T> {
    pub fn generic_count(&self) -> u8 {
        self.generics.types.len() as u8
    }
    pub fn span(&self, scopes: &[Scope<T>]) -> TSpan {
        scopes[self.scope.idx()].span
    }
}

#[derive(Debug, Clone)]
pub struct Method<T: TreeToken> {
    pub name: TSpan,
    pub t_colon_colon: T,
    pub function: FunctionId,
}

#[derive(Debug)]
pub enum TypeContent<T: TreeToken> {
    Struct {
        members: Vec<StructMember<T>>,
    },
    Enum {
        variants: Box<[EnumVariantDefinition<T>]>,
    },
}

#[derive(Debug)]
pub struct StructMember<T: TreeToken> {
    pub attributes: Attributes<T>,
    pub name: TSpan,
    pub ty: UnresolvedType,
}

#[derive(Debug)]
pub struct Scope<T: TreeToken = ()> {
    pub parent: Option<ScopeId>,
    pub definitions: DHashMap<String, Definition<T>>,
    pub span: TSpan,
    pub has_errors: bool,
}
impl<T: TreeToken> Scope<T> {
    pub fn missing() -> Self {
        Self {
            parent: None,
            definitions: dmap::new(),
            span: TSpan::MISSING,
            has_errors: true,
        }
    }

    pub fn from_generics(
        parent: ScopeId,
        src: &str,
        generics: &[GenericDef<T>],
        span: TSpan,
    ) -> Self {
        Self {
            parent: Some(parent),
            definitions: generics
                .iter()
                .enumerate()
                .map(|(i, generic)| (generic.name(src).to_owned(), Definition::Generic(i as u8)))
                .collect(),
            span,
            has_errors: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Definition<T: TreeToken = ()> {
    Expr {
        t_name: T,
        name_span: TSpan,
        t_colon_colon: T::Either<T, (T, T)>,
        id: DefExprId,
    },
    Use {
        t_use: T,
        id: UseId,
        path: IdentPath,
    },
    Global(GlobalId),
    Module(ModuleId),
    Generic(u8),
}

pub struct Item<T: TreeToken> {
    #[allow(unused)] // TODO: remove when using attributes
    pub attributes: Attributes<T>,
    pub value: ItemValue<T>,
}

pub enum ItemValue<T: TreeToken> {
    Definition {
        t_name: T,
        name: String,
        name_span: TSpan,
        t_colon_colon: T::Either<T, (T, T)>,
        annotated_ty: UnresolvedType,
        value: ExprId,
    },
    Use {
        t_use: T,
        path: IdentPath,
    },
    Expr(Expr<T>),
}

#[derive(Debug)]
pub struct Attribute<T: TreeToken> {
    pub t_at: T,
    pub path: IdentPath,
    pub t_parens: T::Opt<(T, T)>,
    pub args: ExprIds,
    pub span: TSpan,
}

pub type Attributes<T> = Box<[Attribute<T>]>;

#[derive(Debug)]
pub struct EnumVariantDefinition<T: TreeToken> {
    pub name_span: TSpan,
    pub t_parens: T::Opt<(T, T)>,
    pub args: Box<[UnresolvedType]>,
    pub end: u32,
}
impl<T: TreeToken> EnumVariantDefinition<T> {
    pub fn span(&self) -> TSpan {
        TSpan::new(self.name_span.start, self.end)
    }
}

#[derive(Debug)]
pub struct TraitDefinition<T: TreeToken = ()> {
    pub t_trait: T,
    pub generics: Generics<T>,
    pub scope: ScopeId,
    pub t_lbrace: T,
    pub functions: Vec<Method<T>>,
    pub t_rbrace: T,
    /// (for, lbrace, rbrace)
    pub t_attached_impls: T::Opt<(T, T, T)>,
    pub impls: Box<[Impl<T>]>,
    pub associated_name: TSpan,
}
impl<T: TreeToken> TraitDefinition<T> {
    pub fn span(&self, scopes: &[Scope<T>]) -> TSpan {
        scopes[self.scope.idx()].span
    }
}

pub type TraitFunctions = Box<[(TSpan, FunctionId)]>;

#[derive(Debug)]
pub struct Impl<T: TreeToken = ()> {
    pub t_impl: T,
    pub t_underscore: T,
    pub implemented_type: UnresolvedType,
    pub t_for: T,
    pub base: BaseImpl<T>,
}

#[derive(Debug)]
pub struct InherentImpl<T: TreeToken = ()> {
    pub attributes: Attributes<T>,
    pub t_impl: T,
    pub implemented_trait: IdentPath,
    pub base: BaseImpl<T>,
}

#[derive(Debug)]
pub struct BaseImpl<T: TreeToken = ()> {
    pub scope: ScopeId,
    pub generics: Generics<T>,
    pub t_brackets: T::Opt<(T, T)>,
    pub trait_generics: Box<[UnresolvedType]>,
    pub trait_generics_span: TSpan,
    pub t_lbrace: T,
    pub functions: Box<[Method<T>]>,
    pub t_rbrace: T,
}

#[derive(Debug)]
pub struct Global<T: TreeToken = ()> {
    pub name: Box<str>,
    pub name_span: TSpan,
    pub t_name: T,
    pub scope: ScopeId,
    pub t_colon_and_equals_or_colon_equals: T::Either<(T, T), T>,
    pub ty: UnresolvedType,
    pub val: ExprId,
    pub span: TSpan,
}

#[derive(Debug)]
pub struct Function<T: TreeToken = ()> {
    pub attributes: Attributes<T>,
    pub t_fn: T,
    pub generics: Generics<T>,
    pub t_parens: T::Opt<(T, T)>,
    pub params: Box<[(TSpan, UnresolvedType)]>,
    pub named_params: Box<[NamedParam<T>]>,
    pub varargs: bool,
    pub t_varargs: T::Opt<T>,
    pub t_arrow: T::Opt<T>,
    pub return_type: UnresolvedType,
    pub t_colon: T::Opt<T>,
    pub t_extern: T::Opt<T>,
    pub body: Option<ExprId>,
    pub scope: ScopeId,
    pub signature_span: TSpan,
    pub associated_name: TSpan,
    pub context: FunctionContext,
}

#[derive(Debug)]
pub enum FunctionContext {
    None,
    Method(TypeId),
    Impl(TraitId, UnresolvedType),
    InherentImpl(TypeId, IdentPath),
}

#[derive(Debug)]
pub struct NamedParam<T: TreeToken = ()> {
    pub name: TSpan,
    pub ty: UnresolvedType,
    pub t_eq: T,
    pub default_value: ExprId,
}

#[derive(Debug)]
pub struct Generics<T: TreeToken = ()> {
    pub t_brackets: T::Opt<(T, T)>,
    pub types: Box<[GenericDef<T>]>,
}
impl<T: TreeToken> Generics<T> {
    pub fn count(&self) -> u8 {
        self.types.len() as u8
    }
}

#[derive(Clone, Debug)]
pub struct GenericDef<T: TreeToken> {
    /// missing span indicates that this is a `Self` type in a trait definition
    pub name: TSpan,
    pub t_colon: T::Opt<T>,
    pub bounds: Box<[TraitBound<T>]>,
}
impl<T: TreeToken> GenericDef<T> {
    pub fn span(&self) -> TSpan {
        TSpan::new(
            self.name.start,
            self.bounds
                .last()
                .map_or(self.name.end, |bound| bound.generics_span.end),
        )
    }

    pub fn name<'s>(&self, src: &'s str) -> &'s str {
        if self.name == TSpan::MISSING {
            "Self"
        } else {
            &src[self.name.range()]
        }
    }
}

#[derive(Clone, Debug)]
pub struct TraitBound<T: TreeToken = ()> {
    pub path: IdentPath,
    pub brackets: T::Opt<(T, T)>,
    pub generics: Box<[UnresolvedType]>,
    pub generics_span: TSpan,
}

#[derive(Debug)]
pub enum Expr<T: TreeToken = ()> {
    Error(TSpan),
    Block {
        t_open: T,
        scope: ScopeId,
        items: ExprIds,
        t_close: T,
    },
    Nested {
        span: TSpan,
        t_lparen: T,
        inner: ExprId,
        t_rparen: T,
    },

    // ---------- value literals ----------
    IntLiteral {
        span: TSpan,
        t: T,
    },
    FloatLiteral {
        span: TSpan,
        t: T,
    },
    StringLiteral {
        span: TSpan,
        t: T,
    },
    Array {
        span: TSpan,
        t_lbracket: T,
        elements: ExprIds,
        t_rbracket: T,
    },
    Tuple {
        span: TSpan,
        t_lparen: T,
        elements: ExprIds,
        named_elements: Box<[(TSpan, ExprId)]>,
        t_rparen: T,
    },
    EnumLiteral {
        span: TSpan,
        t_dot: T,
        ident: TSpan,
        t_ident: T,
        t_parens: T::Opt<(T, T)>,
        args: ExprIds,
    },

    // ---------- definition literals ----------
    Function {
        id: FunctionId,
    },
    Primitive {
        primitive: Primitive,
        start: u32,
        t: T,
    },
    TypeDeclaration {
        id: TypeId,
    },
    Trait {
        id: TraitId,
    },

    // ---------- variables and names ----------
    Ident {
        span: TSpan,
        t: T,
    },
    DeclareWithVal {
        pat: ExprId,
        t_colon_and_equals_or_colon_equals: T::Either<(T, T), T>,
        annotated_ty: UnresolvedType,
        val: ExprId,
    },
    /// underscore: _ for ignoring values
    Hole {
        loc: u32,
        t: T,
    },

    // ---------- operations ----------
    UnOp {
        start_or_end: u32,
        t_op: T,
        op: UnOp,
        inner: ExprId,
    },
    BinOp {
        t_op: T,
        op: Operator,
        l: ExprId,
        r: ExprId,
    },
    As {
        value: ExprId,
        t_as: T,
        ty: UnresolvedType,
    },
    Root {
        start: u32,
        t: T,
    },

    // ---------- members and paths ----------
    MemberAccess {
        left: ExprId,
        t_dot: T,
        name: TSpan,
    },
    Index {
        expr: ExprId,
        t_lbracket: T,
        idx: ExprId,
        t_rbracket: T,
        end: u32,
    },
    TupleIdx {
        left: ExprId,
        t_dot: T,
        t_int: T,
        idx: u32,
        end: u32,
    },

    // ---------- return ----------
    ReturnUnit {
        start: u32,
        t: T,
    },
    Return {
        start: u32,
        t_ret: T,
        val: ExprId,
    },

    // ---------- control flow ----------
    If {
        start: u32,
        t_if: T,
        cond: ExprId,
        t_colon: T::Opt<T>,
        then: ExprId,
    },
    IfElse {
        start: u32,
        t_if: T,
        cond: ExprId,
        t_colon: T::Opt<T>,
        then: ExprId,
        t_else: T,
        else_: ExprId,
    },
    IfPat {
        start: u32,
        t_if: T,
        t_colon_eq: T,
        pat: ExprId,
        t_colon: T::Opt<T>,
        value: ExprId,
        then: ExprId,
    },
    IfPatElse {
        start: u32,
        t_if: T,
        pat: ExprId,
        t_colon_eq: T,
        value: ExprId,
        t_colon: T::Opt<T>,
        then: ExprId,
        t_else: T,
        else_: ExprId,
    },
    Match {
        t_match: T,
        span: TSpan,
        val: ExprId,
        t_lbrace: T,
        branches: ExprIdPairs,
        t_rbrace: T,
    },
    While {
        start: u32,
        t_while: T,
        cond: ExprId,
        t_colon: T::Opt<T>,
        body: ExprId,
    },
    /// while ... := ...
    WhilePat {
        start: u32,
        t_while: T,
        pat: ExprId,
        t_colon_eq: T,
        val: ExprId,
        t_colon: T::Opt<T>,
        body: ExprId,
    },
    For {
        start: u32,
        t_for: T,
        pat: ExprId,
        t_in: T,
        iter: ExprId,
        t_colon: T::Opt<T>,
        body: ExprId,
    },
    FunctionCall(CallId),

    // ---------- other ----------
    Asm {
        t_asm: T,
        t_lparen: T,
        t_string_literal: T,
        asm_str_span: TSpan,
        args: ExprIds,
        t_rparen: T,
        span: TSpan,
    },
    Break {
        start: u32,
        t: T,
    },
    Continue {
        start: u32,
        t: T,
    },
}
impl<T: TreeToken> Expr<T> {
    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block { .. })
    }

    pub fn span(&self, ast: &Ast<T>) -> TSpan {
        self.span_inner(
            &ast.exprs,
            &ast.functions,
            &ast.types,
            &ast.traits,
            &ast.calls,
            &ast.scopes,
        )
    }

    pub fn span_builder(&self, ast: &AstBuilder<T>) -> TSpan {
        self.span_inner(
            &ast.exprs,
            &ast.functions,
            &ast.types,
            &ast.traits,
            &ast.calls,
            &ast.scopes,
        )
    }

    fn span_inner(
        &self,
        exprs: &[Self],
        functions: &[Function<T>],
        types: &[TypeDef<T>],
        traits: &[TraitDefinition<T>],
        calls: &[Call<T>],
        scopes: &[Scope<T>],
    ) -> TSpan {
        // shorthands for getting start and end position of an ExprId
        let s =
            |r: &ExprId| exprs[r.idx()].start_inner(exprs, functions, types, traits, calls, scopes);
        let e =
            |r: &ExprId| exprs[r.idx()].end_inner(exprs, functions, types, traits, calls, scopes);

        match self {
            Expr::Error(span)
            | Expr::StringLiteral { span, .. }
            | Expr::IntLiteral { span, .. }
            | Expr::FloatLiteral { span, .. }
            | Expr::Nested { span, .. }
            | Expr::Ident { span, .. }
            | Expr::Array { span, .. }
            | Expr::Tuple { span, .. }
            | Expr::Match { span, .. }
            | Expr::EnumLiteral { span, .. } => *span,
            Expr::Block { scope, .. } => scopes[scope.idx()].span,
            Expr::Function { id } => scopes[functions[id.idx()].scope.idx()].span,
            Expr::TypeDeclaration { id } => types[id.idx()].span(scopes),
            Expr::Trait { id } => traits[id.idx()].span(scopes),
            Expr::DeclareWithVal { pat, val, .. } => TSpan::new(s(pat), e(val)),
            Expr::Return { start, val, .. } => TSpan::new(*start, e(val)),
            &Expr::ReturnUnit { start, .. } => TSpan::new(start, start + Keyword::Ret.len()),
            &Expr::Hole { loc, .. } => TSpan::new(loc, loc + 1),
            Expr::If { start, then, .. } | Expr::IfPat { start, then, .. } => {
                TSpan::new(*start, e(then))
            }
            Expr::IfElse { start, else_, .. } | Expr::IfPatElse { start, else_, .. } => {
                TSpan::new(*start, e(else_))
            }
            Expr::While { start, body, .. }
            | Expr::WhilePat { start, body, .. }
            | Expr::For { start, body, .. } => TSpan::new(*start, e(body)),
            Expr::FunctionCall(call_id) => {
                let Call {
                    called_expr, end, ..
                } = &calls[call_id.idx()];
                TSpan::new(s(called_expr), *end)
            }
            Expr::UnOp {
                start_or_end,
                op,
                inner,
                ..
            } => {
                if op.postfix() {
                    TSpan::new(s(inner), *start_or_end)
                } else {
                    TSpan::new(*start_or_end, e(inner))
                }
            }
            Expr::BinOp { l, r, .. } => TSpan::new(s(l), e(r)),
            Expr::MemberAccess { left, name, .. } => TSpan::new(s(left), name.end),
            Expr::Index { expr, end, .. } => TSpan::new(s(expr), *end),
            Expr::TupleIdx {
                left: expr, end, ..
            } => TSpan::new(s(expr), *end),
            Expr::As { value, ty, .. } => TSpan::new(s(value), ty.span().end),
            &Expr::Root { start, .. } => TSpan::new(start, start + 4),
            Expr::Asm { span, .. } => *span,
            &Expr::Primitive {
                primitive, start, ..
            } => TSpan::new(start, start + <&str>::from(primitive).len() as u32),
            &Expr::Break { start, .. } => TSpan::new(start, start + 5),
            &Expr::Continue { start, .. } => TSpan::new(start, start + 8),
        }
    }

    pub fn start(&self, ast: &Ast<T>) -> u32 {
        self.start_inner(
            &ast.exprs,
            &ast.functions,
            &ast.types,
            &ast.traits,
            &ast.calls,
            &ast.scopes,
        )
    }

    pub fn end(&self, ast: &Ast<T>) -> u32 {
        self.end_inner(
            &ast.exprs,
            &ast.functions,
            &ast.types,
            &ast.traits,
            &ast.calls,
            &ast.scopes,
        )
    }

    pub fn start_inner(
        &self,
        exprs: &[Self],
        functions: &[Function<T>],
        types: &[TypeDef<T>],
        traits: &[TraitDefinition<T>],
        calls: &[Call<T>],
        scopes: &[Scope<T>],
    ) -> u32 {
        //TODO: more efficient implementation
        self.span_inner(exprs, functions, types, traits, calls, scopes)
            .start
    }

    pub fn end_inner(
        &self,
        exprs: &[Self],
        functions: &[Function<T>],
        types: &[TypeDef<T>],
        traits: &[TraitDefinition<T>],
        calls: &[Call<T>],
        scopes: &[Scope<T>],
    ) -> u32 {
        //TODO: more efficient implementation
        self.span_inner(exprs, functions, types, traits, calls, scopes)
            .end
    }
}

#[derive(Debug)]
pub struct Call<T: TreeToken = ()> {
    pub called_expr: ExprId,
    pub t_lparen: T,
    pub open_paren_start: u32,
    pub args: ExprIds,
    pub named_args: Box<[(TSpan, ExprId)]>,
    pub t_rparen: T,
    pub end: u32,
}
impl Call {
    pub fn total_arg_count(&self) -> u32 {
        self.args.count + self.named_args.len() as u32
    }
}

#[derive(Debug, Clone, Copy)]
pub enum UnOp {
    Neg,
    Not,
    Ref,
    Deref,
}
impl fmt::Display for UnOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let c = match self {
            Self::Neg => '-',
            Self::Not => '!',
            Self::Ref => '&',
            Self::Deref => '^',
        };
        write!(f, "{c}")
    }
}
impl UnOp {
    pub fn postfix(self) -> bool {
        matches!(self, UnOp::Deref)
    }
}

#[derive(Debug, Clone)]
pub enum UnresolvedType {
    Primitive {
        ty: Primitive,
        span_start: u32,
    },
    Unresolved(IdentPath, Option<(Box<[UnresolvedType]>, TSpan)>),
    Pointer(Box<(UnresolvedType, u32)>),
    Array(Box<(UnresolvedType, Option<u32>, TSpan)>),
    Tuple(Vec<UnresolvedType>, TSpan),
    Function {
        span_and_return_type: Box<(TSpan, UnresolvedType)>,
        params: Box<[UnresolvedType]>,
    },
    Infer(TSpan),
}
impl UnresolvedType {
    pub fn to_string(&self, s: &mut String, src: &str) {
        match self {
            &UnresolvedType::Primitive { ty, .. } => s.push_str(ty.into()),
            UnresolvedType::Unresolved(path, generics) => {
                let (root, segments, last) = path.segments(src);
                if root.is_some() {
                    s.push_str("root");
                }
                let mut prev = root.is_some();
                for (segment, _) in segments {
                    if prev {
                        s.push('.');
                    }
                    s.push_str(segment);
                    prev = true;
                }
                if let Some((last, _)) = last {
                    if prev {
                        s.push('.');
                    }
                    s.push_str(last);
                }
                if let Some((generics, _)) = generics {
                    s.push('[');
                    for (i, ty) in generics.iter().enumerate() {
                        if i != 0 {
                            s.push_str(", ");
                        }
                        ty.to_string(s, src);
                    }
                    s.push(']');
                }
            }
            UnresolvedType::Pointer(pointee) => {
                s.push('*');
                pointee.0.to_string(s, src);
            }
            UnresolvedType::Array(b) => {
                s.push('[');
                b.0.to_string(s, src);
                s.push_str("; ");
                use core::fmt::Write;
                match b.1 {
                    Some(count) => write!(s, "{count}]").unwrap(),
                    None => s.push_str("_]"),
                }
            }
            UnresolvedType::Tuple(types, _) => {
                s.push('(');
                for (i, ty) in types.iter().enumerate() {
                    if i != 0 {
                        s.push_str(", ");
                    }
                    ty.to_string(s, src);
                }
                s.push(')');
            }
            UnresolvedType::Function {
                span_and_return_type,
                params,
            } => {
                s.push_str("fn(");
                for (i, ty) in params.iter().enumerate() {
                    if i != 0 {
                        s.push_str(", ");
                    }
                    ty.to_string(s, src);
                }
                s.push_str(") -> ");
                span_and_return_type.1.to_string(s, src);
            }
            UnresolvedType::Infer(_) => s.push('_'),
        }
    }

    pub fn span(&self) -> TSpan {
        match self {
            &UnresolvedType::Primitive { ty, span_start } => {
                TSpan::with_len(span_start, ty.token_len().get())
            }
            UnresolvedType::Tuple(_, span) | UnresolvedType::Infer(span) => *span,
            UnresolvedType::Unresolved(path, generics) => generics.as_ref().map_or_else(
                || path.span(),
                |generics| TSpan::new(path.span().start, generics.1.end),
            ),
            UnresolvedType::Array(array) => array.2,
            UnresolvedType::Pointer(ptr) => {
                let (inner, start) = &**ptr;
                TSpan::new(*start, inner.span().end)
            }
            UnresolvedType::Function {
                span_and_return_type,
                ..
            } => span_and_return_type.0,
        }
    }
}
