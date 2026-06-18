use std::{
    borrow::Cow,
    cell::OnceCell,
    fmt, iter,
    marker::PhantomData,
    mem::transmute,
    num::{NonZero, NonZeroU64},
    ops::{Deref, Index, IndexMut},
};

use color_format::cwrite;

pub mod block_graph;
pub mod builder;
pub mod dialect;
pub mod eval;
pub mod mc;
pub mod modify;
pub mod optimize;
pub mod parse;
pub mod pipeline;
pub mod rewrite;
pub mod slots;
pub mod use_counts;
pub mod verify;

mod argument;
mod bitmap;
mod builtins;
mod display;
mod environment;
mod flags;
mod layout;

pub use argument::{ArgError, Argument, IntoArgs};
pub use bitmap::Bitmap;
pub use block_graph::BlockGraph;
pub use builtins::{BUILTIN, Builtin};
pub use dialect::Primitive;
pub use environment::Environment;
pub use layout::{Layout, offset_in_tuple, type_layout, visit_primitives};

pub struct ModuleOf<I>(ModuleId, PhantomData<*const I>);
impl<I> ModuleOf<I> {
    pub fn id(self) -> ModuleId {
        self.0
    }
}
impl<I: Inst> Deref for ModuleOf<I> {
    type Target = I::InstTable;

    fn deref(&self) -> &Self::Target {
        I::inst_table(self)
    }
}
impl<I> Clone for ModuleOf<I> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<I> Copy for ModuleOf<I> {}
impl<I> From<ModuleOf<I>> for ModuleId {
    fn from(value: ModuleOf<I>) -> Self {
        value.0
    }
}

pub struct Module {
    name: Box<str>,
    functions: SegmentList<Function>,
    globals: SegmentList<Global>,
}
impl Module {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn functions(&self) -> segment_list::Iter<'_, Function> {
        self.functions.iter()
    }

    pub fn function_ids(&self) -> impl use<> + ExactSizeIterator<Item = LocalFunctionId> {
        (0..self.functions.len()).map(LocalFunctionId)
    }

    pub fn globals(&self) -> segment_list::Iter<'_, Global> {
        self.globals.iter()
    }

    pub fn global_ids(&self) -> impl use<> + ExactSizeIterator<Item = u32> {
        0..self.globals.len()
    }
}
impl Index<LocalFunctionId> for Module {
    type Output = Function;

    fn index(&self, index: LocalFunctionId) -> &Self::Output {
        &self.functions[index.0 as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ModuleId(u32);
impl ModuleId {
    pub const BUILTINS: Self = Self(0);

    fn idx(&self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct LocalFunctionId(pub u32);
impl LocalFunctionId {
    pub fn idx(&self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionId {
    pub module: ModuleId,
    pub function: LocalFunctionId,
}
impl FunctionId {
    pub fn new(module: ModuleId, function: LocalFunctionId) -> Self {
        Self { module, function }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalId {
    pub module: ModuleId,
    pub idx: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct TypeId(u32);
impl TypeId {
    pub const UNIT: Self = Self(0);
    pub const I1: Self = Self(1);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeIds {
    idx: u32,
    count: u32,
}
impl TypeIds {
    pub const EMPTY: Self = Self { idx: 0, count: 0 };

    pub fn count(self) -> u32 {
        self.count
    }

    pub fn iter(self) -> impl Iterator<Item = TypeId> {
        (self.idx..self.idx + self.count).map(TypeId)
    }

    pub fn nth(&self, idx: u32) -> TypeId {
        assert!(idx < self.count);
        TypeId(self.idx + idx)
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PrimitiveId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Ref(u32);
impl fmt::Debug for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UNIT => write!(f, "Ref(unit)"),
            Self::FALSE => write!(f, "Ref(false)"),
            Self::TRUE => write!(f, "Ref(true)"),
            _ => f.debug_tuple("Ref").field(&self.0).finish(),
        }
    }
}
impl Ref {
    pub const UNIT: Self = Self(u32::MAX);
    pub const FALSE: Self = Self(u32::MAX - 1);
    pub const TRUE: Self = Self(u32::MAX - 2);

    pub fn index(idx: u32) -> Self {
        Self(idx)
    }

    pub fn is_ref(self) -> bool {
        self.0 < u32::MAX - 2
    }

    pub fn into_ref(self) -> Option<u32> {
        self.is_ref().then_some(self.0)
    }

    pub fn idx(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Refs {
    idx: u32,
    count: u32,
}
impl Refs {
    pub const EMPTY: Self = Self { idx: 0, count: 0 };

    pub fn nth(self, n: u32) -> Ref {
        assert!(
            n < self.count,
            "Refs index out of range, {n} >= {}",
            self.count
        );
        Ref(self.idx + n)
    }

    pub fn iter(self) -> impl DoubleEndedIterator<Item = Ref> {
        (self.idx..self.idx + self.count).map(Ref)
    }

    pub fn count(self) -> u32 {
        self.count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct BlockId(u32);
impl BlockId {
    pub const ENTRY: Self = Self(0);

    pub fn idx(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone)]
pub struct BlockTarget<'a>(pub BlockId, pub Cow<'a, [Ref]>);
impl BlockTarget<'static> {
    pub fn new(block: BlockId) -> Self {
        Self(block, Cow::Borrowed(&[]))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct MCReg(u32);
impl fmt::Debug for MCReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_dead() {
            write!(f, "!")?;
        }
        if let Some(i) = self.virt() {
            write!(f, "${i}")
        } else {
            let i: mc::UnknownRegister = self.phys().unwrap();
            write!(f, "%{}", i.encode())
        }
    }
}
impl MCReg {
    const VIRT_BIT: u32 = 1 << 31;
    const DEAD_BIT: u32 = 1 << 30;

    pub fn from_phys<R: Register>(phys: R) -> Self {
        let v = phys.encode();
        debug_assert!(
            v & (Self::VIRT_BIT | Self::DEAD_BIT) == 0,
            "highest bit shouldn't be set for physical register encoding"
        );
        Self(v)
    }

    pub fn is_virt(self) -> bool {
        self.0 & Self::VIRT_BIT != 0
    }

    pub fn is_phys(self) -> bool {
        !self.is_virt()
    }

    pub fn is_dead(self) -> bool {
        self.0 & Self::DEAD_BIT != 0
    }

    pub fn set_dead(&mut self) {
        self.0 |= Self::DEAD_BIT;
    }

    pub fn phys<R: Register>(self) -> Option<R> {
        self.is_phys().then(|| R::decode(self.0 & !Self::DEAD_BIT))
    }

    pub fn virt(self) -> Option<u32> {
        self.is_virt()
            .then_some(self.0 & !(Self::VIRT_BIT | Self::DEAD_BIT))
    }

    pub const fn from_virt(r: u32) -> MCReg {
        assert!(
            r & (Self::VIRT_BIT | Self::DEAD_BIT) == 0,
            "virtual register value too high"
        );
        Self(r | Self::VIRT_BIT)
    }
}

pub struct Function {
    pub name: Box<str>,
    // TODO: rethink type storage of function, especially with parameters vs content
    types: Types,
    params: Vec<Parameter>,
    varargs: Option<Parameter>,
    flags: InstFlags,
    return_type: Option<TypeId>,
    pub(crate) ir: OnceCell<FunctionIr>,
}
impl Index<TypeId> for Function {
    type Output = Type;

    fn index(&self, index: TypeId) -> &Self::Output {
        &self.types[index]
    }
}
impl Function {
    pub fn empty(name: impl Into<Box<str>>) -> Self {
        Self {
            name: name.into(),
            types: Types::new(),
            params: Vec::new(),
            varargs: None,
            flags: InstFlags::default(),
            return_type: None,
            ir: OnceCell::new(),
        }
    }

    pub fn new(
        name: impl Into<Box<str>>,
        types: Types,
        params: impl IntoIterator<Item = TypeId>,
        return_type: TypeId,
        ir: FunctionIr,
    ) -> Self {
        Self {
            name: name.into(),
            types,
            params: params.into_iter().map(Parameter::RefOf).collect(),
            varargs: None,
            flags: InstFlags::default(),
            return_type: Some(return_type),
            ir: OnceCell::from(ir),
        }
    }

    pub fn declare(
        name: impl Into<Box<str>>,
        types: Types,
        params: impl IntoIterator<Item = TypeId>,
        varargs: bool,
        return_type: TypeId,
    ) -> Self {
        Self {
            name: name.into(),
            types,
            params: params.into_iter().map(Parameter::RefOf).collect(),
            varargs: varargs.then_some(Parameter::Ref),
            flags: InstFlags::default(),
            return_type: Some(return_type),
            ir: OnceCell::new(),
        }
    }

    pub fn declare_inst(
        name: impl Into<Box<str>>,
        types: Types,
        params: Vec<Parameter>,
        attrs: InstAttrs,
    ) -> Self {
        Self {
            name: name.into(),
            types,
            params,
            varargs: attrs.varargs,
            flags: attrs.flags,
            return_type: None,
            ir: OnceCell::new(),
        }
    }

    pub fn take_body(&mut self) -> Option<FunctionIr> {
        self.ir.take()
    }

    #[inline]
    pub fn attach_body(&self, body: FunctionIr) {
        self.ir
            .set(body)
            .unwrap_or_else(|_| panic!("Ir already had a body"));
    }

    pub fn overwrite_types(&mut self, types: Types) {
        self.types = types;
    }

    #[inline]
    pub fn ir(&self) -> Option<&FunctionIr> {
        self.ir.get()
    }

    #[inline]
    pub fn types(&self) -> &Types {
        &self.types
    }

    pub fn params(&self) -> &[Parameter] {
        &self.params
    }

    pub fn varargs(&self) -> Option<Parameter> {
        self.varargs
    }

    pub fn return_type(&self) -> Option<TypeId> {
        self.return_type
    }

    pub fn display<'a>(&'a self, env: &'a Environment) -> crate::display::FunctionDisplay<'a> {
        crate::display::FunctionDisplay {
            env,
            function: self,
        }
    }

    pub fn flags(&self) -> InstFlags {
        self.flags
    }
}

pub struct Global {
    pub name: Box<str>,
    pub align: NonZero<u64>,
    pub value: Box<[u8]>,
    pub readonly: bool,
}

pub struct PrimitiveInfo {
    pub name: Box<str>,
    pub size: u64,
    pub align: NonZeroU64,
}
impl PrimitiveInfo {
    pub fn layout(&self) -> Layout {
        Layout {
            size: self.size,
            align: self.align,
        }
    }
}

#[derive(Clone)]
pub struct Types {
    types: Vec<Type>,
}
impl Types {
    pub fn new() -> Self {
        Self {
            types: vec![Type::UNIT, Type::Primitive(Primitive::I1.into())],
        }
    }

    pub fn add(&mut self, ty: impl Into<Type>) -> TypeId {
        let id = TypeId(self.types.len() as _);
        self.types.push(ty.into());
        id
    }

    pub fn add_multiple(&mut self, types: impl IntoIterator<Item = Type>) -> TypeIds {
        let idx = self.types.len() as u32;
        self.types.extend(types);
        let count = self.types.len() as u32 - idx;
        TypeIds { idx, count }
    }

    pub fn display_type<'a>(
        &'a self,
        ty: TypeId,
        primitives: &'a [PrimitiveInfo],
    ) -> impl fmt::Display + use<'a> {
        TypeDisplay {
            types: self,
            primitives,
            id: ty,
        }
    }

    pub fn is_zero_sized(&self, ty: Type, primitives: &[PrimitiveInfo]) -> bool {
        match ty {
            Type::Primitive(primitive_id) => primitives[primitive_id.0 as usize].size == 0,
            Type::Array(_, 0) => true,
            Type::Array(elem, _) => self.is_zero_sized(self[elem], primitives),
            Type::Tuple(elems) => elems
                .iter()
                .all(|elem| self.is_zero_sized(self[elem], primitives)),
        }
    }

    pub fn are_equal(&self, a: TypeId, b: TypeId) -> bool {
        a == b
            || match (self[a], self[b]) {
                (Type::Primitive(a), Type::Primitive(b)) => a == b,
                (Type::Array(a, a_len), Type::Array(b, b_len)) if a_len == b_len => {
                    self.are_equal(a, b)
                }
                (Type::Tuple(a), Type::Tuple(b)) if a.count == b.count => {
                    a.iter().zip(b.iter()).all(|(a, b)| self.are_equal(a, b))
                }
                _ => false,
            }
    }
}
impl Index<TypeId> for Types {
    type Output = Type;

    fn index(&self, index: TypeId) -> &Self::Output {
        &self.types[index.0 as usize]
    }
}
impl IndexMut<TypeId> for Types {
    fn index_mut(&mut self, index: TypeId) -> &mut Self::Output {
        &mut self.types[index.0 as usize]
    }
}

struct TypeDisplay<'a> {
    types: &'a Types,
    primitives: &'a [PrimitiveInfo],
    id: TypeId,
}
impl fmt::Display for TypeDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.types[self.id] {
            Type::Primitive(primitive_id) => {
                cwrite!(f, "#m<{}>", self.primitives[primitive_id.0 as usize].name)
            }
            Type::Array(elem, count) => {
                let elem_display = TypeDisplay {
                    types: self.types,
                    primitives: self.primitives,
                    id: elem,
                };
                cwrite!(f, "[{elem_display}; #y<{count}>]")
            }
            Type::Tuple(elems) => {
                write!(f, "(")?;
                for i in elems.idx..elems.idx + elems.count {
                    if i != elems.idx {
                        write!(f, ", ")?;
                    }
                    let display = TypeDisplay {
                        types: self.types,
                        primitives: self.primitives,
                        id: TypeId(i),
                    };
                    write!(f, "{display}")?;
                }
                write!(f, ")")
            }
        }
    }
}

impl Default for Types {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Type {
    Primitive(PrimitiveId),
    Array(TypeId, u32),
    Tuple(TypeIds),
}
impl Type {
    pub const UNIT: Self = Self::Tuple(TypeIds::EMPTY);

    pub fn is_unit(&self) -> bool {
        matches!(self, Self::Tuple(elems) if elems.is_empty())
    }
}
impl<T> PartialEq<T> for Type
where
    for<'a> T: Copy + Into<PrimitiveId>,
{
    fn eq(&self, other: &T) -> bool {
        matches!(self, &Type::Primitive(id) if id == (*other).into())
    }
}

#[derive(Clone, Debug)]
pub struct FunctionIr {
    blocks: Vec<BlockInfo>,
    insts: Vec<Instruction>,
    extra: Vec<u32>,
    mc_regs: Vec<RegClass>,
}
impl FunctionIr {
    pub fn block_ids(&self) -> impl ExactSizeIterator<Item = BlockId> + use<> {
        (0..self.blocks.len() as u32).map(BlockId)
    }

    pub fn block_count(&self) -> u32 {
        self.blocks.len() as _
    }

    pub fn inst_count(&self) -> u32 {
        self.insts.len() as _
    }

    pub fn refs(&self) -> impl use<> + ExactSizeIterator<Item = Ref> {
        (0..self.insts.len() as u32).map(Ref)
    }

    pub fn get_refs(
        &self,
        idx: Ref,
        count: u32,
    ) -> impl ExactSizeIterator<Item = (Ref, &Instruction)>
    + DoubleEndedIterator<Item = (Ref, &Instruction)>
    + use<'_> {
        (idx.0..idx.0 + count).map(|i| (Ref(i), &self.insts[i as usize]))
    }

    pub fn get_block_args(&self, id: BlockId) -> Refs {
        let block = &self.blocks[id.0 as usize];
        Refs {
            idx: block.args_idx,
            count: block.arg_count,
        }
    }

    pub fn get_block_refs(&self, block: BlockId) -> Refs {
        let info = &self.blocks[block.idx()];
        Refs {
            idx: info.body_idx,
            count: info.len,
        }
    }

    pub fn block_arg_count(&self, block: BlockId) -> u32 {
        self.blocks[block.0 as usize].arg_count
    }

    pub fn get_block(
        &self,
        id: BlockId,
    ) -> impl ExactSizeIterator<Item = (Ref, &Instruction)>
    + DoubleEndedIterator<Item = (Ref, &Instruction)>
    + use<'_> {
        let block = &self.blocks[id.idx()];
        self.get_refs(Ref(block.body_idx), block.len)
    }

    pub fn preds(&self, block: BlockId) -> &[BlockId] {
        &self.blocks[block.idx()].preds
    }

    pub fn succs(&self, block: BlockId) -> &[BlockId] {
        &self.blocks[block.idx()].succs
    }

    pub fn block_refs(&self, id: BlockId) -> Refs {
        let block = &self.blocks[id.idx()];
        Refs {
            idx: block.args_idx + block.arg_count,
            count: block.len,
        }
    }

    pub fn get_inst(&self, r: Ref) -> &Instruction {
        &self.insts[r.idx()]
    }

    pub fn blocks(&self) -> &[BlockInfo] {
        &self.blocks
    }

    pub fn extra(&self) -> &[u32] {
        &self.extra
    }

    pub fn get_ref_ty(&self, arg: Ref) -> TypeId {
        match arg {
            Ref::UNIT => TypeId::UNIT,
            Ref::TRUE | Ref::FALSE => TypeId::I1,
            _ => self.insts[arg.idx()].ty,
        }
    }

    #[inline]
    #[track_caller]
    pub fn args<'a, A: argument::Args<'a>>(
        &'a self,
        inst: &'a Instruction,
        env: &'a Environment,
    ) -> A {
        // intentionally not using unwrap_or_else here so track_caller works for the panic
        match A::get(self.args_iter(inst, env)) {
            Ok(args) => args,
            Err(err) => {
                let module_name = &env[inst.module()].name;
                let name = &env[inst.function].name;
                panic!("Invalid arguments for {module_name}.{name}: {err:?}");
            }
        }
    }

    pub fn args_iter<'a>(
        &'a self,
        inst: &'a Instruction,
        env: &'a Environment,
    ) -> ArgsIter<'a, 'a> {
        let func = &env[inst.function];
        decode_args(
            &inst.args,
            &func.params,
            func.varargs,
            &self.blocks,
            &self.extra,
        )
    }

    #[inline]
    #[track_caller]
    pub fn typed_args<'a, A: argument::Args<'a>, I: Inst>(
        &'a self,
        inst: &'a TypedInstruction<I>,
    ) -> A {
        // intentionally not using unwrap_or_else here so track_caller works for the panic
        match A::get(self.typed_args_iter(inst)) {
            Ok(args) => args,
            Err(err) => {
                panic!(
                    "Invalid arguments for {}.{}: {err:?}",
                    std::any::type_name::<I>(),
                    inst.op().name()
                );
            }
        }
    }

    pub fn typed_args_iter<'a, I: Inst + 'static>(
        &'a self,
        inst: &'a TypedInstruction<I>,
    ) -> ArgsIter<'a, 'a> {
        inst.args_iter(&self.blocks, &self.extra)
    }

    pub fn args_n<'a, I: Inst + 'static, const N: usize>(
        &'a self,
        inst: &'a TypedInstruction<I>,
    ) -> [Argument<'a>; N] {
        let mut args = self.typed_args_iter(inst);
        let args_array = std::array::from_fn(|_| args.next().expect("not enough args"));
        assert!(args.next().is_none(), "too many args");
        args_array
    }

    pub fn args_n_ignore_extra<'a, I: Inst + 'static, const N: usize>(
        &'a self,
        inst: &'a TypedInstruction<I>,
    ) -> [Argument<'a>; N] {
        let mut args = self.typed_args_iter(inst);
        std::array::from_fn(|_| args.next().expect("not enough args"))
    }

    pub fn args_mut<'a>(&'a mut self, r: Ref, env: &'a Environment) -> ArgIterMut<'a> {
        let inst = &mut self.insts[r.idx()];
        let func = &env[inst.function];
        let params = func.params();
        let param_count = params.iter().map(|p| p.slot_count()).sum();
        let (storage, vararg_count) = if param_count <= INLINE_ARGS && func.varargs.is_none() {
            (&mut inst.args[..param_count], 0)
        } else {
            let vararg_count = if func.varargs.is_some() {
                inst.args[1]
            } else {
                0
            };
            (
                &mut self.extra[inst.args[0] as usize..][..param_count
                    + vararg_count as usize * func.varargs.map_or(0, |p| p.slot_count())],
                vararg_count,
            )
        };
        ArgIterMut {
            params: params.iter(),
            storage: storage.iter_mut(),
            vararg_count,
            vararg_param: func.varargs.unwrap_or(Parameter::Ref),
        }
    }

    pub fn prepare_instruction<'a>(
        &mut self,
        params: &[Parameter],
        varargs: Option<Parameter>,
        block: BlockId,
        (id, args, ty): (FunctionId, impl IntoArgs<'a>, TypeId),
    ) -> Instruction {
        let args = builder::write_args(
            &mut self.extra,
            block,
            &mut self.blocks,
            params,
            varargs,
            args,
        );
        Instruction {
            function: id,
            args,
            ty,
        }
    }

    pub fn new_reg(&mut self, class: RegClass) -> MCReg {
        let r = self.mc_regs.len() as u32;
        self.mc_regs.push(class);
        MCReg::from_virt(r)
    }

    pub fn display<'a>(
        &'a self,
        env: &'a Environment,
        types: &'a Types,
    ) -> crate::display::BodyDisplay<'a> {
        crate::display::BodyDisplay {
            env,
            types,
            ir: self,
            _r: PhantomData,
        }
    }

    pub fn display_with_phys_regs<'a, R: Register>(
        &'a self,
        env: &'a Environment,
        types: &'a Types,
    ) -> crate::display::BodyDisplay<'a, R> {
        crate::display::BodyDisplay {
            env,
            types,
            ir: self,
            _r: PhantomData,
        }
    }

    pub fn mc_reg_count(&self) -> u32 {
        self.mc_regs.len() as u32
    }

    pub fn virtual_reg_class(&self, r: MCReg) -> RegClass {
        self.mc_regs[r.virt().unwrap() as usize]
    }

    pub fn mc_reg_classes(&self) -> &[RegClass] {
        &self.mc_regs
    }

    pub fn find_block_for_position(&self, i: u32) -> BlockId {
        let block_id = self
            .blocks
            .iter()
            .position(|info| (info.args_idx..info.body_idx + info.len).contains(&i))
            .unwrap_or_else(|| panic!("Couldn't find block for index {i}"));
        BlockId(block_id as _)
    }
}
impl block_graph::Blocks for FunctionIr {
    type Block = BlockId;
    type Env = Environment;

    fn block_count(&self) -> u32 {
        self.block_count()
    }

    fn successors<'a>(
        &'a self,
        _env: &'a Environment,
        block: BlockId,
    ) -> impl Iterator<Item = BlockId> + Clone + use<'a> {
        self.blocks[block.idx()].succs.iter().copied()
    }

    /*
    pub fn dominates(&self, ir: &FunctionIr, i: u32, dominating: Ref) -> bool {
        self.block_dominates(
            ir.get_block_from_index(i),
            ir.get_block_from_index(dominating_index),
        )
        todo!()
    }
    */
}

#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub arg_count: u32,
    pub args_idx: u32,
    pub body_idx: u32,
    pub len: u32,
    pub preds: Vec<BlockId>,
    pub succs: Vec<BlockId>,
}
impl BlockInfo {
    pub fn all_refs(&self) -> impl use<> + ExactSizeIterator<Item = Ref> {
        (self.args_idx..self.body_idx + self.len).map(Ref)
    }

    pub fn body(&self) -> impl use<> + ExactSizeIterator<Item = Ref> {
        let s = self.args_idx + self.arg_count;
        (s..s + self.len).map(Ref)
    }

    pub fn replace_pred(&mut self, pred: BlockId, replaced_with: BlockId) {
        let i = self.preds.iter().position(|&p| p == pred).unwrap();
        self.preds[i] = replaced_with;
    }

    pub fn replace_succ(&mut self, succ: BlockId, replaced_with: BlockId) {
        let i = self.succs.iter().position(|&s| s == succ).unwrap();
        self.succs[i] = replaced_with;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Instruction {
    function: FunctionId,
    args: [u32; INLINE_ARGS],
    ty: TypeId,
}
impl Instruction {
    pub const NOTHING: Self = Self {
        function: FunctionId {
            module: crate::ModuleId::BUILTINS,
            function: Builtin::Nothing.id(),
        },
        args: [0, 0],
        ty: TypeId::UNIT,
    };

    pub fn module(&self) -> ModuleId {
        self.function.module
    }

    pub fn function(&self) -> LocalFunctionId {
        self.function.function
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn args_inner<'a, 'args>(
        &'args self,
        params: &'a [Parameter],
        varargs: Option<Parameter>,
        blocks: &'a [BlockInfo],
        extra: &'args [u32],
    ) -> ArgsIter<'a, 'args> {
        decode_args(&self.args, params, varargs, blocks, extra)
    }

    pub fn as_module<I: Inst>(&self, m: ModuleOf<I>) -> Option<TypedInstruction<I>> {
        (self.module() == m.0).then(|| TypedInstruction {
            inst: self.function.function.try_into().unwrap(),
            args: self.args,
            ty: self.ty,
        })
    }
}

pub enum ArgumentMut<'a> {
    Ref(&'a mut Ref),
    BlockId(&'a mut BlockId),
    /// can't be visited mutably for now due to how it is encoded
    BlockTarget,
    Int {
        lo: &'a mut u32,
        hi: &'a mut u32,
    },
    Int32(&'a mut u32),
    Float {
        lo: &'a mut u32,
        hi: &'a mut u32,
    },
    TypeId(&'a mut TypeId),
    FunctionId {
        module: &'a mut ModuleId,
        function: &'a mut LocalFunctionId,
    },
    GlobalId {
        module: &'a mut ModuleId,
        idx: &'a mut u32,
    },
    MCReg(Usage, &'a mut MCReg),
}

/// how many arguments are stored inline with each instruction.
pub const INLINE_ARGS: usize = 2;

#[derive(Clone)]
pub struct ArgsIter<'a, 'args> {
    inner: iter::Chain<iter::Copied<std::slice::Iter<'a, Parameter>>, iter::RepeatN<Parameter>>,
    args: iter::Copied<std::slice::Iter<'args, u32>>,
    blocks: &'a [BlockInfo],
    extra: &'args [u32],
}
impl<'a, 'args> Iterator for ArgsIter<'a, 'args> {
    type Item = Argument<'args>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut arg = || self.args.next().unwrap();

        self.inner.next().map(|param| match param {
            Parameter::Ref | Parameter::RefOf(_) => Argument::Ref(Ref(arg())),
            Parameter::BlockId => Argument::BlockId(BlockId(arg())),
            Parameter::BlockTarget => {
                let id = BlockId(arg());
                let arg_idx = arg();
                let arg_count = self.blocks[id.idx()].arg_count;
                let args: &[u32] = &self.extra[arg_idx as usize..(arg_idx + arg_count) as usize];
                // SAFETY: Ref is repr(transparent)
                let args: &[Ref] = unsafe { std::mem::transmute(args) };
                Argument::BlockTarget(BlockTarget(id, Cow::Borrowed(args)))
            }
            Parameter::Int => Argument::Int(u64::from(arg()) | (u64::from(arg()) << 32)),
            Parameter::Float => {
                Argument::Float(f64::from_bits(u64::from(arg()) | (u64::from(arg()) << 32)))
            }
            Parameter::Int32 => Argument::Int(arg().into()),
            Parameter::TypeId => Argument::TypeId(TypeId(arg())),
            Parameter::FunctionId => Argument::FunctionId(FunctionId {
                module: ModuleId(arg()),
                function: LocalFunctionId(arg()),
            }),
            Parameter::GlobalId => Argument::GlobalId(GlobalId {
                module: ModuleId(arg()),
                idx: arg(),
            }),
            Parameter::MCReg(_) => Argument::MCReg(MCReg(arg())),
        })
    }
}

pub fn update_inst_refs(
    inst: &mut Instruction,
    env: &Environment,
    extra: &mut [u32],
    blocks: &[BlockInfo],
    mut update: impl FnMut(Ref) -> Ref,
    mut update_block: impl FnMut(BlockId) -> BlockId,
) {
    let func = &env[inst.function];
    let slot_count: usize = func.params().iter().map(|p| p.slot_count()).sum();
    if slot_count > INLINE_ARGS || func.varargs().is_some() {
        let mut idx = inst.args[0] as usize;
        let mut visit_param = |param| match param {
            Parameter::Ref | Parameter::RefOf(_) => {
                extra[idx] = update(Ref(extra[idx])).0;
                idx += 1;
            }
            Parameter::BlockId => {
                extra[idx] = update_block(BlockId(extra[idx])).0;
                idx += 1;
            }
            Parameter::BlockTarget => {
                let target = update_block(BlockId(extra[idx]));
                extra[idx] = target.0;
                let arg_idx = extra[idx + 1];
                idx += 2;
                let arg_count = blocks[target.idx()].arg_count;
                for i in arg_idx..arg_idx + arg_count {
                    extra[i as usize] = update(Ref(extra[i as usize])).0;
                }
            }
            Parameter::Int
            | Parameter::Float
            | Parameter::Int32
            | Parameter::TypeId
            | Parameter::FunctionId
            | Parameter::GlobalId
            | Parameter::MCReg(_) => idx += param.slot_count(),
        };
        for &param in func.params() {
            visit_param(param);
        }
        if let Some(param) = func.varargs {
            for _ in 0..inst.args[1] {
                visit_param(param);
            }
        }
    } else {
        let mut idx = 0;
        for param in func.params() {
            match param {
                Parameter::Ref | Parameter::RefOf(_) => {
                    inst.args[idx] = update(Ref(inst.args[idx])).0;
                    idx += 1;
                }
                Parameter::BlockId => {
                    extra[idx] = update_block(BlockId(extra[idx])).0;
                    idx += 1;
                }
                Parameter::BlockTarget => {
                    let target = update_block(BlockId(inst.args[idx]));
                    let args_idx = inst.args[idx + 1];
                    idx += 2;
                    let arg_count = blocks[target.idx()].arg_count;
                    for i in args_idx..args_idx + arg_count {
                        extra[i as usize] = update(Ref(extra[i as usize])).0;
                    }
                }
                Parameter::Int
                | Parameter::Float
                | Parameter::Int32
                | Parameter::TypeId
                | Parameter::FunctionId
                | Parameter::GlobalId
                | Parameter::MCReg(_) => idx += param.slot_count(),
            }
        }
    }
}

fn decode_args<'a, 'args>(
    args: &'args [u32; INLINE_ARGS],
    params: &'a [Parameter],
    varargs: Option<Parameter>,
    blocks: &'a [BlockInfo],
    extra: &'args [u32],
) -> ArgsIter<'a, 'args> {
    let mut count: usize = params.iter().map(|p| p.slot_count()).sum();
    let mut vararg_count = 0;
    let args = if count <= INLINE_ARGS && varargs.is_none() {
        &args[..count]
    } else {
        let i = args[0] as usize;
        if let Some(param) = varargs {
            vararg_count = args[1] as usize;
            count += vararg_count * param.slot_count();
        }
        &extra[i..i + count]
    }
    .iter()
    .copied();

    if vararg_count != 0 {
        debug_assert!(
            varargs.is_some(),
            "Can't have varargs in function without varargs"
        );
    }

    ArgsIter {
        inner: params.iter().copied().chain(std::iter::repeat_n(
            varargs.unwrap_or(Parameter::Ref),
            vararg_count,
        )),
        args,
        blocks,
        extra,
    }
}

pub struct TypedInstruction<I: Inst> {
    pub inst: I,
    args: [u32; 2],
    ty: TypeId,
}
impl<I: Inst> TypedInstruction<I> {
    pub fn op(&self) -> I {
        self.inst
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn args_iter<'a, 'args>(
        &'args self,
        blocks: &'a [BlockInfo],
        extra: &'args [u32],
    ) -> ArgsIter<'a, 'args> {
        let params = self.inst.params();
        let varargs = self.inst.varargs();
        decode_args(&self.args, params, varargs, blocks, extra)
    }
}

pub trait Inst: TryFrom<LocalFunctionId, Error = InvalidInstruction> + Copy + 'static {
    const MODULE_NAME: &'static str;

    type InstTable: 'static;

    fn functions() -> Vec<Function>;
    fn inst_table(module: &ModuleOf<Self>) -> &Self::InstTable;
    fn id(self) -> LocalFunctionId;
    fn name(self) -> &'static str;
    fn params(self) -> &'static [Parameter];
    fn varargs(self) -> Option<Parameter>;
}

pub struct ArgIterMut<'a> {
    params: std::slice::Iter<'a, Parameter>,
    storage: std::slice::IterMut<'a, u32>,
    vararg_count: u32,
    vararg_param: Parameter,
}
impl ArgIterMut<'_> {
    fn next_param(&mut self) -> Option<Parameter> {
        self.params.next().copied().or_else(|| {
            self.vararg_count.checked_sub(1).map(|new_count| {
                self.vararg_count = new_count;
                self.vararg_param
            })
        })
    }

    fn next_param_back(&mut self) -> Option<Parameter> {
        self.vararg_count
            .checked_sub(1)
            .map(|new_count| {
                self.vararg_count = new_count;
                self.vararg_param
            })
            .or_else(|| self.params.next_back().copied())
    }
}
impl<'a> Iterator for ArgIterMut<'a> {
    type Item = ArgumentMut<'a>;

    #[allow(clippy::missing_transmute_annotations)]
    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.next_param()? {
            Parameter::Ref | Parameter::RefOf(_) => {
                ArgumentMut::Ref(unsafe { transmute(self.storage.next().unwrap()) })
            }
            Parameter::BlockId => {
                ArgumentMut::BlockId(unsafe { transmute(self.storage.next().unwrap()) })
            }
            Parameter::BlockTarget => ArgumentMut::BlockTarget,
            Parameter::Int => ArgumentMut::Int {
                lo: self.storage.next().unwrap(),
                hi: self.storage.next().unwrap(),
            },
            Parameter::Int32 => ArgumentMut::Int32(self.storage.next().unwrap()),
            Parameter::Float => ArgumentMut::Float {
                lo: self.storage.next().unwrap(),
                hi: self.storage.next().unwrap(),
            },
            Parameter::TypeId => {
                ArgumentMut::TypeId(unsafe { transmute(self.storage.next().unwrap()) })
            }
            Parameter::FunctionId => ArgumentMut::FunctionId {
                module: unsafe { transmute(self.storage.next().unwrap()) },
                function: unsafe { transmute(self.storage.next().unwrap()) },
            },
            Parameter::GlobalId => ArgumentMut::GlobalId {
                module: unsafe { transmute(self.storage.next().unwrap()) },
                idx: self.storage.next().unwrap(),
            },
            Parameter::MCReg(usage) => {
                ArgumentMut::MCReg(usage, unsafe { transmute(self.storage.next().unwrap()) })
            }
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.params.len(), Some(self.params.len()))
    }
}
impl DoubleEndedIterator for ArgIterMut<'_> {
    #[allow(clippy::missing_transmute_annotations)]
    fn next_back(&mut self) -> Option<Self::Item> {
        Some(match self.next_param_back()? {
            Parameter::Ref | Parameter::RefOf(_) => {
                ArgumentMut::Ref(unsafe { transmute(self.storage.next_back().unwrap()) })
            }
            Parameter::BlockId => {
                ArgumentMut::BlockId(unsafe { transmute(self.storage.next_back().unwrap()) })
            }
            Parameter::BlockTarget => ArgumentMut::BlockTarget,
            Parameter::Int => ArgumentMut::Int {
                hi: self.storage.next_back().unwrap(),
                lo: self.storage.next_back().unwrap(),
            },
            Parameter::Int32 => ArgumentMut::Int32(self.storage.next_back().unwrap()),
            Parameter::Float => ArgumentMut::Float {
                hi: self.storage.next_back().unwrap(),
                lo: self.storage.next_back().unwrap(),
            },
            Parameter::TypeId => {
                ArgumentMut::TypeId(unsafe { transmute(self.storage.next_back().unwrap()) })
            }
            Parameter::FunctionId => ArgumentMut::FunctionId {
                function: unsafe { transmute(self.storage.next_back().unwrap()) },
                module: unsafe { transmute(self.storage.next_back().unwrap()) },
            },
            Parameter::GlobalId => ArgumentMut::GlobalId {
                idx: self.storage.next_back().unwrap(),
                module: unsafe { transmute(self.storage.next_back().unwrap()) },
            },
            Parameter::MCReg(usage) => ArgumentMut::MCReg(usage, unsafe {
                transmute(self.storage.next_back().unwrap())
            }),
        })
    }
}

#[macro_export]
macro_rules! primitives {
    ($($primitive: ident = $size: literal)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, $crate::__FromRepr)]
        pub enum Primitive {
            $($primitive,)*
        }
        impl Primitive {
            pub fn id(self) -> $crate::PrimitiveId {
                $crate::PrimitiveId(self as u32)
            }

            pub fn create_infos() -> ::std::vec::Vec<$crate::PrimitiveInfo> {
                ::std::vec![
                    $($crate::PrimitiveInfo {
                        name: stringify!($primitive).into(),
                        size: $size,
                        align: ::core::num::NonZeroU64::new($size.max(1)).unwrap(),
                    }),*
                ]
            }
        }
        impl From<Primitive> for $crate::Type {
            fn from(value: Primitive) -> Self {
                Self::Primitive(value.id())
            }
        }
        impl From<Primitive> for $crate::PrimitiveId {
            fn from(value: Primitive) -> Self {
                value.id()
            }
        }
        impl TryFrom<$crate::PrimitiveId> for Primitive {
            type Error = $crate::InvalidPrimitive;

            fn try_from(value: $crate::PrimitiveId) -> ::core::result::Result<Self, Self::Error> {
                Self::from_repr(value.0 as usize).ok_or($crate::InvalidPrimitive)
            }
        }
        impl core::str::FromStr for Primitive {
            type Err = $crate::InvalidPrimitive;
            fn from_str(s: &core::primitive::str) -> ::core::result::Result<Self, Self::Err> {
                match s {
                    $(s if s == stringify!($primitive) => Ok(Self::$primitive),)*
                    _ => Err($crate::InvalidPrimitive),
                }
            }
        }
    };
}

pub mod parameter_types {
    pub use super::{BlockId, BlockTarget, FunctionId, GlobalId, MCReg, Ref, TypeId};
    pub type Int = u64;
    pub type Float = f64;
    pub type Int32 = u32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parameter {
    Ref,
    RefOf(TypeId),
    BlockId,
    BlockTarget,
    Int,
    Int32,
    Float,
    TypeId,
    FunctionId,
    GlobalId,
    MCReg(Usage),
}
impl Parameter {
    pub fn slot_count(self) -> usize {
        match self {
            Parameter::Ref
            | Parameter::RefOf(_)
            | Parameter::BlockId
            | Parameter::TypeId
            | Parameter::Int32
            | Parameter::MCReg(_) => 1,
            Parameter::Int
            | Parameter::Float
            | Parameter::BlockTarget
            | Parameter::FunctionId
            | Parameter::GlobalId => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Usage {
    Def,
    Use,
    DefUse,
}
impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Def => write!(f, "def"),
            Self::Use => write!(f, "use"),
            Self::DefUse => write!(f, "def-use"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InvalidInstruction;

#[derive(Debug, Clone, Copy)]
pub struct InvalidPrimitive;

use mc::Register;
use segment_list::SegmentList;
#[doc(hidden)]
pub use strum::FromRepr as __FromRepr;

use crate::{flags::InstFlags, mc::RegClass};

#[macro_export]
macro_rules! lifetime_or_static {
    ($lifetime: lifetime) => {
        impl $crate::IntoArgs<$lifetime>
    };
    () => {
        impl $crate::IntoArgs<'static>
    };
}

#[derive(Default)]
pub struct InstAttrs {
    pub flags: InstFlags,
    pub varargs: Option<Parameter>,
}
impl InstAttrs {
    pub fn varargs(&mut self, varargs: Option<Parameter>) {
        self.varargs = varargs;
    }

    pub fn terminator(&mut self, value: bool) {
        self.flags.set(InstFlags::TERMINATOR, value);
    }

    pub fn pure(&mut self, value: bool) {
        self.flags.set(InstFlags::PURE, value);
    }
}

#[macro_export]
macro_rules! instruction_attrs {
    ($attrs: ident) => {};
    ($attrs: ident !$attr: ident = $value: expr; $($rest: tt)*) => {
        $attrs.$attr($value);
        $crate::instruction_attrs!($attrs $($rest)*);
    };
    ($attrs: ident !$attr: ident $($rest: tt)*) => {
        $attrs.$attr(true);
        $crate::instruction_attrs!($attrs $($rest)*);
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! instruction {
    (
        $table_name: ident, $module_name: ident,
        $(
            #[doc = $doc: literal]
        )*
        $instruction: ident $(<$inst_life: lifetime>)?
        $(
            $arg_name: ident: $arg: ident
            $(<$life: lifetime>)?
            $(($arg_param: expr))?
        )*
        $(!$attr: ident $(= $attr_value: expr)?),*
        ;
    ) => {
        #[allow(non_snake_case)]
        impl $table_name<$crate::ModuleOf<$module_name>> {
            #[inline]

            $(#[doc = $doc])*
            pub fn $instruction<$($inst_life)*>(self, $($arg_name: $crate::parameter_types::$arg $(<$life>)?,)* ty: $crate::TypeId) -> ($crate::FunctionId, $crate::lifetime_or_static!($($inst_life)*), $crate::TypeId)
            where
                $('a: $inst_life)*
            {
                let id = $crate::FunctionId {
                    module: self.0.id(),
                    function: $module_name::$instruction.into(),
                };
                let args = ($($arg_name,)*);
                (id, args, ty)
            }
        }
    };
    (
        $table_name: ident, $module_name: ident,
        $(
            #[doc = $doc: literal]
        )*
        $instruction: ident $(<$inst_life: lifetime>)?
        $(
            $arg_name: ident: $arg: ident
            $(<$life: lifetime>)?
            $(($arg_param: expr))?
        )*
        $(!$attr: ident $(= $attr_value: expr)?),*
        => unit;
    ) => {
        #[allow(non_snake_case)]
        impl $table_name<$crate::ModuleOf<$module_name>> {
            #[inline]

            $(#[doc = $doc])*
            pub fn $instruction<$($inst_life)*>(self, $($arg_name: $crate::parameter_types::$arg $(<$life>)?,)*) -> ($crate::FunctionId, $crate::lifetime_or_static!($($inst_life)*), $crate::TypeId)
            where
                $('a: $inst_life)*
            {
                let id = $crate::FunctionId {
                    module: self.0.id(),
                    function: $module_name::$instruction.into(),
                };
                let args = ($($arg_name,)*);
                (id, args, $crate::TypeId::UNIT)
            }
        }
    };
}

#[macro_export]
macro_rules! filter_varargs_param {
    (varargs = $value: expr $(, $($t: tt)*)?) => {
        $value
    };
    ($a: ident $(= $value: expr)? $(, $($t: tt)* )?) => {
        $crate::filter_varargs_param!($($($t)*)*)
    };
    () => { ::core::option::Option::None };
}

#[macro_export]
#[doc(hidden)]
macro_rules! instructions_inner {
    (
        $table_name: ident, $module_name: ident,
        ! $all_return: ident
        $(
            $(
                #[doc = $doc: literal]
            )*
            $instruction: ident $(<$inst_life: lifetime>)?
            $(
                $arg_name: ident: $arg: ident
                $(<$life: lifetime>)?
                $(($arg_param: expr))?
            )*
            $(!$attr: ident $(= $attr_value: expr)?),*
            $(=> $return: ident)?
            ;
        )*
    ) => {
        $(
            $crate::instruction!{
                $table_name, $module_name,
                $(#[doc = $doc])*
                $instruction $(<$inst_life>)?
                $(
                  $arg_name: $arg $(<$life>)?
                  $(($arg_param))?
                )*
                $(!$attr $(= $attr_value)?),*
                => $all_return
                ;
            }
        )*
    };
    (
        $table_name: ident, $module_name: ident,
        $(
            $(
                #[doc = $doc: literal]
            )*
            $instruction: ident $(<$inst_life: lifetime>)?
            $(
                $arg_name: ident: $arg: ident
                $(<$life: lifetime>)?
                $(($arg_param: expr))?
            )*
            $(!$attr: ident $(= $attr_value: expr)?),*
            $(=> $return: ident)?
            ;
        )*
    ) => {
        $(
            $crate::instruction!{
                $table_name, $module_name,
                $(#[doc = $doc])*
                $instruction $(<$inst_life>)?
                $(
                  $arg_name: $arg $(<$life>)?
                  $(($arg_param))?
                )*
                $(!$attr $(= $attr_value)?),*
                $(=> $return)?
                ;
            }
        )*
    };
}

#[macro_export]
macro_rules! instructions {
    (
        $module_name: ident
        $name: literal
        $table_name: ident
        $(!all_return $all_return: ident)?
        $(
            $(
                #[doc = $doc: literal]
            )*
            $instruction: ident $(<$inst_life: lifetime>)?
            $(
                $arg_name: ident: $arg: ident
                $(<$life: lifetime>)?
                $(($arg_param: expr))?
            )*
            $(!$attr: ident $(= $attr_value: expr)?),*
            $(=> $return: ident)?
            ;
        )*
    ) => {
        #[derive(Debug, Clone, Copy, $crate::__FromRepr, PartialEq, Eq, Hash)]
        #[allow(non_camel_case_types)]
        pub enum $module_name {
            $( $(#[doc = $doc])* $instruction,)*
        }

        #[repr(transparent)]
        #[derive(Debug, Clone, Copy)]
        pub struct $table_name<T>(T);

        $crate::instructions_inner!{
            $table_name, $module_name,
            $(! $all_return)?
            $(
                $(
                    #[doc = $doc]
                )*
                $instruction $(<$inst_life>)?
                $(
                    $arg_name : $arg
                    $(<$life>)?
                    $(($arg_param))?
                )*
                $(!$attr $(= $attr_value)?),*
                $(=> $return)?
                ;
            )*
        }
        impl $module_name {
            pub const fn name(self) -> &'static ::core::primitive::str {
                match self {
                    $(
                        Self::$instruction => stringify!(Self::$instruction),
                    )*
                }
            }
        }
        impl $crate::Inst for $module_name {
            const MODULE_NAME: &'static ::core::primitive::str = $name;

            type InstTable = $table_name<$crate::ModuleOf<Self>>;

            fn functions() -> ::std::vec::Vec<$crate::Function> {
                ::std::vec![
                    $(
                        {
                            #[allow(unused_mut)]
                            let mut attrs = $crate::InstAttrs::default();
                            $crate::instruction_attrs!(attrs $(!$attr $(= $attr_value;)*)*);
                            $crate::Function::declare_inst(
                                 stringify!($instruction),
                                 $crate::Types::new(),
                                 vec![ $( $crate::Parameter::$arg $(($arg_param))*, )* ],
                                 attrs,
                            )
                        },
                    )*
                ]
            }

            fn params(self) -> &'static [$crate::Parameter] {
                match self {
                    $(
                        Self::$instruction => &[$( $crate::Parameter::$arg $(($arg_param))*, )*],
                    )*
                }
            }

            fn id(self) -> $crate::LocalFunctionId {
                $crate::LocalFunctionId(self as u32)
            }

            fn name(self) -> &'static ::core::primitive::str {
                self.name()
            }

            fn varargs(self) -> Option<$crate::Parameter> {
                match self {
                    $(
                        Self::$instruction => $crate::filter_varargs_param!(
                            $($attr $(= $attr_value)*),*
                        ),
                    )*
                }
            }

            fn inst_table(module: &$crate::ModuleOf<Self>) -> &Self::InstTable {
                unsafe { ::core::mem::transmute(module) }
            }

        }
        impl $module_name {
            pub const fn id(self) -> $crate::LocalFunctionId {
                $crate::LocalFunctionId(self as u32)
            }

        }
        impl From<$module_name> for $crate::LocalFunctionId {
            fn from(value: $module_name) -> Self {
                Self(value as u32)
            }
        }
        impl ::core::convert::TryFrom<$crate::LocalFunctionId> for $module_name {
            type Error = $crate::InvalidInstruction;

            fn try_from(value: $crate::LocalFunctionId) -> ::core::result::Result<Self, Self::Error> {
                Self::from_repr(value.0 as usize).ok_or($crate::InvalidInstruction)
            }
        }

    };
}
