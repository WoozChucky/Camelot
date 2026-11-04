# xtask: relax
# Built WITH linker relaxation enabled (the line above tells
# `cargo xtask ld-fixtures` to leave out -mno-relax), so clang pairs each
# relocation with an R_RISCV_RELAX hint. merlin-ld must ignore the hints;
# ld.lld links it with --no-relax.
# No .align in code: with relaxation that would need R_RISCV_ALIGN.

    .text
    .globl relax_fn
relax_fn:                           # relax_word = lib_double(a0) + lib_value
    addi  sp, sp, -16
    sw    ra, 12(sp)
    call  lib_double                # CALL_PLT + RELAX
    lw    t0, lib_value             # PCREL_HI20 + RELAX, PCREL_LO12_I + RELAX
    add   a0, a0, t0
    lui   t1, %hi(relax_word)       # HI20 + RELAX
    sw    a0, %lo(relax_word)(t1)   # LO12_S + RELAX
    li    t2, 3                     # a local loop: clang 23 marks the j
1:  addi  t2, t2, -1                # relaxable, so the branch over it and
    beqz  t2, 2f                    # the j stay relocations: BRANCH .Ltmp1,
    j     1b                        # JAL .Ltmp0 + RELAX
2:  lw    ra, 12(sp)
    addi  sp, sp, 16
    ret

    .data
    .globl relax_word
    .balign 4
relax_word:
    .word 0
