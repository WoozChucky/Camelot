# The RV32 side of rv64.s: the forms that differ by width. tests/disasm.rs
# checks excalibur against llvm-objdump on both (rv32.objdump.txt and
# rv32.no-aliases.txt, made by scripts/gen-excalibur-golden.sh). Plain words
# (.insn 4, ...) are illegal encodings, emitted as instructions so that
# llvm-objdump decodes them; it prints <unknown>.
	.option norvc
	.text
start:
	lui	a0, 0x80000
	auipc	a1, 0xfffff
	jal	zero, start
	jal	ra, fwd
	beq	a0, a1, fwd
fwd:
	lw	a0, -4(sp)
	sw	a1, -4(sp)
	addi	a0, a0, -1
	slli	a0, a0, 31
	srli	a0, a1, 1
	srai	a0, a1, 31
	sub	a0, zero, a2
	mulh	a0, a1, a2
	mulhu	a0, a1, a2
	lr.w.aqrl	a0, (a1)
	sc.w	a0, a1, (a2)
	amomaxu.w	zero, a1, (a2)

	# The counters' high halves and the other RV32-only CSRs have names here.
	csrrs	a0, cycleh, zero
	csrrs	a0, timeh, zero
	csrrs	a0, instreth, zero
	csrrs	a0, mstatush, zero
	csrrs	a0, menvcfgh, zero
	csrrs	a0, mcycleh, zero
	csrrs	a0, minstreth, zero
	csrrs	a0, pmpcfg1, zero
	csrrs	a0, pmpcfg3, zero
	csrrw	zero, mstatush, a0

	# Every CSR excalibur names, by number (tests/disasm.rs checks none is
	# missing): llvm-objdump prints its name, or, on RV64, the number of
	# one that exists only on RV32. The counters print with their rd*
	# aliases.
	csrrs	a0, 0x100, zero
	csrrs	a0, 0x104, zero
	csrrs	a0, 0x105, zero
	csrrs	a0, 0x106, zero
	csrrs	a0, 0x10a, zero
	csrrs	a0, 0x140, zero
	csrrs	a0, 0x141, zero
	csrrs	a0, 0x142, zero
	csrrs	a0, 0x143, zero
	csrrs	a0, 0x144, zero
	csrrs	a0, 0x180, zero
	csrrs	a0, 0x300, zero
	csrrs	a0, 0x301, zero
	csrrs	a0, 0x302, zero
	csrrs	a0, 0x303, zero
	csrrs	a0, 0x304, zero
	csrrs	a0, 0x305, zero
	csrrs	a0, 0x306, zero
	csrrs	a0, 0x30a, zero
	csrrs	a0, 0x310, zero
	csrrs	a0, 0x31a, zero
	csrrs	a0, 0x320, zero
	csrrs	a0, 0x323, zero
	csrrs	a0, 0x324, zero
	csrrs	a0, 0x325, zero
	csrrs	a0, 0x326, zero
	csrrs	a0, 0x327, zero
	csrrs	a0, 0x328, zero
	csrrs	a0, 0x329, zero
	csrrs	a0, 0x32a, zero
	csrrs	a0, 0x32b, zero
	csrrs	a0, 0x32c, zero
	csrrs	a0, 0x32d, zero
	csrrs	a0, 0x32e, zero
	csrrs	a0, 0x32f, zero
	csrrs	a0, 0x330, zero
	csrrs	a0, 0x331, zero
	csrrs	a0, 0x332, zero
	csrrs	a0, 0x333, zero
	csrrs	a0, 0x334, zero
	csrrs	a0, 0x335, zero
	csrrs	a0, 0x336, zero
	csrrs	a0, 0x337, zero
	csrrs	a0, 0x338, zero
	csrrs	a0, 0x339, zero
	csrrs	a0, 0x33a, zero
	csrrs	a0, 0x33b, zero
	csrrs	a0, 0x33c, zero
	csrrs	a0, 0x33d, zero
	csrrs	a0, 0x33e, zero
	csrrs	a0, 0x33f, zero
	csrrs	a0, 0x340, zero
	csrrs	a0, 0x341, zero
	csrrs	a0, 0x342, zero
	csrrs	a0, 0x343, zero
	csrrs	a0, 0x344, zero
	csrrs	a0, 0x3a0, zero
	csrrs	a0, 0x3a1, zero
	csrrs	a0, 0x3a2, zero
	csrrs	a0, 0x3a3, zero
	csrrs	a0, 0x3b0, zero
	csrrs	a0, 0x3b1, zero
	csrrs	a0, 0x3b2, zero
	csrrs	a0, 0x3b3, zero
	csrrs	a0, 0x3b4, zero
	csrrs	a0, 0x3b5, zero
	csrrs	a0, 0x3b6, zero
	csrrs	a0, 0x3b7, zero
	csrrs	a0, 0x3b8, zero
	csrrs	a0, 0x3b9, zero
	csrrs	a0, 0x3ba, zero
	csrrs	a0, 0x3bb, zero
	csrrs	a0, 0x3bc, zero
	csrrs	a0, 0x3bd, zero
	csrrs	a0, 0x3be, zero
	csrrs	a0, 0x3bf, zero
	csrrs	a0, 0x740, zero
	csrrs	a0, 0x741, zero
	csrrs	a0, 0x742, zero
	csrrs	a0, 0x744, zero
	csrrs	a0, 0x7a0, zero
	csrrs	a0, 0x7a1, zero
	csrrs	a0, 0x7a2, zero
	csrrs	a0, 0x7a3, zero
	csrrs	a0, 0x7a4, zero
	csrrs	a0, 0x7a5, zero
	csrrs	a0, 0xb00, zero
	csrrs	a0, 0xb02, zero
	csrrs	a0, 0xb03, zero
	csrrs	a0, 0xb04, zero
	csrrs	a0, 0xb05, zero
	csrrs	a0, 0xb06, zero
	csrrs	a0, 0xb07, zero
	csrrs	a0, 0xb08, zero
	csrrs	a0, 0xb09, zero
	csrrs	a0, 0xb0a, zero
	csrrs	a0, 0xb0b, zero
	csrrs	a0, 0xb0c, zero
	csrrs	a0, 0xb0d, zero
	csrrs	a0, 0xb0e, zero
	csrrs	a0, 0xb0f, zero
	csrrs	a0, 0xb10, zero
	csrrs	a0, 0xb11, zero
	csrrs	a0, 0xb12, zero
	csrrs	a0, 0xb13, zero
	csrrs	a0, 0xb14, zero
	csrrs	a0, 0xb15, zero
	csrrs	a0, 0xb16, zero
	csrrs	a0, 0xb17, zero
	csrrs	a0, 0xb18, zero
	csrrs	a0, 0xb19, zero
	csrrs	a0, 0xb1a, zero
	csrrs	a0, 0xb1b, zero
	csrrs	a0, 0xb1c, zero
	csrrs	a0, 0xb1d, zero
	csrrs	a0, 0xb1e, zero
	csrrs	a0, 0xb1f, zero
	csrrs	a0, 0xb80, zero
	csrrs	a0, 0xb82, zero
	csrrs	a0, 0xb83, zero
	csrrs	a0, 0xb84, zero
	csrrs	a0, 0xb85, zero
	csrrs	a0, 0xb86, zero
	csrrs	a0, 0xb87, zero
	csrrs	a0, 0xb88, zero
	csrrs	a0, 0xb89, zero
	csrrs	a0, 0xb8a, zero
	csrrs	a0, 0xb8b, zero
	csrrs	a0, 0xb8c, zero
	csrrs	a0, 0xb8d, zero
	csrrs	a0, 0xb8e, zero
	csrrs	a0, 0xb8f, zero
	csrrs	a0, 0xb90, zero
	csrrs	a0, 0xb91, zero
	csrrs	a0, 0xb92, zero
	csrrs	a0, 0xb93, zero
	csrrs	a0, 0xb94, zero
	csrrs	a0, 0xb95, zero
	csrrs	a0, 0xb96, zero
	csrrs	a0, 0xb97, zero
	csrrs	a0, 0xb98, zero
	csrrs	a0, 0xb99, zero
	csrrs	a0, 0xb9a, zero
	csrrs	a0, 0xb9b, zero
	csrrs	a0, 0xb9c, zero
	csrrs	a0, 0xb9d, zero
	csrrs	a0, 0xb9e, zero
	csrrs	a0, 0xb9f, zero
	csrrs	a0, 0xc00, zero
	csrrs	a0, 0xc01, zero
	csrrs	a0, 0xc02, zero
	csrrs	a0, 0xc03, zero
	csrrs	a0, 0xc04, zero
	csrrs	a0, 0xc05, zero
	csrrs	a0, 0xc06, zero
	csrrs	a0, 0xc07, zero
	csrrs	a0, 0xc08, zero
	csrrs	a0, 0xc09, zero
	csrrs	a0, 0xc0a, zero
	csrrs	a0, 0xc0b, zero
	csrrs	a0, 0xc0c, zero
	csrrs	a0, 0xc0d, zero
	csrrs	a0, 0xc0e, zero
	csrrs	a0, 0xc0f, zero
	csrrs	a0, 0xc10, zero
	csrrs	a0, 0xc11, zero
	csrrs	a0, 0xc12, zero
	csrrs	a0, 0xc13, zero
	csrrs	a0, 0xc14, zero
	csrrs	a0, 0xc15, zero
	csrrs	a0, 0xc16, zero
	csrrs	a0, 0xc17, zero
	csrrs	a0, 0xc18, zero
	csrrs	a0, 0xc19, zero
	csrrs	a0, 0xc1a, zero
	csrrs	a0, 0xc1b, zero
	csrrs	a0, 0xc1c, zero
	csrrs	a0, 0xc1d, zero
	csrrs	a0, 0xc1e, zero
	csrrs	a0, 0xc1f, zero
	csrrs	a0, 0xc80, zero
	csrrs	a0, 0xc81, zero
	csrrs	a0, 0xc82, zero
	csrrs	a0, 0xc83, zero
	csrrs	a0, 0xc84, zero
	csrrs	a0, 0xc85, zero
	csrrs	a0, 0xc86, zero
	csrrs	a0, 0xc87, zero
	csrrs	a0, 0xc88, zero
	csrrs	a0, 0xc89, zero
	csrrs	a0, 0xc8a, zero
	csrrs	a0, 0xc8b, zero
	csrrs	a0, 0xc8c, zero
	csrrs	a0, 0xc8d, zero
	csrrs	a0, 0xc8e, zero
	csrrs	a0, 0xc8f, zero
	csrrs	a0, 0xc90, zero
	csrrs	a0, 0xc91, zero
	csrrs	a0, 0xc92, zero
	csrrs	a0, 0xc93, zero
	csrrs	a0, 0xc94, zero
	csrrs	a0, 0xc95, zero
	csrrs	a0, 0xc96, zero
	csrrs	a0, 0xc97, zero
	csrrs	a0, 0xc98, zero
	csrrs	a0, 0xc99, zero
	csrrs	a0, 0xc9a, zero
	csrrs	a0, 0xc9b, zero
	csrrs	a0, 0xc9c, zero
	csrrs	a0, 0xc9d, zero
	csrrs	a0, 0xc9e, zero
	csrrs	a0, 0xc9f, zero
	csrrs	a0, 0xf11, zero
	csrrs	a0, 0xf12, zero
	csrrs	a0, 0xf13, zero
	csrrs	a0, 0xf14, zero
	csrrs	a0, 0xf15, zero

	# Illegal on RV32: slli, srli and srai with bit 25 set (shifts of 32 or
	# more on RV64), ld, lwu, sd, addiw, addw, mulw, lr.d, amoadd.d.
	.insn	4, 0x03f51513
	.insn	4, 0x0215d513
	.insn	4, 0x4215d513
	.insn	4, 0xff813503
	.insn	4, 0x00456583
	.insn	4, 0x00113423
	.insn	4, 0xfff5851b
	.insn	4, 0x00c5853b
	.insn	4, 0x02c5853b
	.insn	4, 0x1005b52f
	.insn	4, 0x04b6352f
