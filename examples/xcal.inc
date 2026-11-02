# The Xcal instructions (spec section 9) as assembler macros, for building the
# examples with clang as a reference. merlin-as knows `xcal.dbg rs1` and
# `xcal.halt rs1` and never reads this file; clang's assembler doesn't know
# them, so the reference builds pass `-include examples/xcal.inc`, and each
# use expands to the `.insn` that encodes the same word (custom-0, I-type,
# funct3 0 for dbg and 1 for halt).

.macro xcal.dbg rs1
    .insn i 0x0b, 0, x0, \rs1, 0
.endm

.macro xcal.halt rs1
    .insn i 0x0b, 1, x0, \rs1, 0
.endm
