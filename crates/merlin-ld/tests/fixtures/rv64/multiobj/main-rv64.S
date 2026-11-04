# examples/multiobj for RV64 (main.S is the RV32 one): the same two objects,
# this file and print.S, which both widths share. What crosses from one object
# to the other is a relocation merlin-ld resolves:
#
#   call puts                 R_RISCV_CALL_PLT, to a function in print.S
#   la   t0, bytes_written    R_RISCV_PCREL_HI20/LO12_I, to print.S's .bss
#   .dword greeting           R_RISCV_64 in .data, to a string in print.S's .rodata
#   la   sp, __stack_top      a symbol defined by the linker script
#
# The table of strings holds 64-bit pointers, read with ld (lw would
# sign-extend an address at or above 0x8000_0000), 8 bytes apart. The
# counters stay 32-bit words: lw and sw, as on RV32.
#
# _start prints every string in the `lines` table (.data) with print.S's
# puts, which counts the bytes it writes in `bytes_written` (.bss, so it
# starts at zero). Then it shows that count with xcal.dbg, adds one to `runs`
# (.data, initially 41) and halts with exit code `runs`: 42.
#
# Built with our own tools by `cargo xtask examples multiobj`, which does the same as
#
#   merlin-as -march=rv64ima_zicsr_zifencei examples/multiobj/main-rv64.S -o main-rv64.o
#   merlin-as -march=rv64ima_zicsr_zifencei examples/multiobj/print.S -o print.o
#   merlin-ld -T examples/multiobj/link.ld main-rv64.o print.o -o examples/multiobj/multiobj-rv64.elf
#
# Reference build with LLVM, only for comparison (objects in the same order):
#
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -mcmodel=medany -mno-relax \
#         -nostdlib -static -fuse-ld=lld -Wl,--no-relax,--discard-none -include examples/xcal.inc \
#         -T examples/multiobj/link.ld examples/multiobj/main-rv64.S examples/multiobj/print.S \
#         -o multiobj-rv64.elf

    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    la    s0, lines
1:  ld    a0, 0(s0)             # next string, 0 at the end of the table
    beqz  a0, 2f
    call  puts
    addi  s0, s0, 8
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
    .balign 8
lines:
    .dword greeting, farewell, 0
runs:
    .word 41
