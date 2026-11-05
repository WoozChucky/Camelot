//! C types as merlin knows them, with their sizes for the target: RISC-V's
//! `ilp32` on RV32 and `lp64` on RV64 ([`Xlen`] says which). `char` is 1
//! byte, `short` 2, `int` 4 and `long long` 8 on both; `long` and pointers
//! are 4 bytes on RV32 and 8 on RV64, each aligned to its size (so a `long
//! long` is 8-aligned on RV32 too, as the psABI says). Plain `char` is unsigned,
//! as on every RISC-V target, so `(char)200` stays 200 and `char` loads use
//! `lbu`.
//!
//! The target is a parameter of everything that depends on it
//! ([`IntKind::size`], [`IntKind::range`], [`IntKind::wrap`], [`common`],
//! [`Type::size`], [`Type::align`], [`Record::complete`]) rather than a
//! global, so one process may compile for both widths at once (the tests
//! do). The parser records it in the [`crate::ast::Program`] it makes, and
//! sema, consteval and codegen read it from there.
//!
//! The integer rules follow C99 6.3.1: *integer promotion* turns `_Bool`,
//! `char` and `short` (signed or not) into `int`, since `int` holds all their
//! values, and the *usual arithmetic conversions* bring two operands to a
//! common type by rank and range ([`common`]). On ILP32, where `int` and
//! `long` have the same width, `unsigned int` and `long` meet at `unsigned
//! long`, since `long` cannot hold every `unsigned int`; on LP64 they meet
//! at `long`, which can.
//!
//! Structs and unions ([`Record`]) are laid out the way clang lays them out:
//! each member at the next offset aligned for its type, the whole aligned to
//! its most aligned member. An enum type is an integer type (`unsigned int`,
//! or `int` when a constant is negative, as clang and GCC choose), so it
//! needs no type of its own.
//!
//! Constant values are `i128`s, which hold every value of every integer type
//! (an LP64 `unsigned long` goes up to 2^64 - 1) as the number it is.
//!
//! `const` and `volatile` wrap a type in [`Type::Qual`]. Qualifiers of an
//! array apply to its elements (C99 6.7.3p8), so a `Qual` never wraps an
//! array, nor another `Qual`: [`Type::qualified`] keeps that form.

use crate::Pos;
pub use excalibur::Xlen;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

/// The integer types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntKind {
    /// `_Bool`: 1 byte holding 0 or 1. Converting anything to it gives
    /// `value != 0`.
    Bool,
    /// Plain `char`: a type of its own, unsigned on RISC-V.
    Char,
    SChar,
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Long,
    ULong,
    /// `long long`: 64 bits on both widths, as C99 requires (on RV32 a
    /// register pair, see [`crate::codegen`]).
    LongLong,
    ULongLong,
}

/// The size of a pointer, and of `long`, on the target: 4 bytes on RV32
/// (ILP32), 8 on RV64 (LP64).
pub fn ptr_size(x: Xlen) -> u32 {
    x.bits() / 8
}

impl IntKind {
    pub fn size(self, x: Xlen) -> u32 {
        match self {
            IntKind::Bool | IntKind::Char | IntKind::SChar | IntKind::UChar => 1,
            IntKind::Short | IntKind::UShort => 2,
            IntKind::Int | IntKind::UInt => 4,
            IntKind::Long | IntKind::ULong => ptr_size(x),
            IntKind::LongLong | IntKind::ULongLong => 8,
        }
    }

    /// C's integer conversion rank (C99 6.3.1.1): `_Bool` lowest, then
    /// `char`, `short`, `int`, `long` and `long long`, each type with its
    /// unsigned counterpart.
    fn rank(self) -> u32 {
        match self {
            IntKind::Bool => 0,
            IntKind::Char | IntKind::SChar | IntKind::UChar => 1,
            IntKind::Short | IntKind::UShort => 2,
            IntKind::Int | IntKind::UInt => 3,
            IntKind::Long | IntKind::ULong => 4,
            IntKind::LongLong | IntKind::ULongLong => 5,
        }
    }

    /// `size_t`, what `sizeof` gives: `unsigned int` on ILP32, `unsigned
    /// long` on LP64 (clang's `__SIZE_TYPE__`).
    pub fn size_t(x: Xlen) -> IntKind {
        match x {
            Xlen::Rv32 => IntKind::UInt,
            Xlen::Rv64 => IntKind::ULong,
        }
    }

    /// `ptrdiff_t`, the difference of two pointers: `int` on ILP32, `long`
    /// on LP64 (clang's `__PTRDIFF_TYPE__`).
    pub fn ptrdiff_t(x: Xlen) -> IntKind {
        match x {
            Xlen::Rv32 => IntKind::Int,
            Xlen::Rv64 => IntKind::Long,
        }
    }

    /// The unsigned type of the same rank (for `int` and wider).
    fn to_unsigned(self) -> IntKind {
        match self {
            IntKind::Int => IntKind::UInt,
            IntKind::Long => IntKind::ULong,
            IntKind::LongLong => IntKind::ULongLong,
            k => k,
        }
    }

    pub fn signed(self) -> bool {
        matches!(
            self,
            IntKind::SChar | IntKind::Short | IntKind::Int | IntKind::Long | IntKind::LongLong
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            IntKind::Bool => "_Bool",
            IntKind::Char => "char",
            IntKind::SChar => "signed char",
            IntKind::UChar => "unsigned char",
            IntKind::Short => "short",
            IntKind::UShort => "unsigned short",
            IntKind::Int => "int",
            IntKind::UInt => "unsigned int",
            IntKind::Long => "long",
            IntKind::ULong => "unsigned long",
            IntKind::LongLong => "long long",
            IntKind::ULongLong => "unsigned long long",
        }
    }

    /// Integer promotion: anything of lower rank than `int` (narrower, on
    /// every target) becomes `int`.
    pub fn promoted(self) -> IntKind {
        if self.rank() < IntKind::Int.rank() {
            IntKind::Int
        } else {
            self
        }
    }

    /// The smallest and largest value of the type.
    pub fn range(self, x: Xlen) -> (i128, i128) {
        if self == IntKind::Bool {
            return (0, 1);
        }
        let bits = self.size(x) * 8;
        if self.signed() {
            (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
        } else {
            (0, (1i128 << bits) - 1)
        }
    }

    /// `v` converted to this type: truncated to its width, then sign-extended
    /// if the type is signed (C's conversion rules, with the implementation-
    /// defined signed case done the two's complement way, as clang does).
    /// For `_Bool` it is `v != 0`.
    pub fn wrap(self, v: i128, x: Xlen) -> i128 {
        if self == IntKind::Bool {
            return (v != 0) as i128;
        }
        let bits = self.size(x) * 8;
        let v = v & ((1i128 << bits) - 1);
        if self.signed() && v >> (bits - 1) != 0 {
            v - (1i128 << bits)
        } else {
            v
        }
    }
}

/// The usual arithmetic conversions (C99 6.3.1.8): the type two integer
/// operands meet at, after promotion. Two signed or two unsigned types meet
/// at the one of greater rank. Otherwise the unsigned type wins if its rank
/// is not lower; else the signed type wins if it holds every value of the
/// unsigned one (`long` and `unsigned int` on LP64), and if it does not (the
/// same two on ILP32) they meet at the signed type's unsigned counterpart.
pub fn common(a: IntKind, b: IntKind, x: Xlen) -> IntKind {
    let (a, b) = (a.promoted(), b.promoted());
    if a.signed() == b.signed() {
        return if a.rank() >= b.rank() { a } else { b };
    }
    let (s, u) = if a.signed() { (a, b) } else { (b, a) };
    if u.rank() >= s.rank() {
        u
    } else if s.range(x).1 >= u.range(x).1 {
        s
    } else {
        s.to_unsigned()
    }
}

/// `const` and `volatile`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Quals {
    pub konst: bool,
    pub volatile: bool,
}

impl Quals {
    pub const NONE: Quals = Quals {
        konst: false,
        volatile: false,
    };
    pub const CONST: Quals = Quals {
        konst: true,
        volatile: false,
    };
    pub const VOLATILE: Quals = Quals {
        konst: false,
        volatile: true,
    };

    pub fn is_empty(self) -> bool {
        !self.konst && !self.volatile
    }

    pub fn union(self, other: Quals) -> Quals {
        Quals {
            konst: self.konst || other.konst,
            volatile: self.volatile || other.volatile,
        }
    }

    /// Whether every qualifier of `other` is in `self` too.
    pub fn contains(self, other: Quals) -> bool {
        (self.konst || !other.konst) && (self.volatile || !other.volatile)
    }
}

impl fmt::Display for Quals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match (self.konst, self.volatile) {
            (true, true) => "const volatile",
            (true, false) => "const",
            (false, true) => "volatile",
            (false, false) => "",
        })
    }
}

/// A struct or union type. Two records are the same type exactly when they
/// are the same object: each `struct tag { ... }` (or reference to a tag not
/// seen before) creates one, and every use of the tag shares it.
pub struct Record {
    pub union: bool,
    /// `None` for an anonymous `struct { ... }`.
    pub tag: Option<String>,
    /// Where the type was first declared.
    pub pos: Pos,
    /// The name of the typedef that names it, if it has no tag and is
    /// what a typedef declares (`typedef struct { ... } S;`): clang calls
    /// the type by it (`'S'`), C++'s "typedef name for linkage purposes".
    pub typedef_name: OnceLock<String>,
    /// The members and layout, once the definition has been seen. Until then
    /// the type is incomplete: pointers to it are fine, objects of it are not.
    body: OnceLock<RecordBody>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordBody {
    pub members: Vec<Member>,
    pub size: u32,
    pub align: u32,
    /// Where the definition is (its tag, or `struct` without one), which
    /// may be after where the type was first declared (for `-g`).
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub ty: Type,
    /// Bytes from the start of the struct (always 0 in a union).
    pub offset: u32,
    /// How its declaration spelled its type (for `-g`).
    pub spelled: Spelled,
    /// Where it is declared.
    pub pos: Pos,
}

impl Record {
    pub fn new(union: bool, tag: Option<String>, pos: Pos) -> Arc<Record> {
        Arc::new(Record {
            union,
            tag,
            pos,
            typedef_name: OnceLock::new(),
            body: OnceLock::new(),
        })
    }

    /// `struct` or `union`.
    pub fn keyword(&self) -> &'static str {
        if self.union {
            "union"
        } else {
            "struct"
        }
    }

    /// The members and layout, or `None` while the type is incomplete.
    pub fn body(&self) -> Option<&RecordBody> {
        self.body.get()
    }

    /// Lays out `members` (name and type, in order) the way clang does for
    /// the target and completes the type. Every member type must be complete.
    /// Returns `false` if the type was already complete.
    pub fn complete(&self, members: Vec<(String, Type)>, x: Xlen) -> bool {
        let pos = self.pos;
        let members = members
            .into_iter()
            .map(|(n, t)| (n, t, Spelled::default(), pos))
            .collect();
        self.complete_spelled(members, pos, x)
    }

    /// [`Record::complete`], with each member's spelling and position, and
    /// the definition's, which only `-g` reads.
    pub fn complete_spelled(&self, members: Vec<(String, Type, Spelled, Pos)>, pos: Pos, x: Xlen) -> bool {
        let mut offset = 0u64;
        let mut size = 0u64;
        let mut align = 1;
        let mut laid_out = Vec::with_capacity(members.len());
        for (name, ty, spelled, pos) in members {
            let (msize, malign) = (u64::from(ty.size(x).expect("complete member")), ty.align(x));
            align = align.max(malign);
            let at = if self.union { 0 } else { align_up(offset, malign) };
            offset = at + msize;
            size = size.max(offset);
            laid_out.push(Member {
                name,
                ty,
                offset: at as u32,
                spelled,
                pos,
            });
        }
        let size = align_up(size, align);
        assert!(size <= MAX_OBJECT_SIZE, "the parser checks record sizes");
        self.body
            .set(RecordBody {
                members: laid_out,
                size: size as u32,
                align,
                pos,
            })
            .is_ok()
    }

    /// How clang's messages name the type: `struct node`, `union value`;
    /// without a tag, the typedef's name that names it, or where it is
    /// declared, `struct (unnamed at t.c:3:1)` ([`crate::place`]).
    pub fn name(&self) -> String {
        match (&self.tag, self.typedef_name.get()) {
            (Some(t), _) => format!("{} {t}", self.keyword()),
            (None, Some(name)) => name.clone(),
            (None, None) => format!("{} (unnamed at {})", self.keyword(), crate::place(self.pos)),
        }
    }
}

impl RecordBody {
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }
}

/// `n` rounded up to a multiple of `align` (a power of two).
pub fn align_up(n: u64, align: u32) -> u64 {
    let a = u64::from(align);
    n.div_ceil(a) * a
}

// Records are compared, hashed and printed by identity: the members of a
// self-referential struct lead back to the struct itself.
impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Record {}

impl Hash for Record {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self as *const Record).hash(state);
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// A function's type: what it returns and what it takes. Parameter types are
/// as adjusted for the call (arrays and functions are pointers) and without
/// top-level qualifiers, which do not change the type (C99 6.7.5.3p15).
/// A variadic function (`int printf(const char *, ...)`) takes any number
/// of arguments after those.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FuncType {
    pub ret: Type,
    pub params: Vec<Type>,
    pub variadic: bool,
    /// Each parameter as its declaration has it, for `-g`: its type with
    /// its qualifiers (which `params` leaves out, as they do not change the
    /// function's type) and how it spelled it ([`Spelled`]), since clang
    /// describes a function pointer's parameters so (`const pt` of a
    /// typedef `pt`). Never part of comparing types.
    pub spelled: Spellings,
    /// Declared with `()`, of which C99 says nothing of the parameters
    /// (merlin takes it as `(void)`): clang names the type `int ()`.
    pub unprototyped: Unprototyped,
}

/// Whether a function type was declared with `()` ([`FuncType`]). Any two
/// compare equal: `int ()` is compatible with `int (void)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unprototyped(pub bool);

impl PartialEq for Unprototyped {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Unprototyped {}

impl Hash for Unprototyped {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

/// The declared type and [`Spelled`] of each parameter of a function type,
/// or none (a type made without them). Any two compare equal, as spellings
/// do.
#[derive(Clone, Debug, Default)]
pub struct Spellings(pub Vec<(Type, Spelled)>);

impl PartialEq for Spellings {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Spellings {}

impl Hash for Spellings {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Void,
    Int(IntKind),
    Ptr(Box<Type>),
    /// `T[n]`; `None` is an array of unknown size (`int a[]`), which an
    /// initializer or a later declaration completes.
    Array(Box<Type>, Option<u32>),
    /// `struct tag` or `union tag`.
    Record(Arc<Record>),
    /// A function type: what a function name designates. In expressions it
    /// decays to a pointer to the function, like an array to a pointer.
    Func(Box<FuncType>),
    /// A `const` and/or `volatile` type. Never an array or another `Qual`.
    Qual(Box<Type>, Quals),
}

/// The largest object merlin lays out: sizes must fit a signed 32-bit offset.
pub const MAX_OBJECT_SIZE: u64 = i32::MAX as u64;

/// How a declaration spelled the base of its type, where that says more
/// than the type itself: through a typedef name, or as an enum. C's typedef
/// names are only names, and merlin's enum types are integer types, so
/// neither changes what the type is; but a debugger shows them as written
/// (`Node *`, `enum color`), and `-g` describes them ([`crate::debug`]).
///
/// It is the spelling of the declaration's specifiers. The declarator builds
/// the declared type from the spelled one (`Node *p` points to it), and `-g`
/// finds the spelled type again inside the declared one.
#[derive(Clone)]
pub enum Spelling {
    Typedef(Arc<TypedefName>),
    Enum(Arc<EnumType>),
}

/// A typedef name: what it stands for, and how its own declaration spelled
/// that.
#[derive(Debug)]
pub struct TypedefName {
    pub name: String,
    pub ty: Type,
    pub spelled: Spelled,
    pub pos: Pos,
}

/// An enum: its tag, the integer type it is, and its constants.
#[derive(Debug)]
pub struct EnumType {
    pub tag: Option<String>,
    pub ty: Type,
    pub constants: Vec<(String, i128)>,
    pub pos: Pos,
    /// The typedef's name that names it, if it has no tag ([`Record::typedef_name`]).
    pub typedef_name: OnceLock<String>,
}

impl EnumType {
    /// How clang's messages name the enum type: `enum color`, or without
    /// a tag the typedef's name that names it, or where it is declared
    /// (`enum (unnamed at t.c:1:1)`).
    pub fn name(&self) -> String {
        match (&self.tag, self.typedef_name.get()) {
            (Some(t), _) => format!("enum {t}"),
            (None, Some(name)) => name.clone(),
            (None, None) => format!("enum (unnamed at {})", crate::place(self.pos)),
        }
    }
}

impl Spelling {
    /// The type the spelling stands for.
    pub fn ty(&self) -> &Type {
        match self {
            Spelling::Typedef(t) => &t.ty,
            Spelling::Enum(e) => &e.ty,
        }
    }
}

impl fmt::Debug for Spelling {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Spelling::Typedef(t) => f.write_str(&t.name),
            Spelling::Enum(e) => f.write_str(&e.name()),
        }
    }
}

/// A declaration's [`Spelling`], if it has one. A spelling never changes a
/// type, so any two compare equal: comparing declarations compares what
/// they declare.
#[derive(Clone, Debug, Default)]
pub struct Spelled(pub Option<Spelling>);

impl PartialEq for Spelled {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Spelled {}

impl Hash for Spelled {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

impl Type {
    pub const INT: Type = Type::Int(IntKind::Int);
    pub const UINT: Type = Type::Int(IntKind::UInt);
    pub const CHAR: Type = Type::Int(IntKind::Char);
    pub const BOOL: Type = Type::Int(IntKind::Bool);

    pub fn ptr(to: Type) -> Type {
        Type::Ptr(Box::new(to))
    }

    pub fn array(of: Type, n: Option<u32>) -> Type {
        Type::Array(Box::new(of), n)
    }

    pub fn func(ret: Type, params: Vec<Type>) -> Type {
        Type::Func(Box::new(FuncType {
            ret,
            params,
            variadic: false,
            spelled: Spellings::default(),
            unprototyped: Unprototyped::default(),
        }))
    }

    /// A function taking `params` and then any number of arguments (`...`).
    pub fn variadic_func(ret: Type, params: Vec<Type>) -> Type {
        Type::Func(Box::new(FuncType {
            ret,
            params,
            variadic: true,
            spelled: Spellings::default(),
            unprototyped: Unprototyped::default(),
        }))
    }

    /// The type with qualifiers `q` added, kept in normal form: on an array
    /// they go to the elements, and they merge with any already there.
    /// Qualifiers on a function type mean nothing and are dropped.
    pub fn qualified(self, q: Quals) -> Type {
        if q.is_empty() {
            return self;
        }
        match self {
            Type::Array(elem, n) => Type::Array(Box::new(elem.qualified(q)), n),
            Type::Qual(t, q0) => Type::Qual(t, q0.union(q)),
            Type::Func(_) => self,
            t => Type::Qual(Box::new(t), q),
        }
    }

    /// The type without its top-level qualifiers.
    pub fn unqual(&self) -> &Type {
        match self {
            Type::Qual(t, _) => t,
            t => t,
        }
    }

    /// Whether `self` and `other` are the same type but for their
    /// qualifiers, at any level of their pointers and arrays (`int **` and
    /// `const int *const *`): C's similar types, as clang's
    /// `hasCvrSimilarType` compares them. A cast between two such pointer
    /// types changes nothing (clang's `CK_NoOp`).
    pub fn similar(&self, other: &Type) -> bool {
        match (self.unqual(), other.unqual()) {
            (Type::Ptr(a), Type::Ptr(b)) => a.similar(b),
            (Type::Array(a, n), Type::Array(b, m)) => n == m && a.similar(b),
            (a, b) => a == b,
        }
    }

    /// The qualifiers of an object of this type: its own, or for an array
    /// its elements' (an array of `const int` is itself const).
    pub fn quals(&self) -> Quals {
        match self {
            Type::Qual(_, q) => *q,
            Type::Array(elem, _) => elem.quals(),
            _ => Quals::NONE,
        }
    }

    /// The qualifiers `self` adds to `of`, if it is `of` with qualifiers
    /// added: on an array they go to its elements, so `const row`, of a
    /// typedef `row` of `u32[4]`, is `const u32[4]`, and adds `const` to
    /// `row` (as clang describes it: a const typedef). `None` if `self` is
    /// not `of` so qualified.
    pub fn quals_added_to(&self, of: &Type) -> Option<Quals> {
        let (mine, its) = (self.quals(), of.quals());
        let added = Quals {
            konst: mine.konst && !its.konst,
            volatile: mine.volatile && !its.volatile,
        };
        (of.clone().qualified(added) == *self).then_some(added)
    }

    /// `sizeof`: `None` for `void`, functions, arrays of unknown size and
    /// incomplete structs and unions.
    pub fn size(&self, x: Xlen) -> Option<u32> {
        match self {
            Type::Void | Type::Func(_) => None,
            Type::Int(k) => Some(k.size(x)),
            Type::Ptr(_) => Some(ptr_size(x)),
            Type::Array(elem, n) => Some(elem.size(x)? * (*n)?),
            Type::Record(r) => r.body().map(|b| b.size),
            Type::Qual(t, _) => t.size(x),
        }
    }

    /// The alignment: an integer's or a pointer's is its size.
    pub fn align(&self, x: Xlen) -> u32 {
        match self {
            Type::Void | Type::Func(_) => 1,
            Type::Int(k) => k.size(x),
            Type::Ptr(_) => ptr_size(x),
            Type::Array(elem, _) => elem.align(x),
            Type::Record(r) => r.body().map_or(1, |b| b.align),
            Type::Qual(t, _) => t.align(x),
        }
    }

    pub fn is_integer(&self) -> bool {
        matches!(self.unqual(), Type::Int(_))
    }

    pub fn is_ptr(&self) -> bool {
        matches!(self.unqual(), Type::Ptr(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Type::Array(..))
    }

    pub fn is_func(&self) -> bool {
        matches!(self, Type::Func(_))
    }

    pub fn is_void(&self) -> bool {
        matches!(self.unqual(), Type::Void)
    }

    /// The struct or union, if this is one.
    pub fn record(&self) -> Option<&Arc<Record>> {
        match self.unqual() {
            Type::Record(r) => Some(r),
            _ => None,
        }
    }

    pub fn is_record(&self) -> bool {
        self.record().is_some()
    }

    /// The function type, if this is one.
    pub fn func_type(&self) -> Option<&FuncType> {
        match self {
            Type::Func(f) => Some(f),
            _ => None,
        }
    }

    /// Integers and pointers: what conditions and `!` accept (after decay).
    pub fn is_scalar(&self) -> bool {
        matches!(self.unqual(), Type::Int(_) | Type::Ptr(_))
    }

    /// Whether an expression of this type stands for its address rather than
    /// a value loaded into a register: arrays (which decay to that address),
    /// functions (likewise) and structs and unions (which are copied from
    /// it).
    pub fn is_addressed(&self) -> bool {
        self.is_array() || self.is_func() || self.is_record()
    }

    pub fn int_kind(&self) -> Option<IntKind> {
        match self.unqual() {
            Type::Int(k) => Some(*k),
            _ => None,
        }
    }

    /// What an expression of this type gives as a value: an array decays to
    /// a pointer to its first element, a function to a pointer to itself,
    /// and top-level qualifiers go (C's lvalue conversion). Other types are
    /// unchanged.
    pub fn decay(&self) -> Type {
        match self {
            Type::Array(elem, _) => Type::ptr((**elem).clone()),
            Type::Func(_) => Type::ptr(self.clone()),
            Type::Qual(t, _) => (**t).clone(),
            t => t.clone(),
        }
    }

    /// How many bytes adding 1 to a pointer (or a decayed array) of this
    /// type moves it: the size of what it points to, or 1 for `void` and
    /// a function, the GNU extension clang takes without a word (as
    /// `-Wpointer-arith`, which is not on by default, would say).
    pub fn step(&self, x: Xlen) -> Option<u32> {
        let to = self.pointee()?;
        if to.is_void() || to.is_func() {
            return Some(1);
        }
        to.size(x)
    }

    /// What a pointer (or a decayed array) points to.
    pub fn pointee(&self) -> Option<&Type> {
        match self.unqual() {
            Type::Ptr(t) | Type::Array(t, _) => Some(t),
            _ => None,
        }
    }

    /// Whether operations on the value (after integer promotion) are unsigned:
    /// `unsigned int`, `unsigned long` and pointers (addresses compare
    /// unsigned). Narrower types promote to `int`, which is signed.
    pub fn is_unsigned(&self) -> bool {
        match self.unqual() {
            Type::Int(k) => !k.promoted().signed(),
            Type::Ptr(_) | Type::Array(..) | Type::Func(_) => true,
            _ => false,
        }
    }

    /// The type as clang names it in a message, quoted, as a declaration
    /// `spelled` it: through a typedef name, its name with what it stands
    /// for after it (`'T *' (aka 'int *')`); as an enum, `'enum e'`.
    pub fn named(&self, spelled: &Spelled) -> String {
        // What the type is without its typedef names, clang's "canonical
        // type": an enum (which merlin's types leave out) is still one.
        let mut canonical = spelled;
        while let Some(Spelling::Typedef(t)) = &canonical.0 {
            canonical = &t.spelled;
        }
        let plain = match &canonical.0 {
            Some(Spelling::Enum(e)) => self.declare_with("", Some((&e.ty, &e.name()))),
            _ => self.declare(""),
        };
        match &spelled.0 {
            Some(Spelling::Typedef(t)) => {
                let sugared = self.declare_with("", Some((&t.ty, &t.name)));
                if sugared != plain {
                    format!("'{sugared}' (aka '{plain}')")
                } else {
                    format!("'{sugared}'")
                }
            }
            _ => format!("'{plain}'"),
        }
    }

    /// The type as a C declaration of `name`, e.g. `char *argv[4]`. With an
    /// empty name it is the type name clang prints, e.g. `int (*)[3]`.
    pub fn declare(&self, name: &str) -> String {
        self.declare_with(name, None)
    }

    /// [`Self::declare`], with the part of the type that is `base.0`
    /// written as `base.1` (a typedef's name).
    fn declare_with(&self, name: &str, base: Option<(&Type, &str)>) -> String {
        fn go(t: &Type, inner: String, base: Option<(&Type, &str)>) -> String {
            if let Some((b, written)) = base {
                if t == b {
                    return join(written, &inner);
                }
            }
            let go = |t: &Type, inner: String| go(t, inner, base);
            match t {
                Type::Void => join("void", &inner),
                Type::Int(k) => join(k.name(), &inner),
                Type::Record(r) => join(&r.name(), &inner),
                Type::Ptr(to) => go(to, format!("*{inner}")),
                // `int *const p`, but `const int x`.
                Type::Qual(t, q) => match &**t {
                    Type::Ptr(to) => go(to, format!("*{}", join(&q.to_string(), &inner))),
                    t => format!("{q} {}", go(t, inner)),
                },
                Type::Array(elem, n) => {
                    let n = n.map_or(String::new(), |n| n.to_string());
                    go(elem, format!("{}[{n}]", parenthesize(inner)))
                }
                Type::Func(f) => {
                    let mut params: Vec<String> = f.params.iter().map(|p| p.declare("")).collect();
                    if f.variadic {
                        params.push("...".to_string());
                    }
                    let params = if params.is_empty() && !f.unprototyped.0 {
                        "void".to_string()
                    } else {
                        params.join(", ")
                    };
                    go(&f.ret, format!("{}({params})", parenthesize(inner)))
                }
            }
        }
        /// A declarator that starts with `*` binds looser than `[]` and
        /// `()`, so it needs parentheses before them.
        fn parenthesize(inner: String) -> String {
            if inner.starts_with('*') {
                format!("({inner})")
            } else {
                inner
            }
        }
        /// The base and the declarator, apart, but for an array's brackets
        /// with no name before them, as clang writes them: `int[2]`, `int
        /// *[2]`, `int (*)[2]`.
        fn join(base: &str, inner: &str) -> String {
            if inner.is_empty() || inner.starts_with('[') {
                format!("{base}{inner}")
            } else {
                format!("{base} {inner}")
            }
        }
        go(self, name.to_string(), base)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.declare(""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use IntKind::*;

    const R32: Xlen = Xlen::Rv32;

    #[test]
    fn sizes_and_alignment() {
        assert_eq!(Type::CHAR.size(R32), Some(1));
        assert_eq!(Type::BOOL.size(R32), Some(1));
        assert_eq!(Type::Int(UShort).size(R32), Some(2));
        assert_eq!(Type::Int(Long).size(R32), Some(4));
        assert_eq!(Type::ptr(Type::Void).size(R32), Some(4));
        assert_eq!(Type::Void.size(R32), None);
        let m = Type::array(Type::array(Type::Int(Short), Some(3)), Some(5));
        assert_eq!(m.size(R32), Some(30));
        assert_eq!(m.align(R32), 2);
        assert_eq!(Type::array(Type::INT, None).size(R32), None);
        assert_eq!(Type::func(Type::INT, vec![]).size(R32), None);
        assert_eq!(Type::INT.qualified(Quals::CONST).size(R32), Some(4));
    }

    fn record(union: bool, members: &[(&str, Type)]) -> Type {
        let r = Record::new(union, Some("s".into()), Pos::default());
        assert!(r.complete(
            members.iter().map(|(n, t)| (n.to_string(), t.clone())).collect(),
            R32
        ));
        Type::Record(r)
    }

    fn offsets(t: &Type) -> Vec<u32> {
        t.record()
            .unwrap()
            .body()
            .unwrap()
            .members
            .iter()
            .map(|m| m.offset)
            .collect()
    }

    #[test]
    fn struct_layout_matches_ilp32() {
        // char, then int aligned to 4, then short: 12 bytes, aligned to 4.
        let s = record(
            false,
            &[("c", Type::CHAR), ("i", Type::INT), ("h", Type::Int(Short))],
        );
        assert_eq!(offsets(&s), [0, 4, 8]);
        assert_eq!((s.size(R32), s.align(R32)), (Some(12), 4));
        // Tail padding up to the alignment: { short; char; } is 4 bytes.
        let t = record(false, &[("h", Type::Int(Short)), ("c", Type::CHAR)]);
        assert_eq!((t.size(R32), t.align(R32)), (Some(4), 2));
        // Only chars: no padding at all.
        let c = record(false, &[("a", Type::array(Type::CHAR, Some(3)))]);
        assert_eq!((c.size(R32), c.align(R32)), (Some(3), 1));
        // Nested: the inner struct aligns the outer.
        let n = record(false, &[("c", Type::CHAR), ("in", s.clone()), ("d", Type::CHAR)]);
        assert_eq!(offsets(&n), [0, 4, 16]);
        assert_eq!(n.size(R32), Some(20));
        // A union is as big as its biggest member, rounded to its alignment.
        let u = record(true, &[("c", Type::array(Type::CHAR, Some(5))), ("i", Type::INT)]);
        assert_eq!(offsets(&u), [0, 0]);
        assert_eq!((u.size(R32), u.align(R32)), (Some(8), 4));
    }

    #[test]
    fn records_are_compared_by_identity() {
        let a = Record::new(false, Some("s".into()), Pos::default());
        let b = Record::new(false, Some("s".into()), Pos::default());
        assert_eq!(Type::Record(a.clone()), Type::Record(a.clone()));
        assert_ne!(Type::Record(a.clone()), Type::Record(b));
        assert_eq!(Type::Record(a.clone()).size(R32), None, "incomplete");
        assert!(a.complete(vec![("x".into(), Type::INT)], R32));
        assert!(
            !a.complete(vec![("x".into(), Type::INT)], R32),
            "already complete"
        );
        assert_eq!(Type::Record(a).size(R32), Some(4));
    }

    #[test]
    fn char_is_unsigned_on_riscv() {
        assert!(!Char.signed());
        assert!(SChar.signed());
        assert_eq!(Char.wrap(-1, R32), 255);
        assert_eq!(SChar.wrap(200, R32), -56);
        assert_eq!(UShort.wrap(70000, R32), 4464);
        assert_eq!(Int.wrap(0x8000_0000, R32), i32::MIN.into());
        assert_eq!(UInt.wrap(-1, R32), 0xffff_ffff);
        assert_eq!(Short.range(R32), (-32768, 32767));
    }

    #[test]
    fn bool_converts_to_zero_or_one() {
        assert_eq!(Bool.wrap(0, R32), 0);
        assert_eq!(Bool.wrap(256, R32), 1, "not truncated: any nonzero value is true");
        assert_eq!(Bool.wrap(-1, R32), 1);
        assert_eq!(Bool.range(R32), (0, 1));
        assert_eq!(Bool.promoted(), Int);
        assert_eq!(common(Bool, Bool, R32), Int);
    }

    #[test]
    fn usual_arithmetic_conversions() {
        assert_eq!(common(Char, Char, R32), Int);
        assert_eq!(common(UChar, UShort, R32), Int);
        assert_eq!(common(Int, UInt, R32), UInt);
        assert_eq!(common(Long, Int, R32), Long);
        assert_eq!(common(UInt, Long, R32), ULong);
        assert_eq!(common(SChar, ULong, R32), ULong);
        assert!(!Type::Int(UChar).is_unsigned(), "promotes to int");
        assert!(Type::UINT.is_unsigned());
        assert!(Type::ptr(Type::INT).is_unsigned());
    }

    #[test]
    fn long_long_is_64_bits_on_both_widths() {
        for x in [R32, Xlen::Rv64] {
            assert_eq!(Type::Int(LongLong).size(x), Some(8));
            assert_eq!(Type::Int(ULongLong).align(x), 8);
            assert_eq!(ULongLong.range(x), (0, u64::MAX.into()));
            assert_eq!(LongLong.wrap(1 << 63, x), i64::MIN.into());
            assert_eq!(common(ULongLong, Int, x), ULongLong);
        }
        // On ILP32 a long long holds every unsigned long; on LP64 it does not.
        assert_eq!(common(LongLong, ULong, R32), LongLong);
        assert_eq!(common(LongLong, ULong, Xlen::Rv64), ULongLong);
        assert_eq!(common(LongLong, UInt, R32), LongLong);
        assert_eq!(common(LongLong, Long, Xlen::Rv64), LongLong);
        // 8-aligned on RV32 too: { int; long long } is 16 bytes.
        let r = Record::new(false, Some("s".into()), Pos::default());
        assert!(r.complete(
            vec![("i".into(), Type::INT), ("ll".into(), Type::Int(LongLong))],
            R32
        ));
        let s = Type::Record(r);
        assert_eq!(offsets(&s), [0, 8]);
        assert_eq!((s.size(R32), s.align(R32)), (Some(16), 8));
        assert_eq!(Type::Int(ULongLong).to_string(), "unsigned long long");
    }

    #[test]
    fn lp64() {
        const R64: Xlen = Xlen::Rv64;
        // `long` and pointers are 8 bytes, aligned to 8; `int` stays 4.
        assert_eq!(Type::INT.size(R64), Some(4));
        assert_eq!(Type::Int(Long).size(R64), Some(8));
        assert_eq!(Type::Int(ULong).align(R64), 8);
        assert_eq!(Type::ptr(Type::CHAR).size(R64), Some(8));
        assert_eq!(Type::ptr(Type::Void).align(R64), 8);
        assert_eq!(Type::array(Type::ptr(Type::INT), Some(3)).size(R64), Some(24));
        // A long next to an int: 16 bytes, aligned to 8.
        let r = Record::new(false, Some("s".into()), Pos::default());
        assert!(r.complete(vec![("i".into(), Type::INT), ("l".into(), Type::Int(Long))], R64));
        let s = Type::Record(r);
        assert_eq!(offsets(&s), [0, 8]);
        assert_eq!((s.size(R64), s.align(R64)), (Some(16), 8));
        // Ranges and wrapping in 64 bits.
        assert_eq!(Long.range(R64), (i64::MIN.into(), i64::MAX.into()));
        assert_eq!(ULong.range(R64), (0, u64::MAX.into()));
        assert_eq!(ULong.wrap(-1, R64), u64::MAX.into());
        assert_eq!(Long.wrap(1 << 63, R64), i64::MIN.into());
        assert_eq!(Int.wrap(1 << 32, R64), 0, "int stays 32 bits");
        // `long` holds every `unsigned int` on LP64: they meet at `long`.
        assert_eq!(common(UInt, Long, R64), Long);
        assert_eq!(common(Long, UInt, R64), Long);
        assert_eq!(common(Int, ULong, R64), ULong);
        assert_eq!(common(Int, UInt, R64), UInt);
        assert_eq!(common(Long, ULong, R64), ULong);
        assert_eq!(IntKind::size_t(R64), ULong);
        assert_eq!(IntKind::ptrdiff_t(R64), Long);
        assert_eq!((IntKind::size_t(R32), IntKind::ptrdiff_t(R32)), (UInt, Int));
    }

    #[test]
    fn qualifiers_stay_in_normal_form() {
        let c = Type::INT.qualified(Quals::CONST);
        assert_eq!(c.unqual(), &Type::INT);
        assert_eq!(
            c.clone().qualified(Quals::VOLATILE).quals(),
            Quals::CONST.union(Quals::VOLATILE)
        );
        // On an array, the elements are qualified, and the array reports it.
        let a = Type::array(Type::CHAR, Some(4)).qualified(Quals::CONST);
        assert_eq!(a, Type::array(Type::CHAR.qualified(Quals::CONST), Some(4)));
        assert!(a.quals().konst);
        assert!(c.is_integer() && c.is_scalar());
        assert_eq!(c.decay(), Type::INT, "a value has no qualifiers");
        assert_eq!(a.decay(), Type::ptr(Type::CHAR.qualified(Quals::CONST)));
        assert!(Quals::CONST.union(Quals::VOLATILE).contains(Quals::CONST));
        assert!(!Quals::NONE.contains(Quals::VOLATILE));
    }

    #[test]
    fn names_read_like_c() {
        assert_eq!(Type::ptr(Type::CHAR).to_string(), "char *");
        assert_eq!(Type::array(Type::INT, Some(3)).to_string(), "int[3]");
        assert_eq!(
            Type::ptr(Type::array(Type::INT, Some(3))).to_string(),
            "int (*)[3]"
        );
        assert_eq!(
            Type::array(Type::ptr(Type::CHAR), Some(4)).declare("argv"),
            "char *argv[4]"
        );
        assert_eq!(
            Type::array(Type::array(Type::Int(UChar), Some(2)), None).declare("m"),
            "unsigned char m[][2]"
        );
        assert_eq!(Type::ptr(Type::ptr(Type::Void)).declare("p"), "void **p");
        assert_eq!(Type::INT.declare("x"), "int x");
        assert_eq!(Type::BOOL.declare("b"), "_Bool b");
        // Qualifiers: on the base type in front, on a pointer after its `*`.
        let cchar = Type::CHAR.qualified(Quals::CONST);
        assert_eq!(Type::ptr(cchar.clone()).declare("s"), "const char *s");
        assert_eq!(
            Type::ptr(Type::INT).qualified(Quals::CONST).declare("p"),
            "int *const p"
        );
        assert_eq!(
            Type::ptr(Type::INT).qualified(Quals::VOLATILE).to_string(),
            "int *volatile"
        );
        assert_eq!(
            Type::ptr(Type::ptr(cchar).qualified(Quals::CONST)).declare("pp"),
            "const char *const *pp"
        );
        // Functions and pointers to them.
        let f = Type::func(Type::INT, vec![Type::INT, Type::ptr(Type::CHAR)]);
        assert_eq!(f.declare("f"), "int f(int, char *)");
        assert_eq!(Type::ptr(f.clone()).to_string(), "int (*)(int, char *)");
        assert_eq!(
            Type::array(Type::ptr(Type::func(Type::Void, vec![])), Some(3)).declare("table"),
            "void (*table[3])(void)"
        );
        // A function returning a pointer to a function.
        assert_eq!(
            Type::func(Type::ptr(f), vec![Type::CHAR]).declare("pick"),
            "int (*pick(char))(int, char *)"
        );
        let s = Record::new(false, Some("node".into()), Pos::default());
        assert_eq!(Type::ptr(Type::Record(s)).declare("next"), "struct node *next");
        let u = Record::new(true, None, Pos::new(3, 1));
        assert_eq!(Type::Record(u).to_string(), "union (unnamed at \u{1}0\u{1}3:1)");
    }

    #[test]
    fn decay() {
        let a = Type::array(Type::Int(Short), Some(4));
        assert_eq!(a.decay(), Type::ptr(Type::Int(Short)));
        assert_eq!(a.pointee(), Some(&Type::Int(Short)));
        assert_eq!(Type::INT.decay(), Type::INT);
        let f = Type::func(Type::Void, vec![]);
        assert_eq!(f.decay(), Type::ptr(f.clone()));
    }
}
