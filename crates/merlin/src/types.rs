//! C types as merlin knows them, with ILP32 sizes (RISC-V `ilp32`): `char` is
//! 1 byte, `short` 2, `int`, `long` and pointers 4. Plain `char` is unsigned,
//! as on every RISC-V target, so `(char)200` stays 200 and `char` loads use
//! `lbu`.
//!
//! The integer rules follow C99 6.3.1: *integer promotion* turns `char` and
//! `short` (signed or not) into `int`, since `int` holds all their values, and
//! the *usual arithmetic conversions* bring two operands to a common type. With
//! `int` and `long` the same width, the common type is unsigned exactly when
//! either promoted operand is unsigned (`unsigned int` and `long` meet at
//! `unsigned long`, since `long` cannot hold every `unsigned int`).

use std::fmt;

/// The integer types. `long long` is not supported yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntKind {
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
            IntKind::Char | IntKind::SChar | IntKind::UChar => 1,
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
    pub fn wrap(self, v: i64) -> i64 {
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Void,
    Int(IntKind),
    Ptr(Box<Type>),
    /// `T[n]`; `None` is an array of unknown size (`int a[]`), which an
    /// initializer or a later declaration completes.
    Array(Box<Type>, Option<u32>),
}

/// The largest object merlin lays out: sizes must fit a signed 32-bit offset.
pub const MAX_OBJECT_SIZE: u64 = i32::MAX as u64;

impl Type {
    pub const INT: Type = Type::Int(IntKind::Int);
    pub const UINT: Type = Type::Int(IntKind::UInt);
    pub const CHAR: Type = Type::Int(IntKind::Char);

    pub fn ptr(to: Type) -> Type {
        Type::Ptr(Box::new(to))
    }

    pub fn array(of: Type, n: Option<u32>) -> Type {
        Type::Array(Box::new(of), n)
    }

    /// `sizeof`: `None` for `void` and arrays of unknown size.
    pub fn size(&self) -> Option<u32> {
        match self {
            Type::Void => None,
            Type::Int(k) => Some(k.size()),
            Type::Ptr(_) => Some(4),
            Type::Array(elem, n) => Some(elem.size()? * (*n)?),
        }
    }

    pub fn align(&self) -> u32 {
        match self {
            Type::Void => 1,
            Type::Int(k) => k.size(),
            Type::Ptr(_) => 4,
            Type::Array(elem, _) => elem.align(),
        }
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Type::Int(_))
    }

    pub fn is_ptr(&self) -> bool {
        matches!(self, Type::Ptr(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Type::Array(..))
    }

    /// Integers and pointers: what conditions and `!` accept (after decay).
    pub fn is_scalar(&self) -> bool {
        matches!(self, Type::Int(_) | Type::Ptr(_))
    }

    pub fn int_kind(&self) -> Option<IntKind> {
        match self {
            Type::Int(k) => Some(*k),
            _ => None,
        }
    }

    /// Array-to-pointer decay: in most expressions an array stands for a
    /// pointer to its first element. Other types are unchanged.
    pub fn decay(&self) -> Type {
        match self {
            Type::Array(elem, _) => Type::ptr((**elem).clone()),
            t => t.clone(),
        }
    }

    /// What a pointer (or a decayed array) points to.
    pub fn pointee(&self) -> Option<&Type> {
        match self {
            Type::Ptr(t) | Type::Array(t, _) => Some(t),
            _ => None,
        }
    }

    /// Whether operations on the value (after integer promotion) are unsigned:
    /// `unsigned int`, `unsigned long` and pointers (addresses compare
    /// unsigned). Narrower types promote to `int`, which is signed.
    pub fn is_unsigned(&self) -> bool {
        match self {
            Type::Int(k) => !k.promoted().signed(),
            Type::Ptr(_) | Type::Array(..) => true,
            Type::Void => false,
        }
    }

    /// The type as a C declaration of `name`, e.g. `char *argv[4]`. With an
    /// empty name it is the type name clang prints, e.g. `int (*)[3]`.
    pub fn declare(&self, name: &str) -> String {
        fn go(t: &Type, inner: String) -> String {
            match t {
                Type::Void => join("void", &inner),
                Type::Int(k) => join(k.name(), &inner),
                Type::Ptr(to) => go(to, format!("*{inner}")),
                Type::Array(elem, n) => {
                    let inner = if inner.starts_with('*') {
                        format!("({inner})")
                    } else {
                        inner
                    };
                    let n = n.map_or(String::new(), |n| n.to_string());
                    go(elem, format!("{inner}[{n}]"))
                }
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
        assert_eq!(Type::Int(UShort).size(), Some(2));
        assert_eq!(Type::Int(Long).size(), Some(4));
        assert_eq!(Type::ptr(Type::Void).size(), Some(4));
        assert_eq!(Type::Void.size(), None);
        let m = Type::array(Type::array(Type::Int(Short), Some(3)), Some(5));
        assert_eq!(m.size(), Some(30));
        assert_eq!(m.align(), 2);
        assert_eq!(Type::array(Type::INT, None).size(), None);
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
    }

    #[test]
    fn decay() {
        let a = Type::array(Type::Int(Short), Some(4));
        assert_eq!(a.decay(), Type::ptr(Type::Int(Short)));
        assert_eq!(a.pointee(), Some(&Type::Int(Short)));
        assert_eq!(Type::INT.decay(), Type::INT);
    }
}
