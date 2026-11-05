//! Constant expressions: values known at compile time.
//!
//! Three users: the parser sizes arrays (`int a[4 * sizeof(int)]`), sema finds
//! null pointer constants and checks that global initializers are constant,
//! and code generation turns those initializers into data.
//!
//! Integer constants carry their C type and wrap the way the generated code
//! would at run time on the target (each function takes it: an `int` is 32
//! bits on both widths, a `long` 64 on RV64). Values are `i128`s, so every
//! value of every type, `unsigned long` on LP64 included, is the number it
//! is. The parser calls [`eval`] before sema has typed the
//! tree, so integer types are worked out here from the leaves (literals,
//! casts, `sizeof`) rather than read from [`Expr::ty`]. Address constants
//! (`&g`, `arr + 2`, `"text"`) need the types sema assigns, so they are only
//! found in a checked tree.
//!
//! An address converted to an integer type as wide as a pointer stays an
//! address constant, as clang has it: `(long)&g` (or `(unsigned)&g` on
//! RV32, `(unsigned long long)&g` on RV64) initializes a global with `.word
//! g` or `.quad g`, and adding or subtracting an integer moves it by that
//! many bytes (`(long)&g + 4` is `g+4`). Any other arithmetic on it is not
//! constant, nor is a narrower or wider conversion (`(int)&g` on RV64,
//! `(long long)&g` on RV32): clang rejects those in an initializer too.

use crate::ast::*;
use crate::types::{common, ptr_size, IntKind, Type, Xlen};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Const {
    /// An integer of the given type, in its range.
    Int(i128, IntKind),
    /// An address: `base + offset` bytes, or the plain number `offset` when
    /// there is no base (`(char *)0x10000000`).
    Addr { base: Option<Base>, offset: i128 },
}

/// What an address constant points into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Base {
    /// A global symbol (a global variable or a `static` local).
    Sym(String),
    /// A string literal, which code generation gives a label.
    Str(Vec<u8>),
}

/// The value of an integer constant expression.
pub fn int(e: &Expr, x: Xlen) -> Option<i128> {
    match eval(e, x)? {
        Const::Int(v, _) => Some(v),
        Const::Addr { .. } => None,
    }
}

/// Whether `e` is a null pointer constant: an integer constant expression
/// equal to 0, or one cast to `void *` (C99 6.3.2.3).
pub fn is_null_pointer(e: &Expr, xlen: Xlen) -> bool {
    match &e.kind {
        ExprKind::Cast {
            x, implicit: false, ..
        } if e.ty == Type::ptr(Type::Void) => is_null_pointer(x, xlen),
        _ => e.ty.is_integer() && int(e, xlen) == Some(0),
    }
}

/// The constant value of `e`, or `None` if it is not a constant expression
/// merlin can evaluate (that includes dividing by zero, where the division
/// is evaluated).
pub fn eval(e: &Expr, xlen: Xlen) -> Option<Const> {
    value(e, xlen, true)
}

/// [`eval`] of an operand that is `evaluated` or not. C99 6.6p3 and p11
/// hold a constant expression to what it evaluates: in `1 ? 2 : 1 / 0` the
/// third operand is never divided, only its type counts (the result's is
/// the operands' common type), so there dividing by zero is no error and
/// gives 0, a value nothing reads. clang has it so: `int a[1 ? 2 : 1 / 0];`
/// has two elements.
fn value(e: &Expr, xlen: Xlen, evaluated: bool) -> Option<Const> {
    use Const::*;
    let eval = |e: &Expr, xlen: Xlen| value(e, xlen, evaluated);
    match &e.kind {
        ExprKind::Num(v) => Some(Int(*v, e.ty.int_kind()?)),
        ExprKind::SizeofType(t) => Some(Int(t.size(xlen)?.into(), IntKind::size_t(xlen))),
        ExprKind::Unary(op, x) => {
            let (v, k) = int_of(eval(x, xlen)?)?;
            let k = k.promoted();
            Some(match op {
                UnOp::Neg => Int(k.wrap(-v, xlen), k),
                UnOp::Plus => Int(v, k),
                UnOp::BitNot => Int(k.wrap(!v, xlen), k),
                UnOp::Not => Int((v == 0).into(), IntKind::Int),
            })
        }
        ExprKind::Binary(BinOp::LogAnd, l, r) => {
            let l = int_of(eval(l, xlen)?)?.0 != 0;
            Some(Int((l && int_of(eval(r, xlen)?)?.0 != 0).into(), IntKind::Int))
        }
        ExprKind::Binary(BinOp::LogOr, l, r) => {
            let l = int_of(eval(l, xlen)?)?.0 != 0;
            Some(Int((l || int_of(eval(r, xlen)?)?.0 != 0).into(), IntKind::Int))
        }
        ExprKind::Binary(op, l, r) => {
            let (lc, rc) = (eval(l, xlen)?, eval(r, xlen)?);
            match (lc, rc) {
                (Int(a, ak), Int(b, bk)) => binary(*op, (a, ak), (b, bk), xlen, evaluated),
                // Address arithmetic: pointer plus or minus an integer, by
                // whole elements, or an address converted to an integer
                // plus or minus one, by bytes.
                (Addr { base, offset }, Int(n, _)) if matches!(op, BinOp::Add | BinOp::Sub) => {
                    let size = step(&l.ty, xlen)?;
                    let n = if *op == BinOp::Sub { -n } else { n };
                    Some(Addr {
                        base,
                        offset: offset + n * size,
                    })
                }
                (Int(n, _), Addr { base, offset }) if *op == BinOp::Add => {
                    let size = step(&r.ty, xlen)?;
                    Some(Addr {
                        base,
                        offset: offset + n * size,
                    })
                }
                _ => None,
            }
        }
        ExprKind::Cond(c, a, b) => {
            // Only the chosen operand is evaluated.
            let c = int_of(eval(c, xlen)?)?.0 != 0;
            let a = value(a, xlen, evaluated && c)?;
            let b = value(b, xlen, evaluated && !c)?;
            match (a, b) {
                // The result has the operands' common type, whichever is chosen.
                (Int(x, xk), Int(y, yk)) => {
                    let k = common(xk, yk, xlen);
                    Some(Int(k.wrap(if c { x } else { y }, xlen), k))
                }
                (x, y) => Some(if c { x } else { y }),
            }
        }
        ExprKind::Cast { x, .. } => {
            let v = eval(x, xlen)?;
            match (e.ty.unqual(), v) {
                (Type::Int(k), Int(v, _)) => Some(Int(k.wrap(v, xlen), *k)),
                // A non-null address is true.
                (Type::Int(IntKind::Bool), Addr { base: Some(_), .. }) => Some(Int(1, IntKind::Bool)),
                // A plain number used as an address converts back: that
                // makes `(unsigned)&((struct s *)0)->field` (offsetof) a
                // constant.
                (Type::Int(k), Addr { base: None, offset }) => Some(Int(k.wrap(offset, xlen), *k)),
                // An address converted to an integer as wide as a pointer
                // stays one, what the linker will fill in; to any other
                // width it is not a constant.
                (Type::Int(k), a @ Addr { .. }) if k.size(xlen) == ptr_size(xlen) => Some(a),
                // An address is a pointer-sized unsigned number.
                (Type::Ptr(_), Int(v, _)) => Some(Addr {
                    base: None,
                    offset: IntKind::ULong.wrap(v, xlen),
                }),
                (Type::Ptr(_), a @ Addr { .. }) => Some(a),
                _ => None,
            }
        }
        // Addresses, only in a checked tree. An array or a function stands
        // for its address; any other object's value is not constant.
        ExprKind::Str(_) | ExprKind::Var { .. } | ExprKind::Deref(_) | ExprKind::Member { .. }
            if e.ty.is_array() || e.ty.is_func() =>
        {
            addr_of(e, xlen, evaluated)
        }
        ExprKind::Addr(x) => addr_of(x, xlen, evaluated),
        _ => None,
    }
}

/// The address of an lvalue, if it is an address constant: a global object
/// (or a member or element of one, at a constant offset), a string literal,
/// or what a constant pointer points to.
fn addr_of(e: &Expr, xlen: Xlen, evaluated: bool) -> Option<Const> {
    match &e.kind {
        ExprKind::Var {
            var: Some(VarRef::Global(sym)),
            ..
        } => Some(Const::Addr {
            base: Some(Base::Sym(sym.clone())),
            offset: 0,
        }),
        ExprKind::Str(bytes) => Some(Const::Addr {
            base: Some(Base::Str(bytes.clone())),
            offset: 0,
        }),
        ExprKind::Deref(p) => match value(p, xlen, evaluated)? {
            a @ Const::Addr { .. } => Some(a),
            Const::Int(..) => None,
        },
        ExprKind::Member {
            base,
            arrow,
            offset: member,
            ..
        } => {
            let at = if *arrow {
                value(base, xlen, evaluated)?
            } else {
                addr_of(base, xlen, evaluated)?
            };
            match at {
                Const::Addr { base, offset } => Some(Const::Addr {
                    base,
                    offset: offset + i128::from(*member),
                }),
                Const::Int(..) => None,
            }
        }
        _ => None,
    }
}

/// How many bytes adding 1 to a constant of type `t` moves an address: a
/// pointer's element size, or 1 for an address converted to an integer.
fn step(t: &Type, xlen: Xlen) -> Option<i128> {
    if t.is_integer() {
        return Some(1);
    }
    Some(i128::from(t.pointee()?.size(xlen)?))
}

fn int_of(c: Const) -> Option<(i128, IntKind)> {
    match c {
        Const::Int(v, k) => Some((v, k)),
        Const::Addr { .. } => None,
    }
}

/// An arithmetic, bitwise or comparison operator on two integer constants,
/// each a value and its type. Dividing by zero is not constant where it is
/// `evaluated` (see [`value`]).
fn binary(
    op: BinOp,
    (a, ak): (i128, IntKind),
    (b, bk): (i128, IntKind),
    x: Xlen,
    evaluated: bool,
) -> Option<Const> {
    // Shifts take the promoted left operand's type; the others the common type.
    let k = match op {
        BinOp::Shl | BinOp::Shr => ak.promoted(),
        _ => common(ak, bk, x),
    };
    let (a, b) = (
        k.wrap(a, x),
        if matches!(op, BinOp::Shl | BinOp::Shr) {
            b
        } else {
            k.wrap(b, x)
        },
    );
    // The hardware uses the low 5 bits of a 32-bit shift's amount and the
    // low 6 bits of a 64-bit one's (spec section 10).
    let shift = (b & i128::from(k.size(x) * 8 - 1)) as u32;
    let v = match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a.wrapping_mul(b),
        BinOp::Div | BinOp::Rem if b == 0 && evaluated => return None,
        BinOp::Div | BinOp::Rem if b == 0 => 0,
        // i128 division truncates toward zero, like C; `INT_MIN / -1` wraps below.
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        BinOp::Shl => a << shift,
        // `a` is sign-extended when k is signed (arithmetic shift) and
        // non-negative when it is unsigned (logical shift).
        BinOp::Shr => a >> shift,
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        BinOp::Eq => return Some(Const::Int((a == b).into(), IntKind::Int)),
        BinOp::Ne => return Some(Const::Int((a != b).into(), IntKind::Int)),
        BinOp::Lt => return Some(Const::Int((a < b).into(), IntKind::Int)),
        BinOp::Le => return Some(Const::Int((a <= b).into(), IntKind::Int)),
        BinOp::Gt => return Some(Const::Int((a > b).into(), IntKind::Int)),
        BinOp::Ge => return Some(Const::Int((a >= b).into(), IntKind::Int)),
        BinOp::LogAnd | BinOp::LogOr => unreachable!("handled by eval: they short-circuit"),
    };
    Some(Const::Int(k.wrap(v, x), k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluates `src` as an untyped expression, the way the parser does.
    fn ev(src: &str) -> Option<Const> {
        ev_for(src, Xlen::Rv32)
    }

    fn ev_for(src: &str, x: Xlen) -> Option<Const> {
        let toks = crate::lexer::lex(&format!("int x[1] = {{ {src} }};"), x).unwrap();
        let prog = crate::parser::parse(&toks, x).unwrap();
        let Item::Decl(d) = &prog.items[0] else { panic!() };
        let Some(Init::List(items, _)) = &d[0].init else {
            panic!()
        };
        let Init::Expr(e) = &items[0] else { panic!() };
        eval(e, x)
    }

    fn int(src: &str) -> (i128, IntKind) {
        int_for(src, Xlen::Rv32)
    }

    fn int_for(src: &str, x: Xlen) -> (i128, IntKind) {
        match ev_for(src, x) {
            Some(Const::Int(v, k)) => (v, k),
            c => panic!("{src}: {c:?}"),
        }
    }

    use IntKind::*;

    #[test]
    fn integer_arithmetic_has_c_types() {
        assert_eq!(int("1 + 2 * 3"), (7, Int));
        assert_eq!(int("2147483647 + 1"), (i32::MIN as i128, Int), "wraps");
        assert_eq!(int("-1u"), (0xffff_ffff, UInt));
        assert_eq!(int("0u - 1 > 0"), (1, Int), "unsigned comparison");
        assert_eq!(int("-1 < 0u"), (0, Int), "-1 becomes UINT_MAX");
        assert_eq!(int("-7 / 2"), (-3, Int));
        assert_eq!(int("-7 % 2"), (-1, Int));
        assert_eq!(int("(0u - 1) / 0x10000000 + 1"), (16, UInt));
        assert_eq!(int("(-2147483647 - 1) / -1"), (i32::MIN as i128, Int));
        assert_eq!(int("-8 >> 1"), (-4, Int), "arithmetic shift");
        assert_eq!(int("0x80000000 >> 4"), (0x0800_0000, UInt), "logical shift");
        assert_eq!(int("1 << 31"), (i32::MIN as i128, Int));
        assert_eq!(int("(char)300"), (44, Char));
        assert_eq!(int("(signed char)200"), (-56, SChar));
        assert_eq!(int("(char)300 + 1"), (45, Int), "promoted");
        assert_eq!(int("sizeof(int) * 4"), (16, UInt));
        assert_eq!(int("sizeof(char[3][5])"), (15, UInt));
        assert_eq!(int("~0"), (-1, Int));
        assert_eq!(int("!5 || 3 && 4"), (1, Int));
        assert_eq!(int("1 ? 2 : 3u"), (2, UInt));
        assert_eq!(int("'a' + 1"), (98, Int));
        assert_eq!(int("1L + 1"), (2, Long));
        assert_eq!(int("1u + 1L"), (2, ULong));
        assert_eq!(
            int("1 << 33"),
            (2, Int),
            "the low 5 bits of a 32-bit shift's amount"
        );
    }

    #[test]
    fn lp64_arithmetic() {
        let int = |src: &str| int_for(src, Xlen::Rv64);
        assert_eq!(int("1u + 1L"), (2, Long), "long holds every unsigned int");
        assert_eq!(int("-1 < 0L"), (1, Int));
        assert_eq!(int("0xffffffffu + 1L"), (1 << 32, Long));
        assert_eq!(
            int("0xffffffffu + 1"),
            (0, UInt),
            "unsigned int still wraps at 32 bits"
        );
        assert_eq!(int("2147483647 + 1"), (i32::MIN.into(), Int));
        assert_eq!(int("9223372036854775807L + 1"), (i64::MIN.into(), Long));
        assert_eq!(int("0ul - 1"), (u64::MAX.into(), ULong));
        assert_eq!(int("(0ul - 1) / 2 > 0"), (1, Int), "unsigned 64-bit comparison");
        assert_eq!(int("(0ul - 1) >> 63"), (1, ULong), "logical shift");
        assert_eq!(int("-1L >> 63"), (-1, Long), "arithmetic shift");
        assert_eq!(int("1L << 63"), (i64::MIN.into(), Long));
        assert_eq!(
            int("1L << 65"),
            (2, Long),
            "the low 6 bits of a 64-bit shift's amount"
        );
        assert_eq!(int("1 << 33"), (2, Int), "the low 5 bits of a 32-bit one's");
        assert_eq!(
            int("sizeof(long) + sizeof(char *)"),
            (16, ULong),
            "size_t is unsigned long"
        );
        assert_eq!(int("(-9223372036854775807L - 1) / -1"), (i64::MIN.into(), Long));
        assert_eq!(int("(unsigned)-1L"), (0xffff_ffff, UInt));
        assert_eq!(int("(int)4294967297L"), (1, Int));
    }

    #[test]
    fn only_what_is_evaluated_counts() {
        // C99 6.6p3, as clang has it: the operand not chosen is not divided.
        assert_eq!(int("1 ? 2 : 1 / 0"), (2, Int));
        assert_eq!(int("0 ? 1 / 0 : 2"), (2, Int));
        assert_eq!(int("0 ? 1 % 0 : 2u"), (2, UInt), "its type still counts");
        assert_eq!(int_for("1 ? 2 : 1L / 0", Xlen::Rv64), (2, Long));
        assert_eq!(int("1 ? 2 : (0 ? 3 : 4 / 0)"), (2, Int));
        assert_eq!(int("0 && 1 / 0"), (0, Int));
        assert_eq!(int("1 || 1 / 0"), (1, Int));
    }

    #[test]
    fn not_constant() {
        assert_eq!(ev("1 / 0"), None);
        assert_eq!(ev("1 % (2 - 2)"), None);
        assert_eq!(ev("0 ? 2 : 1 / 0"), None, "the operand chosen is divided");
        assert_eq!(ev("1 ? (0 ? 2 : 1 / 0) : 3"), None);
        assert_eq!(ev("y"), None);
        assert_eq!(ev("f()"), None);
        assert_eq!(ev("(1, 2)"), None, "C: comma is not a constant expression");
    }
}
