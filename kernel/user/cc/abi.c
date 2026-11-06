/* cc1: how arguments and results travel, the RISC-V integer calling
 * convention, LP64, as clang --target=riscv64 -mabi=lp64 implements it
 * (follows merlin's abi.rs, whose comment has the rules). A word is a
 * register: 8 bytes.
 *
 * A scalar takes one argument register; a struct or union of at most one
 * word one register, of up to two words a pair, its bytes as they lie in
 * memory; a bigger one is passed by reference, the address of a copy the
 * caller makes. Arguments take the words of a sequence in order: a0 to a7,
 * then the stack. A result comes back as it would be passed as the first
 * argument; a bigger struct through memory, whose address the caller
 * passes as a hidden first argument in a0. */
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
