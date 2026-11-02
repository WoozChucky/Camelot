//! Constant expressions: values known at compile time.
//!
//! Three users: the parser sizes arrays (`int a[4 * sizeof(int)]`), sema finds
//! null pointer constants and checks that global initializers are constant,
//! and code generation turns those initializers into data.
//!
//! Integer constants carry their C type and wrap the way the generated code
//! would at run time. The parser calls [`eval`] before sema has typed the
//! tree, so integer types are worked out here from the leaves (literals,
//! casts, `sizeof`) rather than read from [`Expr::ty`]. Address constants
//! (`&g`, `arr + 2`, `"text"`) need the types sema assigns, so they are only
//! found in a checked tree.

use crate::ast::*;
use crate::types::{common, IntKind, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Const {
    /// An integer of the given type, in its range.
    Int(i64, IntKind),
    /// An address: `base + offset` bytes, or the plain number `offset` when
    /// there is no base (`(char *)0x10000000`).
    Addr { base: Option<Base>, offset: i64 },
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
pub fn int(e: &Expr) -> Option<i64> {
    match eval(e)? {
        Const::Int(v, _) => Some(v),
        Const::Addr { .. } => None,
    }
}

/// Whether `e` is a null pointer constant: an integer constant expression
/// equal to 0, or one cast to `void *` (C99 6.3.2.3).
pub fn is_null_pointer(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Cast { x, implicit: false } if e.ty == Type::ptr(Type::Void) => is_null_pointer(x),
        _ => e.ty.is_integer() && int(e) == Some(0),
    }
}

/// The constant value of `e`, or `None` if it is not a constant expression
/// merlin can evaluate (that includes dividing by zero).
pub fn eval(e: &Expr) -> Option<Const> {
    use Const::*;
    match &e.kind {
        ExprKind::Num(v) => Some(Int(*v, e.ty.int_kind()?)),
        ExprKind::SizeofType(t) => Some(Int(t.size()?.into(), IntKind::UInt)),
        ExprKind::Unary(op, x) => {
            let (v, k) = int_of(eval(x)?)?;
            let k = k.promoted();
            Some(match op {
                UnOp::Neg => Int(k.wrap(-v), k),
                UnOp::Plus => Int(v, k),
                UnOp::BitNot => Int(k.wrap(!v), k),
                UnOp::Not => Int((v == 0) as i64, IntKind::Int),
            })
        }
        ExprKind::Binary(BinOp::LogAnd, l, r) => {
            let l = int_of(eval(l)?)?.0 != 0;
            Some(Int((l && int_of(eval(r)?)?.0 != 0) as i64, IntKind::Int))
        }
        ExprKind::Binary(BinOp::LogOr, l, r) => {
            let l = int_of(eval(l)?)?.0 != 0;
            Some(Int((l || int_of(eval(r)?)?.0 != 0) as i64, IntKind::Int))
        }
        ExprKind::Binary(op, l, r) => {
            let (lc, rc) = (eval(l)?, eval(r)?);
            match (lc, rc) {
                (Int(a, ak), Int(b, bk)) => binary(*op, a, ak, b, bk),
                // Address arithmetic: pointer plus or minus an integer.
                (Addr { base, offset }, Int(n, _)) if matches!(op, BinOp::Add | BinOp::Sub) => {
                    let size = i64::from(l.ty.pointee()?.size()?);
                    let n = if *op == BinOp::Sub { -n } else { n };
                    Some(Addr {
                        base,
                        offset: offset + n * size,
                    })
                }
                (Int(n, _), Addr { base, offset }) if *op == BinOp::Add => {
                    let size = i64::from(r.ty.pointee()?.size()?);
                    Some(Addr {
                        base,
                        offset: offset + n * size,
                    })
                }
                _ => None,
            }
        }
        ExprKind::Cond(c, a, b) => {
            let c = int_of(eval(c)?)?.0;
            match (eval(a)?, eval(b)?) {
                // The result has the operands' common type, whichever is chosen.
                (Int(x, xk), Int(y, yk)) => {
                    let k = common(xk, yk);
                    Some(Int(k.wrap(if c != 0 { x } else { y }), k))
                }
                (x, y) => Some(if c != 0 { x } else { y }),
            }
        }
        ExprKind::Cast { x, .. } => {
            let v = eval(x)?;
            match (&e.ty, v) {
                (Type::Int(k), Int(v, _)) => Some(Int(k.wrap(v), *k)),
                (Type::Ptr(_), Int(v, _)) => Some(Addr {
                    base: None,
                    offset: IntKind::UInt.wrap(v),
                }),
                (Type::Ptr(_), a @ Addr { .. }) => Some(a),
                _ => None,
            }
        }
        // Addresses, only in a checked tree.
        ExprKind::Str(bytes) => Some(Addr {
            base: Some(Base::Str(bytes.clone())),
            offset: 0,
        }),
        // An array names its address; any other variable's value is not constant.
        ExprKind::Var {
            var: Some(VarRef::Global(sym)),
            ..
        } if e.ty.is_array() => Some(Addr {
            base: Some(Base::Sym(sym.clone())),
            offset: 0,
        }),
        // `*p` of array type is the array at p: its address, like any array.
        ExprKind::Deref(p) if e.ty.is_array() => eval(p),
        ExprKind::Addr(x) => match &x.kind {
            ExprKind::Var {
                var: Some(VarRef::Global(sym)),
                ..
            } => Some(Addr {
                base: Some(Base::Sym(sym.clone())),
                offset: 0,
            }),
            ExprKind::Deref(p) => eval(p),
            ExprKind::Str(_) => eval(x),
            _ => None,
        },
        _ => None,
    }
}

fn int_of(c: Const) -> Option<(i64, IntKind)> {
    match c {
        Const::Int(v, k) => Some((v, k)),
        Const::Addr { .. } => None,
    }
}

/// An arithmetic, bitwise or comparison operator on two integer constants.
fn binary(op: BinOp, a: i64, ak: IntKind, b: i64, bk: IntKind) -> Option<Const> {
    // Shifts take the promoted left operand's type; the others the common type.
    let k = match op {
        BinOp::Shl | BinOp::Shr => ak.promoted(),
        _ => common(ak, bk),
    };
    let (a, b) = (
        k.wrap(a),
        if matches!(op, BinOp::Shl | BinOp::Shr) {
            b
        } else {
            k.wrap(b)
        },
    );
    // The hardware uses the low 5 bits of a shift amount (spec section 10).
    let shift = (b & 31) as u32;
    let v = match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a.wrapping_mul(b),
        BinOp::Div if b == 0 => return None,
        BinOp::Rem if b == 0 => return None,
        // i64 division truncates toward zero, like C; `INT_MIN / -1` wraps below.
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        BinOp::Shl => a << shift,
        // `a` is sign-extended when k is signed (arithmetic shift) and
        // non-negative when it is unsigned (logical shift).
        BinOp::Shr => a >> shift,
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        BinOp::Eq => return Some(Const::Int((a == b) as i64, IntKind::Int)),
        BinOp::Ne => return Some(Const::Int((a != b) as i64, IntKind::Int)),
        BinOp::Lt => return Some(Const::Int((a < b) as i64, IntKind::Int)),
        BinOp::Le => return Some(Const::Int((a <= b) as i64, IntKind::Int)),
        BinOp::Gt => return Some(Const::Int((a > b) as i64, IntKind::Int)),
        BinOp::Ge => return Some(Const::Int((a >= b) as i64, IntKind::Int)),
        BinOp::LogAnd | BinOp::LogOr => unreachable!("handled by eval: they short-circuit"),
    };
    Some(Const::Int(k.wrap(v), k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluates `src` as an untyped expression, the way the parser does.
    fn ev(src: &str) -> Option<Const> {
        let prog =
            crate::parser::parse(&crate::lexer::lex(&format!("int x[1] = {{ {src} }};")).unwrap()).unwrap();
        let Item::Decl(d) = &prog.items[0] else { panic!() };
        let Some(Init::List(items, _)) = &d[0].init else {
            panic!()
        };
        let Init::Expr(e) = &items[0] else { panic!() };
        eval(e)
    }

    fn int(src: &str) -> (i64, IntKind) {
        match ev(src) {
            Some(Const::Int(v, k)) => (v, k),
            c => panic!("{src}: {c:?}"),
        }
    }

    use IntKind::*;

    #[test]
    fn integer_arithmetic_has_c_types() {
        assert_eq!(int("1 + 2 * 3"), (7, Int));
        assert_eq!(int("2147483647 + 1"), (i32::MIN as i64, Int), "wraps");
        assert_eq!(int("-1u"), (0xffff_ffff, UInt));
        assert_eq!(int("0u - 1 > 0"), (1, Int), "unsigned comparison");
        assert_eq!(int("-1 < 0u"), (0, Int), "-1 becomes UINT_MAX");
        assert_eq!(int("-7 / 2"), (-3, Int));
        assert_eq!(int("-7 % 2"), (-1, Int));
        assert_eq!(int("(0u - 1) / 0x10000000 + 1"), (16, UInt));
        assert_eq!(int("(-2147483647 - 1) / -1"), (i32::MIN as i64, Int));
        assert_eq!(int("-8 >> 1"), (-4, Int), "arithmetic shift");
        assert_eq!(int("0x80000000 >> 4"), (0x0800_0000, UInt), "logical shift");
        assert_eq!(int("1 << 31"), (i32::MIN as i64, Int));
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
    }

    #[test]
    fn not_constant() {
        assert_eq!(ev("1 / 0"), None);
        assert_eq!(ev("1 % (2 - 2)"), None);
        assert_eq!(ev("y"), None);
        assert_eq!(ev("f()"), None);
        assert_eq!(ev("(1, 2)"), None, "C: comma is not a constant expression");
    }
}
