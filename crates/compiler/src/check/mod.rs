mod call;
mod cast;
mod closure;
mod exhaust;
pub mod expr;
mod lval;
mod pattern;
pub mod traits;
mod type_def;

use std::cell::RefCell;

use dmap::DHashMap;
use error::{CompileError, Error, Errors, span::TSpan};
use parser::ast::{self, Ast, Expr, ExprId, ModuleId, ScopeId};
pub use traits::trait_def;
pub use type_def::type_def;

use crate::{
    Compiler, InvalidTypeError, Type,
    callconv::CallConv,
    check::closure::CheckedClosure,
    compiler::{
        BodyOrTypes, CheckedFunction, Generics, LocalScope, LocalScopeParent, ModuleSpan,
        Signature, VarId, builtins,
    },
    eval::ConstValueId,
    hir::{CastId, HIRBuilder, Hir, LValue, Node},
    types::{BaseType, TypeFull},
    typing::{
        Bound, LocalOrGlobalInstance, LocalTypeId, LocalTypeIds, TypeInfo, TypeInfoOrIdx, TypeTable,
    },
};

use self::exhaust::Exhaustion;

pub trait Hooks {
    fn on_check_expr(
        &mut self,
        _expr: ExprId,
        _hir: &mut HIRBuilder,
        _scope: &mut LocalScope,
        _ty: LocalTypeId,
        _return_ty: LocalTypeId,
        _noreturn: &mut bool,
    ) {
    }
    fn on_check_pattern(
        &mut self,
        _expr: ExprId,
        _hir: &mut HIRBuilder,
        _scope: &mut LocalScope,
        _ty: LocalTypeId,
    ) {
    }
    fn on_checked_lvalue(
        &mut self,
        _expr: ExprId,
        _hir: &mut HIRBuilder,
        _scope: &mut LocalScope,
        _ty: LocalTypeId,
    ) {
    }
    fn on_exit_scope(&mut self, _scope: &mut LocalScope, _hir: &mut HIRBuilder) {}
}
impl Hooks for () {}

pub fn function<H: Hooks>(
    compiler: &Compiler,
    module: ModuleId,
    id: parser::ast::FunctionId,
    hooks: &mut H,
) -> crate::compiler::CheckedFunction {
    let ast = &compiler.modules[module.idx()].parsed.get().unwrap().ast;

    let function = &ast[id];
    let name = crate::compiler::function_name(ast, function, module, id);

    let _enter = tracing::span!(
        tracing::Level::INFO,
        "check_function",
        module = ?module,
        id = ?id,
        function = name,
    )
    .entered();

    let signature = compiler.get_signature(module, id);
    let mut types = TypeTable::new();

    let param_types = types.add_multiple_unknown(signature.total_arg_count() as u32);
    for ((_, param), r) in signature.all_params().zip(param_types.iter()) {
        let i = TypeInfoOrIdx::TypeInfo(TypeInfo::Known(param));
        types.replace(r, i);
    }

    let return_type = types.add(TypeInfo::Known(signature.return_type));

    let generic_count = signature.generics.count();
    let varargs = signature.varargs;

    let body_or_types = if let Some(body) = function.body {
        let hir = HIRBuilder::new(types);
        let params = signature
            .all_params()
            .zip(param_types.iter())
            .map(|((name, _), id)| (name.into(), id));

        let hir = check(
            compiler,
            ast,
            module,
            &signature.generics,
            function.scope,
            hir,
            params,
            body,
            return_type,
            &name,
            LocalScopeParent::None,
            hooks,
        );
        BodyOrTypes::Body(hir)
    } else {
        let (types, _) = types.finish(compiler, &signature.generics, module);
        BodyOrTypes::Types(types)
    };

    CheckedFunction {
        name,
        params: param_types,
        varargs,
        return_type,
        generic_count,
        body_or_types,
    }
}

pub fn check<H: Hooks>(
    compiler: &Compiler,
    ast: &Ast,
    module: ModuleId,
    generics: &Generics,
    scope: ScopeId,
    mut hir: HIRBuilder,
    params: impl IntoIterator<Item = (Box<str>, LocalTypeId)>,
    expr: ExprId,
    expected: LocalTypeId,
    name: &str,
    parent_scope: LocalScopeParent,
    hooks: &mut H,
) -> Hir {
    let params = params.into_iter();
    let mut param_vars = Vec::with_capacity(params.size_hint().0);
    let variables = params
        .map(|(name, ty)| {
            let var = hir.add_var(ty);
            param_vars.push(var);
            (name, var)
        })
        .collect();

    let mut scope = crate::compiler::LocalScope {
        parent: parent_scope,
        variables,
        module,
        static_scope: Some(scope),
    };
    let mut check_ctx = Ctx {
        compiler,
        ast,
        module,
        generics,
        hir,
        control_flow_stack: Vec::new(),
        deferred_exhaustions: Vec::new(),
        deferred_casts: Vec::new(),
        checked_closures: Vec::new(),
        hooks,
    };
    let root = if check_ctx.hir.types[expected].is_unit() {
        check_ctx.check_statement(expr, &mut scope, expected, &mut false)
    } else {
        check_ctx.check(expr, &mut scope, expected, expected, &mut false)
    };
    check_ctx
        .hooks
        .on_exit_scope(&mut scope, &mut check_ctx.hir);
    check_ctx.finish(root, param_vars, name)
}

pub struct ProjectErrors {
    pub by_file: RefCell<dmap::DHashMap<ModuleId, Errors>>,
    crash_on_error: bool,
}
impl ProjectErrors {
    #[track_caller]
    pub fn emit(&self, module: ModuleId, error: CompileError) {
        if self.crash_on_error {
            panic!(
                "Error encountered and --crash-on-error is enabled. The error is: {error:?} in {module:?}"
            );
        }
        tracing::debug!(target: "error", "Emitting error {error:?}");
        self.by_file
            .borrow_mut()
            .entry(module)
            .or_default()
            .emit_err(error);
    }

    pub fn enable_crash_on_error(&mut self) {
        self.crash_on_error = true;
    }

    pub fn new() -> Self {
        Self {
            by_file: RefCell::new(DHashMap::default()),
            crash_on_error: false,
        }
    }

    pub fn add_module(&self, module: ModuleId, errors: Errors) {
        let previous = self.by_file.borrow_mut().insert(module, errors);
        debug_assert!(previous.is_none(), "Duplicate module inserted into errors");
    }
}

pub struct Ctx<'a, H: Hooks> {
    pub compiler: &'a Compiler,
    pub ast: &'a Ast,
    pub module: ModuleId,
    pub generics: &'a Generics,
    pub hir: HIRBuilder,
    /// tracks a stack of any control flow that break/continue apply to (loops)
    /// labels will make the Vec actually useful to track labeled break/continue
    pub control_flow_stack: Vec<()>,
    /// Exhaustion value, type, corresponding expr
    pub deferred_exhaustions: Vec<(Exhaustion, LocalTypeId, ExprId)>,
    /// from, to, cast_expr
    pub deferred_casts: Vec<(LocalTypeId, LocalTypeId, ExprId, CastId)>,
    pub checked_closures: Vec<CheckedClosure>,
    pub hooks: &'a mut H,
}
impl<H: Hooks> Ctx<'_, H> {
    fn emit(&mut self, error: CompileError) {
        self.compiler.errors.emit(self.module, error);
    }

    fn primitives(&self) -> &builtins::Primitives {
        &self.compiler.builtins.primitives
    }

    fn span(&self, expr: ExprId) -> TSpan {
        self.ast[expr].span(self.ast)
    }

    fn specify(
        &mut self,
        ty: LocalTypeId,
        info: impl Into<TypeInfo>,
        span: impl FnOnce(&Ast) -> TSpan,
    ) {
        let info = info.into();
        self.hir
            .types
            .specify(ty, info, self.generics, self.compiler, || ModuleSpan {
                module: self.module,
                span: span(self.ast),
            })
    }

    fn specify_base(
        &mut self,
        ty: LocalTypeId,
        base: BaseType,
        generic_count: u32,
        span: impl FnOnce(&Ast) -> TSpan,
    ) -> LocalTypeIds {
        self.hir.types.specify_base(
            ty,
            base,
            generic_count,
            self.generics,
            self.compiler,
            || ModuleSpan {
                module: self.module,
                span: span(self.ast),
            },
            |types| types.add_multiple_unknown(generic_count),
        )
    }

    fn unify(&mut self, a: LocalTypeId, b: LocalTypeId, span: impl FnOnce(&Ast) -> TSpan) {
        self.hir
            .types
            .unify(a, b, self.generics, self.compiler, || ModuleSpan {
                module: self.module,
                span: span(self.ast),
            })
    }

    fn invalidate(&mut self, ty: LocalTypeId) {
        self.hir.types.invalidate(ty);
    }

    pub fn from_type_instance<'a>(
        &mut self,
        ty: Type,
        generics: impl Into<LocalOrGlobalInstance<'a>>,
    ) -> TypeInfoOrIdx {
        self.hir
            .types
            .from_type_instance(&self.compiler.types, ty, generics)
    }

    fn auto_ref_deref(
        &mut self,
        mut pointer_count: u32,
        required_pointer_count: u32,
        mut value: Node,
        ty: LocalTypeId,
    ) -> Node {
        let mut current_ty = TypeInfoOrIdx::Idx(ty);
        // try promoting the value to an lvalue first to potentially add one level of pointer
        if pointer_count < required_pointer_count
            && let Some(lval) = LValue::try_from_node(&value, &mut self.hir)
        {
            let value_ty = self.hir.types.add_info_or_idx(current_ty);
            value = Node::AddressOf {
                value: self.hir.add_lvalue(lval),
                value_ty,
            };
            current_ty =
                TypeInfoOrIdx::TypeInfo(TypeInfo::Instance(BaseType::Pointer, value_ty.into()));
            pointer_count += 1
        }
        while pointer_count < required_pointer_count {
            let value_ty = self.hir.types.add_info_or_idx(current_ty);
            let variable = self.hir.add_var(value_ty);
            value = Node::Promote {
                value: self.hir.add(value),
                variable,
            };
            current_ty =
                TypeInfoOrIdx::TypeInfo(TypeInfo::Instance(BaseType::Pointer, value_ty.into()));
            pointer_count += 1;
        }
        while pointer_count > required_pointer_count {
            let pointee = match self.hir.types.get_info_or_idx(current_ty) {
                TypeInfo::Instance(BaseType::Pointer, pointee) => pointee.nth(0).unwrap(),
                TypeInfo::Known(ty) => {
                    let TypeFull::Instance(BaseType::Pointer, &[pointee]) =
                        self.compiler.types.lookup(ty)
                    else {
                        unreachable!()
                    };
                    self.hir.types.add(TypeInfo::Known(pointee))
                }
                _ => unreachable!(),
            };
            let prev_value = self.hir.add(value);
            value = Node::Deref {
                value: prev_value,
                deref_ty: pointee,
            };
            current_ty = pointee.into();
            pointer_count -= 1;
        }
        value
    }

    pub(crate) fn finish(self, root: Node, params: Vec<VarId>, name: &str) -> Hir {
        let mut hir = self
            .hir
            .finish(root, self.compiler, self.generics, self.module, params);
        let parsed = self.compiler.get_parsed_module(self.module);
        let symbols = &parsed.symbols;
        for closure in self.checked_closures {
            let hir = closure.hir.finish_with_types(
                hir.clone_types_for_closure(),
                closure.root,
                closure.params.iter().map(|(_name, id)| *id).collect(),
            );
            symbols.function_signatures[closure.id.idx()].start_resolving();
            let generic_count = closure.generics.count();
            let params: Box<[_]> = closure
                .params
                .iter()
                .map(|(name, id)| (name.clone(), hir[hir.vars[id.idx()].ty()]))
                .collect();
            let named_params: Box<[_]> = Box::new([]); // TODO: named closure params
            let callconv = self
                .compiler
                .get_and_check_function_callconv(
                    self.ast,
                    &self.ast[closure.id],
                    &params,
                    &named_params,
                )
                .expect("todo: handle invalid callconv");
            symbols.function_signatures[closure.id.idx()].put(Signature {
                params,
                named_params,
                varargs: false,
                return_type: hir[closure.return_type],
                generics: closure.generics,
                span: parsed.ast[closure.id].signature_span,
                callconv,
            });
            symbols.functions[closure.id.idx()].start_resolving();
            symbols.functions[closure.id.idx()].put(CheckedFunction {
                name: format!("{name}$closure{}", closure.id.idx()),
                params: closure.param_types,
                varargs: false,
                return_type: closure.return_type,
                generic_count,
                body_or_types: BodyOrTypes::Body(hir),
            });
        }
        for (exhaustion, ty, pat) in self.deferred_exhaustions {
            if let Ok(false) = exhaustion.is_exhausted(hir[ty], self.compiler) {
                let error = Error::Inexhaustive.at_span(self.ast[pat].span(self.ast));
                self.compiler.errors.emit(self.module, error);
            }
        }
        for (from_ty, to_ty, cast_expr, cast_id) in self.deferred_casts {
            let (cast, err) = cast::check(hir[from_ty], hir[to_ty], self.compiler, self.generics);
            hir[cast_id].cast_ty = cast;
            if let Some(err) = err {
                self.compiler
                    .errors
                    .emit(self.module, err.at_span(self.ast[cast_expr].span(self.ast)));
            }
        }
        hir
    }

    fn specify_resolved(
        &mut self,
        var: LocalTypeId,
        ty: Type,
        generics: LocalTypeIds,
        span: impl Fn(&Ast) -> TSpan,
    ) {
        let info = self.from_type_instance(ty, generics);
        match info {
            TypeInfoOrIdx::TypeInfo(info) => self.specify(var, info, span),
            TypeInfoOrIdx::Idx(other) => self.unify(var, other, span),
        }
    }

    fn emit_unknown(&self, bounds: crate::typing::Bounds, span: TSpan) {
        let needed_bound = (!bounds.is_empty()).then(|| {
            let mut s = String::new();
            let mut first = true;
            for bound in bounds.iter() {
                let bound = self.hir.types.get_bound(bound);
                if first {
                    first = false;
                } else {
                    s.push_str(" + ");
                }
                s.push_str(
                    self.compiler
                        .get_trait_name(bound.trait_id.0, bound.trait_id.1),
                );
            }
            s
        });
        let err = Error::TypeMustBeKnownHere { needed_bound };
        self.compiler.errors.emit(self.module, err.at_span(span));
    }

    fn type_to_string(&self, ty: impl Into<TypeInfoOrIdx>) -> String {
        let mut s = String::new();
        self.hir.types.type_to_string_inner(
            self.compiler,
            self.generics,
            self.hir.types.get_info_or_idx(ty.into()),
            &mut s,
        );
        s
    }

    pub fn specify_bound(&mut self, var: LocalTypeId, bound: Bound, span: TSpan) -> bool {
        let (var, info) = self.hir.types.find_shorten(var);
        match self
            .hir
            .types
            .unify_bound_with_info(self.compiler, self.generics, info, bound)
        {
            Ok(Some(info_or_idx)) => {
                self.hir.types.replace_value(var, info_or_idx);
                true
            }
            Ok(None) => {
                let trait_name = self
                    .compiler
                    .get_trait_name(bound.trait_id.0, bound.trait_id.1)
                    .into();
                self.compiler.errors.emit(
                    self.module,
                    Error::UnsatisfiedTraitBound {
                        trait_name,
                        ty: self.type_to_string(var),
                    }
                    .at_span(span),
                );
                self.invalidate(var);
                false
            }
            Err(InvalidTypeError) => {
                self.invalidate(var);
                true
            }
        }
    }

    pub fn specify_or_unify(
        &mut self,
        a: TypeInfoOrIdx,
        b: TypeInfoOrIdx,
        span: impl FnOnce(&Ast) -> TSpan,
    ) {
        match (a, b) {
            (TypeInfoOrIdx::TypeInfo(a), TypeInfoOrIdx::TypeInfo(b)) => {
                self.hir
                    .types
                    .unify_infos_or_error(a, b, self.generics, self.compiler, || ModuleSpan {
                        module: self.module,
                        span: span(self.ast),
                    });
            }
            (TypeInfoOrIdx::Idx(v), TypeInfoOrIdx::TypeInfo(info))
            | (TypeInfoOrIdx::TypeInfo(info), TypeInfoOrIdx::Idx(v)) => {
                self.specify(v, info, span);
            }
            (TypeInfoOrIdx::Idx(a), TypeInfoOrIdx::Idx(b)) => self.unify(a, b, span),
        }
    }
}

pub fn verify_main_signature(
    compiler: &Compiler,
    signature: &Signature,
) -> Result<(), Option<CompileError>> {
    if !signature.params.is_empty() || signature.varargs {
        return Err(Some(Error::MainArgs.at_span(signature.span)));
    }
    if signature.generics.count() != 0 {
        return Err(Some(Error::MainGenerics.at_span(signature.span)));
    }
    match compiler.types.lookup(signature.return_type) {
        TypeFull::Instance(BaseType::Invalid, _) => Err(None),
        TypeFull::Tuple {
            members: &[],
            named_members: &[],
        } => Ok(()),
        TypeFull::Instance(b, _) if b.is_int() => Ok(()),
        _ => Err(Some(
            Error::InvalidMainReturnType(
                compiler
                    .types
                    .display(signature.return_type, &signature.generics)
                    .to_string(),
            )
            .at_span(signature.span),
        )),
    }
}

fn get_string_literal(src: &str, span: TSpan) -> Box<str> {
    let inp = &src[span.start as usize + 1..span.end as usize - 1];
    let mut out = String::with_capacity(inp.len());
    let mut saw_backslash = false;
    for c in inp.chars() {
        if saw_backslash {
            let c = match c {
                '\\' => '\\',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                '\"' => '\"',
                _ => unreachable!("invalid escape should have been caught in parser"),
            };
            out.push(c);
            saw_backslash = false;
        } else if c == '\\' {
            saw_backslash = true;
        } else {
            out.push(c);
        }
    }
    out.into_boxed_str()
}

impl Compiler {
    pub fn get_and_check_function_callconv(
        &self,
        ast: &Ast,
        function: &ast::Function,
        params: &[(Box<str>, Type)],
        named_params: &[(Box<str>, Type, Option<ConstValueId>)],
    ) -> Result<CallConv, CompileError> {
        // TODO: replace with proper attribute resolval in the future
        let Some(attr) = function
            .attributes
            .iter()
            .find(|attr| &ast[attr.path.span()] == "callconv")
        else {
            return Ok(CallConv::default());
        };
        if attr.args.count != 1 {
            return Err(CompileError {
                err: Error::InvalidArgCount {
                    expected: 1,
                    varargs: false,
                    found: attr.args.count,
                },
                span: attr.span,
            });
        }
        let arg = &ast[attr.args][0];
        let &Expr::Ident { span, .. } = arg else {
            return Err(CompileError {
                err: Error::InvalidCallConv,
                span: ast[attr.args][0].span(ast),
            });
        };
        let callconv = match &ast[span] {
            "eye" => CallConv::Eye,
            "fn_trait" => CallConv::FnTrait,
            _ => {
                return Err(CompileError {
                    err: Error::InvalidCallConv,
                    span: arg.span(ast),
                });
            }
        };
        if callconv == CallConv::FnTrait {
            // TODO: proper errors here
            if params.len() != 2 || !named_params.is_empty() {
                panic!(
                    "fn_trait callconv violated: need 2 parameters but got {}",
                    params.len() + named_params.len()
                );
            }
            if !matches!(self.types.lookup(params[1].1), TypeFull::Tuple { .. }) {
                // TODO: this exception will not be allowed in the future without a T: Trait bound
                if !matches!(self.types.lookup(params[1].1), TypeFull::Generic(_)) {
                    panic!(
                        "fn_trait callconv violated: must have an arguments tuple but got {:?}",
                        self.types.lookup(params[1].1)
                    );
                }
            }
        }
        Ok(callconv)
    }
}
