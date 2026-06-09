mod display;

use std::ops::{Index, IndexMut};

use id::id;
use parser::ast::{AssignType, FloatType, FunctionId, GlobalId, IntType, ModuleId, TraitId};

use crate::{
    Compiler, Type,
    compiler::{CaptureId, Generics, VarId},
    eval::ConstValueId,
    typing::{InferredEnumVariant, LocalTypeId, LocalTypeIds, OrdinalType, TypeTable, VariantId},
};

id!(NodeId);
id!(CastId);

/// High-level intermediate representation for a function. It is created during type checking and
/// contains all resolved identifiers and type information.
/// nodes must be non-empty and the last node is the root node
pub struct Hir {
    nodes: Vec<Node>,
    types: Box<[Type]>,
    lvalues: Vec<LValue>,
    patterns: Vec<Pattern>,
    pub vars: Vec<Var>,
    pub params: Vec<VarId>,
    casts: Vec<Cast>,
    pub trait_calls: Vec<Option<(ModuleId, FunctionId)>>,
    variants: Box<[InferredEnumVariant]>,
}
impl Hir {
    pub fn root_id(&self) -> NodeId {
        NodeId((self.nodes.len() - 1) as _)
    }

    pub fn display<'a>(
        &'a self,
        node: NodeId,
        compiler: &'a Compiler,
        generics: &'a Generics,
        indent_count: usize,
    ) -> display::HirDisplay<'a> {
        display::HirDisplay {
            node,
            hir: self,
            compiler,
            generics,
            indent_count,
        }
    }

    fn display_pattern<'a>(
        &'a self,
        pattern: PatternId,
        compiler: &'a Compiler,
        generics: &'a Generics,
    ) -> display::PatternDisplay<'a> {
        display::PatternDisplay {
            pattern,
            hir: self,
            compiler,
            generics,
        }
    }

    fn display_lvalue<'a>(
        &'a self,
        lval: LValueId,
        compiler: &'a Compiler,
        generics: &'a Generics,
        indent_count: usize,
    ) -> display::LValueDisplay<'a> {
        display::LValueDisplay {
            lval,
            hir: self,
            compiler,
            generics,
            indent_count,
        }
    }

    pub fn types(&self) -> &[Type] {
        &self.types
    }

    pub fn clone_types_for_closure(&self) -> (Box<[Type]>, Box<[InferredEnumVariant]>) {
        (self.types.clone(), self.variants.clone())
    }
}
impl Index<NodeId> for Hir {
    type Output = Node;

    fn index(&self, index: NodeId) -> &Self::Output {
        &self.nodes[index.idx()]
    }
}
impl Index<LocalTypeId> for Hir {
    type Output = Type;
    fn index(&self, index: LocalTypeId) -> &Self::Output {
        &self.types[index.idx()]
    }
}
impl Index<LocalTypeIds> for Hir {
    type Output = [Type];
    fn index(&self, index: LocalTypeIds) -> &Self::Output {
        &self.types[index.idx as usize..index.idx as usize + index.count as usize]
    }
}
impl Index<CastId> for Hir {
    type Output = Cast;

    fn index(&self, index: CastId) -> &Self::Output {
        &self.casts[index.idx()]
    }
}
impl IndexMut<CastId> for Hir {
    fn index_mut(&mut self, index: CastId) -> &mut Self::Output {
        &mut self.casts[index.idx()]
    }
}
impl Index<VariantId> for Hir {
    type Output = InferredEnumVariant;
    fn index(&self, index: VariantId) -> &Self::Output {
        &self.variants[index.idx()]
    }
}
#[derive(Debug, Clone, Copy)]
pub struct NodeIds {
    pub index: u32,
    pub count: u32,
}
impl NodeIds {
    pub const EMPTY: Self = Self { index: 0, count: 0 };

    pub fn iter(self) -> impl Iterator<Item = NodeId> {
        (self.index..self.index + self.count).map(NodeId)
    }

    pub fn skip(self, i: u32) -> NodeIds {
        Self {
            index: self.index + i,
            count: self.count - i,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn nth(&self, n: u32) -> Option<NodeId> {
        (n < self.count).then(|| NodeId(self.index + n))
    }
}
impl From<NodeId> for NodeIds {
    fn from(value: NodeId) -> Self {
        Self {
            index: value.0,
            count: 1,
        }
    }
}
impl Index<NodeIds> for Hir {
    type Output = [Node];
    fn index(&self, index: NodeIds) -> &Self::Output {
        &self.nodes[index.index as usize..index.index as usize + index.count as usize]
    }
}
id!(LValueId);
impl Index<LValueId> for Hir {
    type Output = LValue;
    fn index(&self, index: LValueId) -> &Self::Output {
        &self.lvalues[index.idx()]
    }
}
id!(PatternId);
impl Index<PatternId> for Hir {
    type Output = Pattern;

    fn index(&self, index: PatternId) -> &Self::Output {
        &self.patterns[index.idx()]
    }
}
#[derive(Debug, Clone, Copy)]
pub struct PatternIds {
    pub index: u32,
    pub count: u32,
}
impl From<PatternId> for PatternIds {
    fn from(value: PatternId) -> Self {
        Self {
            index: value.0,
            count: 1,
        }
    }
}
impl PatternIds {
    pub fn iter(self) -> impl Iterator<Item = PatternId> {
        (self.index..self.index + self.count).map(PatternId)
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Invalid,
    Block(NodeIds),
    Unit,
    IntLiteral {
        val: u128,
        ty: LocalTypeId,
    },
    FloatLiteral {
        val: f64,
        ty: LocalTypeId,
    },
    BoolLiteral(bool),
    ArrayLiteral {
        elems: NodeIds,
        array_ty: LocalTypeId,
    },
    TupleLiteral {
        // PERF(size): length has to match anyways, could only store it once
        elems: NodeIds,
        elem_types: LocalTypeIds,
    },
    StringLiteral(Box<str>),
    EnumLiteral {
        // PERF(size): length has to match anyways, could only store it once
        elems: NodeIds,
        elem_types: LocalTypeIds,
        enum_ty: LocalTypeId,
    },
    InferredEnumOrdinal(VariantId),

    Declare {
        pattern: PatternId,
    },
    DeclareWithVal {
        pattern: PatternId,
        val: NodeId,
    },
    Variable(VarId),
    Global(ModuleId, GlobalId, LocalTypeId),
    Assign(LValueId, NodeId, AssignType, LocalTypeId),

    Const {
        id: ConstValueId,
        ty: LocalTypeId,
    },

    Negate(NodeId, LocalTypeId),
    Not(NodeId),
    AddressOf {
        value: LValueId,
        value_ty: LocalTypeId,
    },
    Deref {
        value: NodeId,
        deref_ty: LocalTypeId,
    },
    Promote {
        value: NodeId,
        variable: VarId,
    },

    Cast(CastId),
    Comparison {
        l: NodeId,
        r: NodeId,
        cmp: Comparison,
        compared: LocalTypeId,
    },
    Logic {
        l: NodeId,
        r: NodeId,
        logic: Logic,
    },
    Arithmetic(NodeId, NodeId, Arithmetic, LocalTypeId),

    Element {
        tuple_value: NodeId,
        index: u32,
        // tuple-like type: either a tuple or struct
        tuple_type: LocalTypeId,
    },
    ArrayIndex {
        array: NodeId,
        index: NodeId,
        elem_ty: LocalTypeId,
    },

    Return(NodeId),
    IfElse {
        cond: NodeId,
        then: NodeId,
        else_: NodeId,
        resulting_ty: LocalTypeId,
    },
    IfPatElse {
        pat: PatternId,
        val: NodeId,
        then: NodeId,
        else_: NodeId,
        resulting_ty: LocalTypeId,
    },
    Match {
        value: NodeId,
        branch_index: u32,
        pattern_index: u32,
        branch_count: u32,
        resulting_ty: LocalTypeId,
    },
    While {
        cond: NodeId,
        body: NodeId,
    },
    WhilePat {
        pat: PatternId,
        val: NodeId,
        body: NodeId,
    },
    Call {
        function: NodeId,
        args: NodeIds,
        arg_types: LocalTypeIds,
        return_ty: LocalTypeId,
        noreturn: bool,
    },
    TraitCall {
        trait_id: (ModuleId, TraitId),
        trait_generics: LocalTypeIds,
        method_index: u16,
        self_ty: LocalTypeId,
        args: NodeIds,
        return_ty: LocalTypeId,
        noreturn: bool,
    },
    TypeProperty(LocalTypeId, TypeProperty),
    FunctionItem {
        function: (ModuleId, FunctionId),
        generics: LocalTypeIds,
        ty: LocalTypeId,
    },
    Capture(CaptureId),
    Break(u32),
    Continue(u32),
}

#[derive(Clone, Copy, Debug)]
pub enum Var {
    Local(LocalTypeId),
    Capture {
        outer: VarId,
        ty: LocalTypeId,
        capture_idx: u32,
    },
    /// the variable that all captures of a closure are passed through
    CapturesParam(LocalTypeId),
}
impl Var {
    pub fn ty(&self) -> LocalTypeId {
        match *self {
            Var::Local(ty) | Var::Capture { ty, .. } | Var::CapturesParam(ty) => ty,
        }
    }
}

#[derive(Debug, Clone)]
pub enum LValue {
    Invalid,
    Variable(VarId),
    Global(ModuleId, GlobalId),
    Deref(NodeId),
    Member {
        tuple: LValueId,
        index: u32,
        elem_types: LocalTypeIds,
    },
    ArrayIndex {
        array: LValueId,
        index: NodeId,
        elem_ty: LocalTypeId,
    },
}
impl LValue {
    pub fn try_from_node(node: &Node, hir: &mut HIRBuilder) -> Option<Self> {
        Some(match *node {
            Node::Invalid => Self::Invalid,
            Node::Variable(id) => Self::Variable(id),
            Node::Global(module, id, _) => Self::Global(module, id),
            Node::Deref { value, deref_ty: _ } => Self::Deref(value),
            Node::Element {
                tuple_value,
                index,
                elem_types,
            } => {
                let tuple = Self::try_from_node(&hir[tuple_value].clone(), hir)?;
                Self::Member {
                    tuple: hir.add_lvalue(tuple),
                    index,
                    elem_types,
                }
            }
            Node::ArrayIndex {
                array,
                index,
                elem_ty,
            } => {
                let array = Self::try_from_node(&hir[array].clone(), hir)?;
                Self::ArrayIndex {
                    array: hir.add_lvalue(array),
                    index,
                    elem_ty,
                }
            }
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Invalid,
    Variable(VarId),
    Ignore,
    Tuple {
        member_count: u32,
        /// member_count Patterns start index
        patterns: u32,
        /// member_count LocalTypeIds start index
        types: u32,
    },
    Int(bool, u128, LocalTypeId),
    Bool(bool),
    String(Box<str>),
    Range {
        min_max: (u128, u128),
        ty: LocalTypeId,
        min_max_signs: (bool, bool),
        inclusive: bool,
    },
    EnumVariant {
        ordinal: OrdinalType,
        /// always one more than args because it contains the ordinal type first
        types: u32,
        args: PatternIds,
    },
}

#[derive(Debug, Clone)]
pub struct Cast {
    pub val: NodeId,
    pub cast_ty: CastType,
}

#[derive(Debug, Clone)]
pub enum CastType {
    Invalid,
    Noop,
    Int { from: IntType, to: IntType },
    Float { from: FloatType, to: FloatType },
    IntToFloat { from: IntType, to: FloatType },
    FloatToInt { from: FloatType, to: IntType },
    IntToPtr { from: IntType },
    PtrToInt { to: IntType },
    EnumToInt { from: LocalTypeId, to: IntType },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    LT,
    GT,
    LE,
    GE,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Logic {
    And,
    Or,
}

#[derive(Debug, Clone, Copy)]
pub enum Arithmetic {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeProperty {
    Size,
    Align,
    Stride,
}
impl TypeProperty {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "size" => Self::Size,
            "align" => Self::Align,
            "stride" => Self::Stride,
            _ => return None,
        })
    }
}

pub struct HIRBuilder {
    nodes: Vec<Node>,
    lvalues: Vec<LValue>,
    patterns: Vec<Pattern>,
    pub types: TypeTable,
    pub vars: Vec<Var>,
    casts: Vec<Cast>,
}
impl Index<NodeId> for HIRBuilder {
    type Output = Node;
    fn index(&self, index: NodeId) -> &Self::Output {
        &self.nodes[index.0 as usize]
    }
}
impl HIRBuilder {
    pub fn new(types: TypeTable) -> Self {
        Self {
            nodes: Vec::new(),
            lvalues: Vec::new(),
            patterns: Vec::new(),
            types,
            vars: Vec::new(),
            casts: Vec::new(),
        }
    }

    pub fn finish(
        self,
        root: Node,
        compiler: &Compiler,
        generics: &Generics,
        module: ModuleId,
        params: Vec<VarId>,
    ) -> Hir {
        let types = self.types.finish(compiler, generics, module);
        Self::finish_with_types_inner(
            self.nodes,
            self.lvalues,
            self.patterns,
            self.vars,
            self.casts,
            root,
            params,
            types,
        )
    }

    pub fn finish_with_types(
        self,
        types: (Box<[Type]>, Box<[InferredEnumVariant]>),
        root: Node,
        params: Vec<VarId>,
    ) -> Hir {
        Self::finish_with_types_inner(
            self.nodes,
            self.lvalues,
            self.patterns,
            self.vars,
            self.casts,
            root,
            params,
            types,
        )
    }

    fn finish_with_types_inner(
        mut nodes: Vec<Node>,
        lvalues: Vec<LValue>,
        patterns: Vec<Pattern>,
        vars: Vec<Var>,
        casts: Vec<Cast>,
        root: Node,
        params: Vec<VarId>,
        (types, variants): (Box<[Type]>, Box<[InferredEnumVariant]>),
    ) -> Hir {
        nodes.push(root);
        Hir {
            nodes,
            types,
            lvalues,
            patterns,
            vars,
            params,
            casts,
            trait_calls: Vec::new(),
            variants,
        }
    }

    pub fn add(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.nodes.len() as _);
        self.nodes.push(node);
        id
    }

    pub fn add_nodes(&mut self, nodes: impl IntoIterator<Item = Node>) -> NodeIds {
        let start = self.nodes.len();
        self.nodes.extend(nodes);
        let count = self.nodes.len() - start;
        NodeIds {
            index: start as _,
            count: count as _,
        }
    }

    pub fn add_lvalue(&mut self, lvalue: LValue) -> LValueId {
        let id = LValueId(self.lvalues.len() as _);
        self.lvalues.push(lvalue);
        id
    }

    pub fn add_invalid_nodes(&mut self, count: u32) -> NodeIds {
        let start = self.nodes.len();
        self.nodes.extend((0..count).map(|_| Node::Invalid));
        NodeIds {
            index: start as _,
            count,
        }
    }

    pub fn modify_node(&mut self, node_id: NodeId, node: Node) {
        self.nodes[node_id.0 as usize] = node;
    }

    pub fn modify_pattern(&mut self, pattern_id: PatternId, pattern: Pattern) {
        self.patterns[pattern_id.0 as usize] = pattern;
    }

    pub fn add_pattern(&mut self, pattern: Pattern) -> PatternId {
        let id = PatternId(self.patterns.len() as _);
        self.patterns.push(pattern);
        id
    }

    pub fn add_var(&mut self, ty: LocalTypeId) -> VarId {
        let id = VarId(self.vars.len() as _);
        self.vars.push(Var::Local(ty));
        id
    }

    pub fn add_captured_var(&mut self, outer: VarId, ty: LocalTypeId, capture_idx: u32) -> VarId {
        let id = VarId(self.vars.len() as _);
        self.vars.push(Var::Capture {
            outer,
            ty,
            capture_idx,
        });
        id
    }

    pub fn add_captures_param_var(&mut self, ty: LocalTypeId) -> VarId {
        let id = VarId(self.vars.len() as _);
        self.vars.push(Var::CapturesParam(ty));
        id
    }

    pub fn get_var(&mut self, id: VarId) -> Var {
        self.vars[id.idx()]
    }

    pub fn add_patterns(&mut self, patterns: impl IntoIterator<Item = Pattern>) -> PatternIds {
        let start = self.patterns.len();
        self.patterns.extend(patterns);
        let count = self.patterns.len() - start;
        PatternIds {
            index: start as _,
            count: count as _,
        }
    }

    pub fn add_invalid_patterns(&mut self, count: u32) -> PatternIds {
        self.add_patterns((0..count).map(|_| Pattern::Invalid))
    }

    pub fn add_cast(&mut self, cast: Cast) -> CastId {
        let id = CastId(self.casts.len() as _);
        self.casts.push(cast);
        id
    }
}
