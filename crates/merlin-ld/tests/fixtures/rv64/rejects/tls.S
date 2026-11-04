# A thread-local variable. merlin-ld rejects it.
# Built by `cargo xtask ld-fixtures`.
    .section .tdata, "awT", @progbits
    .globl t
    .type t, @tls_object
t:
    .word 1
