//! How arguments and results travel: the RISC-V ILP32 integer calling
//! convention (the psABI's "integer calling convention", as clang
//! `--target=riscv32 -mabi=ilp32` implements it), for the types merlin has.
//!
//! * A scalar (an integer or a pointer) takes one argument register.
//! * A struct or union of at most 4 bytes takes one register, and one of 5 to
//!   8 bytes a pair: its bytes as they lie in memory, loaded as words (the
//!   first 4 bytes in the lower-numbered register; bytes past the end are
//!   undefined).
//! * A bigger struct or union is passed by reference: the caller copies it
//!   and passes the copy's address in one register. The callee may change
//!   the copy.
//! * Arguments take `a0`, `a1`, ... in order, and merlin passes nothing on
//!   the stack, so they must fit in `a0-a7` (the psABI would put the rest,
//!   or the second half of a pair that does not fit, on the stack).
//! * A result comes back the way it would be passed as the first argument:
//!   a scalar or a small struct in `a0` (and `a1`). A bigger struct is
//!   returned through memory: the caller passes its address as a hidden
//!   first argument in `a0`, which moves every other argument one register
//!   up, and the callee stores the result there.
//!
//! There are no 8-byte aligned types without `long long` and `double`, so the
//! even-register alignment the psABI asks of those never comes up, and
//! without floating point the hardware-float rules do not apply.

use crate::types::{FuncType, Type};

/// How one argument (or result) travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// An integer or pointer, in one register.
    Scalar,
    /// A struct or union of at most 8 bytes, as 1 or 2 words in registers.
    Words(u32),
    /// A struct or union of more than 8 bytes: the address of a copy.
    Ref,
}

impl Pass {
    /// How many argument registers it takes.
    pub fn registers(self) -> usize {
        match self {
            Pass::Scalar | Pass::Ref => 1,
            Pass::Words(n) => n as usize,
        }
    }
}

/// How a value of type `ty` (complete, not void) travels.
pub fn classify(ty: &Type) -> Pass {
    if !ty.is_record() {
        return Pass::Scalar;
    }
    match ty.size().expect("a complete struct or union") {
        0..=4 => Pass::Words(1),
        5..=8 => Pass::Words(2),
        _ => Pass::Ref,
    }
}

/// Whether a function returning `ret` gets the address for its result as a
/// hidden first argument (a struct or union bigger than 8 bytes).
pub fn returns_in_memory(ret: &Type) -> bool {
    ret.is_record() && classify(ret) == Pass::Ref
}

/// Where each argument of a call goes: its first register number (`aN`) and
/// how it is passed, plus the number of registers taken in all (counting the
/// hidden result address). Every parameter type must be complete.
pub fn arguments(f: &FuncType) -> (Vec<(usize, Pass)>, usize) {
    let mut next = usize::from(returns_in_memory(&f.ret));
    let mut out = Vec::with_capacity(f.params.len());
    for p in &f.params {
        let pass = classify(p);
        out.push((next, pass));
        next += pass.registers();
    }
    (out, next)
}

/// Argument registers: `a0` to `a7`.
pub const ARG_REGS: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IntKind, Record};
    use crate::Pos;

    fn record(size: u32) -> Type {
        let r = Record::new(false, Some("s".into()), Pos::default());
        r.complete(vec![(
            "bytes".into(),
            Type::array(Type::Int(IntKind::Char), Some(size)),
        )]);
        Type::Record(r)
    }

    #[test]
    fn classification() {
        assert_eq!(classify(&Type::INT), Pass::Scalar);
        assert_eq!(classify(&Type::ptr(Type::Void)), Pass::Scalar);
        assert_eq!(classify(&record(1)), Pass::Words(1));
        assert_eq!(classify(&record(4)), Pass::Words(1));
        assert_eq!(classify(&record(5)), Pass::Words(2));
        assert_eq!(classify(&record(8)), Pass::Words(2));
        assert_eq!(classify(&record(9)), Pass::Ref);
        assert!(returns_in_memory(&record(12)));
        assert!(!returns_in_memory(&record(8)));
        assert!(!returns_in_memory(&Type::INT));
    }

    #[test]
    fn register_assignment() {
        let f = FuncType {
            ret: Type::INT,
            params: vec![Type::INT, record(6), record(16), record(2)],
        };
        let (regs, total) = arguments(&f);
        assert_eq!(
            regs,
            [
                (0, Pass::Scalar),
                (1, Pass::Words(2)),
                (3, Pass::Ref),
                (4, Pass::Words(1))
            ]
        );
        assert_eq!(total, 5);
        // A big result's address comes first, in a0.
        let f = FuncType {
            ret: record(12),
            params: vec![Type::INT],
        };
        assert_eq!(arguments(&f), (vec![(1, Pass::Scalar)], 2));
    }
}
