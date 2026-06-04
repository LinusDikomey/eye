use std::num::NonZeroU64;

use error::Error;
use ir::eval::Val;
use parser::ast::{
    Ast, Expr, ExprId, FloatLiteral, IntLiteral, ModuleId, Primitive, ScopeId, UnresolvedType,
};

use crate::{
    Compiler, Def, Type,
    callconv::CallConv,
    compiler::{Dialects, Generics, Instance, Instances, ModuleSpan},
    hir::HIRBuilder,
    types::TypeFull,
    typing::TypeTable,
};

id::id!(ConstValueId);

#[derive(Debug, Clone)]
pub enum ConstValue {
    /// represents an undefined value, for example when a global is not initialized
    Undefined,
    Unit,
    Int(u64),
    Float(f64),
    Aggregate(Box<[ConstValue]>),
}
impl ConstValue {
    pub fn dump(&self) {
        match self {
            Self::Undefined => print!("undefined"),
            Self::Unit => print!("()"),
            Self::Int(n) => {
                print!("{n}");
            }
            Self::Float(n) => {
                print!("{n}");
            }
            Self::Aggregate(elems) => {
                print!("(");
                for (i, elem) in elems.iter().enumerate() {
                    if i != 0 {
                        print!(", ");
                    }
                    elem.dump();
                }
            }
        }
    }
}

pub fn def_expr(
    compiler: &Compiler,
    module: ModuleId,
    scope: ScopeId,
    ast: &Ast,
    expr: ExprId,
    name: &str,
    ty: &UnresolvedType,
) -> Def {
    let mismatched_type = |compiler: &Compiler, found| {
        let mut expected = String::new();
        ty.to_string(&mut expected, ast.src());
        compiler.errors.emit(
            module,
            Error::MismatchedType { expected, found }.at_span(ast[expr].span(ast)),
        );
    };

    // TODO: support untyped number constants again
    match &ast[expr] {
        &Expr::IntLiteral { span, .. } => {
            let lit = IntLiteral::parse(&ast[span]);
            let Ok(val) = lit.val.try_into() else {
                todo!("handle large constants")
            };
            let val = ConstValue::Int(val);

            let ty = if matches!(ty, UnresolvedType::Infer(_)) {
                Type::I32
            } else {
                let ty = compiler.resolve_type(ty, module, scope);
                if !matches!(compiler.types.lookup(ty), TypeFull::Instance(base, _) if base.is_int())
                {
                    if ty != Type::Invalid {
                        mismatched_type(compiler, "an integer".to_owned());
                    }
                    return Def::Invalid;
                }
                ty
            };
            Def::ConstValue(compiler.add_const_value(val, ty))
        }
        &Expr::FloatLiteral { span, .. } => {
            let lit = FloatLiteral::parse(&ast[span]);
            let val = ConstValue::Float(lit.val);

            let ty = if matches!(ty, UnresolvedType::Infer(_)) {
                Type::F32
            } else {
                let ty = compiler.resolve_type(ty, module, scope);
                if !matches!(compiler.types.lookup(ty), TypeFull::Instance(base, _) if base.is_float())
                {
                    if ty != Type::Invalid {
                        mismatched_type(compiler, "a float".to_owned());
                    }
                    return Def::Invalid;
                }
                ty
            };
            Def::ConstValue(compiler.add_const_value(val, ty))
        }
        &Expr::Ident { span, .. } => {
            let name = &ast[span];
            let def = compiler.resolve_in_scope(module, scope, name, ModuleSpan { module, span });
            match def.check_with_type(module, scope, compiler, ty) {
                Ok(def) => def,
                Err(found) => {
                    mismatched_type(compiler, found.to_owned());
                    Def::Invalid
                }
            }
        }
        Expr::ReturnUnit { .. } => {
            let ty = compiler.resolve_type(ty, module, scope);
            if ty != Type::Unit {
                mismatched_type(
                    compiler,
                    compiler
                        .types
                        .display(Type::Unit, &Generics::EMPTY)
                        .to_string(),
                );
                return Def::Invalid;
            }
            Def::ConstValue(compiler.add_const_value(ConstValue::Unit, ty))
        }
        &Expr::Return { val, .. } => def_expr(compiler, module, scope, ast, val, name, ty),
        &Expr::Function { id } => {
            if compiler
                .check_signature_with_type((module, id), ty, scope, module)
                .is_ok()
            {
                Def::Function(module, id)
            } else {
                Def::Invalid
            }
        }
        &Expr::Type { id } => {
            let Ok(matches) = compiler.annotation_matches_type(ty, Type::Type, module, scope)
            else {
                return Def::Invalid;
            };
            if !matches {
                mismatched_type(compiler, Primitive::Type.to_string());
                return Def::Invalid;
            }
            let symbols = &compiler.get_parsed_module(module).symbols;
            let base = *symbols.types[id.idx()].get_or_init(|| {
                let generic_count = ast[id].generic_count();
                compiler.add_type_def(module, id, name.into(), generic_count)
            });
            let generic_count = compiler.get_base_type_generic_count(base);
            if generic_count == 0 {
                Def::Type(compiler.types.intern(TypeFull::Instance(base, &[])))
            } else {
                Def::BaseType(base)
            }
        }
        &Expr::Trait { id } => {
            if !matches!(ty, UnresolvedType::Infer(_)) {
                mismatched_type(compiler, "a trait".to_owned());
                return Def::Invalid;
            }
            Def::Trait(module, id)
        }
        &Expr::Primitive { primitive, .. } => {
            let Ok(matches) = compiler.annotation_matches_type(ty, Type::Type, module, scope)
            else {
                return Def::Invalid;
            };
            if !matches {
                mismatched_type(compiler, Primitive::Type.to_string());
                return Def::Invalid;
            }
            Def::Type(primitive.into())
        }
        _ => {
            tracing::debug!(target: "eval", "Running evaluator for definition of {name}:");
            let value_name = compiler.module_path(module) + "." + name;
            match value_expr(compiler, module, scope, ast, expr, ty, &value_name) {
                Ok((val, ty)) => Def::ConstValue(compiler.add_const_value(val, ty)),
                Err(err) => {
                    compiler
                        .errors
                        .emit(module, Error::EvalFailed(err).at_span(ast[expr].span(ast)));
                    Def::Invalid
                }
            }
        }
    }
}

pub fn value_expr(
    compiler: &Compiler,
    module: ModuleId,
    scope: ScopeId,
    ast: &Ast,
    expr: ExprId,
    ty: &UnresolvedType,
    name: &str,
) -> Result<(ConstValue, Type), ir::eval::Error> {
    let mut types = TypeTable::new();
    let expected = types.from_annotation(ty, compiler, module, scope);
    let expected = types.add(expected);
    let hir = HIRBuilder::new(types);
    let name = compiler.module_path(module) + name;
    let hir = crate::check::check(
        compiler,
        ast,
        module,
        &Generics::EMPTY,
        scope,
        hir,
        [],
        expr,
        expected,
        &name,
        crate::compiler::LocalScopeParent::None,
        &mut (),
    );
    let ty = hir[expected];
    if let crate::hir::Node::Invalid = hir[hir.root_id()] {
        return Ok((ConstValue::Undefined, ty));
    }

    // TODO: share the environment and cache functions compiled for const eval
    let mut to_generate = Vec::new();
    let mut env = ir::Environment::new(ir::dialect::Primitive::create_infos());
    let dialects = Dialects {
        arith: env.get_dialect_module(),
        tuple: env.get_dialect_module(),
        mem: env.get_dialect_module(),
        cf: env.get_dialect_module(),
        main: env.create_module("main"),
    };
    let mut instances = Instances::new();

    let mut builder = ir::builder::Builder::new(&mut env);
    builder.create_and_begin_block([]);
    let Some(return_ty) = crate::irgen::types::get(
        compiler,
        builder.env,
        &mut builder.types,
        ty,
        Instance::EMPTY,
    ) else {
        return Ok((ConstValue::Undefined, ty));
    };
    let return_ty = builder.types.add(return_ty);
    let (ir, ir_types) = crate::irgen::lower_hir(
        compiler,
        &dialects,
        &mut instances,
        builder,
        &hir,
        &mut to_generate,
        &[],
        ir::Refs::EMPTY,
        return_ty,
        CallConv::Eye,
    );
    while let Some(f) = to_generate.pop() {
        let checked = compiler.get_hir(f.module, f.ast_function_id);

        if let crate::compiler::BodyOrTypes::Body(body) = &checked.body_or_types {
            let callconv = compiler.get_signature(f.module, f.ast_function_id).callconv;
            let return_type = env[f.ir_id].return_type().unwrap();
            let (builder, params) = ir::builder::Builder::begin_function(&mut env, f.ir_id);
            let (body, types) = crate::irgen::lower_hir(
                compiler,
                &dialects,
                &mut instances,
                builder,
                body,
                &mut to_generate,
                &f.generics,
                params,
                return_type,
                callconv,
            );
            env[f.ir_id].attach_body(body);
            env[f.ir_id].overwrite_types(types);
        }
    }
    let mut env = LazyEvalEnv { env: &env };
    ir::eval::eval(&ir, &ir_types, &[], &mut env).map(|val| (to_const_val(val), ty))
}

fn to_const_val(val: Val) -> ConstValue {
    match val {
        Val::Invalid => ConstValue::Undefined,
        Val::Unit => ConstValue::Unit,
        Val::Int(n) => ConstValue::Int(n),
        Val::F32(n) => ConstValue::Float(n as f64),
        Val::F64(n) => ConstValue::Float(n),
        Val::Ptr(_) => todo!("handle constants with compile-time pointers"),
        Val::Array(elems) | Val::Tuple(elems) => {
            ConstValue::Aggregate(elems.into_iter().map(to_const_val).collect())
        }
    }
}

struct LazyEvalEnv<'a> {
    env: &'a ir::Environment,
}
impl ir::eval::EvalEnvironment for LazyEvalEnv<'_> {
    fn env(&self) -> &ir::Environment {
        self.env
    }

    fn debug(&self) -> bool {
        #[cfg(debug_assertions)]
        {
            tracing::enabled!(target: "foo", tracing::Level::DEBUG)
        }
        #[cfg(not(debug_assertions))]
        {
            false
        }
    }

    fn call_extern(
        &mut self,
        id: ir::FunctionId,
        args: &[Val],
        mem: &mut ir::eval::Mem,
    ) -> Result<Val, Box<str>> {
        let func = &self.env[id];
        Ok(match &*func.name {
            "malloc" => {
                let &[Val::Int(size)] = args else {
                    return Err("invalid signature for malloc".into());
                };
                let ptr = mem
                    .malloc(ir::Layout {
                        size,
                        align: NonZeroU64::new(16).unwrap(),
                    })
                    .map_err(|_| "out of compile-time memory")?;
                Val::Ptr(ptr)
            }
            name => {
                return Err(
                    format!("Can't evaluate extern function {name} at compile-time").into(),
                );
            }
        })
    }
}
