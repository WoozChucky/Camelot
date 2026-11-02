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
# struct context (include/proc.h): ra at 0, sp at 4, s0-s11 from 8.

    .text
    .globl swtch
swtch:
    sw    ra, 0(a0)
    sw    sp, 4(a0)
    sw    s0, 8(a0)
    sw    s1, 12(a0)
    sw    s2, 16(a0)
    sw    s3, 20(a0)
    sw    s4, 24(a0)
    sw    s5, 28(a0)
    sw    s6, 32(a0)
    sw    s7, 36(a0)
    sw    s8, 40(a0)
    sw    s9, 44(a0)
    sw    s10, 48(a0)
    sw    s11, 52(a0)

    lw    ra, 0(a1)
    lw    sp, 4(a1)
    lw    s0, 8(a1)
    lw    s1, 12(a1)
    lw    s2, 16(a1)
    lw    s3, 20(a1)
    lw    s4, 24(a1)
    lw    s5, 28(a1)
    lw    s6, 32(a1)
    lw    s7, 36(a1)
    lw    s8, 40(a1)
    lw    s9, 44(a1)
    lw    s10, 48(a1)
    lw    s11, 52(a1)
    ret
