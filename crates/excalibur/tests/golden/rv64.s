# Every RV64IMA + Zicsr + Zifencei instruction excalibur decodes, with
# immediate and register edge cases and every alias llvm-objdump prints:
# tests/disasm.rs checks that excalibur formats each word as llvm-objdump
# does (rv64.objdump.txt and rv64.no-aliases.txt, made by
# scripts/gen-excalibur-golden.sh). Plain words (.insn 4, ...) are illegal
# encodings, emitted as instructions so that llvm-objdump decodes them; it
# prints <unknown>.
	.option norvc
	.text
start:
	# U-type: lui and auipc sign-extend from bit 31 on RV64.
	lui	a0, 0
	lui	a0, 1
	lui	a0, 0x7ffff
	lui	a0, 0x80000
	lui	t6, 0xfffff
	auipc	sp, 0x10
	auipc	a0, 0x80000
	auipc	a1, 0xfffff

	# Jumps and branches, forward and back.
	jal	ra, fwd
	jal	zero, start
	jal	a0, fwd
	jalr	zero, 0(ra)
	jalr	ra, 0(a0)
	jalr	t1, 0(a2)
	jalr	zero, -8(a1)
	jalr	ra, 4(a0)
	jalr	a0, 2047(a1)
	jalr	a0, -2048(a1)
	beq	a0, a1, start
	bne	a0, a1, fwd
	blt	a0, a1, fwd
	bge	a0, a1, fwd
	bltu	a0, a1, fwd
	bgeu	a0, a1, fwd
	beq	t2, zero, fwd
	bne	zero, t2, fwd
	blt	a0, zero, fwd
	blt	zero, a0, fwd
	bge	zero, a0, fwd
	bge	a0, zero, fwd
	blt	zero, zero, fwd
	bge	zero, zero, fwd
fwd:

	# Loads and stores: every width, RV64's ld, lwu and sd included.
	lb	a0, -2048(s0)
	lh	a0, 2047(s0)
	lw	a0, -4(sp)
	lbu	a0, 0(a1)
	lhu	a0, 2(a1)
	lwu	a1, 4(a0)
	ld	a0, -8(sp)
	ld	t6, 2040(t5)
	sb	a1, -1(a0)
	sh	a1, 2(a0)
	sw	a1, -4(sp)
	sd	ra, 8(sp)
	sd	zero, -2048(t0)

	# OP-IMM, with RV64's 6-bit shift amounts.
	addi	zero, zero, 0
	addi	a0, zero, 0
	addi	zero, zero, 5
	addi	zero, a0, 0
	addi	zero, a0, 1
	addi	a0, a0, -1
	addi	a0, a1, 2047
	addi	a0, a1, -2048
	addi	t1, a0, 0
	slti	a0, a1, -1
	sltiu	a0, a1, 1
	sltiu	a0, a1, 2
	xori	a0, a1, -1
	xori	a0, a1, 5
	ori	a0, a1, 0x7ff
	andi	a0, a1, 255
	andi	a0, a1, 254
	slli	a0, a0, 0
	slli	a0, a0, 31
	slli	a0, a0, 32
	slli	a0, a0, 63
	srli	a0, a1, 33
	srli	a0, a1, 63
	srai	a0, a1, 40
	srai	a0, a1, 1

	# OP-IMM-32: addiw (sext.w when the immediate is 0) and the 32-bit shifts.
	addiw	a0, a1, 0
	addiw	a0, a1, -1
	addiw	a0, a1, 2047
	addiw	zero, zero, 0
	slliw	a0, a1, 0
	slliw	a0, a1, 31
	srliw	a0, a1, 1
	sraiw	a0, a1, 31

	# OP.
	add	a0, a1, a2
	sub	a0, a1, a2
	sub	a0, zero, a2
	sll	a0, a1, a2
	slt	a0, a1, a2
	slt	a0, a1, zero
	slt	a0, zero, a2
	slt	a0, zero, zero
	sltu	a0, a1, a2
	sltu	a0, zero, a2
	sltu	a0, zero, zero
	xor	a0, a1, a2
	srl	a0, a1, a2
	sra	a0, a1, a2
	or	a0, a1, a2
	and	a0, a1, a2

	# OP-32: addw, subw (negw from zero) and the 32-bit shifts.
	addw	a0, a1, a2
	subw	a0, a1, a2
	subw	a0, zero, a2
	sllw	a0, a1, a2
	srlw	a0, a1, a2
	sraw	a0, a1, a2

	# M, and its 32-bit forms.
	mul	a0, a1, a2
	mulh	a0, a1, a2
	mulhsu	a0, a1, a2
	mulhu	a0, a1, a2
	div	a0, a1, a2
	divu	a0, a1, a2
	rem	a0, a1, a2
	remu	a0, a1, a2
	mulw	a0, a1, a2
	divw	a0, a1, a2
	divuw	a0, a1, a2
	remw	a0, a1, a2
	remuw	a0, a1, a2

	# A: both widths, every operation, every ordering.
	lr.w	a0, (a1)
	lr.w.aq	a0, (a1)
	lr.w.rl	a0, (a1)
	lr.w.aqrl	a0, (a1)
	sc.w	a0, a1, (a2)
	sc.w.aqrl	t0, t1, (t2)
	sc.w	a0, zero, (a1)
	amoswap.w	a0, a1, (a2)
	amoadd.w.aq	a0, a1, (a2)
	amoxor.w	a0, a1, (a2)
	amoand.w.rl	a0, a1, (a2)
	amoor.w	a0, a1, (a2)
	amomin.w	a0, a1, (a2)
	amomax.w	a0, a1, (a2)
	amominu.w	a0, a1, (a2)
	amomaxu.w	zero, a1, (a2)
	lr.d	a0, (a1)
	lr.d.aq	a0, (a1)
	lr.d.rl	a0, (a1)
	lr.d.aqrl	a0, (a1)
	sc.d	a0, a1, (a2)
	sc.d.aq	a0, a1, (a2)
	sc.d.rl	a0, a1, (a2)
	sc.d.aqrl	t0, t1, (t2)
	amoswap.d	a0, a1, (a2)
	amoswap.d.aqrl	a1, zero, (a1)
	amoadd.d	a0, a1, (a2)
	amoadd.d.aq	a0, a1, (a2)
	amoxor.d	a0, a1, (a2)
	amoand.d	a0, a1, (a2)
	amoand.d.rl	a0, a1, (a2)
	amoor.d	a0, a1, (a2)
	amomin.d	a0, a1, (a2)
	amomax.d	a0, a1, (a2)
	amominu.d	a0, a1, (a2)
	amomaxu.d.aqrl	zero, a1, (a2)

	# MISC-MEM and SYSTEM.
	fence	iorw, iorw
	fence	rw, rw
	# (Not `fence w, 0`: LLVM 23 prints it as Zihintpause's `pause`, LLVM 21
	# and excalibur, without Zihintpause, as `fence w, 0`; a unit test has it.)
	fence.tso
	fence.i
	ecall
	ebreak
	mret
	sret
	wfi
	sfence.vma
	sfence.vma	a0
	sfence.vma	a0, a1
	unimp

	# CSRs: named, numbered, and the RV32-only ones, which RV64 prints
	# by number.
	csrrs	a0, cycle, zero
	csrrs	a0, time, zero
	csrrs	a0, instret, zero
	csrrs	zero, cycle, zero
	csrrs	a0, 0xc80, zero
	csrrs	a0, 0xc81, zero
	csrrs	a0, 0xc82, zero
	csrrs	a0, 0x310, zero
	csrrs	a0, 0x31a, zero
	csrrs	a0, 0xb80, zero
	csrrs	a0, 0x3a1, zero
	csrrs	a0, 0x3a2, zero
	csrrs	a0, mstatus, zero
	csrrs	a0, satp, zero
	csrrs	a0, misa, zero
	csrrs	a0, 0x7c0, zero
	csrrw	zero, mstatus, a0
	csrrw	a0, mscratch, a0
	csrrs	zero, mie, a0
	csrrc	zero, mip, a0
	csrrc	a0, sstatus, a1
	csrrwi	zero, mstatus, 5
	csrrsi	zero, sstatus, 2
	csrrci	zero, sstatus, 2
	csrrwi	a0, mstatus, 31
	csrrsi	a0, mie, 8
	csrrci	a0, mip, 1

	# Illegal on RV64: slliw with bit 25 set, an OP-IMM-32 funct3 of 2,
	# a 32-bit mulh (OP-32 M funct3 1), an OP-32 funct7 of 0x40, an AMO
	# funct3 of 4, a funct6 that is no shift's.
	.insn	4, 0x0205951b
	.insn	4, 0x0005a51b
	.insn	4, 0x02c5953b
	.insn	4, 0x80c5853b
	.insn	4, 0x0005c52f
	.insn	4, 0x08051513
