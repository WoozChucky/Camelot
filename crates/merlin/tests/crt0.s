# Startup code for the merlin test harness: set up a stack, call main, and
# stop Grail with main's return value as the exit code.

    .text
    .globl _start
_start:
    li    sp, 0x88000000    # top of Grail's default 128 MiB of RAM (16-byte aligned)
    call  main
    xcal.halt a0            # exit code = low 8 bits of a0
1:  j     1b

# void *memset(void *dst, int byte, size_t n): clang (for clang_reference)
# clears large local arrays with a call to it, even freestanding. merlin
# clears them inline and never calls it.
    .globl memset
memset:
    mv    t0, a0
    add   a2, a0, a2        # end
2:  beq   t0, a2, 3f
    sb    a1, 0(t0)
    addi  t0, t0, 1
    j     2b
3:  ret
