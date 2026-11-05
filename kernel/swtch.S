# void swtch(struct context *old, struct context *new)
#
# The context switch between kernel threads: the scheduler's (kmain's boot
# stack) and each process's (its kernel stack). Saves the registers a called
# function must preserve, ra, sp and s0-s11, in *old, loads them from *new
# and returns, on new's stack to new's ra: so swtch returns in the other
# thread, from its own earlier call to swtch (or, for a new process, into
# proc_start, which proc_create put in its context). The caller-saved
# registers need no saving: the compiler already assumes swtch clobbers them.
#
# struct context (include/context.h): ra, sp, then s0-s11, a register's
# width each (SZREG, asm.h: 8 bytes).

#include <asm.h>

    .text
    .globl swtch
swtch:
    REG_S ra, 0*SZREG(a0)
    REG_S sp, 1*SZREG(a0)
    REG_S s0, 2*SZREG(a0)
    REG_S s1, 3*SZREG(a0)
    REG_S s2, 4*SZREG(a0)
    REG_S s3, 5*SZREG(a0)
    REG_S s4, 6*SZREG(a0)
    REG_S s5, 7*SZREG(a0)
    REG_S s6, 8*SZREG(a0)
    REG_S s7, 9*SZREG(a0)
    REG_S s8, 10*SZREG(a0)
    REG_S s9, 11*SZREG(a0)
    REG_S s10, 12*SZREG(a0)
    REG_S s11, 13*SZREG(a0)

    REG_L ra, 0*SZREG(a1)
    REG_L sp, 1*SZREG(a1)
    REG_L s0, 2*SZREG(a1)
    REG_L s1, 3*SZREG(a1)
    REG_L s2, 4*SZREG(a1)
    REG_L s3, 5*SZREG(a1)
    REG_L s4, 6*SZREG(a1)
    REG_L s5, 7*SZREG(a1)
    REG_L s6, 8*SZREG(a1)
    REG_L s7, 9*SZREG(a1)
    REG_L s8, 10*SZREG(a1)
    REG_L s9, 11*SZREG(a1)
    REG_L s10, 12*SZREG(a1)
    REG_L s11, 13*SZREG(a1)
    ret
