# Data in every output section kind and no references to other symbols, so
# the object has no relocations. Built by `cargo xtask ld-fixtures`.
    .section .rodata
    .globl table
    .p2align 2
table:
    .word 1, 2, 3, 4

    .section .rodata.str, "a"
    .globl greeting
greeting:
    .asciz "Camelot"

    .data
    .globl counter
    .p2align 3
counter:
    .word 0x12345678, 0x9abcdef0

    .bss
    .globl buffer
    .p2align 4
buffer:
    .zero 256

    .section .text.helper, "ax"
    .globl helper
    .p2align 2
helper:
    addi  a0, a0, 1
    ret
