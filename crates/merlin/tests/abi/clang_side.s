	.attribute	4, 16
	.attribute	5, "rv32i2p1_m2p0_zmmul1p0"
	.file	"clang_side.c"
	.text
	.globl	c_inc1                          # -- Begin function c_inc1
	.p2align	2
	.type	c_inc1,@function
c_inc1:                                 # @c_inc1
# %bb.0:
	addi	a0, a0, 1
	zext.b	a0, a0
	ret
.Lfunc_end0:
	.size	c_inc1, .Lfunc_end0-c_inc1
                                        # -- End function
	.globl	c_make3                         # -- Begin function c_make3
	.p2align	2
	.type	c_make3,@function
c_make3:                                # @c_make3
# %bb.0:
	slli	a2, a2, 16
	slli	a1, a1, 8
	or	a1, a2, a1
	or	a0, a1, a0
	ret
.Lfunc_end1:
	.size	c_make3, .Lfunc_end1-c_make3
                                        # -- End function
	.globl	c_sum3                          # -- Begin function c_sum3
	.p2align	2
	.type	c_sum3,@function
c_sum3:                                 # @c_sum3
# %bb.0:
	zext.b	a1, a0
	slli	a2, a0, 16
	slli	a0, a0, 8
	srli	a2, a2, 24
	add	a1, a2, a1
	srli	a0, a0, 24
	add	a0, a1, a0
	ret
.Lfunc_end2:
	.size	c_sum3, .Lfunc_end2-c_sum3
                                        # -- End function
	.globl	c_swap4                         # -- Begin function c_swap4
	.p2align	2
	.type	c_swap4,@function
c_swap4:                                # @c_swap4
# %bb.0:
	srli	a1, a0, 16
	slli	a0, a0, 16
	or	a0, a0, a1
	ret
.Lfunc_end3:
	.size	c_swap4, .Lfunc_end3-c_swap4
                                        # -- End function
	.globl	c_rev5                          # -- Begin function c_rev5
	.p2align	2
	.type	c_rev5,@function
c_rev5:                                 # @c_rev5
# %bb.0:
	addi	sp, sp, -16
	li	a2, 0
	addi	a3, sp, 4
	sw	a0, 0(sp)
	sb	a1, 4(sp)
	addi	a0, sp, 8
	li	a1, 5
.LBB4_1:                                # =>This Inner Loop Header: Depth=1
	lbu	a4, 0(a3)
	add	a5, a0, a2
	addi	a2, a2, 1
	sb	a4, 0(a5)
	addi	a3, a3, -1
	bne	a2, a1, .LBB4_1
# %bb.2:
	lw	a0, 8(sp)
	lbu	a1, 12(sp)
	addi	sp, sp, 16
	ret
.Lfunc_end4:
	.size	c_rev5, .Lfunc_end4-c_rev5
                                        # -- End function
	.globl	c_make6                         # -- Begin function c_make6
	.p2align	2
	.type	c_make6,@function
c_make6:                                # @c_make6
# %bb.0:
	slli	a1, a1, 16
	lui	a3, 16
	addi	a3, a3, -1
	and	a0, a0, a3
	or	a0, a1, a0
	and	a1, a2, a3
	ret
.Lfunc_end5:
	.size	c_make6, .Lfunc_end5-c_make6
                                        # -- End function
	.globl	c_sum6                          # -- Begin function c_sum6
	.p2align	2
	.type	c_sum6,@function
c_sum6:                                 # @c_sum6
# %bb.0:
	slli	a2, a0, 16
	srai	a0, a0, 16
	slli	a1, a1, 16
	srai	a2, a2, 16
	add	a0, a2, a0
	srai	a1, a1, 16
	add	a0, a0, a1
	ret
.Lfunc_end6:
	.size	c_sum6, .Lfunc_end6-c_sum6
                                        # -- End function
	.globl	c_twice6                        # -- Begin function c_twice6
	.p2align	2
	.type	c_twice6,@function
c_twice6:                               # @c_twice6
# %bb.0:
	slli	a0, a0, 1
	lui	a2, 1048560
	slli	a1, a1, 17
	addi	a2, a2, -2
	and	a0, a0, a2
	srli	a1, a1, 16
	ret
.Lfunc_end7:
	.size	c_twice6, .Lfunc_end7-c_twice6
                                        # -- End function
	.globl	c_swap8                         # -- Begin function c_swap8
	.p2align	2
	.type	c_swap8,@function
c_swap8:                                # @c_swap8
# %bb.0:
	mv	a2, a0
	mv	a0, a1
	mv	a1, a2
	ret
.Lfunc_end8:
	.size	c_swap8, .Lfunc_end8-c_swap8
                                        # -- End function
	.globl	c_mixed                         # -- Begin function c_mixed
	.p2align	2
	.type	c_mixed,@function
c_mixed:                                # @c_mixed
# %bb.0:
	andi	a2, a0, -256
	addi	a0, a0, 1
	slli	a1, a1, 1
	zext.b	a0, a0
	or	a0, a0, a2
	ret
.Lfunc_end9:
	.size	c_mixed, .Lfunc_end9-c_mixed
                                        # -- End function
	.globl	c_union                         # -- Begin function c_union
	.p2align	2
	.type	c_union,@function
c_union:                                # @c_union
# %bb.0:
	addi	a1, a0, 1
	ret
.Lfunc_end10:
	.size	c_union, .Lfunc_end10-c_union
                                        # -- End function
	.globl	c_add12                         # -- Begin function c_add12
	.p2align	2
	.type	c_add12,@function
c_add12:                                # @c_add12
# %bb.0:
	lw	a3, 0(a1)
	lw	a4, 4(a1)
	lw	a1, 8(a1)
	lw	a5, 0(a2)
	lw	a6, 4(a2)
	lw	a2, 8(a2)
	add	a3, a5, a3
	add	a4, a6, a4
	add	a1, a2, a1
	sw	a3, 0(a0)
	sw	a4, 4(a0)
	sw	a1, 8(a0)
	ret
.Lfunc_end11:
	.size	c_add12, .Lfunc_end11-c_add12
                                        # -- End function
	.globl	c_scaled                        # -- Begin function c_scaled
	.p2align	2
	.type	c_scaled,@function
c_scaled:                               # @c_scaled
# %bb.0:
	lw	a3, 0(a2)
	lw	a4, 4(a2)
	lw	a5, 8(a2)
	mul	a3, a3, a1
	mul	a4, a4, a1
	mul	a1, a5, a1
	sw	a3, 0(a2)
	sw	a4, 4(a2)
	sw	a1, 8(a2)
	sw	a3, 0(a0)
	sw	a4, 4(a0)
	sw	a1, 8(a0)
	ret
.Lfunc_end12:
	.size	c_scaled, .Lfunc_end12-c_scaled
                                        # -- End function
	.globl	c_modify12                      # -- Begin function c_modify12
	.p2align	2
	.type	c_modify12,@function
c_modify12:                             # @c_modify12
# %bb.0:
	lw	a1, 4(a0)
	li	a2, 1000
	addi	a1, a1, 1000
	sw	a2, 0(a0)
	mv	a0, a1
	ret
.Lfunc_end13:
	.size	c_modify12, .Lfunc_end13-c_modify12
                                        # -- End function
	.globl	c_many                          # -- Begin function c_many
	.p2align	2
	.type	c_many,@function
c_many:                                 # @c_many
# %bb.0:
	mul	a1, a1, a2
	slli	a2, a3, 16
	srai	a3, a3, 16
	slli	a4, a4, 16
	add	a0, a1, a0
	lw	a1, 0(a5)
	lw	t0, 4(a5)
	lw	a5, 8(a5)
	srai	a2, a2, 16
	srai	a4, a4, 16
	add	a3, a3, a6
	add	a0, a0, a3
	add	a2, a2, a4
	add	a0, a0, a2
	add	a0, a0, a7
	add	a1, a1, t0
	add	a0, a0, a1
	add	a0, a0, a5
	ret
.Lfunc_end14:
	.size	c_many, .Lfunc_end14-c_many
                                        # -- End function
	.globl	c_big                           # -- Begin function c_big
	.p2align	2
	.type	c_big,@function
c_big:                                  # @c_big
# %bb.0:
	li	a2, 0
	addi	a3, a0, 40
.LBB15_1:                               # =>This Inner Loop Header: Depth=1
	sw	a2, 0(a0)
	addi	a0, a0, 4
	add	a2, a2, a1
	bne	a0, a3, .LBB15_1
# %bb.2:
	ret
.Lfunc_end15:
	.size	c_big, .Lfunc_end15-c_big
                                        # -- End function
	.globl	c_sumbig                        # -- Begin function c_sumbig
	.p2align	2
	.type	c_sumbig,@function
c_sumbig:                               # @c_sumbig
# %bb.0:
	mv	a1, a0
	li	a0, 0
	addi	a2, a1, 40
.LBB16_1:                               # =>This Inner Loop Header: Depth=1
	lw	a3, 0(a1)
	addi	a1, a1, 4
	add	a0, a3, a0
	bne	a1, a2, .LBB16_1
# %bb.2:
	ret
.Lfunc_end16:
	.size	c_sumbig, .Lfunc_end16-c_sumbig
                                        # -- End function
	.globl	c_apply6                        # -- Begin function c_apply6
	.p2align	2
	.type	c_apply6,@function
c_apply6:                               # @c_apply6
# %bb.0:
	addi	sp, sp, -16
	sw	ra, 12(sp)                      # 4-byte Folded Spill
	sw	s0, 8(sp)                       # 4-byte Folded Spill
	mv	a4, a1
	mv	a3, a0
	lui	a0, 16
	addi	s0, a0, -1
	and	a1, a2, s0
	mv	a0, a4
	jalr	a3
	and	a1, a1, s0
	lw	ra, 12(sp)                      # 4-byte Folded Reload
	lw	s0, 8(sp)                       # 4-byte Folded Reload
	addi	sp, sp, 16
	ret
.Lfunc_end17:
	.size	c_apply6, .Lfunc_end17-c_apply6
                                        # -- End function
	.globl	c_calls_merlin                  # -- Begin function c_calls_merlin
	.p2align	2
	.type	c_calls_merlin,@function
c_calls_merlin:                         # @c_calls_merlin
# %bb.0:
	addi	sp, sp, -128
	sw	ra, 124(sp)                     # 4-byte Folded Spill
	sw	s0, 120(sp)                     # 4-byte Folded Spill
	sw	s1, 116(sp)                     # 4-byte Folded Spill
	sw	s2, 112(sp)                     # 4-byte Folded Spill
	sw	s3, 108(sp)                     # 4-byte Folded Spill
	sw	s4, 104(sp)                     # 4-byte Folded Spill
	sw	s5, 100(sp)                     # 4-byte Folded Spill
	sw	s6, 96(sp)                      # 4-byte Folded Spill
	sw	s7, 92(sp)                      # 4-byte Folded Spill
	sw	s8, 88(sp)                      # 4-byte Folded Spill
	sw	s9, 84(sp)                      # 4-byte Folded Spill
	sw	s10, 80(sp)                     # 4-byte Folded Spill
	sw	s11, 76(sp)                     # 4-byte Folded Spill
	lui	s6, 32
	addi	s6, s6, 1
	li	a1, 3
	mv	a0, s6
	call	m_sum6
	mv	s0, a0
	mv	a0, sp
	li	a1, 4
	call	m_make12
	lw	s10, 8(sp)
	li	a0, 5
	li	a1, 7
	call	m_swap8
	mv	s1, a0
	mv	s2, a1
	lui	a0, 16432
	addi	a0, a0, 513
	li	a1, 5
	call	m_rev5
	mv	s4, a0
	mv	s3, a1
	li	a0, 1
	li	a1, 2
	li	a2, 3
	call	m_make3
	mv	s5, a0
	li	s8, 10
	li	s7, 30
	li	s9, 20
	sw	s8, 0(sp)
	sw	s9, 4(sp)
	sw	s7, 8(sp)
	li	a0, 1
	li	a4, 3
	mv	a5, sp
	li	a6, 2
	li	a7, 3
	mv	a1, s1
	mv	a2, s2
	mv	a3, s6
	call	m_many
	mv	s6, a0
	mv	a0, sp
	li	a1, 3
	call	m_big
	addi	a0, sp, 40
	mv	a1, sp
	li	a2, 36
	call	memcpy
	lw	a0, 36(sp)
	li	s11, 12
	bne	a0, s11, .LBB18_2
# %bb.1:
	mv	a0, sp
	addi	a1, sp, 40
	li	a2, 36
	call	memcpy
	sw	s11, 36(sp)
	mv	a0, sp
	call	m_sumbig
	addi	a0, a0, -75
	seqz	s11, a0
	j	.LBB18_3
.LBB18_2:
	li	s11, 0
.LBB18_3:
	addi	s0, s0, -6
	addi	s10, s10, -6
	xori	a0, s1, 7
	xori	a1, s2, 5
	zext.b	a2, s4
	zext.b	a3, s3
	or	a0, a0, a1
	lui	a1, 4080
	and	a1, s5, a1
	lui	a4, 48
	addi	a5, s6, -107
	seqz	a6, s0
	seqz	a7, s10
	xori	a2, a2, 5
	xori	a3, a3, 1
	seqz	a0, a0
	or	a2, a2, a3
	xor	a1, a1, a4
	add	a0, a6, a0
	seqz	s0, a2
	seqz	s1, a1
	add	s2, a7, a0
	seqz	s3, a5
	li	a0, 97
	call	m_inc1
	zext.b	a0, a0
	addi	a0, a0, -98
	seqz	s4, a0
	li	a0, 120
	li	a1, 21
	call	m_mixed
	zext.b	a0, a0
	xori	a0, a0, 121
	xori	a1, a1, 42
	or	a0, a0, a1
	seqz	s5, a0
	li	a0, 41
	li	a1, 0
	call	m_union
	addi	a0, a1, -42
	seqz	s6, a0
	sw	s8, 0(sp)
	sw	s9, 4(sp)
	sw	s7, 8(sp)
	mv	a0, sp
	call	m_modify12
	addi	a0, a0, -1020
	add	s1, s2, s1
	add	s0, s0, s3
	add	s4, s11, s4
	seqz	a0, a0
	add	s0, s1, s0
	add	s4, s4, s6
	add	s4, s4, s5
	add	a0, s4, a0
	add	a0, s0, a0
	lw	ra, 124(sp)                     # 4-byte Folded Reload
	lw	s0, 120(sp)                     # 4-byte Folded Reload
	lw	s1, 116(sp)                     # 4-byte Folded Reload
	lw	s2, 112(sp)                     # 4-byte Folded Reload
	lw	s3, 108(sp)                     # 4-byte Folded Reload
	lw	s4, 104(sp)                     # 4-byte Folded Reload
	lw	s5, 100(sp)                     # 4-byte Folded Reload
	lw	s6, 96(sp)                      # 4-byte Folded Reload
	lw	s7, 92(sp)                      # 4-byte Folded Reload
	lw	s8, 88(sp)                      # 4-byte Folded Reload
	lw	s9, 84(sp)                      # 4-byte Folded Reload
	lw	s10, 80(sp)                     # 4-byte Folded Reload
	lw	s11, 76(sp)                     # 4-byte Folded Reload
	addi	sp, sp, 128
	ret
.Lfunc_end18:
	.size	c_calls_merlin, .Lfunc_end18-c_calls_merlin
                                        # -- End function
	.ident	"clang version 21.1.5"
	.section	".note.GNU-stack","",@progbits
