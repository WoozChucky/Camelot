# A COMDAT section group (C++ inline functions, templates). merlin-ld
# rejects it. Built by `cargo xtask ld-fixtures`.
    .section .text.f, "axG", @progbits, f, comdat
    .globl f
f:
    ret
