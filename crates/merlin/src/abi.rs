//! How arguments and results travel: the RISC-V integer calling convention
//! (the psABI's "integer calling convention"), LP64, as clang
//! `--target=riscv64 -mabi=lp64` implements it, for the types merlin has. A
//! *word* here is a register: 8 bytes ([`WORD`]).
//!
//! * A scalar (an integer or a pointer, `long long` included) takes one
//!   argument register. One narrower than the register travels extended as
//!   the psABI says: by its signedness up to 32 bits, then sign-extended to
//!   the register, so an `unsigned int` 0xffffffff is -1 in its register
//!   (see "The 32-bit invariant" in [`crate::codegen`]).
//! * A struct or union of at most one word takes one register, and one of up
//!   to two words (16 bytes) a pair: its bytes as they lie in memory, loaded
//!   as words (the first word in the lower-numbered register; bytes past the
//!   end are undefined).
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
//! The psABI starts a pair aligned to two words (an `__int128`, a `long
//! double`, a struct holding one) at an even word on the stack, and in an
//! even register when variadic; merlin has none of those types, so every
//! argument simply takes the next word. (On RV32, which Excalibur had until
//! #159, a `long long` was such a pair.)
//!
//! Without floating point the hardware-float rules do not apply.

use crate::types::{FuncType, Type, PTR_SIZE};

/// The bytes of a register, and of an argument word.
pub const WORD: u32 = PTR_SIZE;

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

/// How a value of type `ty` (complete, not void) travels.
pub fn classify(ty: &Type) -> Pass {
    if !ty.is_record() {
        return Pass::Scalar;
    }
    match ty.size().expect("a complete struct or union") {
        n if n <= WORD => Pass::Words(1),
        n if n <= 2 * WORD => Pass::Words(2),
        _ => Pass::Ref,
    }
}

/// Whether a function returning `ret` gets the address for its result as a
/// hidden first argument (a struct or union bigger than two registers).
pub fn returns_in_memory(ret: &Type) -> bool {
    ret.is_record() && classify(ret) == Pass::Ref
}

/// Where each named parameter of a function goes: see [`call_arguments`].
pub fn arguments(f: &FuncType) -> (Vec<(usize, Pass)>, usize) {
    call_arguments(&f.ret, &f.params)
}

/// Where each argument of a call goes, given the function's result type and
/// the arguments' types (a variadic call's after promotion): the number of
/// its first word (words below [`ARG_REGS`] are registers, `aN` for word N)
/// and how it is passed, plus the number of words taken in all (counting the
/// hidden result address). Every type must be complete.
pub fn call_arguments(ret: &Type, args: &[Type]) -> (Vec<(usize, Pass)>, usize) {
    let mut next = usize::from(returns_in_memory(ret));
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let pass = classify(a);
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
        r.complete(vec![(
            "bytes".into(),
            Type::array(Type::Int(IntKind::Char), Some(size)),
        )]);
        Type::Record(r)
    }

    #[test]
    fn classification() {
        assert_eq!(classify(&Type::INT), Pass::Scalar);
        assert_eq!(classify(&Type::Int(IntKind::Long)), Pass::Scalar);
        assert_eq!(classify(&Type::Int(IntKind::LongLong)), Pass::Scalar);
        assert_eq!(classify(&Type::ptr(Type::Void)), Pass::Scalar);
        // A word is 8 bytes: one register up to 8, a pair up to 16.
        assert_eq!(classify(&record(1)), Pass::Words(1));
        assert_eq!(classify(&record(8)), Pass::Words(1));
        assert_eq!(classify(&record(9)), Pass::Words(2));
        assert_eq!(classify(&record(16)), Pass::Words(2));
        assert_eq!(classify(&record(17)), Pass::Ref);
        assert!(returns_in_memory(&record(24)));
        assert!(!returns_in_memory(&record(16)));
        assert!(!returns_in_memory(&Type::INT));
    }

    #[test]
    fn register_assignment() {
        let f = FuncType {
            ret: Type::INT,
            params: vec![Type::INT, record(12), record(32), record(2)],
            variadic: false,
            spelled: Default::default(),
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
            ret: record(24),
            params: vec![Type::INT],
            variadic: false,
            spelled: Default::default(),
        };
        assert_eq!(arguments(&f), (vec![(1, Pass::Scalar)], 2));
        // Past a7, words go on the stack; a pair may straddle a7 and the
        // stack, and a long long is one word, after an odd number of others
        // too.
        let args = [
            Type::INT,
            Type::INT,
            Type::INT,
            Type::INT,
            Type::INT,
            Type::INT,
            Type::INT,
            record(16),
            Type::Int(IntKind::LongLong),
        ];
        let (words, total) = call_arguments(&Type::INT, &args);
        assert_eq!(words[7..], [(7, Pass::Words(2)), (9, Pass::Scalar)]);
        assert_eq!((total, stack_words(total)), (10, 2));
        assert_eq!(stack_words(3), 0);
    }
}
