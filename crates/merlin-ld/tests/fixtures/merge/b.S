# See a.S. Built by `cargo xtask ld-fixtures`.
    .section .rodata.str1.1, "aMS", @progbits, 1
    .globl s2
s2:
    .asciz "hello"

    .section .rodata.b, "a"
    .globl after_b
after_b:
    .byte 2
