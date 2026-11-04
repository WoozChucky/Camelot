# xtask: relax
# With relaxation on, padding in code comes with R_RISCV_ALIGN (the linker
# may delete nops and must then redo the alignment). merlin-ld does not
# relax, so it rejects R_RISCV_ALIGN and asks for -mno-relax.
# LLVM 23 writes R_RISCV_ALIGN only when a linker-relaxable instruction
# comes before the padding in its section (only then can the distance
# change), hence the call.
    .text
    .globl _start
_start:
    call  aligned                   # CALL_PLT + RELAX
    nop
    .p2align 4                      # ALIGN at .text+0xc
aligned:
    ret
