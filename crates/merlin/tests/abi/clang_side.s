	.attribute	4, 16
	.attribute	5, "rv32i2p1_m2p0_zmmul1p0"
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
	slli	a0, a0, 24
	srai	a0, a0, 24
	ret
.Lfunc_end1:
	.size	c_trunc_sc, .Lfunc_end1-c_trunc_sc
                                        # -- End function
	.globl	c_trunc_s                       # -- Begin function c_trunc_s
	.p2align	2
	.type	c_trunc_s,@function
c_trunc_s:                              # @c_trunc_s
# %bb.0:
	slli	a0, a0, 16
	srai	a0, a0, 16
	ret
.Lfunc_end2:
	.size	c_trunc_s, .Lfunc_end2-c_trunc_s
                                        # -- End function
	.globl	c_trunc_us                      # -- Begin function c_trunc_us
	.p2align	2
	.type	c_trunc_us,@function
c_trunc_us:                             # @c_trunc_us
# %bb.0:
	slli	a0, a0, 16
	srli	a0, a0, 16
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
	zext.b	a0, a0
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
	zext.b	a1, a0
	slli	a2, a0, 16
	slli	a0, a0, 8
	srli	a2, a2, 24
	add	a1, a2, a1
	srli	a0, a0, 24
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
	srli	a1, a0, 16
	slli	a0, a0, 16
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
	li	a2, 0
	addi	a3, sp, 4
	sw	a0, 0(sp)
	sb	a1, 4(sp)
	addi	a0, sp, 8
	li	a1, 5
.LBB9_1:                                # =>This Inner Loop Header: Depth=1
	lbu	a4, 0(a3)
	add	a5, a0, a2
	addi	a2, a2, 1
	sb	a4, 0(a5)
	addi	a3, a3, -1
	bne	a2, a1, .LBB9_1
# %bb.2:
	lw	a0, 8(sp)
	lbu	a1, 12(sp)
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
	slli	a1, a1, 16
	lui	a3, 16
	addi	a3, a3, -1
	and	a0, a0, a3
	or	a0, a1, a0
	and	a1, a2, a3
	ret
.Lfunc_end10:
	.size	c_make6, .Lfunc_end10-c_make6
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
.Lfunc_end11:
	.size	c_sum6, .Lfunc_end11-c_sum6
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
.Lfunc_end12:
	.size	c_twice6, .Lfunc_end12-c_twice6
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
.Lfunc_end13:
	.size	c_swap8, .Lfunc_end13-c_swap8
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
.Lfunc_end14:
	.size	c_mixed, .Lfunc_end14-c_mixed
                                        # -- End function
	.globl	c_union                         # -- Begin function c_union
	.p2align	2
	.type	c_union,@function
c_union:                                # @c_union
# %bb.0:
	addi	a1, a0, 1
	ret
.Lfunc_end15:
	.size	c_union, .Lfunc_end15-c_union
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
.Lfunc_end16:
	.size	c_add12, .Lfunc_end16-c_add12
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
.Lfunc_end17:
	.size	c_scaled, .Lfunc_end17-c_scaled
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
.Lfunc_end18:
	.size	c_modify12, .Lfunc_end18-c_modify12
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
	add	a0, a3, a0
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
.Lfunc_end22:
	.size	c_apply6, .Lfunc_end22-c_apply6
                                        # -- End function
	.globl	c_vmix                          # -- Begin function c_vmix
	.p2align	2
	.type	c_vmix,@function
c_vmix:                                 # @c_vmix
# %bb.0:
	addi	sp, sp, -112
	sw	ra, 76(sp)                      # 4-byte Folded Spill
	sw	s0, 72(sp)                      # 4-byte Folded Spill
	sw	s1, 68(sp)                      # 4-byte Folded Spill
	sw	s2, 64(sp)                      # 4-byte Folded Spill
	sw	s3, 60(sp)                      # 4-byte Folded Spill
	sw	s4, 56(sp)                      # 4-byte Folded Spill
	sw	s5, 52(sp)                      # 4-byte Folded Spill
	sw	s6, 48(sp)                      # 4-byte Folded Spill
	mv	s0, a0
	li	s1, 0
	sw	a5, 100(sp)
	sw	a6, 104(sp)
	sw	a7, 108(sp)
	sw	a1, 84(sp)
	sw	a2, 88(sp)
	sw	a3, 92(sp)
	sw	a4, 96(sp)
	addi	s5, sp, 84
	addi	s2, sp, 48
	li	s3, 49
	sw	s5, 4(sp)
	lui	s4, %hi(.LJTI23_0)
	addi	s4, s4, %lo(.LJTI23_0)
	j	.LBB23_3
.LBB23_1:                               #   in Loop: Header=BB23_3 Depth=1
	beqz	a0, .LBB23_14
.LBB23_2:                               #   in Loop: Header=BB23_3 Depth=1
	addi	s0, s0, 1
.LBB23_3:                               # =>This Loop Header: Depth=1
                                        #     Child Loop BB23_6 Depth 2
	lbu	a0, 0(s0)
	addi	a1, a0, -66
	bltu	s3, a1, .LBB23_1
# %bb.4:                                #   in Loop: Header=BB23_3 Depth=1
	slli	a1, a1, 2
	add	a1, s4, a1
	lw	a0, 0(a1)
	jr	a0
.LBB23_5:                               #   in Loop: Header=BB23_3 Depth=1
	lw	a1, 0(s5)
	addi	a0, sp, 8
	li	a2, 40
	addi	s6, sp, 8
	call	memcpy
.LBB23_6:                               #   Parent Loop BB23_3 Depth=1
                                        # =>  This Inner Loop Header: Depth=2
	lw	a0, 0(s6)
	addi	s6, s6, 4
	add	s1, a0, s1
	bne	s6, s2, .LBB23_6
# %bb.7:                                #   in Loop: Header=BB23_3 Depth=1
	addi	s5, s5, 4
	j	.LBB23_2
.LBB23_8:                               #   in Loop: Header=BB23_3 Depth=1
	lb	a0, 0(s5)
	j	.LBB23_12
.LBB23_9:                               #   in Loop: Header=BB23_3 Depth=1
	lw	a0, 0(s5)
	lw	a1, 0(a0)
	lw	a2, 4(a0)
	lw	a0, 8(a0)
	addi	s5, s5, 4
	add	a1, a1, s1
	add	a0, a2, a0
	add	s1, a1, a0
	addi	s0, s0, 1
	j	.LBB23_3
.LBB23_10:                              #   in Loop: Header=BB23_3 Depth=1
	lh	a0, 0(s5)
	lh	a1, 2(s5)
	lh	a2, 4(s5)
	addi	s5, s5, 8
	add	a0, s1, a0
	add	a1, a1, a2
	add	s1, a0, a1
	addi	s0, s0, 1
	j	.LBB23_3
.LBB23_11:                              #   in Loop: Header=BB23_3 Depth=1
	lw	a0, 0(s5)
.LBB23_12:                              #   in Loop: Header=BB23_3 Depth=1
	addi	s5, s5, 4
	add	s1, a0, s1
	addi	s0, s0, 1
	j	.LBB23_3
.LBB23_13:                              #   in Loop: Header=BB23_3 Depth=1
	lw	a0, 0(s5)
	lw	a1, 4(s5)
	addi	s5, s5, 8
	mul	a0, a1, a0
	add	s1, a0, s1
	addi	s0, s0, 1
	j	.LBB23_3
.LBB23_14:
	mv	a0, s1
	lw	ra, 76(sp)                      # 4-byte Folded Reload
	lw	s0, 72(sp)                      # 4-byte Folded Reload
	lw	s1, 68(sp)                      # 4-byte Folded Reload
	lw	s2, 64(sp)                      # 4-byte Folded Reload
	lw	s3, 60(sp)                      # 4-byte Folded Reload
	lw	s4, 56(sp)                      # 4-byte Folded Reload
	lw	s5, 52(sp)                      # 4-byte Folded Reload
	lw	s6, 48(sp)                      # 4-byte Folded Reload
	addi	sp, sp, 112
	ret
.Lfunc_end23:
	.size	c_vmix, .Lfunc_end23-c_vmix
	.section	.rodata,"a",@progbits
	.p2align	2, 0x0
.LJTI23_0:
	.word	.LBB23_5
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_9
	.word	.LBB23_8
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_11
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_13
	.word	.LBB23_2
	.word	.LBB23_2
	.word	.LBB23_10
                                        # -- End function
	.text
	.globl	c_vsum                          # -- Begin function c_vsum
	.p2align	2
	.type	c_vsum,@function
c_vsum:                                 # @c_vsum
# %bb.0:
	addi	sp, sp, -48
	mv	t0, a0
	sw	a5, 36(sp)
	sw	a6, 40(sp)
	sw	a7, 44(sp)
	sw	a1, 20(sp)
	sw	a2, 24(sp)
	sw	a3, 28(sp)
	sw	a4, 32(sp)
	addi	a0, sp, 20
	sw	a0, 12(sp)
	li	a0, 0
	blez	t0, .LBB24_3
# %bb.1:
	lw	a1, 12(sp)
	addi	a1, a1, 4
	slli	a2, t0, 2
	add	a2, a1, a2
.LBB24_2:                               # =>This Inner Loop Header: Depth=1
	sw	a1, 12(sp)
	lw	a3, -4(a1)
	addi	a1, a1, 4
	add	a0, a3, a0
	bne	a1, a2, .LBB24_2
.LBB24_3:
	addi	sp, sp, 48
	ret
.Lfunc_end24:
	.size	c_vsum, .Lfunc_end24-c_vsum
                                        # -- End function
	.globl	c_vlate                         # -- Begin function c_vlate
	.p2align	2
	.type	c_vlate,@function
c_vlate:                                # @c_vlate
# %bb.0:
	addi	sp, sp, -16
	lw	t0, 16(sp)
	lw	t1, 20(sp)
	add	a0, a1, a0
	add	a2, a2, a3
	add	a4, a4, a5
	slli	a1, a7, 1
	slli	a3, a7, 3
	add	a0, a0, a2
	add	a4, a4, a6
	add	a0, a0, a4
	add	a1, a3, a1
	add	a0, a0, a1
	add	a0, a0, t0
	add	a0, a0, t1
	sw	a7, 12(sp)
	addi	sp, sp, 16
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
	sw	s0, 12(sp)                      # 4-byte Folded Spill
	lh	t0, 36(sp)
	lw	t1, 28(sp)
	lw	t2, 24(sp)
	lw	t3, 16(sp)
	lw	t4, 32(sp)
	lw	t5, 20(sp)
	li	t6, 1000
	slli	s0, a7, 1
	slli	a7, a7, 3
	add	a0, a0, a1
	add	a2, a2, a3
	add	a4, a4, a5
	add	a7, a7, s0
	add	a0, a0, a2
	add	a4, a4, a6
	add	a0, a0, a4
	add	a0, a0, a7
	lw	a1, 4(t5)
	lw	a2, 8(t5)
	slli	a3, t4, 16
	srai	a4, t4, 16
	add	a0, a0, t3
	add	t1, t2, t1
	srai	a3, a3, 16
	add	a0, a0, t1
	add	a0, a0, a4
	add	a3, a3, t0
	add	a0, a0, a3
	add	a0, a0, a1
	add	a0, a0, a2
	addi	a0, a0, 10
	sw	t6, 0(t5)
	lw	s0, 12(sp)                      # 4-byte Folded Reload
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
	lw	t0, 4(sp)
	lw	t1, 0(sp)
	add	a1, a2, a1
	add	a4, a5, a4
	add	a1, a1, a3
	add	a4, a4, a6
	mul	a2, t1, a7
	add	a2, a2, t0
	sw	a1, 0(a0)
	sw	a4, 4(a0)
	sw	a2, 8(a0)
	ret
.Lfunc_end27:
	.size	c_stack_ret, .Lfunc_end27-c_stack_ret
                                        # -- End function
	.globl	c_calls_merlin                  # -- Begin function c_calls_merlin
	.p2align	2
	.type	c_calls_merlin,@function
c_calls_merlin:                         # @c_calls_merlin
# %bb.0:
	addi	sp, sp, -160
	sw	ra, 156(sp)                     # 4-byte Folded Spill
	sw	s0, 152(sp)                     # 4-byte Folded Spill
	sw	s1, 148(sp)                     # 4-byte Folded Spill
	sw	s2, 144(sp)                     # 4-byte Folded Spill
	sw	s3, 140(sp)                     # 4-byte Folded Spill
	sw	s4, 136(sp)                     # 4-byte Folded Spill
	sw	s5, 132(sp)                     # 4-byte Folded Spill
	sw	s6, 128(sp)                     # 4-byte Folded Spill
	sw	s7, 124(sp)                     # 4-byte Folded Spill
	sw	s8, 120(sp)                     # 4-byte Folded Spill
	sw	s9, 116(sp)                     # 4-byte Folded Spill
	lui	a0, 32
	addi	a0, a0, 1
	li	a1, 3
	call	m_sum6
	lui	s0, %hi(.L_MergedGlobals)
	addi	s1, s0, %lo(.L_MergedGlobals)
	lw	a1, 4(s1)
	addi	a1, a1, 1
	li	s2, 6
	sw	a1, 4(s1)
	beq	a0, s2, .LBB28_3
# %bb.1:
	lw	a0, %lo(.L_MergedGlobals)(s0)
	bnez	a0, .LBB28_3
# %bb.2:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_3:
	addi	a0, sp, 40
	li	a1, 4
	call	m_make12
	lw	a0, 4(s1)
	lw	a1, 48(sp)
	addi	a0, a0, 1
	sw	a0, 4(s1)
	beq	a1, s2, .LBB28_6
# %bb.4:
	lw	a1, %lo(.L_MergedGlobals)(s0)
	bnez	a1, .LBB28_6
# %bb.5:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB28_6:
	li	a0, 5
	li	a1, 7
	call	m_swap8
	mv	s0, a0
	mv	s1, a1
	lui	s2, %hi(.L_MergedGlobals)
	addi	s3, s2, %lo(.L_MergedGlobals)
	lw	a0, 4(s3)
	xori	a1, s0, 7
	xori	a2, s1, 5
	or	a1, a1, a2
	addi	a0, a0, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB28_9
# %bb.7:
	lw	a1, %lo(.L_MergedGlobals)(s2)
	bnez	a1, .LBB28_9
# %bb.8:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB28_9:
	lui	a0, 16432
	addi	a0, a0, 513
	li	a1, 5
	call	m_rev5
	zext.b	a0, a0
	lw	a2, 4(s3)
	xori	a0, a0, 5
	zext.b	a1, a1
	xori	a1, a1, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB28_12
# %bb.10:
	lw	a1, %lo(.L_MergedGlobals)(s2)
	bnez	a1, .LBB28_12
# %bb.11:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB28_12:
	li	a0, 1
	li	a1, 2
	li	a2, 3
	call	m_make3
	lui	s2, %hi(.L_MergedGlobals)
	addi	s3, s2, %lo(.L_MergedGlobals)
	lw	a1, 4(s3)
	lui	a2, 4080
	and	a2, a0, a2
	addi	a0, a1, 1
	lui	a1, 48
	sw	a0, 4(s3)
	beq	a2, a1, .LBB28_15
# %bb.13:
	lw	a1, %lo(.L_MergedGlobals)(s2)
	bnez	a1, .LBB28_15
# %bb.14:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB28_15:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 40(sp)
	sw	a1, 44(sp)
	sw	a2, 48(sp)
	lui	a3, 32
	li	a0, 1
	li	a4, 3
	addi	a5, sp, 40
	li	a6, 2
	addi	a3, a3, 1
	li	a7, 3
	mv	a1, s0
	mv	a2, s1
	call	m_many
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 107
	sw	a1, 4(s3)
	beq	a0, a2, .LBB28_18
# %bb.16:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_18
# %bb.17:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_18:
	addi	a0, sp, 40
	li	a1, 3
	call	m_big
	addi	a0, sp, 80
	addi	a1, sp, 40
	li	a2, 36
	call	memcpy
	lw	s3, 76(sp)
	li	s2, 12
	bne	s3, s2, .LBB28_20
# %bb.19:
	addi	a0, sp, 40
	addi	a1, sp, 80
	li	a2, 36
	call	memcpy
	sw	s2, 76(sp)
	addi	a0, sp, 40
	call	m_sumbig
	addi	a0, a0, -75
	seqz	a0, a0
	j	.LBB28_21
.LBB28_20:
	li	a0, 0
.LBB28_21:
	lui	s2, %hi(.L_MergedGlobals)
	addi	s4, s2, %lo(.L_MergedGlobals)
	lw	a1, 4(s4)
	addi	a1, a1, 1
	sw	a1, 4(s4)
	bnez	a0, .LBB28_24
# %bb.22:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_24
# %bb.23:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_24:
	li	a0, 97
	call	m_inc1
	lw	a1, 4(s4)
	zext.b	a2, a0
	addi	a0, a1, 1
	li	a1, 98
	sw	a0, 4(s4)
	beq	a2, a1, .LBB28_27
# %bb.25:
	lw	a1, %lo(.L_MergedGlobals)(s2)
	bnez	a1, .LBB28_27
# %bb.26:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB28_27:
	li	a0, 120
	li	a1, 21
	call	m_mixed
	zext.b	a0, a0
	lui	s2, %hi(.L_MergedGlobals)
	addi	s4, s2, %lo(.L_MergedGlobals)
	lw	a2, 4(s4)
	xori	a0, a0, 121
	xori	a1, a1, 42
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s4)
	beqz	a1, .LBB28_30
# %bb.28:
	lw	a1, %lo(.L_MergedGlobals)(s2)
	bnez	a1, .LBB28_30
# %bb.29:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB28_30:
	li	a0, 41
	li	a1, 0
	call	m_union
	lw	a0, 4(s4)
	addi	a0, a0, 1
	li	a2, 42
	sw	a0, 4(s4)
	beq	a1, a2, .LBB28_33
# %bb.31:
	lw	a1, %lo(.L_MergedGlobals)(s2)
	bnez	a1, .LBB28_33
# %bb.32:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB28_33:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 40(sp)
	sw	a1, 44(sp)
	sw	a2, 48(sp)
	addi	a0, sp, 40
	call	m_modify12
	lui	s2, %hi(.L_MergedGlobals)
	addi	s4, s2, %lo(.L_MergedGlobals)
	lw	a1, 4(s4)
	addi	a1, a1, 1
	li	a2, 1020
	sw	a1, 4(s4)
	beq	a0, a2, .LBB28_36
# %bb.34:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_36
# %bb.35:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_36:
	lui	s6, 16
	li	a0, -3
	li	a1, -3
	addi	a2, s6, -3
	li	a3, 1
	li	s5, -3
	call	m_widen
	lw	a1, 4(s4)
	addi	a1, a1, 1
	addi	s6, s6, -8
	sw	a1, 4(s4)
	beq	a0, s6, .LBB28_39
# %bb.37:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_39
# %bb.38:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_39:
	lui	a0, 32
	addi	a0, a0, -3
	call	m_trunc_sc
	lui	s2, %hi(.L_MergedGlobals)
	addi	s4, s2, %lo(.L_MergedGlobals)
	lw	a1, 4(s4)
	addi	a1, a1, 1
	sw	a1, 4(s4)
	beq	a0, s5, .LBB28_42
# %bb.40:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_42
# %bb.41:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_42:
	lui	a0, 24
	call	m_trunc_s
	lw	a1, 4(s4)
	addi	a1, a1, 1
	lui	a2, 1048568
	sw	a1, 4(s4)
	beq	a0, a2, .LBB28_45
# %bb.43:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_45
# %bb.44:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_45:
	lui	a0, 32
	addi	a0, a0, -3
	call	m_trunc_us
	lui	s2, %hi(.L_MergedGlobals)
	addi	s4, s2, %lo(.L_MergedGlobals)
	lw	a1, 4(s4)
	lui	a2, 16
	addi	a1, a1, 1
	addi	a2, a2, -3
	sw	a1, 4(s4)
	beq	a0, a2, .LBB28_48
# %bb.46:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_48
# %bb.47:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_48:
	li	a0, 256
	call	m_tobool
	lw	a1, 4(s4)
	addi	a1, a1, 1
	sw	a1, 4(s4)
	bnez	a0, .LBB28_51
# %bb.49:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_51
# %bb.50:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_51:
	li	a0, 0
	call	m_vsum
	lui	s2, %hi(.L_MergedGlobals)
	addi	s4, s2, %lo(.L_MergedGlobals)
	lw	a1, 4(s4)
	addi	a1, a1, 1
	sw	a1, 4(s4)
	beqz	a0, .LBB28_54
# %bb.52:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_54
# %bb.53:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_54:
	li	s6, 10
	li	t0, 9
	li	t1, 8
	li	a0, 10
	li	a1, 1
	li	a2, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a6, 6
	li	a7, 7
	sw	t1, 0(sp)
	sw	t0, 4(sp)
	sw	s6, 8(sp)
	call	m_vsum
	lw	a1, 4(s4)
	addi	a1, a1, 1
	li	a2, 55
	sw	a1, 4(s4)
	beq	a0, a2, .LBB28_57
# %bb.55:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_57
# %bb.56:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_57:
	li	s4, 20
	li	s5, 30
	sw	s6, 28(sp)
	sw	s4, 32(sp)
	sw	s5, 36(sp)
	addi	a0, sp, 40
	addi	a1, sp, 80
	li	a2, 36
	addi	s2, sp, 40
	call	memcpy
	sw	s3, 76(sp)
	sw	s2, 0(sp)
	lui	a0, %hi(.L.str)
	addi	a0, a0, %lo(.L.str)
	lui	s2, 32
	li	a1, 7
	li	a2, -3
	li	a4, 3
	addi	s2, s2, 1
	addi	a7, sp, 28
	li	s8, 7
	mv	a3, s2
	mv	a5, s0
	mv	a6, s1
	call	m_vmix
	lui	s6, %hi(.L_MergedGlobals)
	addi	s7, s6, %lo(.L_MergedGlobals)
	lw	a1, 4(s7)
	addi	a1, a1, 1
	li	s9, 180
	sw	a1, 4(s7)
	beq	a0, s9, .LBB28_60
# %bb.58:
	lw	a0, %lo(.L_MergedGlobals)(s6)
	bnez	a0, .LBB28_60
# %bb.59:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_60:
	addi	a0, sp, 40
	addi	a1, sp, 80
	li	a2, 36
	call	memcpy
	sw	s3, 76(sp)
	li	a0, 10
	sw	a0, 28(sp)
	sw	s4, 32(sp)
	sw	s5, 36(sp)
	lui	a0, %hi(.L.str.1)
	addi	a0, a0, %lo(.L.str.1)
	addi	a1, sp, 40
	addi	a2, sp, 28
	li	a6, 3
	li	a7, -3
	sw	s8, 0(sp)
	mv	a3, s0
	mv	a4, s1
	mv	a5, s2
	call	m_vmix
	lw	a1, 4(s7)
	addi	a1, a1, 1
	sw	a1, 4(s7)
	beq	a0, s9, .LBB28_63
# %bb.61:
	lw	a0, %lo(.L_MergedGlobals)(s6)
	bnez	a0, .LBB28_63
# %bb.62:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_63:
	li	a7, 100
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	sw	s1, 0(sp)
	sw	a7, 4(sp)
	li	s4, 3
	mv	a7, s0
	call	m_vlate
	lui	s2, %hi(.L_MergedGlobals)
	addi	s3, s2, %lo(.L_MergedGlobals)
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 203
	sw	a1, 4(s3)
	beq	a0, a2, .LBB28_66
# %bb.64:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_66
# %bb.65:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB28_66:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 40(sp)
	sw	a1, 44(sp)
	sw	a2, 48(sp)
	lui	a6, 32
	li	a7, -5
	li	t0, 97
	addi	t1, sp, 40
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	addi	a6, a6, 1
	sw	a6, 16(sp)
	sw	s4, 20(sp)
	li	a6, 7
	sw	s1, 0(sp)
	sw	t1, 4(sp)
	sw	t0, 8(sp)
	sw	a7, 12(sp)
	mv	a7, s0
	call	m_stack
	lw	a1, 4(s3)
	addi	a2, a1, 1
	li	a3, 261
	sw	a2, 4(s3)
	beq	a0, a3, .LBB28_69
# %bb.67:
	lw	a0, %lo(.L_MergedGlobals)(s2)
	bnez	a0, .LBB28_69
# %bb.68:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a2, %lo(.L_MergedGlobals)(a0)
.LBB28_69:
	addi	a1, a1, 2
	lui	s0, %hi(.L_MergedGlobals)
	addi	s1, s0, %lo(.L_MergedGlobals)
	sw	a1, 4(s1)
	li	t0, 9
	li	t1, 8
	addi	a0, sp, 40
	li	a1, 1
	li	a2, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a6, 6
	li	a7, 7
	sw	t1, 0(sp)
	sw	t0, 4(sp)
	call	m_stack_ret
	lw	a0, 40(sp)
	lw	a1, 44(sp)
	lw	a2, 48(sp)
	lw	a3, 4(s1)
	xori	a0, a0, 6
	xori	a1, a1, 15
	or	a0, a0, a1
	xori	a1, a2, 65
	or	a1, a0, a1
	addi	a0, a3, 1
	sw	a0, 4(s1)
	beqz	a1, .LBB28_72
# %bb.70:
	lw	a1, %lo(.L_MergedGlobals)(s0)
	bnez	a1, .LBB28_72
# %bb.71:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB28_72:
	lw	a0, %lo(.L_MergedGlobals)(s0)
	lw	ra, 156(sp)                     # 4-byte Folded Reload
	lw	s0, 152(sp)                     # 4-byte Folded Reload
	lw	s1, 148(sp)                     # 4-byte Folded Reload
	lw	s2, 144(sp)                     # 4-byte Folded Reload
	lw	s3, 140(sp)                     # 4-byte Folded Reload
	lw	s4, 136(sp)                     # 4-byte Folded Reload
	lw	s5, 132(sp)                     # 4-byte Folded Reload
	lw	s6, 128(sp)                     # 4-byte Folded Reload
	lw	s7, 124(sp)                     # 4-byte Folded Reload
	lw	s8, 120(sp)                     # 4-byte Folded Reload
	lw	s9, 116(sp)                     # 4-byte Folded Reload
	addi	sp, sp, 160
	ret
.Lfunc_end28:
	.size	c_calls_merlin, .Lfunc_end28-c_calls_merlin
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

	.type	.L_MergedGlobals,@object        # @_MergedGlobals
	.local	.L_MergedGlobals
	.comm	.L_MergedGlobals,8,4
first_failure = .L_MergedGlobals
	.size	first_failure, 4
checks = .L_MergedGlobals+4
	.size	checks, 4
	.ident	"clang version 21.1.5"
	.section	".note.GNU-stack","",@progbits
