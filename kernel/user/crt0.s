# crt0: where every user program starts (user.ld puts .text.start first, at
# 0x1000). The kernel enters U-mode here with a0 = argc and a1 = argv, the
# argument strings and the argv array at the top of the stack and sp below
# them, 16-byte aligned (docs/camelot-syscalls.md, "Starting a program").
# Every other register is zero.
#
# So all that is left is to call main(argc, argv) and exit with what it
# returns. Programs written in assembly define main too.

    .section .text.start
    .globl _start
_start:
    call  main
    call  exit                  # main's return value is already in a0
1:  j     1b                    # exit never returns
