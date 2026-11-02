# Startup code for the merlin test harness: set up a stack, call main, and
# stop Grail with main's return value as the exit code.

    .text
    .globl _start
_start:
    li    sp, 0x88000000    # top of Grail's default 128 MiB of RAM (16-byte aligned)
    call  main
    xcal.halt a0            # exit code = low 8 bits of a0
1:  j     1b
