//! Constant expressions: values known at compile time.
//!
//! Three users: the parser sizes arrays (`int a[4 * sizeof(int)]`), sema finds
//! null pointer constants and checks that global initializers are constant,
//! and code generation turns those initializers into data and folds every
//! integer constant expression in code into one constant, as clang does, and
//! decides the conditions clang's code generator decides ([`condition`],
//! which also knows that an object's address is not null).
//!
//! Integer constants carry their C type and wrap the way the generated code
//! would at run time on the target (each function takes it: an `int` is 32
//! bits on both widths, a `long` 64 on RV64). Values are `i128`s, so every
//! value of every type, `unsigned long` on LP64 included, is the number it
//! is. The parser calls [`parsed_int`] before sema has typed the
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

/// The value of an integer constant expression, in a checked tree.
pub fn int(e: &Expr, x: Xlen) -> Option<i128> {
    int_of(eval(e, x)?).map(|(v, _)| v)
}

/// [`int`] before sema has typed the tree, for the parser: types come from
/// the leaves, so an operand `?:` does not choose must be constant here,
/// for its type, which the result's depends on. The parser first turns one
/// that is not into a 0 of the type sema would give it (`1 ? 2 : f()`:
/// `Parser::resolve_sizeof`), so in its contexts too only its type counts.
pub fn parsed_int(e: &Expr, x: Xlen) -> Option<i128> {
    int_of(value(e, x, true, false)?).map(|(v, _)| v)
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

/// The constant value of `e`, in a tree sema has checked, or `None` if it
/// is not a constant expression merlin can evaluate (that includes dividing
/// by zero, where the division is evaluated).
pub fn eval(e: &Expr, xlen: Xlen) -> Option<Const> {
    value(e, xlen, true, true)
}

/// [`eval`] of an operand that is `evaluated` or not, in a tree sema has
/// `checked` or not. C99 6.6p3 and p11 hold a constant expression to what
/// it evaluates: in `1 ? 2 : 1 / 0` the third operand is never divided,
/// only its type counts (the result's is the operands' common type), so
/// there dividing by zero is no error and gives 0, a value nothing reads.
/// clang has it so: `int a[1 ? 2 : 1 / 0];` has two elements. In a checked
/// tree the operand `?:` does not choose need not be constant at all, as
/// the right operand of `0 && h()` need not: sema gave it its type, and
/// `1 ? 2 : h()` is 2, which clang folds too.
fn value(e: &Expr, xlen: Xlen, evaluated: bool, checked: bool) -> Option<Const> {
    use Const::*;
    let eval = |e: &Expr, xlen: Xlen| value(e, xlen, evaluated, checked);
    match &e.kind {
        ExprKind::Num(v) | ExprKind::Enumerator { value: v, .. } => Some(Int(*v, e.ty.int_kind()?)),
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
            // Only the chosen operand is evaluated; of the other only the
            // type counts: sema's, or before sema what its value has.
            let (chosen, other) = if int_of(eval(c, xlen)?)?.0 != 0 {
                (a, b)
            } else {
                (b, a)
            };
            let v = eval(chosen, xlen)?;
            let other = if checked {
                other.ty.int_kind()
            } else {
                int_of(value(other, xlen, false, false)?).map(|(_, k)| k)
            };
            match (v, other) {
                // The result has the operands' common type, whichever is chosen.
                (Int(x, xk), Some(yk)) => {
                    let k = common(xk, yk, xlen);
                    Some(Int(k.wrap(x, xlen), k))
                }
                (v, _) => Some(v),
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
            addr_of(e, xlen, evaluated, checked)
        }
        ExprKind::Addr(x) => addr_of(x, xlen, evaluated, checked),
        _ => None,
    }
}

/// The address of an lvalue, if it is an address constant: a global object
/// (or a member or element of one, at a constant offset), a string literal,
/// or what a constant pointer points to.
fn addr_of(e: &Expr, xlen: Xlen, evaluated: bool, checked: bool) -> Option<Const> {
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
        ExprKind::Deref(p) => match value(p, xlen, evaluated, checked)? {
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
                value(base, xlen, evaluated, checked)?
            } else {
                addr_of(base, xlen, evaluated, checked)?
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

// ---- conditions ----

/// The value clang's code generator folds a condition to at `-O0`
/// (`CodeGenFunction::ConstantFoldsToSimpleInteger`, which asks
/// `Expr::EvaluateAsInt` in its folding mode), or `None` if it does not
/// fold it. That is an integer constant expression's value, and more:
/// what is known of where objects are, never null and never one another,
/// as clang 23 folds it.
///
/// * An object's address is not null: a global's, a local's (a
///   parameter's too), a string literal's, a compound literal's, a
///   function's. So `&g != 0` is 1, `!&g` is 0 and `&g && x` is `x`'s
///   truth.
/// * Two objects' addresses differ (`&g == &h` is 0), unless one points
///   just past its object and the other at its start, where the two may
///   meet (`&g + 1 == &h`, but `&a[4] == &s.n` is 0), or one is an array
///   of unknown size, which may hold nothing (`extern int ea[];`: `ea ==
///   &g`), or both are string literals whose bytes could be shared (`"a"
///   == "a"`, not `&"ab"[1] == "ab"`). An address knows it is just past
///   its object only while it designates something ([`Designator`]): not
///   once cast to a pointer to another type, so `(char *)&g + 4 == (char
///   *)&h` is 0.
/// * Addresses in one object are equal and subtract by their offsets
///   (`&a[2] - &a[0]` is 2), and are ordered by them inside the object or
///   just past its end (`&a[1] < &a[2]` is 1, `&a[1] < &a[5]` not known).
///
/// What is not known stays code: a pointer variable's value (`p != 0`), an
/// address converted to an integer (`(long)&g`), two objects' order
/// (`&g < &h`). A condition of pointer type is not folded at all
/// (`if (&g)`): clang folds only integer ones.
pub fn condition(e: &Expr, x: Xlen) -> Option<i128> {
    if !e.ty.is_integer() {
        return None;
    }
    match fold(e, x)? {
        Folded::Int(v, _) => Some(v),
        Folded::Addr(..) => None,
    }
}

/// A value [`condition`] works out: an integer, or an address in an object
/// (none: a plain number used as an address, `(char *)16`, or null), at an
/// offset in bytes, with what the address designates ([`Designator`]).
#[derive(Clone, Debug)]
enum Folded {
    Int(i128, IntKind),
    Addr(Option<Object>, i128, Designator),
}

/// What an address designates, as clang's evaluator tracks it
/// (`SubobjectDesignator`): the bytes of the object or subobject it is in,
/// from the start of its object, which arithmetic may move it across up to
/// just past the end (`&a[4]` of an `int a[4]`, `&s.m + 1`); or `None`
/// once it no longer designates one (`Invalid`): moved out of them
/// (`&a[5]`), or cast to a pointer to another type (`(char *)&g`) but
/// `void *`. Two objects' addresses are equal only where one is just past
/// the end of its object, and clang knows that only of an address that
/// still designates something (or one into an object of incomplete type).
type Designator = Option<(i128, i128)>;

/// An object whose address [`condition`] knows: what it is, and its size in
/// bytes (none for a function or an object of incomplete type).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Object {
    what: Place,
    size: Option<i128>,
    /// An object of incomplete type (`extern struct s es;`).
    incomplete: bool,
    /// An array of unknown size (`extern int ea[];`), which may hold no
    /// element at all: clang's `isZeroSized`, whose address it never
    /// compares with another object's, since the two could be one.
    zero_sized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    /// A global symbol: a global variable, a `static` local, a function.
    Sym(String),
    /// A local of the function (a parameter too, a compound literal in a
    /// function).
    Local(LocalId),
    /// A string literal.
    Str(Vec<u8>),
    /// A compound literal of the file's scope, by the address of its
    /// expression: each is an object of its own.
    Literal(usize),
}

/// Whether a value is true, as a condition tests it.
fn truth(v: &Folded) -> bool {
    match v {
        Folded::Int(v, _) => *v != 0,
        Folded::Addr(Some(_), ..) => true,
        Folded::Addr(None, offset, _) => *offset != 0,
    }
}

fn fold(e: &Expr, x: Xlen) -> Option<Folded> {
    use Folded::*;
    // An integer constant expression, as everywhere.
    if let Some(Const::Int(v, k)) = eval(e, x) {
        return Some(Int(v, k));
    }
    let as_int = |v: Folded| match v {
        Int(v, k) => Some((v, k)),
        Addr(..) => None,
    };
    // An address moved by `by` bytes: it designates what it did while it
    // stays in it, up to just past its end.
    let moved = |o: Option<Object>, at: i128, d: Designator, by: i128| {
        let to = at + by;
        Addr(o, to, d.filter(|&(lo, hi)| (lo..=hi).contains(&to)))
    };
    match &e.kind {
        ExprKind::Unary(UnOp::Not, a) => Some(Int((!truth(&fold(a, x)?)).into(), IntKind::Int)),
        ExprKind::Unary(op, a) => {
            let (v, k) = as_int(fold(a, x)?)?;
            let k = k.promoted();
            Some(Int(
                match op {
                    UnOp::Neg => k.wrap(-v, x),
                    UnOp::BitNot => k.wrap(!v, x),
                    _ => v,
                },
                k,
            ))
        }
        // Short-circuits: what the left operand decides is enough.
        ExprKind::Binary(op @ (BinOp::LogAnd | BinOp::LogOr), l, r) => {
            let l = truth(&fold(l, x)?);
            let v = if l == (*op == BinOp::LogOr) {
                l
            } else {
                truth(&fold(r, x)?)
            };
            Some(Int(v.into(), IntKind::Int))
        }
        ExprKind::Binary(op, l, r) => match (fold(l, x)?, fold(r, x)?) {
            (Int(a, ak), Int(b, bk)) => match binary(*op, (a, ak), (b, bk), x, true)? {
                Const::Int(v, k) => Some(Int(v, k)),
                Const::Addr { .. } => None,
            },
            // A pointer moved by whole elements.
            (Addr(o, at, d), Int(n, _)) if matches!(op, BinOp::Add | BinOp::Sub) => {
                let n = if *op == BinOp::Sub { -n } else { n };
                Some(moved(o, at, d, n * step(&l.ty, x)?))
            }
            (Int(n, _), Addr(o, at, d)) if *op == BinOp::Add => Some(moved(o, at, d, n * step(&r.ty, x)?)),
            (Addr(a, at, ad), Addr(b, bt, bd)) => compare(*op, (a, at, ad), (b, bt, bd), &l.ty, x),
            // An address compared with a null pointer constant, `&g != 0`.
            (Addr(a, at, ad), Int(n, _)) => compare(*op, (a, at, ad), (None, n, None), &l.ty, x),
            (Int(n, _), Addr(b, bt, bd)) => compare(*op, (None, n, None), (b, bt, bd), &r.ty, x),
        },
        ExprKind::Cond(c, a, b) => {
            let v = fold(if truth(&fold(c, x)?) { a } else { b }, x)?;
            match (v, e.ty.int_kind()) {
                (Int(v, _), Some(k)) => Some(Int(k.wrap(v, x), k)),
                (v, _) => Some(v),
            }
        }
        ExprKind::Cast { x: a, .. } => match (e.ty.unqual(), fold(a, x)?) {
            (Type::Int(IntKind::Bool), v) => Some(Int(truth(&v).into(), IntKind::Bool)),
            (Type::Int(k), Int(v, _)) => Some(Int(k.wrap(v, x), *k)),
            // A plain number used as an address is that number again; an
            // object's address as a number is not known.
            (Type::Int(k), Addr(None, at, _)) => Some(Int(k.wrap(at, x), *k)),
            (Type::Ptr(_), Int(v, _)) => Some(Addr(None, IntKind::ULong.wrap(v, x), None)),
            // A pointer cast to another pointer type designates nothing any
            // more (clang's `CK_BitCast`), but to `void *` or to a pointer
            // to a similar type (`CK_NoOp`: qualifiers added); an array or
            // a function decays to a pointer to its elements or to itself
            // first.
            (Type::Ptr(to), Addr(o, at, d)) => {
                let keeps = to.is_void()
                    || match a.ty.unqual() {
                        Type::Ptr(from) | Type::Array(from, _) => from.similar(to),
                        f @ Type::Func(_) => f.similar(to),
                        _ => true,
                    };
                Some(Addr(o, at, d.filter(|_| keeps)))
            }
            _ => None,
        },
        // An array or a function stands for its address.
        ExprKind::Str(_)
        | ExprKind::Var { .. }
        | ExprKind::Deref(_)
        | ExprKind::Member { .. }
        | ExprKind::CompoundLit { .. }
            if e.ty.is_array() || e.ty.is_func() =>
        {
            place(e, x)
        }
        ExprKind::Addr(a) => place(a, x),
        _ => None,
    }
}

/// The address of an lvalue, if [`condition`] knows it.
fn place(e: &Expr, x: Xlen) -> Option<Folded> {
    let size = e.ty.size(x).map(i128::from);
    let object = |what: Place| {
        // An incomplete type has no size; a function has none either.
        let incomplete = size.is_none() && !e.ty.is_func();
        Some(Folded::Addr(
            Some(Object {
                what,
                size,
                incomplete,
                zero_sized: incomplete && e.ty.is_array(),
            }),
            0,
            Some((0, size.unwrap_or(0))),
        ))
    };
    match &e.kind {
        ExprKind::Var {
            var: Some(VarRef::Global(sym)),
            ..
        } => object(Place::Sym(sym.clone())),
        ExprKind::Var {
            var: Some(VarRef::Local(id)),
            ..
        } => object(Place::Local(*id)),
        ExprKind::Str(bytes) => object(Place::Str(bytes.clone())),
        // A compound literal is an object, never null (sema gives one in
        // a function a local).
        ExprKind::CompoundLit { local: Some(id), .. } => object(Place::Local(*id)),
        ExprKind::CompoundLit { local: None, .. } => object(Place::Literal(e as *const Expr as usize)),
        ExprKind::Deref(p) => match fold(p, x)? {
            // An array element that is an array itself (`m[1]` of an
            // `int m[2][4]`) is the subobject designated.
            Folded::Addr(o, at, d) if e.ty.is_array() => {
                Some(Folded::Addr(o, at, d.and(size.map(|s| (at, at + s)))))
            }
            a @ Folded::Addr(..) => Some(a),
            Folded::Int(..) => None,
        },
        // A member is the subobject designated.
        ExprKind::Member {
            base,
            arrow,
            offset: member,
            ..
        } => match if *arrow { fold(base, x)? } else { place(base, x)? } {
            Folded::Addr(o, at, d) => {
                let at = at + i128::from(*member);
                Some(Folded::Addr(o, at, d.and(size.map(|s| (at, at + s)))))
            }
            Folded::Int(..) => None,
        },
        _ => None,
    }
}

/// `a op b` of two addresses, `l` the type of the left one: a comparison
/// or a difference, where what is known of the objects decides it, as
/// clang's `EvaluateComparisonBinaryOperator` decides it.
fn compare(
    op: BinOp,
    (a, at, ad): (Option<Object>, i128, Designator),
    (b, bt, bd): (Option<Object>, i128, Designator),
    l: &Type,
    x: Xlen,
) -> Option<Folded> {
    let int = |v: bool| Some(Folded::Int(v.into(), IntKind::Int));
    // Two string literals may share their bytes, or not: they are at one
    // address only if they could be, their bytes the same where they would
    // overlap (clang's `ArePotentiallyOverlappingStringLiterals`).
    if let (
        Some(Object {
            what: Place::Str(l), ..
        }),
        Some(Object {
            what: Place::Str(r), ..
        }),
    ) = (&a, &b)
    {
        if overlap((l, at), (r, bt)) {
            return None;
        }
        return match op {
            BinOp::Eq => int(false),
            BinOp::Ne => int(true),
            _ => None,
        };
    }
    let same = match (&a, &b) {
        // In one object (or two plain numbers): by the offsets.
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        // An object's address is not null; nor any plain number but 0.
        (Some(_), None) | (None, Some(_)) => {
            let null = if a.is_none() { at == 0 } else { bt == 0 };
            return match op {
                BinOp::Eq if null => int(false),
                BinOp::Ne if null => int(true),
                _ => None,
            };
        }
    };
    if same {
        // Ordered only inside the object, or just past its end, whose size
        // must be known: `&a[1] < &a[5]` of an `int a[4]` is not folded, nor
        // any order in an array of unknown size.
        let inside = |at: i128| {
            a.as_ref()
                .is_none_or(|o| o.size.is_some_and(|size| (0..=size).contains(&at)))
        };
        let ordered = inside(at) && inside(bt);
        return match op {
            BinOp::Eq => int(at == bt),
            BinOp::Ne => int(at != bt),
            BinOp::Lt if ordered => int(at < bt),
            BinOp::Le if ordered => int(at <= bt),
            BinOp::Gt if ordered => int(at > bt),
            BinOp::Ge if ordered => int(at >= bt),
            BinOp::Sub => Some(Folded::Int((at - bt) / step(l, x)?, IntKind::ptrdiff_t(x))),
            _ => None,
        };
    }
    // Two objects: never at one address, unless one may hold nothing at
    // all, or one points just past its end and the other at its start,
    // where the two could meet (C++ DR1652, which clang applies to C).
    let (a, b) = (a?, b?);
    if a.zero_sized || b.zero_sized {
        return None;
    }
    // Just past the end: what is known of an address that designates
    // something, by its offset; of one that does not, only that it may be
    // if its object's size is unknown (`isOnePastTheEndOfCompleteObject`).
    let past = |o: &Object, at: i128, d: Designator| match d {
        Some(_) => o.size == Some(at),
        None => o.incomplete,
    };
    if (bt == 0 && past(&a, at, ad)) || (at == 0 && past(&b, bt, bd)) {
        return None;
    }
    match op {
        BinOp::Eq => int(false),
        BinOp::Ne => int(true),
        _ => None,
    }
}

/// Whether string literals `l` and `r` (their bytes, without the NUL), at
/// offsets `at` and `bt` in them, could be at one address: where the two
/// would overlap, their bytes (the NULs too) are the same.
fn overlap((l, at): (&[u8], i128), (r, bt): (&[u8], i128)) -> bool {
    let byte = |s: &[u8], i: i128| match usize::try_from(i) {
        Ok(i) if i < s.len() => Some(s[i]),
        Ok(i) if i == s.len() => Some(0),
        _ => None,
    };
    // From where the earlier start would be to where the earlier end is.
    let from = -(at.min(bt));
    let to = (l.len() as i128 + 1 - at).min(r.len() as i128 + 1 - bt);
    (from..to).all(|d| byte(l, at + d) == byte(r, bt + d))
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
        value(e, x, true, false)
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
        // Before sema, the operand `?:` does not choose has a type here only
        // if it is constant (the parser gives `f()` its type first: its
        // `the_operand_not_chosen_counts_by_its_type`; codegen's tests fold
        // `1 ? 2 : h()` once sema has typed it).
        assert_eq!(ev("1 ? 2 : f()"), None);
        assert_eq!(ev("0 ? f() : 2"), None);
    }

    /// What [`condition`] folds of `cond`, the condition of an `if` in a
    /// checked program with globals `g`, `h`, `a[4]`, `s`, arrays of
    /// unknown size `ea` and `eb`, a function `fn`, a parameter `x` and
    /// locals `l` and `p` (pointing to `l`).
    fn cond(src: &str) -> Option<i128> {
        let prog = crate::parse(&format!(
            "int g, h, a[4]; struct {{ int m, n; }} s; int fn(void); extern int ea[], eb[];\n\
             int f(int x) {{ int l = x; int *p = &l; if ({src}) return 1; return 0; }}"
        ))
        .unwrap_or_else(|e| panic!("{src}: {e:?}"));
        let f = prog.funcs().find(|f| f.name == "f").unwrap();
        let body = f.body.as_ref().unwrap();
        let StmtKind::If { cond, .. } = &body[2].kind else {
            panic!("{body:?}")
        };
        condition(cond, Xlen::Rv32)
    }

    /// clang 23 folds a condition of addresses it knows: an object's is not
    /// null, nor another's, and those in one object go by their offsets.
    #[test]
    fn conditions_of_addresses_fold_as_clangs() {
        for (src, v) in [
            ("&g != 0", 1),
            ("&g == 0", 0),
            ("0 != &g", 1),
            ("&g != (void *)0", 1),
            ("&a[1] == 0", 0),
            ("a != 0", 1),
            ("&s.n != 0", 1),
            ("\"s\" != 0", 1),
            ("fn != 0", 1),
            ("&x != 0", 1),
            ("&l != 0", 1),
            ("!&g", 0),
            ("&g && 1", 1),
            ("(&g ? 2 : 0) == 2", 1),
            ("1 && &g != 0", 1),
            ("(&g != 0) + 1", 2),
            ("&g == &h", 0),
            ("&g == &g", 1),
            ("&l == &x", 0),
            ("&a[1] == &g", 0),
            ("&a[5] == &g", 0),
            ("&a[5] != 0", 1),
            ("(void *)&g == (void *)&s", 0),
            ("&s.m == &s.n", 0),
            ("&a[1] < &a[2]", 1),
            ("&a[2] - &a[0]", 2),
            ("&g - &g", 0),
            ("&g + 1 != 0", 1),
            ("\"a\" != (char *)&g", 1),
            ("&\"ab\"[1] == \"ab\"", 0),
            ("(char *)0 == 0", 1),
            ("(char *)16 != 0", 1),
            // Just past the end of one object, the start of no other.
            ("&a[4] == &s.n", 0),
            ("&s.m + 1 == &g", 0),
            // Out of what it designates (`&s.m + 2`), or cast to a pointer
            // to another type, an address designates nothing, and is just
            // past the end of nothing.
            ("&s.m + 2 == &g", 0),
            ("(char *)&g + 4 == (char *)&h", 0),
            ("(int *)((char *)&a + 16) == &h", 0),
            ("(char *)a + 16 == (char *)&g", 0),
            ("(unsigned *)&a[4] == (unsigned *)&g", 0),
            ("(int *)(void *)&a[4] == &g", 0),
            // In one object, equal and subtracted by the offsets, out of it
            // too; ordered inside it or just past its end.
            ("&a[5] == &a[5]", 1),
            ("&a[6] - &a[0]", 6),
            ("&a[0] <= &a[4]", 1),
            ("(char *)&a + 1 < (char *)&a + 2", 1),
            ("&s.m + 2 > &s.m", 1),
            // An array of unknown size: not null, and equal and subtracted
            // in itself.
            ("ea != 0", 1),
            ("&ea[1] == &ea[1]", 1),
            ("&ea[2] == &ea[3]", 0),
            ("&ea[5] - &ea[1]", 4),
            // A compound literal is an object.
            ("&(int){1} != 0", 1),
            ("&(int){1} == &g", 0),
            ("&(int){1} == &(int){1}", 0),
        ] {
            assert_eq!(cond(src), Some(v), "{src}");
        }
        // What is not known: a pointer's value, an address as a number,
        // two objects' order, one just past its object, literals that could
        // share bytes, an address as the condition itself.
        for src in [
            "p != 0",
            "(long)&g != 0",
            "&g < &h",
            "&g + 1 == &h",
            "&a[4] == &g",
            "\"a\" == \"a\"",
            "&g == (int *)16",
            "&g",
            "x",
            // Just past the end of one object, where another starts: also
            // through `void *`, a cast that keeps what it designates, or
            // one just past a member that ends its struct.
            "&a[1] + 3 == &h",
            "(void *)&a[4] == (void *)&g",
            "(const int *)&a[4] == &g",
            "&s.n + 1 == &g",
            // Ordered out of the object.
            "&a[1] < &a[5]",
            "&a[-1] < &a[1]",
            "(char *)&a + 1 < (char *)&a + 20",
            // An array of unknown size may hold nothing, and so be at
            // another object's address (clang's `isZeroSized`), whatever
            // the offsets; and its addresses are not ordered.
            "ea == &g",
            "&ea[3] == &g",
            "&ea[1] == &s.n",
            "ea == eb",
            "(char *)&ea[3] == (char *)&g",
            "&ea[2] < &ea[3]",
            "(char *)&ea[1] < (char *)&ea[2]",
        ] {
            assert_eq!(cond(src), None, "{src}");
        }
    }
}
