use dmap::DHashMap;
use error::{Error, span::TSpan};

use crate::{
    Type,
    check::Hooks,
    compiler::{
        Def, LocalItem, LocalScope, LocalScopeParent, ModuleSpan, ResolvedTypeContent, builtins,
    },
    eval::ConstValueId,
    hir::{self, Comparison, LValue, Logic, Node, NodeIds, Pattern},
    types::{BaseType, TypeFull, Types},
    typing::{
        Bound, Bounds, LocalTypeId, LocalTypeIds, OrdinalType, TypeInfo, TypeInfoOrIdx, TypeTable,
    },
};
use parser::ast::{
    Ast, Expr, ExprId, ExprIds, FloatLiteral, FunctionId, IntLiteral, ModuleId, Operator,
    Primitive, UnOp,
};

use super::{Ctx, exhaust::Exhaustion, lval, pattern};

impl<'a, H: Hooks> Ctx<'a, H> {
    pub fn check(
        &mut self,
        expr: ExprId,
        scope: &mut LocalScope,
        expected: LocalTypeId,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
    ) -> Node {
        self.check_inner(expr, scope, expected, return_ty, noreturn, true)
    }

    pub fn check_statement(
        &mut self,
        expr: ExprId,
        scope: &mut LocalScope,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
    ) -> Node {
        let statement_ty = self.hir.types.add_unknown();
        self.check_inner(expr, scope, statement_ty, return_ty, noreturn, false)
    }

    pub fn check_inner(
        &mut self,
        expr: ExprId,
        scope: &mut LocalScope,
        expected: LocalTypeId,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
        used: bool,
    ) -> Node {
        self.hooks
            .on_check_expr(expr, &mut self.hir, scope, expected, return_ty, noreturn);
        let ast = self.ast;
        match &ast[expr] {
            Expr::Error(_) => {
                *noreturn = true;
                Node::Invalid
            }
            &Expr::Block {
                scope: static_scope,
                items,
                ..
            } => {
                let module = scope.module;
                let mut scope = LocalScope {
                    parent: LocalScopeParent::Some(&*scope),
                    variables: dmap::new(),
                    module,
                    static_scope: Some(static_scope),
                };
                let mut item_nodes = self.hir.add_invalid_nodes(items.count);
                for ((item, r), i) in items.into_iter().zip(item_nodes.iter()).zip(0..) {
                    let is_last = i as usize + 1 == items.len();
                    let node = if is_last && used {
                        // return last expr out of a block in expr position
                        self.check(item, &mut scope, expected, return_ty, noreturn)
                    } else {
                        self.check_statement(item, &mut scope, return_ty, noreturn)
                    };
                    self.hir.modify_node(r, node);
                    if *noreturn {
                        // just shorten the node list. We waste the remaining nodes but this is not the
                        // common case. Usually, there is no unreachable code in blocks.
                        // TODO: unreachable code warning for the rest of the lines
                        item_nodes = NodeIds {
                            index: item_nodes.index,
                            count: i + 1,
                        };
                        break;
                    }
                }
                if !*noreturn && (!used || items.is_empty()) {
                    self.specify(expected, TypeInfo::UNIT, |ast| ast[expr].span(ast));
                }
                self.hooks.on_exit_scope(&mut scope);
                Node::Block(item_nodes)
            }
            &Expr::Nested { inner, .. } => self.check(inner, scope, expected, return_ty, noreturn),
            &Expr::IntLiteral { span, .. } => {
                let lit = IntLiteral::parse(&ast.src()[span.range()]);
                let info = lit.ty.map_or(TypeInfo::Integer, |int| {
                    TypeInfo::Instance(Primitive::from(int).into(), LocalTypeIds::EMPTY)
                });
                self.specify(expected, info, |_| span);
                Node::IntLiteral {
                    val: lit.val,
                    ty: expected,
                }
            }
            &Expr::FloatLiteral { span, .. } => {
                let lit = FloatLiteral::parse(&ast.src()[span.range()]);
                let info = lit.ty.map_or(TypeInfo::Float, |float| {
                    TypeInfo::Instance(Primitive::from(float).into(), LocalTypeIds::EMPTY)
                });
                self.specify(expected, info, |_| span);
                Node::FloatLiteral {
                    val: lit.val,
                    ty: expected,
                }
            }
            &Expr::StringLiteral { span, .. } => {
                let str = super::get_string_literal(self.ast.src(), span);
                let str_ty = builtins::get_str(self.compiler);
                self.specify(expected, TypeInfo::Known(str_ty), |_| span);
                Node::StringLiteral(str)
            }
            &Expr::Array { span, elements, .. } => {
                let elem_ty_and_count = self.hir.types.specify_base(
                    expected,
                    BaseType::Array,
                    2,
                    self.generics,
                    self.compiler,
                    || ModuleSpan {
                        module: self.module,
                        span,
                    },
                    |types| {
                        types.add_multiple([
                            TypeInfo::Unknown(Bounds::EMPTY),
                            TypeInfo::UnknownConst,
                        ])
                    },
                );
                let elem_ty = elem_ty_and_count.nth(0).unwrap();
                let count = elem_ty_and_count.nth(1).unwrap();
                let const_count = self
                    .compiler
                    .types
                    .intern(TypeFull::Const(elements.count as u64));
                self.specify(count, TypeInfo::Known(const_count), |_| span);
                let nodes = self.hir.add_invalid_nodes(elements.count);
                for (node, elem) in nodes.iter().zip(elements) {
                    let elem_node = self.check(elem, scope, elem_ty, return_ty, noreturn);
                    // TODO: can we return early here in case of noreturn?
                    self.hir.modify_node(node, elem_node);
                }
                Node::ArrayLiteral {
                    elems: nodes,
                    array_ty: expected,
                }
            }
            Expr::Tuple { span, elements, .. } => {
                let elem_types =
                    self.specify_base(expected, BaseType::Tuple, elements.count, |_| *span);
                let elems = self.hir.add_invalid_nodes(elements.count);
                for ((value, ty), node_id) in elements
                    .into_iter()
                    .zip(elem_types.iter())
                    .zip(elems.iter())
                {
                    let node = self.check(value, scope, ty, return_ty, noreturn);
                    // TODO: can we return early here in case of noreturn?
                    self.hir.modify_node(node_id, node);
                }
                Node::TupleLiteral { elems, elem_types }
            }
            &Expr::EnumLiteral {
                span, ident, args, ..
            } => self.check_enum_literal(scope, expected, return_ty, span, ident, args, noreturn),
            &Expr::Function { id } => {
                let function_span = self.ast[expr].span(self.ast);
                let (node, info) = self.closure(id, scope);
                self.specify(expected, info, |_| function_span);
                node
            }
            &Expr::Primitive { primitive, .. } => {
                self.specify(expected, TypeInfo::BaseTypeItem(primitive.into()), |ast| {
                    ast[expr].span(ast)
                });
                Node::Invalid
            }
            &Expr::Type { id } => {
                let generic_count = self.ast[id].generic_count();
                let base = self.compiler.add_type_def(
                    self.module,
                    id,
                    "<anonymous type>".into(),
                    generic_count,
                );
                self.compiler.get_parsed_module(self.module).symbols.types[id.idx()]
                    .set(base)
                    .expect("Type defined multiple times");
                let span = self.ast[expr].span(ast);
                self.specify(expected, TypeInfo::BaseTypeItem(base), |_| span);
                Node::Invalid
            }
            &Expr::Trait { id } => {
                self.specify(
                    expected,
                    TypeInfo::TraitItem {
                        module: self.module,
                        id,
                    },
                    |ast| ast[expr].span(ast),
                );
                Node::Invalid
            }

            &Expr::Ident { span, .. } => self.check_ident(scope, expected, span),
            Expr::Declare { .. } => todo!("check variable declarations without values"),
            Expr::DeclareWithVal {
                pat,
                annotated_ty,
                val,
                ..
            } => {
                self.specify(expected, TypeInfo::UNIT, |ast| ast[expr].span(ast));
                let mut exhaustion = Exhaustion::None;
                let ty = self.hir.types.from_annotation(
                    annotated_ty,
                    self.compiler,
                    self.module,
                    scope.get_innermost_static_scope(),
                );
                let ty = self.hir.types.add(ty);
                let val = self.check(*val, scope, ty, return_ty, noreturn);
                let pattern = pattern::check(self, scope, &mut exhaustion, *pat, ty);
                if !exhaustion.is_trivially_exhausted() {
                    self.deferred_exhaustions.push((exhaustion, ty, *pat));
                }
                Node::DeclareWithVal {
                    pattern: self.hir.add_pattern(pattern),
                    val: self.hir.add(val),
                }
            }
            Expr::Hole { .. } => {
                self.invalidate(expected);
                self.emit(Error::HoleLHSOnly.at_span(self.span(expr)));
                Node::Invalid
            }

            &Expr::UnOp { op, inner, .. } => {
                match op {
                    UnOp::Neg => {
                        // TODO(trait): this should constrain the value with a trait
                        let value = self.check(inner, scope, expected, return_ty, noreturn);
                        Node::Negate(self.hir.add(value), expected)
                    }
                    UnOp::Not => {
                        self.specify(expected, self.primitives().bool_info(), |ast| {
                            ast[expr].span(ast)
                        });
                        let value = self.check(inner, scope, expected, return_ty, noreturn);
                        Node::Not(self.hir.add(value))
                    }
                    UnOp::Ref => {
                        let pointee = self
                            .specify_base(expected, BaseType::Pointer, 1, |ast| ast[expr].span(ast))
                            .nth(0)
                            .unwrap();
                        let value = self.check(inner, scope, pointee, return_ty, noreturn);
                        if let Some(lval) = LValue::try_from_node(&value, &mut self.hir) {
                            Node::AddressOf {
                                value: self.hir.add_lvalue(lval),
                                value_ty: pointee,
                            }
                        } else {
                            // promote the value to a variable
                            let promoted = self.hir.add_var(pointee);
                            Node::Promote {
                                value: self.hir.add(value),
                                variable: promoted,
                            }
                        }
                    }
                    UnOp::Deref => {
                        let ptr_ty = self
                            .hir
                            .types
                            .add(TypeInfo::Instance(BaseType::Pointer, expected.into()));
                        let value = self.check(inner, scope, ptr_ty, return_ty, noreturn);
                        Node::Deref {
                            value: self.hir.add(value),
                            deref_ty: expected,
                        }
                    }
                }
            }
            &Expr::BinOp { op, l, r, .. } => {
                match op {
                    Operator::Add
                    | Operator::Sub
                    | Operator::Mul
                    | Operator::Div
                    | Operator::Mod => {
                        // TODO: this will be have to bound/handled by traits
                        let l = self.check(l, scope, expected, return_ty, noreturn);
                        let r = self.check(r, scope, expected, return_ty, noreturn);
                        let op = match op {
                            Operator::Add => hir::Arithmetic::Add,
                            Operator::Sub => hir::Arithmetic::Sub,
                            Operator::Mul => hir::Arithmetic::Mul,
                            Operator::Div => hir::Arithmetic::Div,
                            Operator::Mod => hir::Arithmetic::Mod,
                            _ => unreachable!(),
                        };
                        Node::Arithmetic(self.hir.add(l), self.hir.add(r), op, expected)
                    }
                    Operator::Assignment(assign_ty) => {
                        // TODO: arithmetic assignment operations will be have to bound/handled by traits
                        self.specify(expected, TypeInfo::UNIT, |ast| ast[expr].span(ast));
                        let (lval, ty) = lval::check(self, l, scope, return_ty, noreturn);
                        let val = self.check(r, scope, ty, return_ty, noreturn);
                        Node::Assign(self.hir.add_lvalue(lval), self.hir.add(val), assign_ty, ty)
                    }
                    Operator::Or | Operator::And => {
                        self.specify(expected, self.primitives().bool_info(), |ast| {
                            ast[expr].span(ast)
                        });
                        let l = self.check(l, scope, expected, return_ty, noreturn);
                        let r = self.check(r, scope, expected, return_ty, noreturn);
                        let l = self.hir.add(l);
                        let r = self.hir.add(r);
                        let logic = if op == Operator::Or {
                            Logic::Or
                        } else {
                            Logic::And
                        };
                        Node::Logic { l, r, logic }
                    }
                    Operator::Equals | Operator::NotEquals => {
                        self.specify(expected, self.primitives().bool_info(), |ast| {
                            ast[expr].span(ast)
                        });
                        let eq_trait = builtins::get_eq_trait(self.compiler);
                        let span = self.span(expr);
                        let bounds = self.hir.types.add_bounds([Bound {
                            trait_id: eq_trait,
                            generics: LocalTypeIds::EMPTY,
                            span,
                        }]);
                        let compared = self.hir.types.add(TypeInfo::Unknown(bounds));
                        let l = self.check(l, scope, compared, return_ty, noreturn);
                        let r = self.check(r, scope, compared, return_ty, noreturn);
                        let args = self.hir.add_nodes([l, r]);
                        let node = Node::TraitCall {
                            trait_id: eq_trait,
                            trait_generics: LocalTypeIds::EMPTY,
                            method_index: 0,
                            self_ty: compared,
                            args,
                            return_ty: expected,
                            noreturn: false,
                        };
                        if op == Operator::Equals {
                            node
                        } else {
                            Node::Not(self.hir.add(node))
                        }
                    }
                    Operator::LT | Operator::GT | Operator::LE | Operator::GE => {
                        self.specify(expected, self.primitives().bool_info(), |ast| {
                            ast[expr].span(ast)
                        });
                        // FIXME: this will have to be bound by type like Ord
                        let compared = self.hir.types.add_unknown();
                        let l = self.check(l, scope, compared, return_ty, noreturn);
                        let r = self.check(r, scope, compared, return_ty, noreturn);
                        let l = self.hir.add(l);
                        let r = self.hir.add(r);
                        let cmp = match op {
                            Operator::LT => Comparison::LT,
                            Operator::GT => Comparison::GT,
                            Operator::LE => Comparison::LE,
                            Operator::GE => Comparison::GE,
                            _ => unreachable!(),
                        };
                        Node::Comparison {
                            l,
                            r,
                            cmp,
                            compared,
                        }
                    }
                    Operator::Range | Operator::RangeExclusive => {
                        todo!("range types not implemented yet")
                    }
                }
            }
            Expr::As {
                value, ty: new_ty, ..
            } => {
                let from_ty = self.hir.types.add_unknown();
                let val = self.check(*value, scope, from_ty, return_ty, noreturn);
                let val = self.hir.add(val);
                let new_type_info = self.hir.types.from_annotation(
                    new_ty,
                    self.compiler,
                    self.module,
                    scope.get_innermost_static_scope(),
                );
                self.specify(expected, new_type_info, |_| new_ty.span());
                let cast_id = self.hir.add_cast(hir::Cast {
                    val,
                    cast_ty: hir::CastType::Invalid, // will be filled in in deferred check
                });
                self.deferred_casts.push((from_ty, expected, expr, cast_id));
                Node::Cast(cast_id)
            }
            &Expr::Root { .. } => {
                let root_module = self.compiler.get_root_module(self.module);
                self.specify(expected, TypeInfo::ModuleItem(root_module), |ast| {
                    ast[expr].span(ast)
                });
                Node::Invalid
            }
            &Expr::MemberAccess {
                left,
                name: name_span,
                ..
            } => self
                .check_member_access(left, name_span, expr, scope, expected, return_ty, noreturn),
            &Expr::Index { expr, idx, .. } => {
                let elem_and_count = self.hir.types.add_multiple_info_or_idx([
                    TypeInfoOrIdx::Idx(expected),
                    TypeInfo::UnknownConst.into(),
                ]);
                let array_ty = self
                    .hir
                    .types
                    .add(TypeInfo::Instance(BaseType::Array, elem_and_count));
                let array = self.check(expr, scope, array_ty, return_ty, noreturn);
                let index_ty = self.hir.types.add(TypeInfo::Integer);
                let index = self.check(idx, scope, index_ty, return_ty, noreturn);
                Node::ArrayIndex {
                    array: self.hir.add(array),
                    index: self.hir.add(index),
                    elem_ty: expected,
                }
            }
            &Expr::TupleIdx { left, idx, .. } => {
                let tuple_ty = self.hir.types.add_unknown(); // add Size::AtLeast tuple here maybe
                let tuple_value = self.check(left, scope, tuple_ty, return_ty, noreturn);
                match self.hir.types[tuple_ty] {
                    TypeInfo::Instance(BaseType::Tuple, elem_types) => {
                        return if let Some(elem_ty) = elem_types.nth(idx) {
                            self.unify(expected, elem_ty, |ast| ast[expr].span(ast));
                            Node::Element {
                                tuple_value: self.hir.add(tuple_value),
                                index: idx,
                                elem_types,
                            }
                        } else {
                            self.emit(Error::TupleIndexOutOfRange.at_span(self.span(expr)));
                            Node::Invalid
                        };
                    }
                    TypeInfo::Instance(BaseType::Invalid, _) => return Node::Invalid,
                    TypeInfo::Known(ty) => {
                        if let TypeFull::Instance(BaseType::Tuple, elem_types) =
                            self.compiler.types.lookup(ty)
                        {
                            return if let Some(&elem_ty) = elem_types.get(idx as usize) {
                                self.specify(expected, TypeInfo::Known(elem_ty), |ast| {
                                    ast[expr].span(ast)
                                });
                                let elem_types = self
                                    .hir
                                    .types
                                    .add_multiple(elem_types.iter().copied().map(TypeInfo::Known));
                                Node::Element {
                                    tuple_value: self.hir.add(tuple_value),
                                    index: idx,
                                    elem_types,
                                }
                            } else {
                                self.emit(Error::TupleIndexOutOfRange.at_span(self.span(expr)));
                                Node::Invalid
                            };
                        };
                    }
                    _ => {}
                };

                // FIXME: emit invalid type error on a known but wrong type
                // TODO: could add TupleCountMode and stuff again to unify with tuple with
                // Size::AtLeast. Not doing that for now since it is very rare.
                self.emit(
                    Error::TypeMustBeKnownHere { needed_bound: None }.at_span(self.span(left)),
                );
                self.invalidate(expected);
                Node::Invalid
            }

            Expr::ReturnUnit { .. } => {
                self.specify(return_ty, TypeInfo::UNIT, |ast| ast[expr].span(ast));
                Node::Return(self.hir.add(Node::Unit))
            }
            &Expr::Return { val, .. } => {
                let val = self.check(val, scope, return_ty, return_ty, noreturn);
                *noreturn = true;
                Node::Return(self.hir.add(val))
            }

            // FIXME: some code duplication going on with the 4 different If variants
            &Expr::If { cond, then, .. } => {
                self.specify(expected, TypeInfo::UNIT, |ast| ast[expr].span(ast));
                let bool_ty = self.hir.types.add(self.primitives().bool_info());
                let cond = self.check(cond, scope, bool_ty, return_ty, noreturn);
                let then = self.check_statement(then, scope, return_ty, &mut false);
                Node::IfElse {
                    cond: self.hir.add(cond),
                    then: self.hir.add(then),
                    else_: self.hir.add(Node::Unit),
                    resulting_ty: expected,
                }
            }
            &Expr::IfElse {
                cond, then, else_, ..
            } => {
                let bool_ty = self.hir.types.add(self.primitives().bool_info());
                let cond = self.check(cond, scope, bool_ty, return_ty, noreturn);
                let mut then_noreturn = false;
                let mut else_noreturn = false;
                let (then, else_) = if used {
                    (
                        self.check(then, scope, expected, return_ty, &mut then_noreturn),
                        self.check(else_, scope, expected, return_ty, &mut else_noreturn),
                    )
                } else {
                    (
                        self.check_statement(then, scope, return_ty, &mut then_noreturn),
                        self.check_statement(else_, scope, return_ty, &mut else_noreturn),
                    )
                };
                if then_noreturn && else_noreturn {
                    *noreturn = true;
                }
                Node::IfElse {
                    cond: self.hir.add(cond),
                    then: self.hir.add(then),
                    else_: self.hir.add(else_),
                    resulting_ty: expected,
                }
            }
            &Expr::IfPat {
                pat, value, then, ..
            } => {
                self.specify(expected, TypeInfo::UNIT, |ast| ast[expr].span(ast));
                let pattern_ty = self.hir.types.add_unknown();
                let value = self.check(value, scope, pattern_ty, return_ty, noreturn);
                let mut body_scope = LocalScope {
                    module: scope.module,
                    parent: LocalScopeParent::Some(&*scope),
                    static_scope: None,
                    variables: dmap::new(),
                };
                let mut exhaustion = Exhaustion::None;
                let pat = pattern::check(self, &mut body_scope, &mut exhaustion, pat, pattern_ty);
                if exhaustion.is_trivially_exhausted() {
                    // TODO: Error::ConditionIsAlwaysTrue
                    // maybe even defer this exhaustion to check for this warning in non-trivial case
                }
                let then = if used {
                    self.check(then, &mut body_scope, expected, return_ty, &mut false)
                } else {
                    self.check_statement(then, &mut body_scope, return_ty, &mut false)
                };
                Node::IfPatElse {
                    pat: self.hir.add_pattern(pat),
                    val: self.hir.add(value),
                    then: self.hir.add(then),
                    else_: self.hir.add(Node::Unit),
                    resulting_ty: expected,
                }
            }
            &Expr::IfPatElse {
                pat,
                value,
                then,
                else_,
                ..
            } => {
                let pattern_ty = self.hir.types.add_unknown();
                let value = self.check(value, scope, pattern_ty, return_ty, noreturn);
                let mut body_scope = LocalScope {
                    module: scope.module,
                    parent: LocalScopeParent::Some(&*scope),
                    static_scope: None,
                    variables: dmap::new(),
                };
                let mut exhaustion = Exhaustion::None;
                let pat = pattern::check(self, &mut body_scope, &mut exhaustion, pat, pattern_ty);
                if exhaustion.is_trivially_exhausted() {
                    // TODO: Error::ConditionIsAlwaysTrue
                    // maybe even defer this exhaustion to check for this warning in non-trivial case
                }
                let pat = self.hir.add_pattern(pat);
                let val = self.hir.add(value);
                let mut then_noreturn = false;
                let mut else_noreturn = false;
                let (then, else_) = if used {
                    (
                        self.check(
                            then,
                            &mut body_scope,
                            expected,
                            return_ty,
                            &mut then_noreturn,
                        ),
                        self.check(
                            else_,
                            &mut body_scope,
                            expected,
                            return_ty,
                            &mut else_noreturn,
                        ),
                    )
                } else {
                    (
                        self.check_statement(then, &mut body_scope, return_ty, &mut then_noreturn),
                        self.check_statement(then, &mut body_scope, return_ty, &mut else_noreturn),
                    )
                };
                if then_noreturn && else_noreturn {
                    *noreturn = true;
                }
                Node::IfPatElse {
                    pat,
                    val,
                    then: self.hir.add(then),
                    else_: self.hir.add(else_),
                    resulting_ty: expected,
                }
            }
            &Expr::Match { val, branches, .. } => {
                let branch_count = branches.pair_count();
                let matched_ty = self.hir.types.add_unknown();
                let value = self.check(val, scope, matched_ty, return_ty, noreturn);
                let value = self.hir.add(value);
                let mut exhaustion = Exhaustion::None;
                let patterns = self.hir.add_invalid_patterns(branch_count);
                let branch_nodes = self.hir.add_invalid_nodes(branch_count);
                let mut all_branches_noreturn = true;
                let mut branch_scope = LocalScope {
                    module: scope.module,
                    parent: LocalScopeParent::Some(&*scope),
                    variables: DHashMap::default(),
                    static_scope: None,
                };
                for ((pat, branch), i) in branches.into_iter().zip(0..) {
                    branch_scope.variables.clear();
                    let pat =
                        pattern::check(self, &mut branch_scope, &mut exhaustion, pat, matched_ty);
                    self.hir
                        .modify_pattern(hir::PatternId(patterns.index + i), pat);
                    let mut branch_noreturn = false;
                    let branch = if used {
                        self.check(
                            branch,
                            &mut branch_scope,
                            expected,
                            return_ty,
                            &mut branch_noreturn,
                        )
                    } else {
                        self.check_statement(
                            branch,
                            &mut branch_scope,
                            return_ty,
                            &mut branch_noreturn,
                        )
                    };
                    if !branch_noreturn {
                        all_branches_noreturn = false;
                    }
                    self.hir
                        .modify_node(hir::NodeId(branch_nodes.index + i), branch);
                }
                if all_branches_noreturn {
                    *noreturn = true;
                }
                Node::Match {
                    value,
                    branch_index: branch_nodes.index,
                    pattern_index: patterns.index,
                    branch_count,
                    resulting_ty: expected,
                }
            }
            &Expr::While { cond, body, .. } => {
                let bool_ty = self.hir.types.add(self.primitives().bool_info());
                let cond = self.check(cond, scope, bool_ty, return_ty, noreturn);
                let body_ty = self.hir.types.add_unknown();
                self.control_flow_stack.push(());
                let body = self.check(body, scope, body_ty, return_ty, &mut false);
                self.control_flow_stack.pop();
                Node::While {
                    cond: self.hir.add(cond),
                    body: self.hir.add(body),
                }
            }
            &Expr::WhilePat { pat, val, body, .. } => {
                self.control_flow_stack.push(());
                let value_ty = self.hir.types.add_unknown();
                let val = self.check(val, scope, value_ty, return_ty, noreturn);
                let mut body_scope = LocalScope {
                    module: scope.module,
                    parent: LocalScopeParent::Some(&*scope),
                    variables: dmap::new(),
                    static_scope: None,
                };
                let mut exhaustion = Exhaustion::None;
                let pat = pattern::check(self, &mut body_scope, &mut exhaustion, pat, value_ty);
                let pat = self.hir.add_pattern(pat);
                let val = self.hir.add(val);
                let body_ty = self.hir.types.add_unknown();
                self.control_flow_stack.push(());
                let body = self.check(body, &mut body_scope, body_ty, return_ty, &mut false);
                self.control_flow_stack.pop();
                Node::WhilePat {
                    pat,
                    val,
                    body: self.hir.add(body),
                }
            }
            &Expr::For {
                pat, iter, body, ..
            } => {
                let iterator_trait = builtins::get_iterator(self.compiler);
                let option_ty = builtins::get_option(self.compiler);
                let some_variant_types = self.hir.types.add_multiple([
                    TypeInfo::from(Primitive::U8),
                    TypeInfo::Unknown(Bounds::EMPTY),
                ]);
                let item_ty = some_variant_types.nth(1).unwrap();
                let bounds = self.hir.types.add_bounds([Bound {
                    trait_id: iterator_trait,
                    generics: item_ty.into(),
                    span: self.ast[iter].span(self.ast),
                }]);
                let iter_ty = self.hir.types.add(TypeInfo::Unknown(bounds));
                let iter = self.check(iter, scope, iter_ty, return_ty, noreturn);
                let iter_var_id = self.hir.add_var(iter_ty);
                let decl_iter_var = Node::DeclareWithVal {
                    pattern: self.hir.add_pattern(Pattern::Variable(iter_var_id)),
                    val: self.hir.add(iter),
                };

                let mut body_scope = LocalScope {
                    module: scope.module,
                    parent: LocalScopeParent::Some(&*scope),
                    variables: dmap::new(),
                    static_scope: None,
                };
                let option_item_ty = self
                    .hir
                    .types
                    .add(TypeInfo::Instance(option_ty, item_ty.into()));
                let iter_var_ref = Node::AddressOf {
                    value: self.hir.add_lvalue(LValue::Variable(iter_var_id)),
                    value_ty: iter_ty,
                };
                let next_call = Node::TraitCall {
                    trait_id: iterator_trait,
                    trait_generics: item_ty.into(),
                    method_index: 0, // TODO: don't hardcode index of next method
                    self_ty: iter_ty,
                    args: self.hir.add(iter_var_ref).into(),
                    return_ty: option_item_ty,
                    noreturn: false,
                };
                let mut exhaustion = Exhaustion::None;
                let pat_item_node =
                    pattern::check(self, &mut body_scope, &mut exhaustion, pat, item_ty);
                if exhaustion.is_trivially_exhausted() {
                    self.deferred_exhaustions.push((exhaustion, item_ty, pat));
                }
                // TODO: don't hardcode 0 for Some
                let pat_node = Pattern::EnumVariant {
                    ordinal: OrdinalType::Known(0),
                    types: some_variant_types.idx,
                    args: self.hir.add_pattern(pat_item_node).into(),
                };
                let body_ty = self.hir.types.add_unknown();
                let mut body_noreturn = false;
                self.control_flow_stack.push(());
                let body = self.check(
                    body,
                    &mut body_scope,
                    body_ty,
                    return_ty,
                    &mut body_noreturn,
                );
                self.control_flow_stack.pop();
                let loop_node = Node::WhilePat {
                    pat: self.hir.add_pattern(pat_node),
                    val: self.hir.add(next_call),
                    body: self.hir.add(body),
                };
                Node::Block(self.hir.add_nodes([decl_iter_var, loop_node]))
            }
            &Expr::FunctionCall(call) => {
                self.check_call(&ast[call], expr, scope, expected, return_ty, noreturn)
            }
            Expr::Asm { .. } => todo!("implement inline assembly properly"),
            Expr::Break { .. } => {
                if self.control_flow_stack.is_empty() {
                    self.emit(Error::BreakOutsideLoop.at_span(self.span(expr)));
                    return Node::Invalid;
                }
                Node::Break(0)
            }
            Expr::Continue { .. } => {
                if self.control_flow_stack.is_empty() {
                    self.emit(Error::ContinueOutsideLoop.at_span(self.span(expr)));
                    return Node::Invalid;
                }
                Node::Continue(0)
            }
        }
    }

    fn check_ident(
        &mut self,
        scope: &mut LocalScope<'_>,
        expected: LocalTypeId,
        span: TSpan,
    ) -> Node {
        let name = &self.ast[span];
        match scope.resolve(name, span, self.compiler, &mut self.hir.vars) {
            LocalItem::Var(var) => {
                let ty = self.hir.get_var(var).ty();
                self.unify(expected, ty, |_| span);
                Node::Variable(var)
            }
            LocalItem::Def(def) => self.def_to_node(def, expected, span),
            LocalItem::Invalid => {
                self.specify(expected, TypeInfo::INVALID, |_| span);
                self.invalidate(expected);
                Node::Invalid
            }
        }
    }

    fn def_to_node(&mut self, def: Def, expected: LocalTypeId, span: TSpan) -> Node {
        match def {
            Def::Invalid => {
                self.specify(expected, TypeInfo::INVALID, |_| span);
                Node::Invalid
            }
            Def::Function(module, id) => self.function_item(module, id, expected, span),
            Def::BaseType(id) => {
                self.specify(expected, TypeInfo::BaseTypeItem(id), |_| span);
                Node::Invalid
            }
            Def::Type(ty) => {
                let ty = self.hir.types.add(TypeInfo::Known(ty));
                self.specify(expected, TypeInfo::TypeItem(ty), |_| span);
                Node::Invalid
            }
            Def::Trait(module, id) => {
                self.specify(expected, TypeInfo::TraitItem { module, id }, |_| span);
                Node::Invalid
            }
            Def::ConstValue(const_val) => self.const_value_to_node(expected, const_val, span),
            Def::Module(id) => {
                self.specify(expected, TypeInfo::ModuleItem(id), |_| span);
                Node::Invalid
            }
            Def::Global(module, id) => {
                let (_, ty) = self.compiler.get_checked_global(module, id);
                // PERF: cloning type
                let ty = *ty;
                self.specify_resolved(expected, ty, LocalTypeIds::EMPTY, |_| span);
                Node::Global(module, id, expected)
            }
        }
    }

    fn const_value_to_node(
        &mut self,
        expected: LocalTypeId,
        const_val: ConstValueId,
        span: TSpan,
    ) -> Node {
        let (_, ty) = &self.compiler.const_values[const_val.idx()];
        // PERF: clone of Type
        let ty = *ty;
        self.specify_resolved(expected, ty, LocalTypeIds::EMPTY, |_| span);
        Node::Const {
            id: const_val,
            ty: expected,
        }
    }

    fn check_enum_literal(
        &mut self,
        scope: &mut LocalScope,
        expected: LocalTypeId,
        return_ty: LocalTypeId,
        span: TSpan,
        ident: TSpan,
        args: ExprIds,
        noreturn: &mut bool,
    ) -> Node {
        let name = &self.ast[ident];
        let res = self.hir.types.specify_enum_literal(
            expected,
            name,
            args.count,
            self.compiler,
            self.generics,
        );
        match res {
            Ok((variant_index, arg_type_ids)) => {
                debug_assert_eq!(arg_type_ids.count, args.count + 1);
                let arg_nodes = self.hir.add_invalid_nodes(arg_type_ids.count);
                let mut arg_node_iter = arg_nodes.iter();
                let mut arg_type_iter = arg_type_ids.iter();
                let ordinal_type = arg_type_iter.next().unwrap();
                let ordinal_node = match variant_index {
                    OrdinalType::Known(ordinal) => Node::IntLiteral {
                        val: ordinal as u128,
                        ty: ordinal_type,
                    },
                    OrdinalType::Inferred(variant) => Node::InferredEnumOrdinal(variant),
                };
                self.hir
                    .modify_node(arg_node_iter.next().unwrap(), ordinal_node);
                for ((arg, ty), r) in args.into_iter().zip(arg_type_iter).zip(arg_node_iter) {
                    let arg_node = self.check(arg, scope, ty, return_ty, noreturn);
                    self.hir.modify_node(r, arg_node);
                }
                Node::EnumLiteral {
                    elems: arg_nodes,
                    elem_types: arg_type_ids,
                    enum_ty: expected,
                }
            }
            Err(err) => {
                self.invalidate(expected);
                if let Some(err) = err {
                    self.emit(err.at_span(span));
                }
                // the expected type was invalid, still check the arguments against an unknown type
                for arg in args.into_iter() {
                    let ty = self.hir.types.add_unknown();
                    self.check(arg, scope, ty, return_ty, noreturn);
                }
                Node::Invalid
            }
        }
    }

    fn function_item(
        &mut self,
        function_module: ModuleId,
        function: FunctionId,
        expected: LocalTypeId,
        span: TSpan,
    ) -> Node {
        let signature = self.compiler.get_signature(function_module, function);
        let generics =
            signature
                .generics
                .instantiate(&mut self.hir.types, &self.compiler.types, span);
        // a function item immediately becomes a fn() type only if it was expected to become one
        // already from the type, meaning passing to functions or assigning to an annotated variable
        // work without requiring subtyping.
        if let Some(mut args_and_return) =
            self.hir.types[expected].as_base(self.compiler, BaseType::Function)
            // TODO: emit more specific error when one of the following conditions fail
            && signature.named_params.is_empty()
            && !signature.varargs
            && args_and_return.len() == signature.params.len() + 1
        {
            let return_ty = args_and_return.next().unwrap();
            let signature_return_ty = self.from_type_instance(signature.return_type, generics);
            self.specify_or_unify(signature_return_ty, return_ty, |_| span);
            let args = args_and_return;
            for (&(_, signature_ty), ty) in signature.params.iter().zip(args) {
                let signature_ty = self.from_type_instance(signature_ty, generics);
                self.specify_or_unify(signature_ty, ty, |_| span);
            }
        } else {
            self.specify(
                expected,
                TypeInfo::FunctionItem {
                    module: function_module,
                    function,
                    generics,
                },
                |_| span,
            );
        }
        Node::FunctionItem {
            function: (function_module, function),
            generics,
            ty: expected,
        }
    }

    fn check_member_access(
        &mut self,
        left: ExprId,
        name_span: TSpan,
        expr: ExprId,
        scope: &mut LocalScope,
        expected: LocalTypeId,
        return_ty: LocalTypeId,
        noreturn: &mut bool,
    ) -> Node {
        let left_ty = self.hir.types.add_unknown();
        let left_node = self.check(left, scope, left_ty, return_ty, noreturn);
        let name = &self.ast.src()[name_span.range()];
        match self.hir.types[left_ty] {
            TypeInfo::BaseTypeItem(base) => {
                let generic_count = self.compiler.types.get_base(base).generic_count;
                let generics = self.hir.types.add_multiple_unknown(generic_count.into());
                let type_var = self.hir.types.add(TypeInfo::Instance(base, generics));
                return self.check_type_item_member_access(
                    type_var,
                    name,
                    name_span,
                    expected,
                    left,
                    |ast| ast[expr].span(ast),
                );
            }
            TypeInfo::TypeItem(var) => {
                return self.check_type_item_member_access(
                    var,
                    name,
                    name_span,
                    expected,
                    left,
                    |ast| ast[expr].span(ast),
                );
            }
            TypeInfo::TraitItem { module, id } => {
                let Some(checked) = self.compiler.get_checked_trait(module, id) else {
                    // invalid definition, error was already emitted
                    return Node::Invalid;
                };
                let Some(&method_index) = checked.functions_by_name.get(name) else {
                    self.emit(Error::NonexistantMember(None).at_span(name_span));
                    self.invalidate(expected);
                    return Node::Invalid;
                };
                self.specify(
                    expected,
                    TypeInfo::TraitMethodItem {
                        module,
                        trait_id: id,
                        method_index,
                    },
                    |ast| ast[expr].span(ast),
                );
                return Node::Invalid;
            }
            TypeInfo::ModuleItem(id) => {
                let def = self.compiler.resolve_in_module(
                    id,
                    name,
                    ModuleSpan {
                        module: self.module,
                        span: name_span,
                    },
                );
                return self.def_to_node(def, expected, name_span);
            }
            _ => {}
        }
        // now check the type while allowing auto deref for field access, methods
        let mut current_ty = self.hir.types[left_ty];
        let mut pointer_count = 0;
        loop {
            match current_ty {
                TypeInfo::Instance(BaseType::Pointer, pointee) => {
                    pointer_count += 1;
                    current_ty = self.hir.types[pointee.nth(0).unwrap()];
                }
                TypeInfo::Instance(BaseType::Invalid, _) => {
                    self.invalidate(expected);
                    return Node::Invalid;
                }
                TypeInfo::Known(ty) => match self.compiler.types.lookup(ty) {
                    TypeFull::Instance(BaseType::Pointer, &[pointee]) => {
                        pointer_count += 1;
                        current_ty = TypeInfo::Known(pointee);
                    }
                    TypeFull::Instance(base, generics) => {
                        let generics = self
                            .hir
                            .types
                            .add_multiple(generics.iter().map(|&ty| TypeInfo::Known(ty)));
                        return self.instance_member(
                            name_span,
                            expr,
                            expected,
                            pointer_count,
                            left_node,
                            left_ty,
                            base,
                            generics,
                        );
                    }
                    TypeFull::Generic(_) | TypeFull::FunctionItem { .. } | TypeFull::Const(_) => {
                        self.emit(Error::NonexistantMember(None).at_span(name_span));
                        self.invalidate(expected);
                        return Node::Invalid;
                    }
                },
                TypeInfo::Instance(base, generics) => {
                    return self.instance_member(
                        name_span,
                        expr,
                        expected,
                        pointer_count,
                        left_node,
                        left_ty,
                        base,
                        generics,
                    );
                }
                TypeInfo::Unknown(bounds) => {
                    self.emit_unknown(bounds, self.span(left));
                    self.invalidate(expected);
                    return Node::Invalid;
                }
                other => {
                    let hint = matches!(other, TypeInfo::Enum(_))
                        .then_some(error::MemberHint::InferredEnum);
                    self.emit(Error::NonexistantMember(hint).at_span(name_span));
                    self.invalidate(expected);
                    return Node::Invalid;
                }
            }
        }
    }

    fn instance_member(
        &mut self,
        name_span: TSpan,
        expr: ExprId,
        expected: LocalTypeId,
        pointer_count: u32,
        left_node: Node,
        left_ty: LocalTypeId,
        base: BaseType,
        generics: LocalTypeIds,
    ) -> Node {
        let name = &self.ast[name_span];
        let resolved = self.compiler.get_base_type_def(base);
        if let Some(&method) = resolved.methods.get(name) {
            let module = resolved.module;
            let signature = self.compiler.get_signature(module, method);
            let Some((required_pointer_count, call_generics)) =
                self.check_is_instance_method(signature, base, generics, |ast| ast[expr].span(ast))
            else {
                self.emit(Error::NotAnInstanceMethod.at_span(self.span(expr)));
                self.invalidate(expected);
                return Node::Invalid;
            };
            let self_value =
                self.auto_ref_deref(pointer_count, required_pointer_count, left_node, left_ty);
            self.specify(
                expected,
                TypeInfo::MethodItem {
                    module,
                    function: method,
                    generics: call_generics,
                },
                |ast| ast[expr].span(ast),
            );
            return self_value;
        }
        if let ResolvedTypeContent::Struct(def) = &resolved.def {
            let (indexed_field, elem_types) = def.get_indexed_field(self, generics, name);
            if let Some((index, field_ty)) = indexed_field {
                // perform auto deref first
                let mut dereffed_node = left_node;
                let mut current_ty = self.hir.types[left_ty];
                for _ in 0..pointer_count {
                    // the deref was already checked so we know the type is wrapped in
                    // `pointer_count` pointers
                    let pointee = match current_ty {
                        TypeInfo::Instance(BaseType::Pointer, pointee) => {
                            let pointee = pointee.nth(0).unwrap();
                            current_ty = self.hir.types[pointee];
                            pointee
                        }
                        TypeInfo::Known(ty) => {
                            let TypeFull::Instance(BaseType::Pointer, &[pointee]) =
                                self.compiler.types.lookup(ty)
                            else {
                                unreachable!()
                            };
                            current_ty = TypeInfo::Known(pointee);
                            self.hir.types.add(TypeInfo::Known(pointee))
                        }
                        _ => unreachable!(),
                    };
                    let value = self.hir.add(dereffed_node);
                    dereffed_node = Node::Deref {
                        value,
                        deref_ty: pointee,
                    };
                }
                self.unify(expected, field_ty, |ast| ast[expr].span(ast));
                return Node::Element {
                    tuple_value: self.hir.add(dereffed_node),
                    index,
                    elem_types,
                };
            }
        }
        self.emit(Error::NonexistantMember(None).at_span(name_span));
        self.invalidate(expected);
        Node::Invalid
    }

    /// checks if the provided method signature is valid as an instance method of the provided type.
    /// Returns the number of pointer indirections of the self type and the call's generics
    /// or None if the function is not a valid instance method.
    fn check_is_instance_method(
        &mut self,
        signature: &crate::compiler::Signature,
        id: BaseType,
        generics: LocalTypeIds,
        span: impl Copy + FnOnce(&Ast) -> TSpan,
    ) -> Option<(u32, LocalTypeIds)> {
        let (_, self_param_ty) = signature.all_params().next()?;
        let call_generics = Self::create_method_call_generics(
            &mut self.hir.types,
            &self.compiler.types,
            signature,
            generics,
            span(self.ast),
        );
        let mut required_pointer_count = 0;
        let mut current_ty = self_param_ty;
        loop {
            match self.compiler.types.lookup(current_ty) {
                TypeFull::Instance(BaseType::Pointer, pointee) => {
                    required_pointer_count += 1;
                    current_ty = pointee[0];
                }
                TypeFull::Instance(base, signature_generics) if id == base => {
                    // generics count should always matched because the type is already checked and
                    // resolved
                    debug_assert_eq!(generics.count as usize, signature_generics.len());
                    for (ty, &signature_ty) in generics.iter().zip(signature_generics) {
                        self.hir.types.specify_type_instance(
                            ty,
                            signature_ty,
                            generics,
                            self.generics,
                            self.compiler,
                            || ModuleSpan {
                                module: self.module,
                                span: span(self.ast),
                            },
                        );
                    }
                    return Some((required_pointer_count, call_generics));
                }
                _ => {
                    return None;
                }
            }
        }
    }

    fn create_method_call_generics(
        table: &mut TypeTable,
        types: &Types,
        signature: &crate::compiler::Signature,
        type_generics: LocalTypeIds,
        span: TSpan,
    ) -> LocalTypeIds {
        if signature.generics.count() as u32 != type_generics.count {
            debug_assert!(signature.generics.count() as u32 > type_generics.count);
            // perf: instantiates the outer type's generics again unnecessarily
            let call_generics = signature.generics.instantiate(table, types, span);
            for (r, generic) in call_generics.iter().zip(type_generics.iter()) {
                table.replace(r, generic);
            }
            call_generics
        } else {
            type_generics
        }
    }

    fn check_type_item_member_access(
        &mut self,
        ty: LocalTypeId,
        name: &str,
        name_span: TSpan,
        expected: LocalTypeId,
        left: ExprId,
        span: impl Copy + Fn(&Ast) -> TSpan,
    ) -> Node {
        // first check if the member is a special type property
        if let Some(property) = hir::TypeProperty::from_name(name) {
            // TODO: usize type
            self.specify(expected, Primitive::U64, span);
            return Node::TypeProperty(ty, property);
        }
        match self.hir.types[ty] {
            TypeInfo::Instance(BaseType::Invalid, _) => return Node::Invalid,
            TypeInfo::Instance(base, generics) => {
                return self.instance_type_item_member_access(
                    base, generics, name, name_span, expected, span,
                );
            }
            TypeInfo::Known(ty) => {
                if let TypeFull::Instance(base, generics) = self.compiler.types.lookup(ty) {
                    let generics = self
                        .hir
                        .types
                        .add_multiple(generics.iter().map(|&ty| TypeInfo::Known(ty)));
                    return self.instance_type_item_member_access(
                        base, generics, name, name_span, expected, span,
                    );
                }
            }
            TypeInfo::Enum { .. } => todo!("local enum variant from type item"),
            TypeInfo::Unknown(bounds) => {
                let needed_bound = bounds.iter().next().map(|bound| {
                    let id = self.hir.types.get_bound(bound).trait_id;
                    self.compiler.get_trait_name(id.0, id.1).to_owned()
                });
                self.emit(Error::TypeMustBeKnownHere { needed_bound }.at_span(self.span(left)));
                return Node::Invalid;
            }
            _ => {}
        };
        self.emit(Error::NonexistantMember(None).at_span(name_span));
        self.invalidate(expected);
        Node::Invalid
    }

    fn instance_type_item_member_access(
        &mut self,
        base: BaseType,
        generics: LocalTypeIds,
        name: &str,
        name_span: TSpan,
        expected: LocalTypeId,
        span: impl Copy + Fn(&Ast) -> TSpan,
    ) -> Node {
        let ty = self.compiler.get_base_type_def(base);
        if let Some(&method) = ty.methods.get(name) {
            let module = ty.module;
            let signature = self.compiler.get_signature(module, method);
            let call_generics = Self::create_method_call_generics(
                &mut self.hir.types,
                &self.compiler.types,
                signature,
                generics,
                span(self.ast),
            );
            self.specify(
                expected,
                TypeInfo::FunctionItem {
                    module,
                    function: method,
                    generics: call_generics,
                },
                span,
            );
            return Node::FunctionItem {
                function: (module, method),
                generics: call_generics,
                ty: expected,
            };
        }
        if let ResolvedTypeContent::Enum(def) = &ty.def
            && let Some((_, ordinal, args)) = def.get_by_name(name)
        {
            let ordinal_ty = type_info_from_variant_count(def.variants.len() as u32);
            let node = if args.is_empty() {
                self.specify(expected, TypeInfo::Instance(base, generics), span);
                let ordinal_ty = self.hir.types.add(ordinal_ty);
                let elem_types = self.hir.types.add_multiple_info_or_idx([ordinal_ty.into()]);
                let elems = self.hir.add_nodes([Node::IntLiteral {
                    val: ordinal as u128,
                    ty: ordinal_ty,
                }]);
                Node::TupleLiteral { elems, elem_types }
            } else {
                let arg_types = self.hir.types.add_multiple_unknown(args.len() as u32 + 1);
                let mut arg_type_iter = arg_types.iter();
                self.hir
                    .types
                    .replace(arg_type_iter.next().unwrap(), ordinal_ty);
                for (r, &ty) in arg_type_iter.zip(args.iter()) {
                    let ty = self.from_type_instance(ty, generics);
                    self.hir.types.replace(r, ty);
                }
                self.specify(
                    expected,
                    TypeInfo::EnumVariantItem {
                        enum_type: base,
                        generics,
                        ordinal,
                        arg_types,
                    },
                    span,
                );
                Node::Invalid
            };
            return node;
        }
        self.emit(Error::NonexistantMember(None).at_span(name_span));
        self.invalidate(expected);
        Node::Invalid
    }
}

pub fn type_from_variant_count(count: u32) -> Type {
    int_primitive_from_variant_count(count).map_or(Type::Unit, Type::from)
}

pub fn type_info_from_variant_count(count: u32) -> TypeInfo {
    int_primitive_from_variant_count(count).map_or(TypeInfo::UNIT, |p| {
        TypeInfo::Instance(p.into(), LocalTypeIds::EMPTY)
    })
}

pub fn int_primitive_from_variant_count(count: u32) -> Option<Primitive> {
    match count {
        0 | 1 => None,
        2..=255 => Some(Primitive::U8),
        256..=65535 => Some(Primitive::U16),
        _ => Some(Primitive::U32),
    }
}
