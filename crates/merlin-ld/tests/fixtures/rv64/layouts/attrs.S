# Sections for layouts/attrs.ld: one that /DISCARD/ throws away (with a
# global nobody references, which leaves the symbol table with it), and data
# in a section the script makes NOLOAD (memory only, its bytes dropped).
# No relocations. Built by `cargo xtask ld-fixtures`.
    .section .discard.me, "a"
    .globl dropped
dropped:
    .word 0xdeadbeef

    .section .noload.data, "aw"
    .p2align 2
    .word 1, 2, 3
