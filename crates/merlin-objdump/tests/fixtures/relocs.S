# merlin-objdump fixture: every relocation type merlin-as and merlin-ld
# deal with, with and without addends, in code and in data.
# Built by scripts/gen-objdump-golden.sh (with linker relaxation on, so
# R_RISCV_RELAX shows up too):
#   clang --target=riscv32 -march=rv32im_zicsr_zifencei -mabi=ilp32 -c relocs.S -o relocs.o

    .text
    .globl func
    .type func, @function
func:
    call ext_func
    tail ext_func
    la a0, local_data
    lla a1, ext_data
    lw a2, local_data
    sw a2, ext_data, t0
    lui a3, %hi(ext_data)
    addi a3, a3, %lo(ext_data)
    lw a4, %lo(ext_data+8)(a3)
    sw a4, %lo(ext_data-4)(a3)
    .option push
    .option exact
    beq a0, a1, ext_func
    .option pop
    jal ext_func
    j local_label
    bnez a0, local_label
local_label:
    ret
    .size func, .-func

    .section .rodata
    .type local_data, @object
local_data:
    .word 0x12345678
    .word ext_data
    .word ext_data + 16
    .word local_label
    .size local_data, 16

    .data
    .globl table
    .type table, @object
table:
    .word func
    .word table + 4
    .byte 1, 2, 3
    .size table, 11

    .bss
    .globl buffer
buffer:
    .zero 64
