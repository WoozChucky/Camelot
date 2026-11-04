# Two objects linked into one program: this file and print.S. Everything that
# crosses from one object to the other is a relocation merlin-ld resolves:
#
#   call puts                 R_RISCV_CALL_PLT, to a function in print.S
#   la   t0, bytes_written    R_RISCV_PCREL_HI20/LO12_I, to print.S's .bss
#   RISCV_PTR greeting        R_RISCV_32 (.word) on RV32, R_RISCV_64 (.dword) on
#                             RV64, in .data, to a string in print.S's .rodata
#   la   sp, __stack_top      a symbol defined by the linker script
#
# _start prints every string in the `lines` table (.data) with print.S's
# puts, which counts the bytes it writes in `bytes_written` (.bss, so it
# starts at zero). Then it shows that count with xcal.dbg, adds one to `runs`
# (.data, initially 41) and halts with exit code `runs`: 42.
#
# The table holds pointers, RISCV_SZPTR bytes each, read with REG_L: on RV64
# they are 64-bit, read with ld (lw would sign-extend an address at or above
# 0x8000_0000). The counters are 32-bit words at both widths, lw and sw.
#
# Built with our own tools by `cargo xtask examples multiobj`, which does the same as
#
#   merlin -E -I include examples/multiobj/main.S -o main.s
#   merlin-as main.s -o main.o
#   merlin -E -I include examples/multiobj/print.S -o print.s
#   merlin-as print.s -o print.o
#   merlin-ld -T examples/multiobj/link.ld main.o print.o -o examples/multiobj/multiobj.elf
#
# and, for RV64, the same as
#
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/multiobj/main.S -o main.s
#   merlin-as -march=rv64ima_zicsr_zifencei main.s -o main.o
#   merlin -E -march=rv64ima_zicsr_zifencei -I include examples/multiobj/print.S -o print.s
#   merlin-as -march=rv64ima_zicsr_zifencei print.s -o print.o
#   merlin-ld -T examples/multiobj/link.ld main.o print.o -o examples/multiobj/multiobj-rv64.elf
#
# Reference build with LLVM, only for comparison (objects in the same order):
#
#   clang --target=riscv32 -march=rv32im_zicsr -mabi=ilp32 -mno-relax -nostdlib -static \
#         -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc -I include \
#         -T examples/multiobj/link.ld examples/multiobj/main.S examples/multiobj/print.S \
#         -o multiobj.elf
#
# and for RV64:
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -I include -T examples/multiobj/link.ld examples/multiobj/main.S \
#         examples/multiobj/print.S -o multiobj-rv64.elf

#include <asm.h>

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    la    s0, lines
1:  REG_L a0, 0(s0)             # next string, 0 at the end of the table
    beqz  a0, 2f
    call  puts
    addi  s0, s0, RISCV_SZPTR
    j     1b

2:  la    t0, bytes_written
    lw    a0, 0(t0)
    xcal.dbg a0                 # 47: the bytes of both strings

    la    t0, runs              # .data is writable: runs += 1
    lw    a0, 0(t0)
    addi  a0, a0, 1
    sw    a0, 0(t0)
    xcal.halt a0                # exit code 42

    .data
    .balign RISCV_SZPTR
lines:
    RISCV_PTR greeting, farewell, 0
runs:
    .word 41
