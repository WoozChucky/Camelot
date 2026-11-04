# Startup code for prog.c (tests/session.rs): a stack, main, and main's
# result as the exit code. s0 starts at 0, the end of the frame chain.

    .text
    .globl _start
_start:
    li    sp, 0x80400000    # top of Grail's default 4 MiB of RAM (16-byte aligned)
    li    s0, 0
    call  main
    xcal.halt a0            # exit code = low 8 bits of a0
1:  j     1b
