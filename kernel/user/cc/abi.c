/* cc1: how arguments and results travel (follows merlin's abi.rs): the
 * RISC-V integer calling convention (the psABI's "integer calling
 * convention"), LP64, as clang --target=riscv64 -mabi=lp64 implements it,
 * for the types merlin has. A word here is a register: 8 bytes (WORD).
 *
 * - A scalar (an integer or a pointer, long long included) takes one
 *   argument register. One narrower than the register travels extended as
 *   the psABI says: by its signedness up to 32 bits, then sign-extended to
 *   the register, so an unsigned int 0xffffffff is -1 in its register (see
 *   "The 32-bit invariant" in codegen.c).
 * - A struct or union of at most one word takes one register, and one of
 *   up to two words (16 bytes) a pair: its bytes as they lie in memory,
 *   loaded as words (the first word in the lower-numbered register; bytes
 *   past the end are undefined).
 * - A bigger struct or union is passed by reference: the caller copies it
 *   and passes the copy's address in one register. The callee may change
 *   the copy.
 * - Arguments take the words of a sequence in order: words 0 to 7 are a0
 *   to a7, word 8 is at 0(sp) when the call is made, word 9 one word above,
 *   and so on. A pair that starts in a7 has its second word at 0(sp). The
 *   caller makes room for the stack words; the callee finds them at its s0,
 *   the sp it was called with.
 * - The arguments a variadic function's `...` takes travel exactly like
 *   named ones (each promoted first: char and short to int). The callee
 *   stores the argument registers its named parameters leave free right
 *   below its incoming sp, next to the words on the stack, so that every
 *   variadic argument lies in one run of words that va_arg walks (see
 *   "Variadic functions" in codegen.c).
 * - A result comes back the way it would be passed as the first argument:
 *   a scalar or a small struct in a0 (and a1). A bigger struct is returned
 *   through memory: the caller passes its address as a hidden first
 *   argument in a0, which moves every other argument one register up, and
 *   the callee stores the result there.
 *
 * The psABI starts a pair aligned to two words (an __int128, a long
 * double, a struct holding one) at an even word on the stack, and in an
 * even register when variadic; merlin has none of those types, so every
 * argument simply takes the next word. This is no departure from the
 * psABI for the types there are: it is the rule for each of them.
 *
 * Without floating point, the hardware-float rules do not apply. */
#include "cc.h"

/* How a value of type ty (complete, not void) travels. An array is an
 * aggregate as a struct is: no argument is one, but va_arg may read one. */
int abi_classify(struct type *ty) {
    if (ty_record(ty) == NULL && !ty_is_array(ty)) {
        return PASS_SCALAR;
    }
    int64_t n = ty_size(ty);
    if (n <= WORD) {
        return PASS_WORDS1;
    }
    if (n <= 2 * WORD) {
        return PASS_WORDS2;
    }
    return PASS_REF;
}

/* How many argument words (registers, or stack words past a7) it takes. */
uint abi_registers(int pass) {
    return pass == PASS_WORDS2 ? 2 : 1;
}

/* Whether a function returning ret gets the address for its result as a
 * hidden first argument (a struct or union bigger than two registers). */
int abi_returns_in_memory(struct type *ret) {
    return ty_record(ret) != NULL && abi_classify(ret) == PASS_REF;
}

/* Where each argument of a call goes, given the function's result type and
 * the arguments' types (a variadic call's after promotion): the number of
 * its first word (words below ARG_REGS are registers) in words[i], how it
 * is passed in passes[i]; the number of words taken in all (the hidden
 * result address counted). */
uint abi_call_arguments(struct type *ret, struct type **args, uint n, uint *words, int *passes) {
    uint next = abi_returns_in_memory(ret) ? 1 : 0;
    for (uint i = 0; i < n; i++) {
        int pass = abi_classify(args[i]);
        words[i] = next;
        passes[i] = pass;
        next += abi_registers(pass);
    }
    return next;
}

/* How many words of total go on the stack. */
uint abi_stack_words(uint total) {
    return total > ARG_REGS ? total - ARG_REGS : 0;
}
