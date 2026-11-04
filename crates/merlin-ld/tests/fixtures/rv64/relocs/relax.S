# xtask: relax
# Built WITH linker relaxation enabled (the line above tells
# `cargo xtask ld-fixtures` to leave out -mno-relax), so clang pairs each
# relaxable relocation with an R_RISCV_RELAX hint. merlin-ld must ignore the
# hints; ld.lld links it with --no-relax. The RV64 twin of ../../relocs/relax.S,
# with a pc-relative store where that has lui (RAM is out of lui's reach on
# RV64) and lui of an absolute symbol instead.
# No .align in code: with relaxation that would need R_RISCV_ALIGN.

    .text
    .globl relax_fn
relax_fn:                           # relax_word = a0 = lib_double(a0) + lib_value; a1 = hi_max
    addi  sp, sp, -16
    sd    ra, 8(sp)
    call  lib_double                # CALL_PLT + RELAX
    ld    t0, lib_value             # PCREL_HI20 + RELAX, PCREL_LO12_I + RELAX
    add   a0, a0, t0
    sd    a0, relax_word, t1        # PCREL_HI20 + RELAX, PCREL_LO12_S + RELAX
    lui   a1, %hi(hi_max)           # HI20 + RELAX
    addi  a1, a1, %lo(hi_max)       # LO12_I + RELAX
    li    t2, 3                     # a local loop: clang 23 marks the j
1:  addi  t2, t2, -1                # relaxable, so the branch over it and
    beqz  t2, 2f                    # the j stay relocations: BRANCH .Ltmp1,
    j     1b                        # JAL .Ltmp0 + RELAX
2:  ld    ra, 8(sp)
    addi  sp, sp, 16
    ret

    .data
    .globl relax_word
    .balign 8
relax_word:
    .dword 0
