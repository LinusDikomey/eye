use core::{convert::Into, iter::Iterator};
use std::{
    cell::RefCell,
    fmt,
    hash::{Hash, Hasher},
};

use color_format::cwrite;
use dmap::DHashMap;
use fxhash::FxHasher;
use hashbrown::HashTable;
use parser::ast::{self, FunctionId, ModuleId};
use segment_list::SegmentList;

use crate::{
    Type,
    compiler::{Generics, Resolvable, ResolvableTypeDef, ResolvedTypeDef},
    types::{BaseType, BuiltinType, TypeFull},
};

pub struct Types {
    tags: SegmentList<Tag>,
    indices: SegmentList<u32>,
    map: RefCell<HashTable<Type>>,

    instances: SegmentList<(u32, Box<[Type]>)>,
    bases: SegmentList<ResolvableTypeDef>,
    function_items: SegmentList<((ModuleId, FunctionId), Box<[Type]>)>,
    tuples: SegmentList<(Box<[Type]>, Box<[(Box<str>, Type)]>)>,
    consts: SegmentList<u64>,
}
impl Types {
    pub fn new() -> Self {
        let tags = SegmentList::new();
        let indices = SegmentList::new();
        let mut map = HashTable::new();

        let instances = SegmentList::new();
        let bases = SegmentList::new();
        let function_items = SegmentList::new();
        let tuples: SegmentList<(Box<[_]>, Box<[_]>)> = SegmentList::new();
        let consts = SegmentList::new();

        for (builtin, i) in BuiltinType::VARIANTS.into_iter().zip(0..) {
            let module = ModuleId::MISSING;
            let generics = builtin.generics();
            bases.add(ResolvableTypeDef {
                module,
                id: ast::TypeId::from_inner(0),
                name: builtin.name().into(),
                generic_count: generics.count(),
                resolved: crate::compiler::Resolvable::resolved(ResolvedTypeDef {
                    def: crate::compiler::ResolvedTypeContent::Builtin(builtin),
                    module,
                    methods: DHashMap::default(),
                    generics: Generics::EMPTY,
                    inherent_trait_impls: DHashMap::default(),
                }),
            });
            if generics.count() == 0 {
                let value = if builtin == BuiltinType::Unit {
                    let tuple = tuples.add((Box::new([]), Box::new([])));
                    let j = tags.add(Tag::Tuple);
                    let k = indices.add(tuple);
                    debug_assert_eq!(i, j);
                    debug_assert_eq!(j, k);
                    TypeFull::Tuple {
                        members: &[],
                        named_members: &[],
                    }
                } else {
                    let instance = instances.add((i, Box::new([]) as _));
                    let j = tags.add(Tag::Instance);
                    let k = indices.add(instance);
                    debug_assert_eq!(i, j);
                    debug_assert_eq!(j, k);
                    let base = BaseType(i);
                    TypeFull::Instance(base, &[])
                };
                let hash = hash_full(&value);
                map.insert_unique(hash, Type(i), |&ty| {
                    hash_full(&Self::lookup_type(
                        &tags,
                        &indices,
                        &instances,
                        &function_items,
                        &tuples,
                        &consts,
                        ty,
                    ))
                });
            }
        }

        Self {
            tags,
            indices,
            map: RefCell::new(map),

            instances,
            bases,
            function_items,
            tuples,
            consts,
        }
    }

    pub fn intern(&self, ty: TypeFull) -> Type {
        let hash = hash_full(&ty);
        let mut map = self.map.borrow_mut();
        map.find(hash, |&existing| {
            let existing_ty = self.lookup(existing);
            existing_ty == ty
        })
        .copied()
        .unwrap_or_else(|| {
            let (interned, idx) = match ty {
                TypeFull::Instance(base, generics) => {
                    (Tag::Instance, self.instances.add((base.0, generics.into())))
                }
                TypeFull::FunctionItem { function, generics } => (
                    Tag::FunctionItem,
                    self.function_items.add((function, generics.into())),
                ),
                TypeFull::Tuple {
                    members,
                    named_members,
                } => (
                    Tag::Tuple,
                    self.tuples.add((members.into(), named_members.into())),
                ),
                TypeFull::Generic(i) => (Tag::Generic, i as u32),
                TypeFull::Const(value) => (Tag::Const, self.consts.add(value)),
            };
            let ty = Type(self.tags.add(interned));
            let idx_idx = self.indices.add(idx);
            debug_assert_eq!(ty.0, idx_idx);
            map.insert_unique(hash, ty, |&ty| {
                hash_full(&Self::lookup_type(
                    &self.tags,
                    &self.indices,
                    &self.instances,
                    &self.function_items,
                    &self.tuples,
                    &self.consts,
                    ty,
                ))
            });
            ty
        })
    }

    pub fn lookup<'a>(&'a self, ty: Type) -> TypeFull<'a> {
        Self::lookup_type(
            &self.tags,
            &self.indices,
            &self.instances,
            &self.function_items,
            &self.tuples,
            &self.consts,
            ty,
        )
    }

    fn lookup_type<'a>(
        tags: &'a SegmentList<Tag>,
        indices: &'a SegmentList<u32>,
        instances: &'a SegmentList<(u32, Box<[Type]>)>,
        function_items: &'a SegmentList<((ModuleId, FunctionId), Box<[Type]>)>,
        tuples: &'a SegmentList<(Box<[Type]>, Box<[(Box<str>, Type)]>)>,
        consts: &'a SegmentList<u64>,
        ty: Type,
    ) -> TypeFull<'a> {
        let idx = *indices.get(ty.0);
        match tags.get(ty.0) {
            Tag::Instance => {
                let (base, generics) = instances.get(idx);
                TypeFull::Instance(BaseType(*base), generics)
            }
            Tag::FunctionItem => {
                let (function, ref generics) = *function_items.get(idx);
                TypeFull::FunctionItem { function, generics }
            }
            Tag::Tuple => {
                let (members, named_members) = tuples.get(idx);
                TypeFull::Tuple {
                    members,
                    named_members,
                }
            }
            Tag::Generic => TypeFull::Generic(idx as u8),
            Tag::Const => TypeFull::Const(*consts.get(idx)),
        }
    }

    pub fn add_base(
        &self,
        module: ModuleId,
        id: ast::TypeId,
        name: Box<str>,
        generic_count: u8,
    ) -> BaseType {
        BaseType(self.bases.add(ResolvableTypeDef {
            generic_count,
            module,
            id,
            name,
            resolved: Resolvable::new(),
        }))
    }

    pub fn add_resolved_base(
        &self,
        module: ModuleId,
        id: ast::TypeId,
        name: Box<str>,
        generic_count: u8,
        resolved: ResolvedTypeDef,
    ) -> BaseType {
        BaseType(self.bases.add(ResolvableTypeDef {
            generic_count,
            module,
            id,
            name,
            resolved: Resolvable::resolved(resolved),
        }))
    }

    pub fn get_base(&self, ty: BaseType) -> &ResolvableTypeDef {
        self.bases.get(ty.0)
    }

    pub fn instantiate(&self, ty: Type, generics: &[Type]) -> Type {
        // fast path if there are no generics
        if generics.is_empty() {
            return ty;
        }
        // PERF: temporary Vec for storage of allocated types
        // PERF: could save an allocation by not calling intern() which takes a reference to generics
        // because we have an owned type already
        match self.lookup(ty) {
            TypeFull::Instance(base, instance_generics) => {
                if instance_generics.is_empty() {
                    return ty;
                }
                let instance_generics: Box<[Type]> = instance_generics.into();
                let instance_generics: Box<[Type]> = instance_generics
                    .into_iter()
                    .map(|ty| self.instantiate(ty, generics))
                    .collect();
                self.intern(TypeFull::Instance(base, &instance_generics))
            }
            TypeFull::FunctionItem {
                function,
                generics: item_generics,
            } => {
                if item_generics.is_empty() {
                    return ty;
                }
                let item_generics: Box<[Type]> = item_generics.into();
                let item_generics: Box<[Type]> = item_generics
                    .into_iter()
                    .map(|ty| self.instantiate(ty, generics))
                    .collect();
                self.intern(TypeFull::FunctionItem {
                    function,
                    generics: &item_generics,
                })
            }
            TypeFull::Tuple {
                members,
                named_members,
            } => self.intern(TypeFull::Tuple {
                members: &members
                    .iter()
                    .map(|&ty| self.instantiate(ty, generics))
                    .collect::<Box<[Type]>>(),
                named_members: &named_members
                    .iter()
                    .map(|(name, ty)| (name.clone(), self.instantiate(*ty, generics)))
                    .collect::<Box<[(Box<str>, Type)]>>(),
            }),
            TypeFull::Generic(i) => generics[usize::from(i)],
            TypeFull::Const(_) => ty,
        }
    }

    pub fn display<'a>(&'a self, ty: Type, generics: &'a Generics) -> TypeDisplay<'a> {
        TypeDisplay {
            types: self,
            generics,
            ty,
        }
    }
}

pub struct TypeDisplay<'a> {
    types: &'a Types,
    generics: &'a Generics,
    ty: Type,
}
impl<'a> fmt::Display for TypeDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.types.lookup(self.ty) {
            TypeFull::Instance(base, generics) => match base {
                BaseType::Invalid => write!(f, "<invalid>"),
                BaseType::Array => write!(
                    f,
                    "[{}; {}]",
                    self.types.display(generics[0], self.generics),
                    self.types.display(generics[1], self.generics)
                ),
                BaseType::Pointer => {
                    write!(f, "*{}", self.types.display(generics[0], self.generics))
                }
                BaseType::Function => {
                    write!(f, "fn(")?;
                    let mut first = true;
                    for &arg in &generics[1..] {
                        if !first {
                            write!(f, ", ")?;
                        }
                        first = false;
                        write!(f, "{}", self.types.display(arg, self.generics))?;
                    }
                    write!(f, ") -> {}", self.types.display(generics[0], self.generics))
                }
                _ => {
                    let name = &self.types.get_base(base).name;
                    cwrite!(f, "#r<{name}>")?;
                    self.write_generics(f, generics)
                }
            },
            TypeFull::FunctionItem { function, generics } => {
                // TODO: display function items differently than just their id, maybe by the
                // showing the function's mangled name
                cwrite!(
                    f,
                    "#m<fn item>{}:{}",
                    function.0.into_inner(),
                    function.1.into_inner()
                )?;
                self.write_generics(f, generics)
            }
            TypeFull::Tuple {
                members,
                named_members,
            } => {
                cwrite!(f, "(")?;
                let mut first = true;
                for &member in members {
                    if first {
                        first = false;
                    } else {
                        cwrite!(f, ", ")?;
                    }
                    cwrite!(f, "{}", self.types.display(member, self.generics))?;
                }
                for (name, member) in named_members {
                    if first {
                        first = false;
                    } else {
                        cwrite!(f, ", ")?;
                    }
                    cwrite!(f, "{name}: {}", self.types.display(*member, self.generics))?;
                }
                if members.len() == 1 && named_members.is_empty() {
                    cwrite!(f, ",")?;
                }
                cwrite!(f, ")")
            }
            TypeFull::Generic(i) => write!(f, "{}", self.generics.get_name(i)),
            TypeFull::Const(n) => write!(f, "{n}"),
        }
    }
}
impl<'a> TypeDisplay<'a> {
    fn write_generics(&self, f: &mut fmt::Formatter<'_>, generics: &[Type]) -> fmt::Result {
        if generics.is_empty() {
            return Ok(());
        }
        write!(f, "[")?;
        let mut first = true;
        for &generic in generics {
            if !first {
                write!(f, ", ")?;
            }
            first = false;
            write!(
                f,
                "{}",
                TypeDisplay {
                    types: self.types,
                    generics: self.generics,
                    ty: generic
                }
            )?;
        }
        write!(f, "]")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Tag {
    #[default]
    Instance,
    FunctionItem,
    Tuple,
    Generic,
    Const,
}

fn hash_full(full: &TypeFull) -> u64 {
    let mut hasher = FxHasher::default();
    Hash::hash(full, &mut hasher);
    hasher.finish()
}
