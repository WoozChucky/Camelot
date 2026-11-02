# A global defined in a section that is not loaded. merlin-ld rejects it: the
# symbol would have no address. Built by `cargo xtask ld-fixtures`.
    .section .note.camelot, "", @progbits
    .globl g
g:
    .word 1
