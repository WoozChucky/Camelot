# crt0: where every user program starts (user.ld puts .text.start first, at
# 0x1000). The kernel enters U-mode here as Linux starts an ELF program and
# the RISC-V psABI describes it ("Process Initialization";
# docs/camelot-syscalls.md, "Starting a program"): sp, 16-byte aligned,
# points at argc, a long; above it are argv[0] ... argv[argc - 1] and a
# NULL, the environment's pointers and a NULL, and the auxiliary vector,
# each a pointer's size. a0 holds the address of a function to register
# with atexit, 0 here (there is no dynamic linker), and every other register
# is zero.
#
# So all that is left is to find argc, argv and envp, to make envp the C
# library's environ (lib/environ.c), which getenv looks in and programs pass
# on to spawn and exec, and to call main(argc, argv, envp), then exit with
# what it returns, as glibc's __libc_start_main does (its `__environ = ev`
# first, the exit last). Programs written in assembly define main too.
# Preprocessed for each width: argc and the pointers are 4 bytes on RV32
# and 8 on RV64 (asm.h's REG_L, REG_S and SZREG).

#include <asm.h>

    .section .text.start
    .globl _start
_start:
    REG_L a0, 0(sp)             # argc
    addi  a1, sp, SZREG         # argv: the pointers right above it
    slli  a2, a0, RISCV_LGPTR   # envp: past argv's argc pointers and its NULL
    add   a2, a1, a2
    addi  a2, a2, SZREG
    lla   t0, environ           # environ = envp
    REG_S a2, 0(t0)
    call  main
    call  exit                  # main's return value is already in a0
1:  j     1b                    # exit never returns
