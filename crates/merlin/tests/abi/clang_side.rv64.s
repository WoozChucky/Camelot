	.attribute	4, 16
	.attribute	5, "rv64i2p1_m2p0_a2p1_zicsr2p0_zifencei2p0_zmmul1p0_zaamo1p0_zalrsc1p0"
	.file	"clang_side.c"
	.text
	.globl	c_widen                         # -- Begin function c_widen
	.p2align	2
	.type	c_widen,@function
c_widen:                                # @c_widen
# %bb.0:
	add	a0, a1, a0
	add	a2, a2, a3
	add	a0, a0, a2
	ret
.Lfunc_end0:
	.size	c_widen, .Lfunc_end0-c_widen
                                        # -- End function
	.globl	c_trunc_sc                      # -- Begin function c_trunc_sc
	.p2align	2
	.type	c_trunc_sc,@function
c_trunc_sc:                             # @c_trunc_sc
# %bb.0:
	slli	a0, a0, 56
	srai	a0, a0, 56
	ret
.Lfunc_end1:
	.size	c_trunc_sc, .Lfunc_end1-c_trunc_sc
                                        # -- End function
	.globl	c_trunc_s                       # -- Begin function c_trunc_s
	.p2align	2
	.type	c_trunc_s,@function
c_trunc_s:                              # @c_trunc_s
# %bb.0:
	slli	a0, a0, 48
	srai	a0, a0, 48
	ret
.Lfunc_end2:
	.size	c_trunc_s, .Lfunc_end2-c_trunc_s
                                        # -- End function
	.globl	c_trunc_us                      # -- Begin function c_trunc_us
	.p2align	2
	.type	c_trunc_us,@function
c_trunc_us:                             # @c_trunc_us
# %bb.0:
	slli	a0, a0, 48
	srli	a0, a0, 48
	ret
.Lfunc_end3:
	.size	c_trunc_us, .Lfunc_end3-c_trunc_us
                                        # -- End function
	.globl	c_tobool                        # -- Begin function c_tobool
	.p2align	2
	.type	c_tobool,@function
c_tobool:                               # @c_tobool
# %bb.0:
	snez	a0, a0
	ret
.Lfunc_end4:
	.size	c_tobool, .Lfunc_end4-c_tobool
                                        # -- End function
	.globl	c_inc1                          # -- Begin function c_inc1
	.p2align	2
	.type	c_inc1,@function
c_inc1:                                 # @c_inc1
# %bb.0:
	addi	a0, a0, 1
	ret
.Lfunc_end5:
	.size	c_inc1, .Lfunc_end5-c_inc1
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
.Lfunc_end6:
	.size	c_make3, .Lfunc_end6-c_make3
                                        # -- End function
	.globl	c_sum3                          # -- Begin function c_sum3
	.p2align	2
	.type	c_sum3,@function
c_sum3:                                 # @c_sum3
# %bb.0:
	slli	a1, a0, 40
	srli	a1, a1, 56
	zext.b	a2, a0
	slli	a0, a0, 48
	srli	a0, a0, 56
	add	a1, a1, a2
	add	a0, a1, a0
	ret
.Lfunc_end7:
	.size	c_sum3, .Lfunc_end7-c_sum3
                                        # -- End function
	.globl	c_swap4                         # -- Begin function c_swap4
	.p2align	2
	.type	c_swap4,@function
c_swap4:                                # @c_swap4
# %bb.0:
	srliw	a1, a0, 16
	slliw	a0, a0, 16
	or	a0, a0, a1
	ret
.Lfunc_end8:
	.size	c_swap4, .Lfunc_end8-c_swap4
                                        # -- End function
	.globl	c_rev5                          # -- Begin function c_rev5
	.p2align	2
	.type	c_rev5,@function
c_rev5:                                 # @c_rev5
# %bb.0:
	addi	sp, sp, -16
	addi	a1, sp, 4
	srli	a2, a0, 32
	sw	a0, 0(sp)
	sb	a2, 4(sp)
	addi	a0, sp, 8
	addi	a2, sp, -1
.LBB9_1:                                # =>This Inner Loop Header: Depth=1
	lbu	a3, 0(a1)
	addi	a1, a1, -1
	sb	a3, 0(a0)
	addi	a0, a0, 1
	bne	a1, a2, .LBB9_1
# %bb.2:
	lbu	a0, 12(sp)
	lwu	a1, 8(sp)
	slli	a0, a0, 32
	or	a0, a1, a0
	addi	sp, sp, 16
	ret
.Lfunc_end9:
	.size	c_rev5, .Lfunc_end9-c_rev5
                                        # -- End function
	.globl	c_make6                         # -- Begin function c_make6
	.p2align	2
	.type	c_make6,@function
c_make6:                                # @c_make6
# %bb.0:
	slli	a2, a2, 32
	slli	a1, a1, 48
	srli	a1, a1, 32
	slli	a0, a0, 48
	or	a1, a2, a1
	srli	a0, a0, 48
	or	a0, a1, a0
	ret
.Lfunc_end10:
	.size	c_make6, .Lfunc_end10-c_make6
                                        # -- End function
	.globl	c_sum6                          # -- Begin function c_sum6
	.p2align	2
	.type	c_sum6,@function
c_sum6:                                 # @c_sum6
# %bb.0:
	slli	a1, a0, 16
	slli	a2, a0, 48
	srai	a2, a2, 48
	srai	a1, a1, 48
	sraiw	a0, a0, 16
	add	a1, a1, a2
	add	a0, a1, a0
	ret
.Lfunc_end11:
	.size	c_sum6, .Lfunc_end11-c_sum6
                                        # -- End function
	.globl	c_twice6                        # -- Begin function c_twice6
	.p2align	2
	.type	c_twice6,@function
c_twice6:                               # @c_twice6
# %bb.0:
	lui	a1, 983039
	slli	a1, a1, 4
	slli	a0, a0, 1
	addi	a1, a1, -2
	and	a0, a0, a1
	ret
.Lfunc_end12:
	.size	c_twice6, .Lfunc_end12-c_twice6
                                        # -- End function
	.globl	c_swap8                         # -- Begin function c_swap8
	.p2align	2
	.type	c_swap8,@function
c_swap8:                                # @c_swap8
# %bb.0:
	srli	a1, a0, 32
	slli	a0, a0, 32
	or	a0, a0, a1
	ret
.Lfunc_end13:
	.size	c_swap8, .Lfunc_end13-c_swap8
                                        # -- End function
	.globl	c_mixed                         # -- Begin function c_mixed
	.p2align	2
	.type	c_mixed,@function
c_mixed:                                # @c_mixed
# %bb.0:
	addi	a1, a0, 1
	srli	a2, a0, 32
	srliw	a0, a0, 8
	slli	a2, a2, 33
	slli	a0, a0, 8
	or	a0, a2, a0
	zext.b	a1, a1
	or	a0, a0, a1
	ret
.Lfunc_end14:
	.size	c_mixed, .Lfunc_end14-c_mixed
                                        # -- End function
	.globl	c_union                         # -- Begin function c_union
	.p2align	2
	.type	c_union,@function
c_union:                                # @c_union
# %bb.0:
	li	a1, -1
	slli	a0, a0, 32
	srli	a1, a1, 32
	or	a1, a0, a1
	srli	a0, a0, 32
	add	a0, a1, a0
	addi	a0, a0, 1
	ret
.Lfunc_end15:
	.size	c_union, .Lfunc_end15-c_union
                                        # -- End function
	.globl	c_add12                         # -- Begin function c_add12
	.p2align	2
	.type	c_add12,@function
c_add12:                                # @c_add12
# %bb.0:
	li	a4, -1
	slli	a4, a4, 32
	and	a5, a2, a4
	add	a2, a2, a0
	add	a0, a5, a0
	add	a1, a3, a1
	and	a0, a0, a4
	slli	a2, a2, 32
	srli	a2, a2, 32
	slli	a1, a1, 32
	or	a0, a0, a2
	srli	a1, a1, 32
	ret
.Lfunc_end16:
	.size	c_add12, .Lfunc_end16-c_add12
                                        # -- End function
	.globl	c_scaled                        # -- Begin function c_scaled
	.p2align	2
	.type	c_scaled,@function
c_scaled:                               # @c_scaled
# %bb.0:
	srli	a3, a1, 32
	mul	a1, a0, a1
	mul	a3, a0, a3
	mul	a0, a0, a2
	slli	a1, a1, 32
	slli	a3, a3, 32
	srli	a1, a1, 32
	slli	a2, a0, 32
	or	a0, a3, a1
	srli	a1, a2, 32
	ret
.Lfunc_end17:
	.size	c_scaled, .Lfunc_end17-c_scaled
                                        # -- End function
	.globl	c_modify12                      # -- Begin function c_modify12
	.p2align	2
	.type	c_modify12,@function
c_modify12:                             # @c_modify12
# %bb.0:
	srli	a0, a0, 32
	addiw	a0, a0, 1000
	ret
.Lfunc_end18:
	.size	c_modify12, .Lfunc_end18-c_modify12
                                        # -- End function
	.globl	c_many                          # -- Begin function c_many
	.p2align	2
	.type	c_many,@function
c_many:                                 # @c_many
# %bb.0:
	srli	a7, a1, 32
	slli	t0, a2, 16
	mul	a1, a7, a1
	srli	a7, a3, 32
	slli	t1, a2, 48
	srai	t1, t1, 48
	sraiw	a2, a2, 16
	add	a0, a0, t1
	add	a2, a5, a2
	srai	a5, t0, 48
	add	a2, a2, a3
	add	a0, a0, a5
	add	a2, a2, a4
	add	a0, a1, a0
	add	a2, a2, a7
	add	a0, a0, a2
	addw	a0, a0, a6
	ret
.Lfunc_end19:
	.size	c_many, .Lfunc_end19-c_many
                                        # -- End function
	.globl	c_big                           # -- Begin function c_big
	.p2align	2
	.type	c_big,@function
c_big:                                  # @c_big
# %bb.0:
	li	a2, 0
	addi	a3, a0, 40
.LBB20_1:                               # =>This Inner Loop Header: Depth=1
	sw	a2, 0(a0)
	addi	a0, a0, 4
	add	a2, a2, a1
	bne	a0, a3, .LBB20_1
# %bb.2:
	ret
.Lfunc_end20:
	.size	c_big, .Lfunc_end20-c_big
                                        # -- End function
	.globl	c_sumbig                        # -- Begin function c_sumbig
	.p2align	2
	.type	c_sumbig,@function
c_sumbig:                               # @c_sumbig
# %bb.0:
	mv	a1, a0
	li	a0, 0
	addi	a2, a1, 40
.LBB21_1:                               # =>This Inner Loop Header: Depth=1
	lw	a3, 0(a1)
	addi	a1, a1, 4
	addw	a0, a3, a0
	bne	a1, a2, .LBB21_1
# %bb.2:
	ret
.Lfunc_end21:
	.size	c_sumbig, .Lfunc_end21-c_sumbig
                                        # -- End function
	.globl	c_apply6                        # -- Begin function c_apply6
	.p2align	2
	.type	c_apply6,@function
c_apply6:                               # @c_apply6
# %bb.0:
	mv	t1, a0
	mv	a0, a1
	jr	t1
.Lfunc_end22:
	.size	c_apply6, .Lfunc_end22-c_apply6
                                        # -- End function
	.globl	c_vmix                          # -- Begin function c_vmix
	.p2align	2
	.type	c_vmix,@function
c_vmix:                                 # @c_vmix
# %bb.0:
	addi	sp, sp, -128
	sd	s0, 56(sp)                      # 8-byte Folded Spill
	li	t0, 0
	sd	a5, 104(sp)
	sd	a6, 112(sp)
	sd	a7, 120(sp)
	addi	a5, sp, 72
	sd	a1, 72(sp)
	sd	a2, 80(sp)
	sd	a3, 88(sp)
	sd	a4, 96(sp)
	sd	a5, 8(sp)
	addi	a1, sp, 56
.Lpcrel_hi0:
	auipc	a3, %pcrel_hi(.LJTI23_0)
	li	a2, 49
	addi	a3, a3, %pcrel_lo(.Lpcrel_hi0)
	j	.LBB23_3
.LBB23_1:                               #   in Loop: Header=BB23_3 Depth=1
	lb	a4, 0(a5)
.LBB23_2:                               #   in Loop: Header=BB23_3 Depth=1
	addi	a5, a5, 8
	addw	t0, a4, t0
	addi	a0, a0, 1
.LBB23_3:                               # =>This Loop Header: Depth=1
                                        #     Child Loop BB23_6 Depth 2
	lbu	a4, 0(a0)
	addi	a6, a4, -66
	bltu	a2, a6, .LBB23_13
# %bb.4:                                #   in Loop: Header=BB23_3 Depth=1
	slli	a6, a6, 3
	add	a6, a3, a6
	ld	a4, 0(a6)
	jr	a4
.LBB23_5:                               #   in Loop: Header=BB23_3 Depth=1
	ld	a4, 0(a5)
	lw	a6, 36(a4)
	lwu	a7, 32(a4)
	lwu	t1, 16(a4)
	lw	t2, 20(a4)
	lwu	t3, 24(a4)
	lw	t4, 28(a4)
	slli	a6, a6, 32
	lw	t5, 4(a4)
	lwu	t6, 8(a4)
	lw	s0, 12(a4)
	lwu	a4, 0(a4)
	or	a6, a6, a7
	slli	t4, t4, 32
	slli	t2, t2, 32
	or	a7, t4, t3
	or	t1, t2, t1
	slli	s0, s0, 32
	slli	t5, t5, 32
	or	t2, s0, t6
	or	a4, t5, a4
	sd	a6, 48(sp)
	sd	a4, 16(sp)
	sd	t2, 24(sp)
	sd	t1, 32(sp)
	sd	a7, 40(sp)
	addi	a4, sp, 16
.LBB23_6:                               #   Parent Loop BB23_3 Depth=1
                                        # =>  This Inner Loop Header: Depth=2
	lw	a6, 0(a4)
	addi	a4, a4, 4
	addw	t0, a6, t0
	bne	a4, a1, .LBB23_6
# %bb.7:                                #   in Loop: Header=BB23_3 Depth=1
	addi	a5, a5, 8
	j	.LBB23_14
.LBB23_8:                               #   in Loop: Header=BB23_3 Depth=1
	lw	a4, 0(a5)
	lw	a6, 4(a5)
	lw	a7, 8(a5)
	addi	a5, a5, 16
	add	a4, a4, t0
	j	.LBB23_10
.LBB23_9:                               #   in Loop: Header=BB23_3 Depth=1
	lh	a4, 0(a5)
	lh	a6, 2(a5)
	lh	a7, 4(a5)
	addi	a5, a5, 8
	add	a4, t0, a4
.LBB23_10:                              #   in Loop: Header=BB23_3 Depth=1
	add	a6, a6, a7
	addw	t0, a4, a6
	addi	a0, a0, 1
	j	.LBB23_3
.LBB23_11:                              #   in Loop: Header=BB23_3 Depth=1
	lw	a4, 0(a5)
	j	.LBB23_2
.LBB23_12:                              #   in Loop: Header=BB23_3 Depth=1
	lw	a4, 0(a5)
	lw	a6, 4(a5)
	mul	a4, a6, a4
	j	.LBB23_2
.LBB23_13:                              #   in Loop: Header=BB23_3 Depth=1
	beqz	a4, .LBB23_15
.LBB23_14:                              #   in Loop: Header=BB23_3 Depth=1
	addi	a0, a0, 1
	j	.LBB23_3
.LBB23_15:
	mv	a0, t0
	ld	s0, 56(sp)                      # 8-byte Folded Reload
	addi	sp, sp, 128
	ret
.Lfunc_end23:
	.size	c_vmix, .Lfunc_end23-c_vmix
	.section	.rodata,"a",@progbits
	.p2align	3, 0x0
.LJTI23_0:
	.quad	.LBB23_5
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_8
	.quad	.LBB23_1
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_11
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_12
	.quad	.LBB23_14
	.quad	.LBB23_14
	.quad	.LBB23_9
                                        # -- End function
	.text
	.globl	c_vsum                          # -- Begin function c_vsum
	.p2align	2
	.type	c_vsum,@function
c_vsum:                                 # @c_vsum
# %bb.0:
	addi	sp, sp, -80
	mv	t0, a0
	sd	a5, 56(sp)
	sd	a6, 64(sp)
	sd	a7, 72(sp)
	addi	a0, sp, 24
	sd	a1, 24(sp)
	sd	a2, 32(sp)
	sd	a3, 40(sp)
	sd	a4, 48(sp)
	sd	a0, 8(sp)
	li	a0, 0
	blez	t0, .LBB24_3
# %bb.1:
	ld	a1, 8(sp)
	addi	t0, t0, -1
	slli	t0, t0, 32
	srli	a2, t0, 29
	add	a2, a2, a1
	addi	a1, a1, 8
	addi	a2, a2, 16
.LBB24_2:                               # =>This Inner Loop Header: Depth=1
	sd	a1, 8(sp)
	lw	a3, -8(a1)
	addi	a1, a1, 8
	addw	a0, a3, a0
	bne	a1, a2, .LBB24_2
.LBB24_3:
	addi	sp, sp, 80
	ret
.Lfunc_end24:
	.size	c_vsum, .Lfunc_end24-c_vsum
                                        # -- End function
	.globl	c_vlate                         # -- Begin function c_vlate
	.p2align	2
	.type	c_vlate,@function
c_vlate:                                # @c_vlate
# %bb.0:
	addi	sp, sp, -32
	sd	a7, 24(sp)
	lw	a7, 24(sp)
	lw	t0, 28(sp)
	lw	t1, 32(sp)
	add	a0, a1, a0
	add	a2, a2, a3
	add	a4, a4, a5
	add	a0, a0, a2
	add	a4, a4, a6
	add	a0, a0, a4
	slli	a1, a7, 1
	slli	a7, a7, 3
	add	a0, a0, t0
	add	a1, a7, a1
	add	a0, a0, t1
	addw	a0, a0, a1
	addi	sp, sp, 32
	ret
.Lfunc_end25:
	.size	c_vlate, .Lfunc_end25-c_vlate
                                        # -- End function
	.globl	c_stack                         # -- Begin function c_stack
	.p2align	2
	.type	c_stack,@function
c_stack:                                # @c_stack
# %bb.0:
	addi	sp, sp, -16
	sd	s0, 8(sp)                       # 8-byte Folded Spill
	ld	t0, 48(sp)
	slli	t1, t0, 16
	ld	t2, 40(sp)
	lw	t3, 20(sp)
	ld	t4, 32(sp)
	lw	t5, 24(sp)
	slli	t6, a7, 1
	slli	s0, a7, 3
	srli	a7, a7, 32
	add	t6, s0, t6
	slli	s0, t0, 48
	add	a0, a0, a1
	add	a2, a2, a3
	add	a4, a4, a5
	add	a0, a0, a2
	add	a4, a4, a6
	add	a0, a0, a4
	add	a7, t6, a7
	srai	s0, s0, 48
	add	a0, a0, a7
	add	a0, a0, t5
	add	t3, t4, t3
	srai	a1, t1, 48
	add	a0, a0, t3
	add	a0, a0, t2
	add	a1, s0, a1
	sraiw	a2, t0, 16
	add	a0, a0, a1
	add	a0, a0, a2
	addiw	a0, a0, 10
	ld	s0, 8(sp)                       # 8-byte Folded Reload
	addi	sp, sp, 16
	ret
.Lfunc_end26:
	.size	c_stack, .Lfunc_end26-c_stack
                                        # -- End function
	.globl	c_stack_ret                     # -- Begin function c_stack_ret
	.p2align	2
	.type	c_stack_ret,@function
c_stack_ret:                            # @c_stack_ret
# %bb.0:
	mul	a6, a7, a6
	ld	a7, 0(sp)
	add	a0, a1, a0
	add	a3, a4, a3
	add	a0, a0, a2
	add	a3, a3, a5
	add	a6, a6, a7
	slli	a3, a3, 32
	slli	a0, a0, 32
	srli	a0, a0, 32
	slli	a1, a6, 32
	or	a0, a3, a0
	srli	a1, a1, 32
	ret
.Lfunc_end27:
	.size	c_stack_ret, .Lfunc_end27-c_stack_ret
                                        # -- End function
	.globl	c_mixw                          # -- Begin function c_mixw
	.p2align	2
	.type	c_mixw,@function
c_mixw:                                 # @c_mixw
# %bb.0:
	lbu	a4, 1(a4)
	srliw	a1, a1, 4
	add	a0, a0, a2
	srli	a3, a3, 4
	add	a0, a0, a1
	add	a0, a0, a3
	add	a0, a0, a4
	addi	a0, a0, -48
	ret
.Lfunc_end28:
	.size	c_mixw, .Lfunc_end28-c_mixw
                                        # -- End function
	.globl	c_umax                          # -- Begin function c_umax
	.p2align	2
	.type	c_umax,@function
c_umax:                                 # @c_umax
# %bb.0:
	addi	a0, a0, 1
	seqz	a0, a0
	ret
.Lfunc_end29:
	.size	c_umax, .Lfunc_end29-c_umax
                                        # -- End function
	.globl	c_sext                          # -- Begin function c_sext
	.p2align	2
	.type	c_sext,@function
c_sext:                                 # @c_sext
# %bb.0:
	ret
.Lfunc_end30:
	.size	c_sext, .Lfunc_end30-c_sext
                                        # -- End function
	.globl	c_zext                          # -- Begin function c_zext
	.p2align	2
	.type	c_zext,@function
c_zext:                                 # @c_zext
# %bb.0:
	slli	a0, a0, 32
	srli	a0, a0, 32
	ret
.Lfunc_end31:
	.size	c_zext, .Lfunc_end31-c_zext
                                        # -- End function
	.globl	c_narrow                        # -- Begin function c_narrow
	.p2align	2
	.type	c_narrow,@function
c_narrow:                               # @c_narrow
# %bb.0:
	sext.w	a0, a0
	ret
.Lfunc_end32:
	.size	c_narrow, .Lfunc_end32-c_narrow
                                        # -- End function
	.globl	c_unarrow                       # -- Begin function c_unarrow
	.p2align	2
	.type	c_unarrow,@function
c_unarrow:                              # @c_unarrow
# %bb.0:
	sext.w	a0, a0
	ret
.Lfunc_end33:
	.size	c_unarrow, .Lfunc_end33-c_unarrow
                                        # -- End function
	.globl	c_incl1                         # -- Begin function c_incl1
	.p2align	2
	.type	c_incl1,@function
c_incl1:                                # @c_incl1
# %bb.0:
	addi	a0, a0, 1
	ret
.Lfunc_end34:
	.size	c_incl1, .Lfunc_end34-c_incl1
                                        # -- End function
	.globl	c_swapl2                        # -- Begin function c_swapl2
	.p2align	2
	.type	c_swapl2,@function
c_swapl2:                               # @c_swapl2
# %bb.0:
	mv	a2, a0
	mv	a0, a1
	mv	a1, a2
	ret
.Lfunc_end35:
	.size	c_swapl2, .Lfunc_end35-c_swapl2
                                        # -- End function
	.globl	c_li                            # -- Begin function c_li
	.p2align	2
	.type	c_li,@function
c_li:                                   # @c_li
# %bb.0:
	sext.w	a2, a1
	neg	a3, a1
	slli	a3, a3, 32
	srli	a1, a1, 32
	srli	a3, a3, 32
	slli	a1, a1, 32
	add	a0, a2, a0
	or	a1, a1, a3
	ret
.Lfunc_end36:
	.size	c_li, .Lfunc_end36-c_li
                                        # -- End function
	.globl	c_cl                            # -- Begin function c_cl
	.p2align	2
	.type	c_cl,@function
c_cl:                                   # @c_cl
# %bb.0:
	addi	a2, a0, 1
	andi	a0, a0, -256
	zext.b	a2, a2
	neg	a1, a1
	or	a0, a2, a0
	ret
.Lfunc_end37:
	.size	c_cl, .Lfunc_end37-c_cl
                                        # -- End function
	.globl	c_addl3                         # -- Begin function c_addl3
	.p2align	2
	.type	c_addl3,@function
c_addl3:                                # @c_addl3
# %bb.0:
	ld	a3, 0(a2)
	ld	a4, 8(a2)
	ld	a5, 0(a1)
	ld	a6, 8(a1)
	ld	a1, 16(a1)
	ld	a2, 16(a2)
	add	a3, a3, a5
	add	a4, a4, a6
	add	a1, a2, a1
	sd	a3, 0(a0)
	sd	a4, 8(a0)
	sd	a1, 16(a0)
	ret
.Lfunc_end38:
	.size	c_addl3, .Lfunc_end38-c_addl3
                                        # -- End function
	.globl	c_vmixw                         # -- Begin function c_vmixw
	.p2align	2
	.type	c_vmixw,@function
c_vmixw:                                # @c_vmixw
# %bb.0:
	addi	sp, sp, -80
	li	t0, 0
	sd	a5, 56(sp)
	sd	a6, 64(sp)
	sd	a7, 72(sp)
	sd	a1, 24(sp)
	sd	a2, 32(sp)
	sd	a3, 40(sp)
	sd	a4, 48(sp)
	addi	a3, sp, 24
	sd	a3, 8(sp)
.Lpcrel_hi1:
	auipc	a2, %pcrel_hi(.LJTI39_0)
	li	a1, 41
	addi	a2, a2, %pcrel_lo(.Lpcrel_hi1)
	j	.LBB39_3
.LBB39_1:                               #   in Loop: Header=BB39_3 Depth=1
	addi	a4, a3, 8
	sd	a4, 8(sp)
	ld	a5, 0(a3)
	mv	a3, a4
	ld	a4, 0(a5)
	ld	a6, 8(a5)
	ld	a5, 16(a5)
	add	a4, a4, t0
	add	a5, a6, a5
	add	t0, a4, a5
.LBB39_2:                               #   in Loop: Header=BB39_3 Depth=1
	addi	a0, a0, 1
.LBB39_3:                               # =>This Inner Loop Header: Depth=1
	lbu	a4, 0(a0)
	addi	a5, a4, -76
	bltu	a1, a5, .LBB39_12
# %bb.4:                                #   in Loop: Header=BB39_3 Depth=1
	slli	a5, a5, 3
	add	a5, a2, a5
	ld	a4, 0(a5)
	jr	a4
.LBB39_5:                               #   in Loop: Header=BB39_3 Depth=1
	addi	a4, a3, 8
	sd	a4, 8(sp)
	ld	a5, 0(a3)
	mv	a3, a4
	srli	a5, a5, 4
	add	t0, a5, t0
	addi	a0, a0, 1
	j	.LBB39_3
.LBB39_6:                               #   in Loop: Header=BB39_3 Depth=1
	addi	a4, a3, 16
	sd	a4, 8(sp)
	ld	a5, 0(a3)
	ld	a3, 8(a3)
	slli	a6, a5, 1
	slli	a5, a5, 3
	add	a5, a5, a6
	add	a3, a3, t0
	add	t0, a3, a5
	mv	a3, a4
	addi	a0, a0, 1
	j	.LBB39_3
.LBB39_7:                               #   in Loop: Header=BB39_3 Depth=1
	addi	a4, a3, 8
	sd	a4, 8(sp)
	lwu	a5, 0(a3)
	mv	a3, a4
	srli	a5, a5, 4
	add	t0, t0, a5
	addi	a0, a0, 1
	j	.LBB39_3
.LBB39_8:                               #   in Loop: Header=BB39_3 Depth=1
	addi	a4, a3, 8
	sd	a4, 8(sp)
	ld	a5, 0(a3)
	mv	a3, a4
	add	t0, a5, t0
	addi	a0, a0, 1
	j	.LBB39_3
.LBB39_9:                               #   in Loop: Header=BB39_3 Depth=1
	addi	a4, a3, 8
	sd	a4, 8(sp)
	lw	a5, 0(a3)
	mv	a3, a4
	add	t0, t0, a5
	addi	a0, a0, 1
	j	.LBB39_3
.LBB39_10:                              #   in Loop: Header=BB39_3 Depth=1
	addi	a4, a3, 8
	sd	a4, 8(sp)
	ld	a5, 0(a3)
	mv	a3, a4
	lbu	a4, 0(a5)
	add	t0, t0, a4
	addi	a0, a0, 1
	j	.LBB39_3
.LBB39_11:                              #   in Loop: Header=BB39_3 Depth=1
	addi	a4, a3, 16
	sd	a4, 8(sp)
	ld	a5, 0(a3)
	lw	a3, 8(a3)
	slli	a6, a5, 1
	slli	a5, a5, 3
	add	a5, a5, a6
	add	a3, t0, a3
	add	t0, a5, a3
	mv	a3, a4
	addi	a0, a0, 1
	j	.LBB39_3
.LBB39_12:                              #   in Loop: Header=BB39_3 Depth=1
	bnez	a4, .LBB39_2
# %bb.13:
	mv	a0, t0
	addi	sp, sp, 80
	ret
.Lfunc_end39:
	.size	c_vmixw, .Lfunc_end39-c_vmixw
	.section	.rodata,"a",@progbits
	.p2align	3, 0x0
.LJTI39_0:
	.quad	.LBB39_5
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_1
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_9
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_8
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_10
	.quad	.LBB39_6
	.quad	.LBB39_2
	.quad	.LBB39_2
	.quad	.LBB39_11
	.quad	.LBB39_7
                                        # -- End function
	.text
	.globl	c_stackw                        # -- Begin function c_stackw
	.p2align	2
	.type	c_stackw,@function
c_stackw:                               # @c_stackw
# %bb.0:
	ld	t0, 24(sp)
	ld	t1, 8(sp)
	ld	t2, 16(sp)
	ld	t3, 0(sp)
	add	a0, a1, a0
	add	a2, a2, a3
	add	a4, a4, a5
	add	a0, a0, a2
	add	a4, a4, a6
	slli	a1, a7, 1
	li	a2, 100
	slli	a7, a7, 3
	mul	a2, t3, a2
	addw	a0, a0, a4
	add	a1, a7, a1
	add	a0, a1, a0
	add	a0, a0, t1
	srliw	a1, t2, 4
	add	a0, a0, t0
	add	a0, a0, a1
	add	a0, a0, a2
	ret
.Lfunc_end40:
	.size	c_stackw, .Lfunc_end40-c_stackw
                                        # -- End function
	.section	.rodata.cst8,"aM",@progbits,8
	.p2align	3, 0x0                          # -- Begin function c_calls_merlin
.LCPI41_0:
	.quad	2305843009347911894             # 0x20000000080000d6
.LCPI41_1:
	.quad	1152921504875282743             # 0x1000000010000137
	.text
	.globl	c_calls_merlin
	.p2align	2
	.type	c_calls_merlin,@function
c_calls_merlin:                         # @c_calls_merlin
# %bb.0:
	addi	sp, sp, -256
	sd	ra, 248(sp)                     # 8-byte Folded Spill
	sd	s0, 240(sp)                     # 8-byte Folded Spill
	sd	s1, 232(sp)                     # 8-byte Folded Spill
	sd	s2, 224(sp)                     # 8-byte Folded Spill
	sd	s3, 216(sp)                     # 8-byte Folded Spill
	sd	s4, 208(sp)                     # 8-byte Folded Spill
	sd	s5, 200(sp)                     # 8-byte Folded Spill
	sd	s6, 192(sp)                     # 8-byte Folded Spill
	sd	s7, 184(sp)                     # 8-byte Folded Spill
	sd	s8, 176(sp)                     # 8-byte Folded Spill
	sd	s9, 168(sp)                     # 8-byte Folded Spill
	lui	a0, 98305
	slli	a0, a0, 5
	addi	a0, a0, 1
	call	m_sum6
.Lpcrel_hi2:
	auipc	s2, %pcrel_hi(.L_MergedGlobals)
	addi	s2, s2, %pcrel_lo(.Lpcrel_hi2)
	lw	a1, 4(s2)
	addi	a1, a1, 1
	li	s0, 6
	sw	a1, 4(s2)
	beq	a0, s0, .LBB41_3
# %bb.1:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_3
# %bb.2:
	sw	a1, 0(s2)
.LBB41_3:
	li	a0, 4
	call	m_make12
	lw	a0, 4(s2)
	sext.w	a1, a1
	addi	a0, a0, 1
	sw	a0, 4(s2)
	beq	a1, s0, .LBB41_6
# %bb.4:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_6
# %bb.5:
	sw	a0, 0(s2)
.LBB41_6:
	li	a0, 7
	slli	a0, a0, 32
	addi	a0, a0, 5
	call	m_swap8
	mv	s0, a0
	lw	a0, 4(s2)
	li	a1, 5
	slli	a1, a1, 32
	addi	a0, a0, 1
	addi	a1, a1, 7
	sw	a0, 4(s2)
	beq	s0, a1, .LBB41_9
# %bb.7:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_9
# %bb.8:
	sw	a0, 0(s2)
.LBB41_9:
	lui	a0, 328707
	slli	a0, a0, 4
	addi	a0, a0, 513
	call	m_rev5
	li	a1, 255
	lw	a2, 4(s2)
	slli	a1, a1, 32
	addi	a1, a1, 255
	li	a3, 1
	and	a1, a0, a1
	slli	a3, a3, 32
	addi	a0, a2, 1
	addi	a3, a3, 5
	sw	a0, 4(s2)
	beq	a1, a3, .LBB41_12
# %bb.10:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_12
# %bb.11:
	sw	a0, 0(s2)
.LBB41_12:
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	s1, 3
	call	m_make3
	lw	a1, 4(s2)
	slli	a0, a0, 40
	srli	a2, a0, 56
	addi	a0, a1, 1
	sw	a0, 4(s2)
	beq	a2, s1, .LBB41_15
# %bb.13:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_15
# %bb.14:
	sw	a0, 0(s2)
.LBB41_15:
	lui	a0, 98305
	li	a1, 5
	slli	a0, a0, 5
	slli	a1, a1, 34
	addi	a2, a0, 1
	addi	a3, a1, 10
	li	a0, 1
	li	a4, 30
	li	a5, 2
	li	a6, 3
	mv	a1, s0
	call	m_many
	lw	a1, 4(s2)
	addi	a1, a1, 1
	li	a2, 107
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_18
# %bb.16:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_18
# %bb.17:
	sw	a1, 0(s2)
.LBB41_18:
	addi	a0, sp, 88
	li	a1, 3
	call	m_big
	lw	a0, 120(sp)
	lw	a1, 108(sp)
	lwu	a2, 112(sp)
	lw	a3, 116(sp)
	lwu	a4, 104(sp)
	lw	a5, 92(sp)
	lwu	a6, 96(sp)
	lw	a7, 100(sp)
	lwu	t0, 88(sp)
	lw	s4, 124(sp)
	sw	a0, 160(sp)
	slli	a3, a3, 32
	or	a2, a3, a2
	slli	a1, a1, 32
	or	a1, a1, a4
	slli	a7, a7, 32
	or	a3, a7, a6
	slli	a5, a5, 32
	or	a4, a5, t0
	li	a0, 12
	sd	a4, 128(sp)
	sd	a3, 136(sp)
	sd	a1, 144(sp)
	sd	a2, 152(sp)
	bne	s4, a0, .LBB41_20
# %bb.19:
	ld	a1, 128(sp)
	ld	a2, 136(sp)
	ld	a3, 144(sp)
	ld	a4, 152(sp)
	lw	a5, 160(sp)
	sd	a1, 88(sp)
	sd	a2, 96(sp)
	sd	a3, 104(sp)
	sd	a4, 112(sp)
	sw	a5, 120(sp)
	sw	a0, 124(sp)
	addi	a0, sp, 88
	call	m_sumbig
	addi	a0, a0, -75
	seqz	a1, a0
	j	.LBB41_21
.LBB41_20:
	li	a1, 0
.LBB41_21:
	lw	a0, 4(s2)
	addi	a0, a0, 1
	sw	a0, 4(s2)
	bnez	a1, .LBB41_24
# %bb.22:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_24
# %bb.23:
	sw	a0, 0(s2)
.LBB41_24:
	li	a0, 97
	call	m_inc1
	lw	a1, 4(s2)
	zext.b	a2, a0
	addi	a0, a1, 1
	li	a1, 98
	sw	a0, 4(s2)
	beq	a2, a1, .LBB41_27
# %bb.25:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_27
# %bb.26:
	sw	a0, 0(s2)
.LBB41_27:
	li	s3, -1
	slli	s3, s3, 32
	li	s1, 21
	slli	a0, s1, 32
	addi	a0, a0, 120
	call	m_mixed
	lw	a1, 4(s2)
	addi	a2, s3, 255
	and	a2, a0, a2
	slli	s1, s1, 33
	addi	a0, a1, 1
	addi	a1, s1, 121
	sw	a0, 4(s2)
	beq	a2, a1, .LBB41_30
# %bb.28:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_30
# %bb.29:
	sw	a0, 0(s2)
.LBB41_30:
	li	a0, 41
	call	m_union
	lw	a1, 4(s2)
	and	a2, a0, s3
	addi	a0, a1, 1
	sw	a0, 4(s2)
	beq	a2, s1, .LBB41_33
# %bb.31:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_33
# %bb.32:
	sw	a0, 0(s2)
.LBB41_33:
	li	a0, 5
	slli	a0, a0, 34
	addi	a0, a0, 10
	li	a1, 30
	call	m_modify12
	lw	a1, 4(s2)
	addi	a1, a1, 1
	li	a2, 1020
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_36
# %bb.34:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_36
# %bb.35:
	sw	a1, 0(s2)
.LBB41_36:
	lui	s5, 16
	addi	a2, s5, -3
	li	a0, -3
	li	a1, -3
	li	a3, 1
	li	s1, -3
	call	m_widen
	lw	a1, 4(s2)
	addi	a1, a1, 1
	addi	s5, s5, -8
	sw	a1, 4(s2)
	beq	a0, s5, .LBB41_39
# %bb.37:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_39
# %bb.38:
	sw	a1, 0(s2)
.LBB41_39:
	lui	a0, 32
	addi	a0, a0, -3
	call	m_trunc_sc
	lw	a1, 4(s2)
	addi	a1, a1, 1
	sw	a1, 4(s2)
	beq	a0, s1, .LBB41_42
# %bb.40:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_42
# %bb.41:
	sw	a1, 0(s2)
.LBB41_42:
	lui	a0, 24
	call	m_trunc_s
	lw	a1, 4(s2)
	addi	a1, a1, 1
	lui	a2, 1048568
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_45
# %bb.43:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_45
# %bb.44:
	sw	a1, 0(s2)
.LBB41_45:
	lui	a0, 32
	addi	a0, a0, -3
	call	m_trunc_us
	lw	a1, 4(s2)
	lui	a2, 16
	addi	a1, a1, 1
	addi	a2, a2, -3
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_48
# %bb.46:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_48
# %bb.47:
	sw	a1, 0(s2)
.LBB41_48:
	li	a0, 256
	call	m_tobool
	lw	a1, 4(s2)
	addi	a1, a1, 1
	sw	a1, 4(s2)
	bnez	a0, .LBB41_51
# %bb.49:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_51
# %bb.50:
	sw	a1, 0(s2)
.LBB41_51:
	li	a0, 0
	call	m_vsum
	lw	a1, 4(s2)
	addi	a1, a1, 1
	sw	a1, 4(s2)
	beqz	a0, .LBB41_54
# %bb.52:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_54
# %bb.53:
	sw	a1, 0(s2)
.LBB41_54:
	li	t0, 10
	li	t1, 9
	li	t2, 8
	li	a0, 10
	li	a1, 1
	li	a2, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a6, 6
	li	a7, 7
	sd	t2, 0(sp)
	sd	t1, 8(sp)
	sd	t0, 16(sp)
	li	s1, 5
	call	m_vsum
	lw	a1, 4(s2)
	addi	a1, a1, 1
	li	a2, 55
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_57
# %bb.55:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_57
# %bb.56:
	sw	a1, 0(s2)
.LBB41_57:
	ld	a0, 128(sp)
	ld	a1, 136(sp)
	ld	a2, 144(sp)
	ld	a3, 152(sp)
	lw	a4, 160(sp)
	sd	a0, 88(sp)
	sd	a1, 96(sp)
	sd	a2, 104(sp)
	sd	a3, 112(sp)
	sw	a4, 120(sp)
	sw	s4, 124(sp)
.Lpcrel_hi3:
	auipc	a0, %pcrel_hi(.L.str)
	lui	a1, 98305
	addi	a0, a0, %pcrel_lo(.Lpcrel_hi3)
	slli	a1, a1, 5
	slli	a5, s1, 34
	addi	s1, a1, 1
	addi	a5, a5, 10
	li	a1, 7
	li	a2, -3
	li	a6, 30
	addi	a7, sp, 88
	mv	a3, s1
	mv	a4, s0
	call	m_vmix
	lw	a1, 4(s2)
	addi	a1, a1, 1
	li	s5, 180
	sw	a1, 4(s2)
	beq	a0, s5, .LBB41_60
# %bb.58:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_60
# %bb.59:
	sw	a1, 0(s2)
.LBB41_60:
	ld	a0, 128(sp)
	ld	a1, 136(sp)
	ld	a2, 144(sp)
	ld	a3, 152(sp)
	lw	a4, 160(sp)
	sd	a0, 88(sp)
	sd	a1, 96(sp)
	sd	a2, 104(sp)
	sd	a3, 112(sp)
	sw	a4, 120(sp)
	sw	s4, 124(sp)
.Lpcrel_hi4:
	auipc	a0, %pcrel_hi(.L.str.1)
	li	a1, 5
	addi	a0, a0, %pcrel_lo(.Lpcrel_hi4)
	slli	a2, a1, 34
	addi	a1, sp, 88
	addi	a2, a2, 10
	li	a3, 30
	li	a6, -3
	li	a7, 7
	mv	a4, s0
	mv	a5, s1
	call	m_vmix
	lw	a1, 4(s2)
	addi	a1, a1, 1
	sw	a1, 4(s2)
	beq	a0, s5, .LBB41_63
# %bb.61:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_63
# %bb.62:
	sw	a1, 0(s2)
.LBB41_63:
	li	t0, 100
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	mv	a7, s0
	sd	t0, 0(sp)
	li	s1, 5
	call	m_vlate
	lw	a1, 4(s2)
	addi	a1, a1, 1
	li	a2, 203
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_66
# %bb.64:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_66
# %bb.65:
	sw	a1, 0(s2)
.LBB41_66:
	li	t0, -5
	li	a0, 97
	li	a1, 30
	lui	a2, 98305
	slli	s1, s1, 34
	slli	a2, a2, 5
	addi	s1, s1, 10
	sd	s1, 0(sp)
	sd	a1, 8(sp)
	sd	a0, 16(sp)
	addi	t1, a2, 1
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	mv	a7, s0
	sd	t0, 24(sp)
	sd	t1, 32(sp)
	call	m_stack
	lw	a1, 4(s2)
	addi	a2, a1, 1
	li	a3, 261
	sw	a2, 4(s2)
	beq	a0, a3, .LBB41_69
# %bb.67:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_69
# %bb.68:
	sw	a2, 0(s2)
.LBB41_69:
	addi	a1, a1, 2
	sw	a1, 4(s2)
	li	t0, 9
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	li	a7, 8
	sd	t0, 0(sp)
	li	s0, 1
	call	m_stack_ret
	li	a2, 15
	lw	a3, 4(s2)
	slli	a2, a2, 32
	slli	a1, a1, 32
	addi	a2, a2, 6
	srli	a1, a1, 32
	xor	a0, a0, a2
	xori	a1, a1, 65
	or	a1, a0, a1
	addi	a0, a3, 1
	sw	a0, 4(s2)
	beqz	a1, .LBB41_72
# %bb.70:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_72
# %bb.71:
	sw	a0, 0(s2)
.LBB41_72:
.Lpcrel_hi5:
	auipc	a1, %pcrel_hi(.L.str.2)
	slli	a2, s0, 60
	li	a0, -5
	addi	a4, a1, %pcrel_lo(.Lpcrel_hi5)
	li	a1, -1
	lui	a3, 524288
	call	m_mixw
	lw	a1, 4(s2)
	lui	a2, 32768
	addi	a2, a2, 1
	slli	a3, a2, 61
	addi	a1, a1, 1
	add	a2, a2, a3
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_75
# %bb.73:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_75
# %bb.74:
	sw	a1, 0(s2)
.LBB41_75:
	li	a0, -1
	call	m_umax
	beqz	a0, .LBB41_77
# %bb.76:
	lui	a0, 524288
	addiw	a0, a0, -1
	call	m_umax
	seqz	a1, a0
	j	.LBB41_78
.LBB41_77:
	li	a1, 0
.LBB41_78:
	lw	a0, 4(s2)
	addi	a0, a0, 1
	sw	a0, 4(s2)
	bnez	a1, .LBB41_81
# %bb.79:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_81
# %bb.80:
	sw	a0, 0(s2)
.LBB41_81:
	li	a0, -5
	li	s0, -5
	call	m_sext
	bne	a0, s0, .LBB41_83
# %bb.82:
	lui	s0, 524288
	addiw	s0, s0, -1
	mv	a0, s0
	call	m_sext
	xor	a0, a0, s0
	seqz	a1, a0
	j	.LBB41_84
.LBB41_83:
	li	a1, 0
.LBB41_84:
	lw	a0, 4(s2)
	addi	a0, a0, 1
	sw	a0, 4(s2)
	bnez	a1, .LBB41_87
# %bb.85:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_87
# %bb.86:
	sw	a0, 0(s2)
.LBB41_87:
	li	a0, -1
	li	s0, -1
	call	m_zext
	srli	s0, s0, 32
	bne	a0, s0, .LBB41_89
# %bb.88:
	lui	a0, 524288
	call	m_zext
	snez	a1, a0
	j	.LBB41_90
.LBB41_89:
	li	a1, 0
.LBB41_90:
	lw	a0, 4(s2)
	addi	a0, a0, 1
	sw	a0, 4(s2)
	bnez	a1, .LBB41_93
# %bb.91:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_93
# %bb.92:
	sw	a0, 0(s2)
.LBB41_93:
	lui	s1, 524288
	addi	s0, s1, 5
	mv	a0, s0
	call	m_narrow
	lw	a1, 4(s2)
	addi	a1, a1, 1
	sw	a1, 4(s2)
	beq	a0, s0, .LBB41_96
# %bb.94:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_96
# %bb.95:
	sw	a1, 0(s2)
.LBB41_96:
	lui	a0, 524288
	call	m_unarrow
	bne	a0, s1, .LBB41_98
# %bb.97:
	lui	a0, 524288
	call	m_unarrow
	snez	a1, a0
	j	.LBB41_99
.LBB41_98:
	li	a1, 0
.LBB41_99:
	lw	a0, 4(s2)
	addi	a0, a0, 1
	sw	a0, 4(s2)
	bnez	a1, .LBB41_102
# %bb.100:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_102
# %bb.101:
	sw	a0, 0(s2)
.LBB41_102:
	li	s0, 1
	slli	s0, s0, 60
	mv	a0, s0
	call	m_incl1
	lw	a1, 4(s2)
	addi	a1, a1, 1
	addi	a2, s0, 1
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_105
# %bb.103:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_105
# %bb.104:
	sw	a1, 0(s2)
.LBB41_105:
	li	a1, -7
	mv	a0, s0
	call	m_swapl2
	lw	a2, 4(s2)
	xor	a1, a1, s0
	xori	a0, a0, -7
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s2)
	beqz	a1, .LBB41_108
# %bb.106:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_108
# %bb.107:
	sw	a0, 0(s2)
.LBB41_108:
	li	s1, 1
	slli	a1, s1, 32
	slli	s1, s1, 60
	addi	a1, a1, -3
	mv	a0, s1
	call	m_li
	mv	s0, a1
	lw	a1, 4(s2)
	li	a2, -33
	slli	a3, s0, 32
	srli	a2, a2, 4
	srli	a3, a3, 32
	xor	a0, a0, a2
	xori	a2, a3, 3
	or	a2, a0, a2
	addi	a0, a1, 1
	sw	a0, 4(s2)
	beqz	a2, .LBB41_111
# %bb.109:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_111
# %bb.110:
	sw	a0, 0(s2)
.LBB41_111:
	li	a0, 97
	mv	a1, s1
	call	m_cl
	lw	a2, 4(s2)
	li	s8, -1
	zext.b	a3, a0
	slli	a0, s8, 60
	xori	a3, a3, 98
	xor	a1, a1, a0
	or	a3, a3, a1
	addi	a1, a2, 1
	sw	a1, 4(s2)
	beqz	a3, .LBB41_114
# %bb.112:
	lw	a2, 0(s2)
	bnez	a2, .LBB41_114
# %bb.113:
	sw	a1, 0(s2)
.LBB41_114:
	li	s9, 1
	li	a1, 2
	slli	s1, s9, 60
	sd	s1, 64(sp)
	sd	a1, 72(sp)
	sd	a0, 80(sp)
	sd	s1, 40(sp)
	sd	a1, 48(sp)
	sd	a0, 56(sp)
	addi	a0, sp, 88
	addi	a1, sp, 64
	addi	a2, sp, 40
	addi	s7, sp, 88
	call	m_addl3
	ld	s4, 88(sp)
	ld	s5, 96(sp)
	ld	s6, 104(sp)
	slli	s9, s9, 61
	lw	a0, 4(s2)
	xor	a1, s4, s9
	xori	a2, s5, 4
	slli	s8, s8, 61
	or	a1, a1, a2
	xor	a2, s6, s8
	or	a1, a1, a2
	addi	a0, a0, 1
	sw	a0, 4(s2)
	beqz	a1, .LBB41_117
# %bb.115:
	lw	a1, 0(s2)
	bnez	a1, .LBB41_117
# %bb.116:
	sw	a0, 0(s2)
.LBB41_117:
	and	s0, s0, s3
	li	a0, 5
	addi	s0, s0, 6
	sd	s4, 88(sp)
	sd	s5, 96(sp)
	sd	s6, 104(sp)
	sd	a0, 0(sp)
	sd	s0, 8(sp)
.Lpcrel_hi6:
	auipc	a0, %pcrel_hi(.L.str.3)
.Lpcrel_hi7:
	auipc	a1, %pcrel_hi(.L.str.4)
	addi	a0, a0, %pcrel_lo(.Lpcrel_hi6)
	addi	a5, a1, %pcrel_lo(.Lpcrel_hi7)
	li	a1, -1
	li	a2, -1
	lui	a4, 524288
	li	a6, 3
	li	a7, 4
	mv	a3, s1
	sd	s7, 16(sp)
	li	s7, -1
	call	m_vmixw
	lw	a1, 4(s2)
.Lpcrel_hi8:
	auipc	a2, %pcrel_hi(.LCPI41_0)
	ld	s3, %pcrel_lo(.Lpcrel_hi8)(a2)
	addi	a1, a1, 1
	sw	a1, 4(s2)
	beq	a0, s3, .LBB41_120
# %bb.118:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_120
# %bb.119:
	sw	a1, 0(s2)
.LBB41_120:
	sd	s4, 88(sp)
	sd	s5, 96(sp)
	sd	s6, 104(sp)
	li	s1, 1
.Lpcrel_hi9:
	auipc	a0, %pcrel_hi(.L.str.5)
	slli	s1, s1, 60
	sd	s1, 0(sp)
	sd	s7, 8(sp)
	sd	s7, 16(sp)
.Lpcrel_hi10:
	auipc	a1, %pcrel_hi(.L.str.4)
	addi	a0, a0, %pcrel_lo(.Lpcrel_hi9)
	addi	a6, a1, %pcrel_lo(.Lpcrel_hi10)
	addi	a1, sp, 88
	li	a2, 5
	li	a4, 3
	li	a5, 4
	lui	a7, 524288
	mv	a3, s0
	li	s0, 3
	li	s4, 4
	call	m_vmixw
	lw	a1, 4(s2)
	addi	a1, a1, 1
	sw	a1, 4(s2)
	beq	a0, s3, .LBB41_123
# %bb.121:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_123
# %bb.122:
	sw	a1, 0(s2)
.LBB41_123:
	li	t0, -1
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	li	a7, -2
	sd	s0, 0(sp)
	sd	s4, 8(sp)
	sd	t0, 16(sp)
	sd	s1, 24(sp)
	call	m_stackw
	lw	a1, 4(s2)
.Lpcrel_hi11:
	auipc	a2, %pcrel_hi(.LCPI41_1)
	ld	a2, %pcrel_lo(.Lpcrel_hi11)(a2)
	addi	a1, a1, 1
	sw	a1, 4(s2)
	beq	a0, a2, .LBB41_126
# %bb.124:
	lw	a0, 0(s2)
	bnez	a0, .LBB41_126
# %bb.125:
	sw	a1, 0(s2)
.LBB41_126:
	lw	a0, 0(s2)
	ld	ra, 248(sp)                     # 8-byte Folded Reload
	ld	s0, 240(sp)                     # 8-byte Folded Reload
	ld	s1, 232(sp)                     # 8-byte Folded Reload
	ld	s2, 224(sp)                     # 8-byte Folded Reload
	ld	s3, 216(sp)                     # 8-byte Folded Reload
	ld	s4, 208(sp)                     # 8-byte Folded Reload
	ld	s5, 200(sp)                     # 8-byte Folded Reload
	ld	s6, 192(sp)                     # 8-byte Folded Reload
	ld	s7, 184(sp)                     # 8-byte Folded Reload
	ld	s8, 176(sp)                     # 8-byte Folded Reload
	ld	s9, 168(sp)                     # 8-byte Folded Reload
	addi	sp, sp, 256
	ret
.Lfunc_end41:
	.size	c_calls_merlin, .Lfunc_end41-c_calls_merlin
                                        # -- End function
	.type	.L.str,@object                  # @.str
	.section	.rodata.str1.1,"aMS",@progbits,1
.L.str:
	.asciz	"icspbB"
	.size	.L.str, 7

	.type	.L.str.1,@object                # @.str.1
.L.str.1:
	.asciz	"Bbpsci"
	.size	.L.str.1, 7

	.type	.L.str.2,@object                # @.str.2
.L.str.2:
	.asciz	"x7"
	.size	.L.str.2, 3

	.type	.L.str.3,@object                # @.str.3
.L.str.3:
	.asciz	"iulLpqtT"
	.size	.L.str.3, 9

	.type	.L.str.4,@object                # @.str.4
.L.str.4:
	.asciz	"z"
	.size	.L.str.4, 2

	.type	.L.str.5,@object                # @.str.5
.L.str.5:
	.asciz	"TtqpLlui"
	.size	.L.str.5, 9

	.type	.L_MergedGlobals,@object        # @_MergedGlobals
	.local	.L_MergedGlobals
	.comm	.L_MergedGlobals,8,4
first_failure = .L_MergedGlobals
	.size	first_failure, 4
checks = .L_MergedGlobals+4
	.size	checks, 4
	.ident	"clang version 23.1.4 (https://github.com/llvm/llvm-project 7a941dfb2877a0b8cf30323fefa85ad019e0b0f5)"
	.section	".note.GNU-stack","",@progbits
