# merlin-objdump fixture, RV64: relocs.S for RV64, with 8-byte addresses
# in data (R_RISCV_64) next to 4-byte ones (R_RISCV_32), and doubleword
# loads and stores.
# Built by scripts/gen-objdump-golden.sh (with linker relaxation on, so
# R_RISCV_RELAX shows up too):
#   clang --target=riscv64 -march=rv64ima_zicsr_zifencei -mabi=lp64 -c rv64/relocs.S -o rv64/relocs.o

    .text
    .globl func
    .type func, @function
func:
    call ext_func
    tail ext_func
    la a0, local_data
    lla a1, ext_data
    ld a2, local_data
    sd a2, ext_data, t0
    lui a3, %hi(ext_data)
    addi a3, a3, %lo(ext_data)
    ld a4, %lo(ext_data+8)(a3)
    sd a4, %lo(ext_data-4)(a3)
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
    .dword 0x1122334455667788
    .dword ext_data
    .dword ext_data + 16
    .dword local_label
    .word ext_data - 8
    .size local_data, 36

    .data
    .globl table
    .type table, @object
table:
    .dword func
    .dword table + 8
    .byte 1, 2, 3
    .size table, 19

    .bss
    .globl buffer
buffer:
    .zero 64
