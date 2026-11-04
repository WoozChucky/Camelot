# merlin-objdump fixture for -l: a line table (.file/.loc) with no compile
# unit, which LLVM's symbolizer never reads: llvm-objdump -l prints the
# function names (from the symbol table) and no lines. Linked after units.o
# in units.elf, it is code between compile units.
# Built by scripts/gen-objdump-golden.sh:
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -c nocu.s -o nocu.o

    .text
    .file 1 "nocu.c"
    .globl h
    .type h, @function
h:
    .loc 1 3 0
    addi a0, a0, 1
    .loc 1 4 0
    slli a0, a0, 1
    .loc 1 5 0
    ret
    .size h, . - h
