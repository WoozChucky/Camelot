//! C types as merlin knows them, with ILP32 sizes (RISC-V `ilp32`): `char` is
//! 1 byte, `short` 2, `int`, `long` and pointers 4. Plain `char` is unsigned,
//! as on every RISC-V target, so `(char)200` stays 200 and `char` loads use
//! `lbu`.
//!
//! The integer rules follow C99 6.3.1: *integer promotion* turns `_Bool`,
//! `char` and `short` (signed or not) into `int`, since `int` holds all their
//! values, and the *usual arithmetic conversions* bring two operands to a
//! common type. With `int` and `long` the same width, the common type is
//! unsigned exactly when either promoted operand is unsigned (`unsigned int`
//! and `long` meet at `unsigned long`, since `long` cannot hold every
//! `unsigned int`).
//!
//! Structs and unions ([`Record`]) are laid out the way clang lays them out
//! for ILP32: each member at the next offset aligned for its type, the whole
//! aligned to its most aligned member. An enum type is an integer type
//! (`unsigned int`, or `int` when a constant is negative, as clang and GCC
//! choose), so it needs no type of its own.
//!
//! `const` and `volatile` wrap a type in [`Type::Qual`]. Qualifiers of an
//! array apply to its elements (C99 6.7.3p8), so a `Qual` never wraps an
//! array, nor another `Qual`: [`Type::qualified`] keeps that form.

use crate::Pos;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

/// The integer types. `long long` is not supported yet.
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
}

impl IntKind {
    pub fn size(self) -> u32 {
        match self {
            IntKind::Bool | IntKind::Char | IntKind::SChar | IntKind::UChar => 1,
            IntKind::Short | IntKind::UShort => 2,
            IntKind::Int | IntKind::UInt | IntKind::Long | IntKind::ULong => 4,
        }
    }

    pub fn signed(self) -> bool {
        matches!(
            self,
            IntKind::SChar | IntKind::Short | IntKind::Int | IntKind::Long
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
        }
    }

    /// Integer promotion: anything narrower than `int` becomes `int`.
    pub fn promoted(self) -> IntKind {
        if self.size() < 4 {
            IntKind::Int
        } else {
            self
        }
    }

    /// The smallest and largest value of the type.
    pub fn range(self) -> (i64, i64) {
        if self == IntKind::Bool {
            return (0, 1);
        }
        let bits = self.size() * 8;
        if self.signed() {
            (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
        } else {
            (0, (1i64 << bits) - 1)
        }
    }

    /// `v` converted to this type: truncated to its width, then sign-extended
    /// if the type is signed (C's conversion rules, with the implementation-
    /// defined signed case done the two's complement way, as clang does).
    /// For `_Bool` it is `v != 0`.
    pub fn wrap(self, v: i64) -> i64 {
        if self == IntKind::Bool {
            return (v != 0) as i64;
        }
        let bits = self.size() * 8;
        let v = v & ((1i64 << bits) - 1);
        if self.signed() && v >> (bits - 1) != 0 {
            v - (1i64 << bits)
        } else {
            v
        }
    }
}

/// The usual arithmetic conversions: the type two integer operands meet at.
pub fn common(a: IntKind, b: IntKind) -> IntKind {
    let (a, b) = (a.promoted(), b.promoted());
    let long = matches!(a, IntKind::Long | IntKind::ULong) || matches!(b, IntKind::Long | IntKind::ULong);
    let unsigned = !a.signed() || !b.signed();
    match (long, unsigned) {
        (false, false) => IntKind::Int,
        (false, true) => IntKind::UInt,
        (true, false) => IntKind::Long,
        (true, true) => IntKind::ULong,
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
    /// The members and layout, once the definition has been seen. Until then
    /// the type is incomplete: pointers to it are fine, objects of it are not.
    body: OnceLock<RecordBody>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordBody {
    pub members: Vec<Member>,
    pub size: u32,
    pub align: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub ty: Type,
    /// Bytes from the start of the struct (always 0 in a union).
    pub offset: u32,
}

impl Record {
    pub fn new(union: bool, tag: Option<String>, pos: Pos) -> Arc<Record> {
        Arc::new(Record {
            union,
            tag,
            pos,
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
    /// ILP32 and completes the type. Every member type must be complete.
    /// Returns `false` if the type was already complete.
    pub fn complete(&self, members: Vec<(String, Type)>) -> bool {
        let mut offset = 0u64;
        let mut size = 0u64;
        let mut align = 1;
        let mut laid_out = Vec::with_capacity(members.len());
        for (name, ty) in members {
            let (msize, malign) = (u64::from(ty.size().expect("complete member")), ty.align());
            align = align.max(malign);
            let at = if self.union { 0 } else { align_up(offset, malign) };
            offset = at + msize;
            size = size.max(offset);
            laid_out.push(Member {
                name,
                ty,
                offset: at as u32,
            });
        }
        let size = align_up(size, align);
        assert!(size <= MAX_OBJECT_SIZE, "the parser checks record sizes");
        self.body
            .set(RecordBody {
                members: laid_out,
                size: size as u32,
                align,
            })
            .is_ok()
    }

    /// How the type is written: `struct node`, `union value`, or
    /// `struct (anonymous at 3:1)`.
    pub fn name(&self) -> String {
        match &self.tag {
            Some(t) => format!("{} {t}", self.keyword()),
            None => format!("{} (anonymous at {})", self.keyword(), self.pos),
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
        }))
    }

    /// A function taking `params` and then any number of arguments (`...`).
    pub fn variadic_func(ret: Type, params: Vec<Type>) -> Type {
        Type::Func(Box::new(FuncType {
            ret,
            params,
            variadic: true,
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

    /// The qualifiers of an object of this type: its own, or for an array
    /// its elements' (an array of `const int` is itself const).
    pub fn quals(&self) -> Quals {
        match self {
            Type::Qual(_, q) => *q,
            Type::Array(elem, _) => elem.quals(),
            _ => Quals::NONE,
        }
    }

    /// `sizeof`: `None` for `void`, functions, arrays of unknown size and
    /// incomplete structs and unions.
    pub fn size(&self) -> Option<u32> {
        match self {
            Type::Void | Type::Func(_) => None,
            Type::Int(k) => Some(k.size()),
            Type::Ptr(_) => Some(4),
            Type::Array(elem, n) => Some(elem.size()? * (*n)?),
            Type::Record(r) => r.body().map(|b| b.size),
            Type::Qual(t, _) => t.size(),
        }
    }

    pub fn align(&self) -> u32 {
        match self {
            Type::Void | Type::Func(_) => 1,
            Type::Int(k) => k.size(),
            Type::Ptr(_) => 4,
            Type::Array(elem, _) => elem.align(),
            Type::Record(r) => r.body().map_or(1, |b| b.align),
            Type::Qual(t, _) => t.align(),
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

    /// The type as a C declaration of `name`, e.g. `char *argv[4]`. With an
    /// empty name it is the type name clang prints, e.g. `int (*)[3]`.
    pub fn declare(&self, name: &str) -> String {
        fn go(t: &Type, inner: String) -> String {
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
                    let params = if params.is_empty() {
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
        fn join(base: &str, inner: &str) -> String {
            if inner.is_empty() {
                base.to_string()
            } else {
                format!("{base} {inner}")
            }
        }
        go(self, name.to_string())
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

    #[test]
    fn sizes_and_alignment() {
        assert_eq!(Type::CHAR.size(), Some(1));
        assert_eq!(Type::BOOL.size(), Some(1));
        assert_eq!(Type::Int(UShort).size(), Some(2));
        assert_eq!(Type::Int(Long).size(), Some(4));
        assert_eq!(Type::ptr(Type::Void).size(), Some(4));
        assert_eq!(Type::Void.size(), None);
        let m = Type::array(Type::array(Type::Int(Short), Some(3)), Some(5));
        assert_eq!(m.size(), Some(30));
        assert_eq!(m.align(), 2);
        assert_eq!(Type::array(Type::INT, None).size(), None);
        assert_eq!(Type::func(Type::INT, vec![]).size(), None);
        assert_eq!(Type::INT.qualified(Quals::CONST).size(), Some(4));
    }

    fn record(union: bool, members: &[(&str, Type)]) -> Type {
        let r = Record::new(union, Some("s".into()), Pos::default());
        assert!(r.complete(members.iter().map(|(n, t)| (n.to_string(), t.clone())).collect()));
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
        assert_eq!((s.size(), s.align()), (Some(12), 4));
        // Tail padding up to the alignment: { short; char; } is 4 bytes.
        let t = record(false, &[("h", Type::Int(Short)), ("c", Type::CHAR)]);
        assert_eq!((t.size(), t.align()), (Some(4), 2));
        // Only chars: no padding at all.
        let c = record(false, &[("a", Type::array(Type::CHAR, Some(3)))]);
        assert_eq!((c.size(), c.align()), (Some(3), 1));
        // Nested: the inner struct aligns the outer.
        let n = record(false, &[("c", Type::CHAR), ("in", s.clone()), ("d", Type::CHAR)]);
        assert_eq!(offsets(&n), [0, 4, 16]);
        assert_eq!(n.size(), Some(20));
        // A union is as big as its biggest member, rounded to its alignment.
        let u = record(true, &[("c", Type::array(Type::CHAR, Some(5))), ("i", Type::INT)]);
        assert_eq!(offsets(&u), [0, 0]);
        assert_eq!((u.size(), u.align()), (Some(8), 4));
    }

    #[test]
    fn records_are_compared_by_identity() {
        let a = Record::new(false, Some("s".into()), Pos::default());
        let b = Record::new(false, Some("s".into()), Pos::default());
        assert_eq!(Type::Record(a.clone()), Type::Record(a.clone()));
        assert_ne!(Type::Record(a.clone()), Type::Record(b));
        assert_eq!(Type::Record(a.clone()).size(), None, "incomplete");
        assert!(a.complete(vec![("x".into(), Type::INT)]));
        assert!(!a.complete(vec![("x".into(), Type::INT)]), "already complete");
        assert_eq!(Type::Record(a).size(), Some(4));
    }

    #[test]
    fn char_is_unsigned_on_riscv() {
        assert!(!Char.signed());
        assert!(SChar.signed());
        assert_eq!(Char.wrap(-1), 255);
        assert_eq!(SChar.wrap(200), -56);
        assert_eq!(UShort.wrap(70000), 4464);
        assert_eq!(Int.wrap(0x8000_0000), i32::MIN as i64);
        assert_eq!(UInt.wrap(-1), 0xffff_ffff);
        assert_eq!(Short.range(), (-32768, 32767));
    }

    #[test]
    fn bool_converts_to_zero_or_one() {
        assert_eq!(Bool.wrap(0), 0);
        assert_eq!(Bool.wrap(256), 1, "not truncated: any nonzero value is true");
        assert_eq!(Bool.wrap(-1), 1);
        assert_eq!(Bool.range(), (0, 1));
        assert_eq!(Bool.promoted(), Int);
        assert_eq!(common(Bool, Bool), Int);
    }

    #[test]
    fn usual_arithmetic_conversions() {
        assert_eq!(common(Char, Char), Int);
        assert_eq!(common(UChar, UShort), Int);
        assert_eq!(common(Int, UInt), UInt);
        assert_eq!(common(Long, Int), Long);
        assert_eq!(common(UInt, Long), ULong);
        assert_eq!(common(SChar, ULong), ULong);
        assert!(!Type::Int(UChar).is_unsigned(), "promotes to int");
        assert!(Type::UINT.is_unsigned());
        assert!(Type::ptr(Type::INT).is_unsigned());
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
        assert_eq!(Type::array(Type::INT, Some(3)).to_string(), "int [3]");
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
        assert_eq!(Type::Record(u).to_string(), "union (anonymous at 3:1)");
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
