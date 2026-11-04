# A third object: more .data with a larger alignment and a .bss.* section,
# to check that merging honors each input section's alignment.
# Built by `cargo xtask ld-fixtures`.
    .section .data.more, "aw"
    .globl flag
flag:
    .byte 0x5a

    .globl wide
    .p2align 4
wide:
    .word 0xcafef00d

    .section .bss.more, "aw", @nobits
    .globl scratch
    .p2align 3
scratch:
    .zero 24
