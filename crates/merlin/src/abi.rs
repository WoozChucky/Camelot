//! How arguments and results travel: the RISC-V integer calling convention
//! (the psABI's "integer calling convention"), ILP32 on RV32 and LP64 on
//! RV64, as clang `--target=riscv32 -mabi=ilp32` and `--target=riscv64
//! -mabi=lp64` implement it, for the types merlin has. A *word* here is a
//! register: 4 bytes on RV32 (XLEN 32), 8 on RV64.
//!
//! * A scalar (an integer or a pointer) takes one argument register. One
//!   narrower than the register travels extended as the psABI says: by its
//!   signedness up to 32 bits, then sign-extended to the register, so on
//!   RV64 an `unsigned int` 0xffffffff is -1 in its register (see "The 32-bit
//!   invariant" in [`crate::codegen`]).
//! * A struct or union of at most one word takes one register, and one of up
//!   to two words (8 bytes on RV32, 16 on RV64) a pair: its bytes as they
//!   lie in memory, loaded as words (the first word in the lower-numbered
//!   register; bytes past the end are undefined).
//! * A bigger struct or union is passed by reference: the caller copies it
//!   and passes the copy's address in one register. The callee may change
//!   the copy.
//! * Arguments take the words of a sequence in order: words 0 to 7 are `a0`
//!   to `a7`, word 8 is at `0(sp)` when the call is made, word 9 one word
//!   above, and so on. A pair that starts in `a7` has its second word at
//!   `0(sp)`. The caller makes room for the stack words; the callee finds
//!   them at its `s0`, the `sp` it was called with.
//! * The arguments a variadic function's `...` takes travel exactly like
//!   named ones (each promoted first: `char` and `short` to `int`). The
//!   callee stores the argument registers its named parameters leave free
//!   right below its incoming `sp`, next to the words on the stack, so that
//!   every variadic argument lies in one run of words that `va_arg` walks
//!   (see [`crate::codegen`]).
//! * A result comes back the way it would be passed as the first argument:
//!   a scalar or a small struct in `a0` (and `a1`). A bigger struct is
//!   returned through memory: the caller passes its address as a hidden
//!   first argument in `a0`, which moves every other argument one register
//!   up, and the callee stores the result there.
//!
//! Without `long long` and `double` on RV32, and without `__int128` and
//! `long double` on RV64, there is no type aligned to two words, so the
//! even-register alignment the psABI asks of those never comes up, and
//! without floating point the hardware-float rules do not apply.

use crate::types::{ptr_size, FuncType, Type, Xlen};

/// How one argument (or result) travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// An integer or pointer, in one register.
    Scalar,
    /// A struct or union of at most two words, as 1 or 2 words in registers.
    Words(u32),
    /// A struct or union of more than two words: the address of a copy.
    Ref,
}

impl Pass {
    /// How many argument words (registers, or stack words past `a7`) it takes.
    pub fn registers(self) -> usize {
        match self {
            Pass::Scalar | Pass::Ref => 1,
            Pass::Words(n) => n as usize,
        }
    }
}

/// How a value of type `ty` (complete, not void) travels on the target.
pub fn classify(ty: &Type, x: Xlen) -> Pass {
    if !ty.is_record() {
        return Pass::Scalar;
    }
    let word = ptr_size(x);
    match ty.size(x).expect("a complete struct or union") {
        n if n <= word => Pass::Words(1),
        n if n <= 2 * word => Pass::Words(2),
        _ => Pass::Ref,
    }
}

/// Whether a function returning `ret` gets the address for its result as a
/// hidden first argument (a struct or union bigger than two registers).
pub fn returns_in_memory(ret: &Type, x: Xlen) -> bool {
    ret.is_record() && classify(ret, x) == Pass::Ref
}

/// Where each named parameter of a function goes: see [`call_arguments`].
pub fn arguments(f: &FuncType, x: Xlen) -> (Vec<(usize, Pass)>, usize) {
    call_arguments(&f.ret, &f.params, x)
}

/// Where each argument of a call goes, given the function's result type and
/// the arguments' types (a variadic call's after promotion): the number of
/// its first word (words below [`ARG_REGS`] are registers, `aN` for word N)
/// and how it is passed, plus the number of words taken in all (counting the
/// hidden result address). Every type must be complete.
pub fn call_arguments(ret: &Type, args: &[Type], x: Xlen) -> (Vec<(usize, Pass)>, usize) {
    let mut next = usize::from(returns_in_memory(ret, x));
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let pass = classify(a, x);
        out.push((next, pass));
        next += pass.registers();
    }
    (out, next)
}

/// Argument registers: `a0` to `a7`, the first words of the arguments.
pub const ARG_REGS: usize = 8;

/// How many words of `total` go on the stack.
pub fn stack_words(total: usize) -> usize {
    total.saturating_sub(ARG_REGS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{IntKind, Record};
    use crate::Pos;

    fn record(size: u32) -> Type {
        let r = Record::new(false, Some("s".into()), Pos::default());
        r.complete(
            vec![("bytes".into(), Type::array(Type::Int(IntKind::Char), Some(size)))],
            Xlen::Rv32,
        );
        Type::Record(r)
    }

    #[test]
    fn classification() {
        assert_eq!(classify(&Type::INT, Xlen::Rv32), Pass::Scalar);
        assert_eq!(classify(&Type::ptr(Type::Void), Xlen::Rv32), Pass::Scalar);
        assert_eq!(classify(&record(1), Xlen::Rv32), Pass::Words(1));
        assert_eq!(classify(&record(4), Xlen::Rv32), Pass::Words(1));
        assert_eq!(classify(&record(5), Xlen::Rv32), Pass::Words(2));
        assert_eq!(classify(&record(8), Xlen::Rv32), Pass::Words(2));
        assert_eq!(classify(&record(9), Xlen::Rv32), Pass::Ref);
        assert!(returns_in_memory(&record(12), Xlen::Rv32));
        assert!(!returns_in_memory(&record(8), Xlen::Rv32));
        assert!(!returns_in_memory(&Type::INT, Xlen::Rv32));
    }

    #[test]
    fn classification_on_rv64() {
        let x = Xlen::Rv64;
        assert_eq!(classify(&Type::Int(IntKind::Long), x), Pass::Scalar);
        // A word is 8 bytes: one register up to 8, a pair up to 16.
        assert_eq!(classify(&record(8), x), Pass::Words(1));
        assert_eq!(classify(&record(9), x), Pass::Words(2));
        assert_eq!(classify(&record(16), x), Pass::Words(2));
        assert_eq!(classify(&record(17), x), Pass::Ref);
        assert!(returns_in_memory(&record(24), x));
        assert!(!returns_in_memory(&record(12), x));
    }

    #[test]
    fn register_assignment() {
        let f = FuncType {
            ret: Type::INT,
            params: vec![Type::INT, record(6), record(16), record(2)],
            variadic: false,
        };
        let (regs, total) = arguments(&f, Xlen::Rv32);
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
            variadic: false,
        };
        assert_eq!(arguments(&f, Xlen::Rv32), (vec![(1, Pass::Scalar)], 2));
        // Past a7, words go on the stack; a pair may straddle a7 and the stack.
        let args = [
            Type::INT,
            Type::INT,
            Type::INT,
            Type::INT,
            Type::INT,
            Type::INT,
            Type::INT,
            record(8),
            Type::INT,
        ];
        let (words, total) = call_arguments(&Type::INT, &args, Xlen::Rv32);
        assert_eq!(words[7..], [(7, Pass::Words(2)), (9, Pass::Scalar)]);
        assert_eq!((total, stack_words(total)), (10, 2));
        assert_eq!(stack_words(3), 0);
    }
}
