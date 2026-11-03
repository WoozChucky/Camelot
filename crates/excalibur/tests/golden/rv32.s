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

	# Illegal on RV32: slli with bit 25 set (a shift of 63 on RV64), ld,
	# lwu, sd, addiw, addw, mulw, lr.d, amoadd.d.
	.insn	4, 0x03f51513
	.insn	4, 0xff813503
	.insn	4, 0x00456583
	.insn	4, 0x00113423
	.insn	4, 0xfff5851b
	.insn	4, 0x00c5853b
	.insn	4, 0x02c5853b
	.insn	4, 0x1005b52f
	.insn	4, 0x04b6352f
