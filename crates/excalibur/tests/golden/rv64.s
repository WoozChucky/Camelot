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
	bne	t2, zero, fwd
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

	# Hints LLVM 23 names (Excalibur executes them as the instructions they
	# are): Zihintpause's pause (fence w, 0), Zihintntl's ntl.* (add zero,
	# zero, x2..x5), Zicfilp's lpad (auipc zero), aliases; Zicbop's
	# prefetches (ori zero with imm[4:0] 0, 1 or 3), instructions. Next to
	# them, the neighbours that are none of these.
	fence	w, 0
	fence	0, w
	add	zero, zero, ra
	add	zero, zero, sp
	add	zero, zero, gp
	add	zero, zero, tp
	add	zero, zero, t0
	add	zero, zero, t1
	add	sp, zero, sp
	auipc	zero, 0x1
	auipc	zero, 0xfffff
	ori	zero, a0, 0x20
	ori	zero, a0, 0x21
	ori	zero, a0, 0x22
	ori	zero, a0, 0x23
	ori	zero, a0, 0x24
	ori	zero, zero, 0
	ori	zero, a1, -2048
	ori	zero, a1, -31
	ori	ra, a0, 0x20

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

	# Illegal on RV64:
	# - the 32-bit shifts with bit 25 set (slliw, srliw, sraiw);
	.insn	4, 0x0205951b
	.insn	4, 0x0205d51b
	.insn	4, 0x4205d51b
	# - funct6 values that are no shift's;
	.insn	4, 0x08051513
	.insn	4, 0x40051513
	.insn	4, 0x44055513
	# - OP-IMM-32 funct3 2, 3, 4, 6, 7;
	.insn	4, 0x0005a51b
	.insn	4, 0x0005b51b
	.insn	4, 0x0005c51b
	.insn	4, 0x0005e51b
	.insn	4, 0x0005f51b
	# - OP-32: M funct3 1-3 (no 32-bit mulh), base funct3 2, 3, 4, 6, 7,
	#   sllw with funct7 0x20, a funct7 of 0x40;
	.insn	4, 0x02c5953b
	.insn	4, 0x02c5a53b
	.insn	4, 0x02c5b53b
	.insn	4, 0x00c5a53b
	.insn	4, 0x00c5b53b
	.insn	4, 0x00c5c53b
	.insn	4, 0x00c5e53b
	.insn	4, 0x00c5f53b
	.insn	4, 0x40c5953b
	.insn	4, 0x80c5853b
	# - load funct3 7, store funct3 4-7;
	.insn	4, 0x0005f503
	.insn	4, 0x00b54023
	.insn	4, 0x00b55023
	.insn	4, 0x00b56023
	.insn	4, 0x00b57023
	# - AMO funct3 0, 1, 4, 5, and lr.d with rs2 != 0.
	.insn	4, 0x0005852f
	.insn	4, 0x0005952f
	.insn	4, 0x0005c52f
	.insn	4, 0x0005d52f
	.insn	4, 0x1015b52f
