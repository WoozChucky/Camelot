# Weak definitions and a weak undefined reference, for symbol resolution
# tests (layout only: the .word needs an R_RISCV_32 relocation).
# Built by `cargo xtask ld-fixtures`.
    .data
    .weak helper            # data.o defines a strong helper: that one wins
    .weak weak_only         # nobody else defines it: this one is used
    .weak missing           # never defined: resolves to 0, not an error
helper:
    .word 1
weak_only:
    .word 2
    .word missing
