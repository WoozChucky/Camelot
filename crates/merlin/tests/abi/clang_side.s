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
	slli	a1, a0, 8
	srli	a1, a1, 24
	zext.b	a2, a0
	slli	a0, a0, 16
	srli	a0, a0, 24
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
	lui	a3, 16
	addi	a3, a3, -1
	slli	a1, a1, 16
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
	srai	a2, a2, 16
	srai	a0, a0, 16
	slli	a1, a1, 16
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
	lui	a2, 1048560
	slli	a0, a0, 1
	addi	a2, a2, -2
	slli	a1, a1, 17
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
	addi	a2, a0, 1
	andi	a0, a0, -256
	zext.b	a2, a2
	slli	a1, a1, 1
	or	a0, a2, a0
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
	lw	a3, 0(a2)
	lw	a4, 4(a2)
	lw	a5, 0(a1)
	lw	a6, 4(a1)
	lw	a1, 8(a1)
	lw	a2, 8(a2)
	add	a3, a3, a5
	add	a4, a4, a6
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
	mv	a1, a0
	lw	a0, 4(a0)
	addi	a0, a0, 1000
	li	a2, 1000
	sw	a2, 0(a1)
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
	srai	a2, a2, 16
	srai	a3, a3, 16
	slli	a4, a4, 16
	add	a0, a0, a3
	lw	a3, 0(a5)
	lw	t0, 4(a5)
	lw	a5, 8(a5)
	add	a0, a0, a6
	srai	a4, a4, 16
	add	a0, a0, a2
	add	a0, a0, a4
	add	a0, a0, a7
	add	a3, a3, t0
	add	a0, a1, a0
	add	a3, a3, a5
	add	a0, a0, a3
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
	sw	s5, 4(sp)
	addi	s2, sp, 48
	li	s3, 49
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
	mul	a0, a1, a0
	addi	s5, s5, 8
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
	addi	a0, sp, 20
	sw	a1, 20(sp)
	sw	a2, 24(sp)
	sw	a3, 28(sp)
	sw	a4, 32(sp)
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
	add	a0, a0, a2
	add	a4, a4, a6
	slli	a1, a7, 1
	slli	a2, a7, 3
	add	a0, a0, a4
	add	a1, a2, a1
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
	lw	t1, 20(sp)
	lw	t2, 28(sp)
	lw	t3, 24(sp)
	lw	t4, 16(sp)
	lw	t5, 32(sp)
	slli	t6, a7, 1
	slli	a7, a7, 3
	add	a7, a7, t6
	lw	t6, 4(t1)
	lw	s0, 8(t1)
	add	a0, a0, a1
	add	a2, a2, a3
	add	a4, a4, a5
	add	a0, a0, a2
	add	a4, a4, a6
	slli	a1, t5, 16
	add	a0, a0, a4
	srai	a1, a1, 16
	add	a0, a0, a7
	add	a0, a0, t4
	add	t2, t3, t2
	srai	a2, t5, 16
	add	a0, a0, t2
	add	a0, a0, a2
	add	a1, a1, t0
	add	a0, a0, a1
	add	a0, a0, t6
	add	a0, a0, s0
	addi	a0, a0, 10
	li	a1, 1000
	sw	a1, 0(t1)
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
	lw	t0, 0(sp)
	mul	a7, t0, a7
	lw	t0, 4(sp)
	add	a1, a2, a1
	add	a4, a5, a4
	add	a1, a1, a3
	add	a4, a4, a6
	add	a7, a7, t0
	sw	a1, 0(a0)
	sw	a4, 4(a0)
	sw	a7, 8(a0)
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
	srli	a1, a1, 4
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
	ret
.Lfunc_end31:
	.size	c_zext, .Lfunc_end31-c_zext
                                        # -- End function
	.globl	c_narrow                        # -- Begin function c_narrow
	.p2align	2
	.type	c_narrow,@function
c_narrow:                               # @c_narrow
# %bb.0:
	ret
.Lfunc_end32:
	.size	c_narrow, .Lfunc_end32-c_narrow
                                        # -- End function
	.globl	c_unarrow                       # -- Begin function c_unarrow
	.p2align	2
	.type	c_unarrow,@function
c_unarrow:                              # @c_unarrow
# %bb.0:
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
	add	a0, a0, a1
	neg	a1, a1
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
	lw	a3, 0(a2)
	lw	a4, 4(a2)
	lw	a5, 0(a1)
	lw	a6, 4(a1)
	lw	a1, 8(a1)
	lw	a2, 8(a2)
	add	a3, a3, a5
	add	a4, a4, a6
	add	a1, a2, a1
	sw	a3, 0(a0)
	sw	a4, 4(a0)
	sw	a1, 8(a0)
	ret
.Lfunc_end38:
	.size	c_addl3, .Lfunc_end38-c_addl3
                                        # -- End function
	.globl	c_vmixw                         # -- Begin function c_vmixw
	.p2align	2
	.type	c_vmixw,@function
c_vmixw:                                # @c_vmixw
# %bb.0:
	addi	sp, sp, -48
	li	t0, 0
	sw	a5, 36(sp)
	sw	a6, 40(sp)
	sw	a7, 44(sp)
	addi	a5, sp, 20
	sw	a1, 20(sp)
	sw	a2, 24(sp)
	sw	a3, 28(sp)
	sw	a4, 32(sp)
	sw	a5, 12(sp)
	li	a1, 41
	lui	a2, %hi(.LJTI39_0)
	addi	a2, a2, %lo(.LJTI39_0)
	j	.LBB39_2
.LBB39_1:                               #   in Loop: Header=BB39_2 Depth=1
	addi	a3, a5, 4
	sw	a3, 12(sp)
	lw	a4, 0(a5)
	mv	a5, a3
	srli	a4, a4, 4
	add	t0, a4, t0
	addi	a0, a0, 1
.LBB39_2:                               # =>This Inner Loop Header: Depth=1
	lbu	a3, 0(a0)
	addi	a4, a3, -76
	bltu	a1, a4, .LBB39_8
# %bb.3:                                #   in Loop: Header=BB39_2 Depth=1
	slli	a4, a4, 2
	add	a4, a2, a4
	lw	a3, 0(a4)
	jr	a3
.LBB39_4:                               #   in Loop: Header=BB39_2 Depth=1
	addi	a3, a5, 4
	sw	a3, 12(sp)
	lw	a4, 0(a5)
	mv	a5, a3
	add	t0, a4, t0
	addi	a0, a0, 1
	j	.LBB39_2
.LBB39_5:                               #   in Loop: Header=BB39_2 Depth=1
	addi	a3, a5, 8
	sw	a3, 12(sp)
	lw	a4, 0(a5)
	lw	a5, 4(a5)
	slli	a6, a4, 1
	slli	a4, a4, 3
	add	a4, a4, a6
	add	a5, a5, t0
	add	t0, a5, a4
	mv	a5, a3
	addi	a0, a0, 1
	j	.LBB39_2
.LBB39_6:                               #   in Loop: Header=BB39_2 Depth=1
	addi	a3, a5, 4
	sw	a3, 12(sp)
	lw	a4, 0(a5)
	mv	a5, a3
	lbu	a3, 0(a4)
	add	t0, t0, a3
	addi	a0, a0, 1
	j	.LBB39_2
.LBB39_7:                               #   in Loop: Header=BB39_2 Depth=1
	addi	a3, a5, 4
	sw	a3, 12(sp)
	lw	a4, 0(a5)
	mv	a5, a3
	lw	a3, 0(a4)
	lw	a6, 4(a4)
	lw	a4, 8(a4)
	add	a3, a3, t0
	add	a4, a6, a4
	add	t0, a3, a4
	j	.LBB39_9
.LBB39_8:                               #   in Loop: Header=BB39_2 Depth=1
	beqz	a3, .LBB39_10
.LBB39_9:                               #   in Loop: Header=BB39_2 Depth=1
	addi	a0, a0, 1
	j	.LBB39_2
.LBB39_10:
	mv	a0, t0
	addi	sp, sp, 48
	ret
.Lfunc_end39:
	.size	c_vmixw, .Lfunc_end39-c_vmixw
	.section	.rodata,"a",@progbits
	.p2align	2, 0x0
.LJTI39_0:
	.word	.LBB39_1
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_7
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_4
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_4
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_6
	.word	.LBB39_5
	.word	.LBB39_9
	.word	.LBB39_9
	.word	.LBB39_5
	.word	.LBB39_1
                                        # -- End function
	.text
	.globl	c_stackw                        # -- Begin function c_stackw
	.p2align	2
	.type	c_stackw,@function
c_stackw:                               # @c_stackw
# %bb.0:
	lw	t0, 12(sp)
	lw	t1, 4(sp)
	lw	t2, 8(sp)
	lw	t3, 0(sp)
	add	a0, a1, a0
	add	a2, a2, a3
	add	a4, a4, a5
	add	a0, a0, a2
	add	a4, a4, a6
	slli	a1, a7, 1
	li	a2, 100
	slli	a7, a7, 3
	mul	a2, t3, a2
	add	a0, a0, a4
	add	a1, a7, a1
	add	a0, a0, a1
	srli	a1, t2, 4
	add	a0, a0, t1
	add	a0, a0, a1
	add	a0, a0, t0
	add	a0, a0, a2
	ret
.Lfunc_end40:
	.size	c_stackw, .Lfunc_end40-c_stackw
                                        # -- End function
	.globl	c_calls_merlin                  # -- Begin function c_calls_merlin
	.p2align	2
	.type	c_calls_merlin,@function
c_calls_merlin:                         # @c_calls_merlin
# %bb.0:
	addi	sp, sp, -176
	sw	ra, 172(sp)                     # 4-byte Folded Spill
	sw	s0, 168(sp)                     # 4-byte Folded Spill
	sw	s1, 164(sp)                     # 4-byte Folded Spill
	sw	s2, 160(sp)                     # 4-byte Folded Spill
	sw	s3, 156(sp)                     # 4-byte Folded Spill
	sw	s4, 152(sp)                     # 4-byte Folded Spill
	sw	s5, 148(sp)                     # 4-byte Folded Spill
	sw	s6, 144(sp)                     # 4-byte Folded Spill
	sw	s7, 140(sp)                     # 4-byte Folded Spill
	sw	s8, 136(sp)                     # 4-byte Folded Spill
	lui	a0, 32
	addi	a0, a0, 1
	li	a1, 3
	call	m_sum6
	lui	s3, %hi(.L_MergedGlobals)
	addi	s3, s3, %lo(.L_MergedGlobals)
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	s0, 6
	sw	a1, 4(s3)
	beq	a0, s0, .LBB41_3
# %bb.1:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_3
# %bb.2:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_3:
	addi	a0, sp, 60
	li	a1, 4
	call	m_make12
	lw	a0, 4(s3)
	lw	a1, 68(sp)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	beq	a1, s0, .LBB41_6
# %bb.4:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_6
# %bb.5:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_6:
	li	a0, 5
	li	a1, 7
	call	m_swap8
	mv	s0, a0
	mv	s1, a1
	lw	a0, 4(s3)
	xori	a1, s0, 7
	xori	a2, s1, 5
	or	a1, a1, a2
	addi	a0, a0, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB41_9
# %bb.7:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_9
# %bb.8:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_9:
	lui	a0, 16432
	addi	a0, a0, 513
	li	a1, 5
	call	m_rev5
	lw	a2, 4(s3)
	zext.b	a0, a0
	zext.b	a1, a1
	xori	a0, a0, 5
	xori	a1, a1, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB41_12
# %bb.10:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_12
# %bb.11:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_12:
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	s2, 3
	call	m_make3
	lw	a1, 4(s3)
	slli	a0, a0, 8
	srli	a2, a0, 24
	addi	a0, a1, 1
	sw	a0, 4(s3)
	beq	a2, s2, .LBB41_15
# %bb.13:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_15
# %bb.14:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_15:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 60(sp)
	sw	a1, 64(sp)
	sw	a2, 68(sp)
	lui	a3, 32
	li	a0, 1
	li	a4, 3
	addi	a3, a3, 1
	addi	a5, sp, 60
	li	a6, 2
	li	a7, 3
	mv	a1, s0
	mv	a2, s1
	call	m_many
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 107
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_18
# %bb.16:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_18
# %bb.17:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_18:
	addi	a0, sp, 60
	li	a1, 3
	call	m_big
	addi	a0, sp, 100
	addi	a1, sp, 60
	li	a2, 36
	call	memcpy
	lw	s4, 96(sp)
	li	s2, 12
	bne	s4, s2, .LBB41_20
# %bb.19:
	addi	a0, sp, 60
	addi	a1, sp, 100
	li	a2, 36
	call	memcpy
	sw	s2, 96(sp)
	addi	a0, sp, 60
	call	m_sumbig
	addi	a0, a0, -75
	seqz	a1, a0
	j	.LBB41_21
.LBB41_20:
	li	a1, 0
.LBB41_21:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB41_24
# %bb.22:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_24
# %bb.23:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_24:
	li	a0, 97
	call	m_inc1
	lw	a1, 4(s3)
	zext.b	a2, a0
	addi	a0, a1, 1
	li	a1, 98
	sw	a0, 4(s3)
	beq	a2, a1, .LBB41_27
# %bb.25:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_27
# %bb.26:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_27:
	li	a0, 120
	li	a1, 21
	call	m_mixed
	lw	a2, 4(s3)
	zext.b	a0, a0
	xori	a0, a0, 121
	xori	a1, a1, 42
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB41_30
# %bb.28:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_30
# %bb.29:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_30:
	li	a0, 41
	li	a1, 0
	call	m_union
	lw	a0, 4(s3)
	addi	a0, a0, 1
	li	a2, 42
	sw	a0, 4(s3)
	beq	a1, a2, .LBB41_33
# %bb.31:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_33
# %bb.32:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_33:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 60(sp)
	sw	a1, 64(sp)
	sw	a2, 68(sp)
	addi	a0, sp, 60
	call	m_modify12
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 1020
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_36
# %bb.34:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_36
# %bb.35:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_36:
	lui	s5, 16
	addi	a2, s5, -3
	li	a0, -3
	li	a1, -3
	li	a3, 1
	li	s2, -3
	call	m_widen
	lw	a1, 4(s3)
	addi	a1, a1, 1
	addi	s5, s5, -8
	sw	a1, 4(s3)
	beq	a0, s5, .LBB41_39
# %bb.37:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_39
# %bb.38:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_39:
	lui	a0, 32
	addi	a0, a0, -3
	call	m_trunc_sc
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beq	a0, s2, .LBB41_42
# %bb.40:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_42
# %bb.41:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_42:
	lui	a0, 24
	call	m_trunc_s
	lw	a1, 4(s3)
	addi	a1, a1, 1
	lui	a2, 1048568
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_45
# %bb.43:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_45
# %bb.44:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_45:
	lui	a0, 32
	addi	a0, a0, -3
	call	m_trunc_us
	lw	a1, 4(s3)
	lui	a2, 16
	addi	a1, a1, 1
	addi	a2, a2, -3
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_48
# %bb.46:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_48
# %bb.47:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_48:
	li	a0, 256
	call	m_tobool
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	bnez	a0, .LBB41_51
# %bb.49:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_51
# %bb.50:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_51:
	li	a0, 0
	call	m_vsum
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beqz	a0, .LBB41_54
# %bb.52:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_54
# %bb.53:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_54:
	li	s2, 10
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
	sw	s2, 8(sp)
	call	m_vsum
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 55
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_57
# %bb.55:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_57
# %bb.56:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_57:
	li	s5, 20
	li	s6, 30
	sw	s2, 48(sp)
	sw	s5, 52(sp)
	sw	s6, 56(sp)
	addi	a0, sp, 60
	addi	a1, sp, 100
	li	a2, 36
	addi	s7, sp, 60
	call	memcpy
	sw	s4, 96(sp)
	lui	a0, %hi(.L.str)
	addi	a0, a0, %lo(.L.str)
	lui	s2, 32
	li	a1, 7
	addi	s2, s2, 1
	li	a2, -3
	li	a4, 3
	addi	a7, sp, 48
	mv	a3, s2
	mv	a5, s0
	mv	a6, s1
	sw	s7, 0(sp)
	li	s7, 7
	call	m_vmix
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	s8, 180
	sw	a1, 4(s3)
	beq	a0, s8, .LBB41_60
# %bb.58:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_60
# %bb.59:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_60:
	addi	a0, sp, 60
	addi	a1, sp, 100
	li	a2, 36
	call	memcpy
	li	a0, 10
	sw	s4, 96(sp)
	sw	a0, 48(sp)
	sw	s5, 52(sp)
	sw	s6, 56(sp)
	lui	a0, %hi(.L.str.1)
	addi	a0, a0, %lo(.L.str.1)
	addi	a1, sp, 60
	addi	a2, sp, 48
	li	a6, 3
	li	a7, -3
	mv	a3, s0
	mv	a4, s1
	mv	a5, s2
	sw	s7, 0(sp)
	call	m_vmix
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beq	a0, s8, .LBB41_63
# %bb.61:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_63
# %bb.62:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_63:
	li	t0, 100
	sw	s1, 0(sp)
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	mv	a7, s0
	sw	t0, 4(sp)
	li	s2, 3
	call	m_vlate
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 203
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_66
# %bb.64:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_66
# %bb.65:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_66:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 60(sp)
	sw	a1, 64(sp)
	sw	a2, 68(sp)
	lui	a0, 32
	li	t0, -5
	addi	a0, a0, 1
	li	t1, 97
	sw	a0, 16(sp)
	sw	s2, 20(sp)
	addi	a0, sp, 60
	sw	s1, 0(sp)
	sw	a0, 4(sp)
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	mv	a7, s0
	sw	t1, 8(sp)
	sw	t0, 12(sp)
	call	m_stack
	lw	a1, 4(s3)
	addi	a2, a1, 1
	li	a3, 261
	sw	a2, 4(s3)
	beq	a0, a3, .LBB41_69
# %bb.67:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_69
# %bb.68:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a2, %lo(.L_MergedGlobals)(a0)
.LBB41_69:
	addi	a1, a1, 2
	sw	a1, 4(s3)
	li	t0, 9
	li	t1, 8
	addi	a0, sp, 60
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
	lw	a0, 60(sp)
	lw	a1, 64(sp)
	lw	a2, 68(sp)
	lw	a3, 4(s3)
	xori	a0, a0, 6
	xori	a1, a1, 15
	or	a0, a0, a1
	xori	a1, a2, 65
	or	a1, a0, a1
	addi	a0, a3, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB41_72
# %bb.70:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_72
# %bb.71:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_72:
	lui	a4, %hi(.L.str.2)
	addi	a4, a4, %lo(.L.str.2)
	li	a0, -5
	li	a1, -1
	lui	a2, 65536
	lui	a3, 524288
	call	m_mixw
	lw	a1, 4(s3)
	lui	a2, 163840
	addi	a1, a1, 1
	addi	a2, a2, 1
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_75
# %bb.73:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_75
# %bb.74:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_75:
	li	a0, -1
	call	m_umax
	beqz	a0, .LBB41_77
# %bb.76:
	lui	a0, 524288
	addi	a0, a0, -1
	call	m_umax
	seqz	a1, a0
	j	.LBB41_78
.LBB41_77:
	li	a1, 0
.LBB41_78:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB41_81
# %bb.79:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_81
# %bb.80:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_81:
	li	a0, -5
	li	s0, -5
	call	m_sext
	bne	a0, s0, .LBB41_83
# %bb.82:
	lui	s0, 524288
	addi	s0, s0, -1
	mv	a0, s0
	call	m_sext
	xor	a0, a0, s0
	seqz	a1, a0
	j	.LBB41_84
.LBB41_83:
	li	a1, 0
.LBB41_84:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB41_87
# %bb.85:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_87
# %bb.86:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_87:
	li	a0, -1
	li	s0, -1
	call	m_zext
	beq	a0, s0, .LBB41_89
# %bb.88:
	li	a1, 0
	j	.LBB41_90
.LBB41_89:
	lui	a0, 524288
	call	m_zext
	snez	a1, a0
.LBB41_90:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB41_93
# %bb.91:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_93
# %bb.92:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_93:
	lui	s1, 524288
	addi	s0, s1, 5
	mv	a0, s0
	call	m_narrow
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beq	a0, s0, .LBB41_96
# %bb.94:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_96
# %bb.95:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
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
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB41_102
# %bb.100:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_102
# %bb.101:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_102:
	lui	a0, 65536
	lui	s0, 65536
	call	m_incl1
	lw	a1, 4(s3)
	addi	a1, a1, 1
	addi	a2, s0, 1
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_105
# %bb.103:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_105
# %bb.104:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_105:
	lui	a0, 65536
	li	a1, -7
	call	m_swapl2
	lw	a2, 4(s3)
	xor	a1, a1, s0
	xori	a0, a0, -7
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB41_108
# %bb.106:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_108
# %bb.107:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_108:
	lui	a0, 65536
	li	a1, -3
	lui	s0, 65536
	call	m_li
	lw	a2, 4(s3)
	addi	s0, s0, -3
	xor	a0, a0, s0
	xori	a1, a1, 3
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB41_111
# %bb.109:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_111
# %bb.110:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_111:
	li	a0, 97
	lui	a1, 65536
	lui	s0, 65536
	call	m_cl
	lw	a2, 4(s3)
	zext.b	a3, a0
	lui	a0, 983040
	xori	a3, a3, 98
	xor	a1, a1, a0
	or	a3, a3, a1
	addi	a1, a2, 1
	sw	a1, 4(s3)
	beqz	a3, .LBB41_114
# %bb.112:
	lw	a2, 0(s3)
	bnez	a2, .LBB41_114
# %bb.113:
	lui	a2, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a2)
.LBB41_114:
	li	a1, 2
	sw	s0, 48(sp)
	sw	a1, 52(sp)
	sw	a0, 56(sp)
	sw	s0, 36(sp)
	sw	a1, 40(sp)
	sw	a0, 44(sp)
	addi	a0, sp, 60
	addi	a1, sp, 48
	addi	a2, sp, 36
	addi	s4, sp, 60
	call	m_addl3
	lw	s0, 60(sp)
	lw	s1, 64(sp)
	lw	s2, 68(sp)
	lui	a0, 131072
	lw	a1, 4(s3)
	xor	a0, s0, a0
	xori	a2, s1, 4
	lui	a3, 917504
	or	a0, a0, a2
	xor	a2, s2, a3
	or	a2, a0, a2
	addi	a0, a1, 1
	sw	a0, 4(s3)
	beqz	a2, .LBB41_117
# %bb.115:
	lw	a1, 0(s3)
	bnez	a1, .LBB41_117
# %bb.116:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB41_117:
	sw	s0, 60(sp)
	sw	s1, 64(sp)
	sw	s2, 68(sp)
	li	a0, 5
	li	a1, 6
	sw	a0, 0(sp)
	sw	a1, 4(sp)
	sw	s4, 8(sp)
	lui	a0, %hi(.L.str.3)
	addi	a0, a0, %lo(.L.str.3)
	lui	a5, %hi(.L.str.4)
	addi	a5, a5, %lo(.L.str.4)
	li	a1, -1
	li	a2, -1
	lui	a3, 65536
	lui	a4, 524288
	li	a6, 3
	li	a7, 4
	li	s5, -1
	lui	s6, 65536
	call	m_vmixw
	lw	a1, 4(s3)
	lui	a2, 163840
	addi	a1, a1, 1
	addi	s4, a2, 214
	sw	a1, 4(s3)
	beq	a0, s4, .LBB41_120
# %bb.118:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_120
# %bb.119:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_120:
	sw	s0, 60(sp)
	sw	s1, 64(sp)
	sw	s2, 68(sp)
	sw	s6, 0(sp)
	sw	s5, 4(sp)
	sw	s5, 8(sp)
	lui	a0, %hi(.L.str.5)
	addi	a0, a0, %lo(.L.str.5)
	lui	a6, %hi(.L.str.4)
	addi	a6, a6, %lo(.L.str.4)
	addi	a1, sp, 60
	li	a2, 5
	li	a3, 6
	li	a4, 3
	li	a5, 4
	lui	a7, 524288
	li	s0, 3
	li	s1, 4
	call	m_vmixw
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beq	a0, s4, .LBB41_123
# %bb.121:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_123
# %bb.122:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_123:
	lui	t0, 65536
	li	t1, -1
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	li	a7, -2
	sw	s0, 0(sp)
	sw	s1, 4(sp)
	sw	t1, 8(sp)
	sw	t0, 12(sp)
	call	m_stackw
	lw	a1, 4(s3)
	lui	a2, 131072
	addi	a1, a1, 1
	addi	a2, a2, 311
	sw	a1, 4(s3)
	beq	a0, a2, .LBB41_126
# %bb.124:
	lw	a0, 0(s3)
	bnez	a0, .LBB41_126
# %bb.125:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB41_126:
	lui	a0, %hi(.L_MergedGlobals)
	lw	a0, %lo(.L_MergedGlobals)(a0)
	lw	ra, 172(sp)                     # 4-byte Folded Reload
	lw	s0, 168(sp)                     # 4-byte Folded Reload
	lw	s1, 164(sp)                     # 4-byte Folded Reload
	lw	s2, 160(sp)                     # 4-byte Folded Reload
	lw	s3, 156(sp)                     # 4-byte Folded Reload
	lw	s4, 152(sp)                     # 4-byte Folded Reload
	lw	s5, 148(sp)                     # 4-byte Folded Reload
	lw	s6, 144(sp)                     # 4-byte Folded Reload
	lw	s7, 140(sp)                     # 4-byte Folded Reload
	lw	s8, 136(sp)                     # 4-byte Folded Reload
	addi	sp, sp, 176
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
