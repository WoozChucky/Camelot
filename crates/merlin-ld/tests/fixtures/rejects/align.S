# xtask: relax
# With relaxation on, padding in code comes with R_RISCV_ALIGN (the linker
# may delete nops and must then redo the alignment). merlin-ld does not
# relax, so it rejects R_RISCV_ALIGN and asks for -mno-relax.
    .text
    .globl _start
_start:
    nop
    .p2align 4
aligned:
    ret
