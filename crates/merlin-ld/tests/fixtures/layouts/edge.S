# Layout edge cases, linked with norelocs/start.o under the scripts in this
# directory: empty but aligned input sections, a weak definition, data on
# both sides of a NOBITS section. No relocations.
# Built by `cargo xtask ld-fixtures`.
    .data
    .p2align 4              # empty, but still aligns its output section

    .section .rodata.empty, "a"
    .p2align 3              # empty and 8-aligned

    .section .data.a, "aw"
    .globl da
da:
    .word 1
    .weak wk                # a weak definition nobody overrides
wk:
    .word 2

    .section .bss.b, "aw", @nobits
    .globl bb
bb:
    .zero 3

    .section .data.c, "aw"
    .globl dc
dc:
    .word 3
