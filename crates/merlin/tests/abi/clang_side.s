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
	.globl	c_qadd                          # -- Begin function c_qadd
	.p2align	2
	.type	c_qadd,@function
c_qadd:                                 # @c_qadd
# %bb.0:
	srai	a5, a2, 31
	add	a2, a0, a2
	sltu	a0, a2, a0
	add	a1, a1, a5
	add	a1, a1, a0
	add	a0, a2, a3
	sltu	a2, a0, a2
	add	a1, a1, a4
	add	a1, a1, a2
	ret
.Lfunc_end41:
	.size	c_qadd, .Lfunc_end41-c_qadd
                                        # -- End function
	.globl	c_qhigh                         # -- Begin function c_qhigh
	.p2align	2
	.type	c_qhigh,@function
c_qhigh:                                # @c_qhigh
# %bb.0:
	mv	a0, a1
	li	a1, 0
	ret
.Lfunc_end42:
	.size	c_qhigh, .Lfunc_end42-c_qhigh
                                        # -- End function
	.globl	c_qlate                         # -- Begin function c_qlate
	.p2align	2
	.type	c_qlate,@function
c_qlate:                                # @c_qlate
# %bb.0:
	lw	t2, 16(sp)
	srai	t3, t2, 31
	lw	t0, 28(sp)
	lw	t1, 24(sp)
	lw	t4, 12(sp)
	lw	t5, 8(sp)
	add	a0, a1, a0
	add	a2, a2, a3
	add	a4, a4, a5
	add	a0, a0, a2
	add	a4, a4, a6
	lw	a1, 0(sp)
	add	a0, a0, a4
	srai	a2, a0, 31
	li	a3, 3
	slli	a4, a7, 1
	mulhu	a3, t5, a3
	srli	a5, a7, 31
	add	a0, a4, a0
	slli	a1, a1, 1
	sltu	a4, a0, a4
	or	a1, a1, a5
	add	a2, a2, a4
	add	a1, a1, a2
	slli	a2, t4, 1
	add	a2, a2, t4
	slli	a4, t5, 1
	add	a2, a3, a2
	add	a4, a4, t5
	add	a4, a0, a4
	li	a3, 5
	sltu	a0, a4, a0
	mulhu	a3, t1, a3
	add	a0, a1, a0
	add	t2, a4, t2
	sltu	a1, t2, a4
	add	a0, a0, t3
	add	a1, a0, a1
	slli	a0, t1, 2
	slli	a4, t0, 2
	add	a0, a0, t1
	add	a4, a4, t0
	add	a0, t2, a0
	add	a3, a3, a4
	sltu	a4, a0, t2
	add	a1, a1, a2
	add	a3, a3, a4
	add	a1, a1, a3
	ret
.Lfunc_end43:
	.size	c_qlate, .Lfunc_end43-c_qlate
                                        # -- End function
	.globl	c_qtwice                        # -- Begin function c_qtwice
	.p2align	2
	.type	c_qtwice,@function
c_qtwice:                               # @c_qtwice
# %bb.0:
	srli	a2, a0, 31
	slli	a1, a1, 1
	or	a1, a1, a2
	slli	a0, a0, 1
	ret
.Lfunc_end44:
	.size	c_qtwice, .Lfunc_end44-c_qtwice
                                        # -- End function
	.globl	c_qsum                          # -- Begin function c_qsum
	.p2align	2
	.type	c_qsum,@function
c_qsum:                                 # @c_qsum
# %bb.0:
	lw	a3, 0(a0)
	lw	a4, 8(a0)
	lw	a5, 12(a0)
	srai	a0, a3, 31
	add	a3, a1, a3
	sltu	a1, a3, a1
	add	a0, a2, a0
	add	a1, a0, a1
	add	a0, a3, a4
	sltu	a2, a0, a3
	add	a1, a1, a5
	add	a1, a1, a2
	ret
.Lfunc_end45:
	.size	c_qsum, .Lfunc_end45-c_qsum
                                        # -- End function
	.globl	c_vq                            # -- Begin function c_vq
	.p2align	2
	.type	c_vq,@function
c_vq:                                   # @c_vq
# %bb.0:
	addi	sp, sp, -48
	mv	t1, a1
	mv	t0, a0
	li	a0, 0
	li	a1, 0
	sw	a5, 36(sp)
	sw	a6, 40(sp)
	sw	a7, 44(sp)
	addi	a5, sp, 20
	sw	t1, 20(sp)
	sw	a2, 24(sp)
	sw	a3, 28(sp)
	sw	a4, 32(sp)
	sw	a5, 12(sp)
	li	a2, 105
	li	a3, 113
	j	.LBB46_3
.LBB46_1:                               #   in Loop: Header=BB46_3 Depth=1
	addi	a5, a5, 7
	andi	a6, a5, -8
	addi	a5, a6, 8
	sw	a5, 12(sp)
	lw	a4, 0(a6)
	lw	a6, 4(a6)
.LBB46_2:                               #   in Loop: Header=BB46_3 Depth=1
	add	a0, a4, a0
	sltu	a4, a0, a4
	add	a1, a6, a1
	add	a1, a1, a4
	addi	t0, t0, 1
.LBB46_3:                               # =>This Inner Loop Header: Depth=1
	lbu	a4, 0(t0)
	beq	a4, a2, .LBB46_6
# %bb.4:                                #   in Loop: Header=BB46_3 Depth=1
	beq	a4, a3, .LBB46_1
# %bb.5:                                #   in Loop: Header=BB46_3 Depth=1
	bnez	a4, .LBB46_1
	j	.LBB46_7
.LBB46_6:                               #   in Loop: Header=BB46_3 Depth=1
	addi	a6, a5, 4
	sw	a6, 12(sp)
	lw	a4, 0(a5)
	mv	a5, a6
	srai	a6, a4, 31
	j	.LBB46_2
.LBB46_7:
	addi	sp, sp, 48
	ret
.Lfunc_end46:
	.size	c_vq, .Lfunc_end46-c_vq
                                        # -- End function
	.globl	c_vqlate                        # -- Begin function c_vqlate
	.p2align	2
	.type	c_vqlate,@function
c_vqlate:                               # @c_vqlate
# %bb.0:
	addi	sp, sp, -16
	sw	s0, 4(sp)                       # 4-byte Folded Spill
	sw	a7, 12(sp)
	addi	a7, sp, 12
	addi	t0, sp, 19
	sw	a7, 0(sp)
	andi	a7, t0, -8
	lw	t0, 0(a7)
	lw	a7, 4(a7)
	srai	t1, a0, 31
	srai	t2, a1, 31
	srai	t3, a2, 31
	srai	t4, a3, 31
	srai	t5, a4, 31
	srai	t6, a5, 31
	srai	s0, a6, 31
	add	a0, a1, a0
	sltu	a1, a0, a1
	add	t1, t2, t1
	add	a1, t1, a1
	add	a2, a0, a2
	sltu	a0, a2, a0
	add	a1, a1, t3
	add	a0, a0, t4
	add	a3, a2, a3
	add	a0, a1, a0
	sltu	a1, a3, a2
	add	a1, a1, t5
	add	a4, a3, a4
	add	a0, a0, a1
	sltu	a1, a4, a3
	add	a1, a1, t6
	add	a5, a4, a5
	add	a0, a0, a1
	sltu	a1, a5, a4
	add	a1, a1, s0
	add	a6, a5, a6
	add	a1, a0, a1
	sltu	a0, a6, a5
	add	a7, a0, a7
	add	a0, a6, t0
	add	a1, a1, a7
	sltu	a2, a0, a6
	add	a1, a1, a2
	lw	s0, 4(sp)                       # 4-byte Folded Reload
	addi	sp, sp, 16
	ret
.Lfunc_end47:
	.size	c_vqlate, .Lfunc_end47-c_vqlate
                                        # -- End function
	.globl	c_q67                           # -- Begin function c_q67
	.p2align	2
	.type	c_q67,@function
c_q67:                                  # @c_q67
# %bb.0:
	add	a0, a1, a0
	add	a2, a2, a3
	add	a0, a0, a2
	add	a4, a4, a5
	li	a1, 3
	add	a0, a0, a4
	mulhu	a1, a6, a1
	srai	a2, a0, 31
	slli	a3, a6, 1
	slli	a4, a7, 1
	add	a3, a3, a6
	add	a4, a4, a7
	add	a0, a3, a0
	sltu	a3, a0, a3
	add	a2, a4, a2
	add	a2, a2, a3
	add	a1, a1, a2
	ret
.Lfunc_end48:
	.size	c_q67, .Lfunc_end48-c_q67
                                        # -- End function
	.globl	c_oddstack                      # -- Begin function c_oddstack
	.p2align	2
	.type	c_oddstack,@function
c_oddstack:                             # @c_oddstack
# %bb.0:
	lw	t0, 20(sp)
	lw	t1, 8(sp)
	lw	t2, 12(sp)
	lw	t3, 4(sp)
	lw	t4, 0(sp)
	lw	t5, 28(sp)
	add	a0, a1, a0
	add	a2, a2, a3
	add	a0, a0, a2
	add	a4, a4, a5
	add	a4, a4, a6
	li	a1, 100
	mul	a1, t3, a1
	li	a2, 1000
	add	a0, a0, a4
	mul	a2, t2, a2
	lh	a3, 32(sp)
	add	a0, a0, a7
	slli	a4, t4, 1
	slli	t4, t4, 3
	add	a4, t4, a4
	add	a0, a0, t1
	lw	a5, 24(sp)
	add	a0, a0, a4
	lw	a4, 16(sp)
	add	a0, a0, a1
	srai	a1, a3, 31
	add	a0, a0, a2
	srai	a2, a0, 31
	slli	a6, t5, 16
	srai	a7, a5, 31
	srai	t1, a6, 31
	srai	a6, a6, 16
	srai	t2, t5, 16
	srai	t3, t5, 31
	add	a5, a4, a5
	sltu	a4, a5, a4
	add	a7, t0, a7
	add	a4, a7, a4
	add	a0, a5, a0
	add	a2, a4, a2
	sltu	a4, a0, a5
	add	a2, a2, a4
	add	a6, a0, a6
	sltu	a0, a6, a0
	add	a2, a2, t1
	add	a0, a0, t3
	add	t2, a6, t2
	add	a2, a2, a0
	sltu	a0, t2, a6
	add	a1, a0, a1
	add	a0, t2, a3
	add	a1, a2, a1
	sltu	a2, a0, t2
	add	a1, a1, a2
	ret
.Lfunc_end49:
	.size	c_oddstack, .Lfunc_end49-c_oddstack
                                        # -- End function
	.globl	c_vq3                           # -- Begin function c_vq3
	.p2align	2
	.type	c_vq3,@function
c_vq3:                                  # @c_vq3
# %bb.0:
	addi	sp, sp, -32
	sw	a7, 28(sp)
	sw	a3, 12(sp)
	sw	a4, 16(sp)
	sw	a5, 20(sp)
	sw	a6, 24(sp)
	add	a0, a1, a0
	addi	a5, sp, 12
	srai	a1, a0, 31
	sw	a5, 4(sp)
	li	a3, 105
	li	a4, 113
	j	.LBB50_3
.LBB50_1:                               #   in Loop: Header=BB50_3 Depth=1
	addi	a5, a5, 7
	andi	a7, a5, -8
	addi	a5, a7, 8
	sw	a5, 4(sp)
	lw	a6, 0(a7)
	lw	a7, 4(a7)
.LBB50_2:                               #   in Loop: Header=BB50_3 Depth=1
	add	a0, a6, a0
	sltu	a6, a0, a6
	add	a1, a7, a1
	add	a1, a1, a6
	addi	a2, a2, 1
.LBB50_3:                               # =>This Inner Loop Header: Depth=1
	lbu	a6, 0(a2)
	beq	a6, a3, .LBB50_6
# %bb.4:                                #   in Loop: Header=BB50_3 Depth=1
	beq	a6, a4, .LBB50_1
# %bb.5:                                #   in Loop: Header=BB50_3 Depth=1
	bnez	a6, .LBB50_1
	j	.LBB50_7
.LBB50_6:                               #   in Loop: Header=BB50_3 Depth=1
	addi	a7, a5, 4
	sw	a7, 4(sp)
	lw	a6, 0(a5)
	mv	a5, a7
	srai	a7, a6, 31
	j	.LBB50_2
.LBB50_7:
	addi	sp, sp, 32
	ret
.Lfunc_end50:
	.size	c_vq3, .Lfunc_end50-c_vq3
                                        # -- End function
	.globl	c_rev9                          # -- Begin function c_rev9
	.p2align	2
	.type	c_rev9,@function
c_rev9:                                 # @c_rev9
# %bb.0:
	li	a2, 8
	li	a3, -1
.LBB51_1:                               # =>This Inner Loop Header: Depth=1
	add	a4, a1, a2
	lbu	a4, 0(a4)
	addi	a2, a2, -1
	sb	a4, 0(a0)
	addi	a0, a0, 1
	bne	a2, a3, .LBB51_1
# %bb.2:
	ret
.Lfunc_end51:
	.size	c_rev9, .Lfunc_end51-c_rev9
                                        # -- End function
	.globl	c_neg10                         # -- Begin function c_neg10
	.p2align	2
	.type	c_neg10,@function
c_neg10:                                # @c_neg10
# %bb.0:
	addi	a2, a1, 10
	mv	a3, a1
.LBB52_1:                               # =>This Inner Loop Header: Depth=1
	lh	a4, 0(a3)
	neg	a4, a4
	sh	a4, 0(a3)
	addi	a3, a3, 2
	bne	a3, a2, .LBB52_1
# %bb.2:
	lh	a2, 8(a1)
	lh	a3, 0(a1)
	lh	a4, 2(a1)
	lh	a5, 4(a1)
	lh	a1, 6(a1)
	sh	a2, 8(a0)
	sh	a3, 0(a0)
	sh	a4, 2(a0)
	sh	a5, 4(a0)
	sh	a1, 6(a0)
	ret
.Lfunc_end52:
	.size	c_neg10, .Lfunc_end52-c_neg10
                                        # -- End function
	.globl	c_late910                       # -- Begin function c_late910
	.p2align	2
	.type	c_late910,@function
c_late910:                              # @c_late910
# %bb.0:
	lw	t0, 0(sp)
	add	a0, a1, a0
	add	a2, a2, a3
	lh	a1, 8(t0)
	add	a0, a0, a2
	lbu	a2, 0(a7)
	lbu	a3, 8(a7)
	add	a4, a4, a5
	li	a5, 100
	add	a4, a4, a6
	mul	a1, a1, a5
	lh	a5, 0(t0)
	add	a0, a0, a4
	slli	a4, a3, 1
	slli	a3, a3, 3
	add	a0, a0, a2
	add	a3, a3, a4
	add	a0, a0, a3
	add	a0, a0, a5
	add	a0, a0, a1
	ret
.Lfunc_end53:
	.size	c_late910, .Lfunc_end53-c_late910
                                        # -- End function
	.globl	c_v910                          # -- Begin function c_v910
	.p2align	2
	.type	c_v910,@function
c_v910:                                 # @c_v910
# %bb.0:
	addi	sp, sp, -48
	mv	t1, a1
	mv	t0, a0
	li	a0, 0
	li	a1, 0
	sw	a5, 36(sp)
	sw	a6, 40(sp)
	sw	a7, 44(sp)
	sw	t1, 20(sp)
	sw	a2, 24(sp)
	sw	a3, 28(sp)
	sw	a4, 32(sp)
	addi	a2, sp, 20
	sw	a2, 12(sp)
	li	a3, 56
	li	a4, 48
	li	a5, 100
	li	a6, 57
	li	a7, 105
	j	.LBB54_3
.LBB54_1:                               #   in Loop: Header=BB54_3 Depth=1
	li	t1, 0
	addi	t2, a2, 4
	sw	t2, 12(sp)
	lw	t3, 0(a2)
	mv	a2, t2
	lbu	t2, 8(t3)
	lbu	t3, 0(t3)
	slli	t4, t2, 1
	slli	t2, t2, 3
	add	t2, t2, t4
	add	t2, t2, t3
.LBB54_2:                               #   in Loop: Header=BB54_3 Depth=1
	add	a0, t2, a0
	sltu	t2, a0, t2
	add	a1, t1, a1
	add	a1, a1, t2
	addi	t0, t0, 1
.LBB54_3:                               # =>This Inner Loop Header: Depth=1
	lbu	t1, 0(t0)
	bge	a3, t1, .LBB54_7
# %bb.4:                                #   in Loop: Header=BB54_3 Depth=1
	beq	t1, a6, .LBB54_1
# %bb.5:                                #   in Loop: Header=BB54_3 Depth=1
	bne	t1, a7, .LBB54_11
# %bb.6:                                #   in Loop: Header=BB54_3 Depth=1
	addi	t1, a2, 4
	sw	t1, 12(sp)
	lw	t2, 0(a2)
	mv	a2, t1
	j	.LBB54_9
.LBB54_7:                               #   in Loop: Header=BB54_3 Depth=1
	bne	t1, a4, .LBB54_10
# %bb.8:                                #   in Loop: Header=BB54_3 Depth=1
	addi	t1, a2, 4
	sw	t1, 12(sp)
	lw	t2, 0(a2)
	mv	a2, t1
	lh	t1, 8(t2)
	mul	t1, t1, a5
	lh	t2, 0(t2)
	add	t2, t1, t2
.LBB54_9:                               #   in Loop: Header=BB54_3 Depth=1
	srai	t1, t2, 31
	j	.LBB54_2
.LBB54_10:                              #   in Loop: Header=BB54_3 Depth=1
	beqz	t1, .LBB54_12
.LBB54_11:                              #   in Loop: Header=BB54_3 Depth=1
	addi	t1, a2, 4
	sw	t1, 12(sp)
	lw	t2, 0(a2)
	mv	a2, t1
	lbu	t1, 0(t2)
	lw	t3, 8(t2)
	lw	t4, 12(t2)
	add	t2, t3, t1
	sltu	t1, t2, t3
	add	t1, t4, t1
	j	.LBB54_2
.LBB54_12:
	addi	sp, sp, 48
	ret
.Lfunc_end54:
	.size	c_v910, .Lfunc_end54-c_v910
                                        # -- End function
	.globl	c_cq                            # -- Begin function c_cq
	.p2align	2
	.type	c_cq,@function
c_cq:                                   # @c_cq
# %bb.0:
	lbu	a2, 0(a1)
	lw	a3, 8(a1)
	lw	a4, 12(a1)
	lw	a5, 4(a1)
	addi	a2, a2, 1
	sb	a2, 0(a1)
	lw	a2, 0(a1)
	neg	a4, a4
	snez	a6, a3
	sub	a4, a4, a6
	neg	a3, a3
	sw	a3, 8(a1)
	sw	a4, 12(a1)
	sw	a2, 0(a0)
	sw	a5, 4(a0)
	sw	a3, 8(a0)
	sw	a4, 12(a0)
	ret
.Lfunc_end55:
	.size	c_cq, .Lfunc_end55-c_cq
                                        # -- End function
	.globl	c_cqsum                         # -- Begin function c_cqsum
	.p2align	2
	.type	c_cqsum,@function
c_cqsum:                                # @c_cqsum
# %bb.0:
	lbu	a3, 0(a1)
	lw	a4, 8(a1)
	lw	a1, 12(a1)
	add	a0, a0, a3
	lbu	a3, 0(a2)
	lw	a5, 8(a2)
	srai	a6, a0, 31
	add	a1, a1, a6
	add	a0, a4, a0
	lw	a2, 12(a2)
	sltu	a4, a0, a4
	add	a1, a1, a4
	slli	a4, a3, 1
	slli	a3, a3, 3
	add	a5, a0, a5
	add	a3, a3, a4
	sltu	a4, a5, a0
	add	a1, a1, a2
	add	a0, a5, a3
	add	a1, a1, a4
	sltu	a2, a0, a5
	add	a1, a1, a2
	ret
.Lfunc_end56:
	.size	c_cqsum, .Lfunc_end56-c_cqsum
                                        # -- End function
	.globl	c_ucmp                          # -- Begin function c_ucmp
	.p2align	2
	.type	c_ucmp,@function
c_ucmp:                                 # @c_ucmp
# %bb.0:
	lui	a2, 524288
	xor	a2, a0, a2
	sltu	a3, a0, a1
	seqz	a2, a2
	addi	a1, a1, 1
	srli	a0, a0, 28
	seqz	a1, a1
	andi	a0, a0, 8
	slli	a1, a1, 2
	or	a0, a0, a3
	slli	a2, a2, 1
	or	a0, a0, a1
	or	a0, a0, a2
	ret
.Lfunc_end57:
	.size	c_ucmp, .Lfunc_end57-c_ucmp
                                        # -- End function
	.globl	c_uadd                          # -- Begin function c_uadd
	.p2align	2
	.type	c_uadd,@function
c_uadd:                                 # @c_uadd
# %bb.0:
	add	a0, a1, a0
	ret
.Lfunc_end58:
	.size	c_uadd, .Lfunc_end58-c_uadd
                                        # -- End function
	.globl	c_ushr                          # -- Begin function c_ushr
	.p2align	2
	.type	c_ushr,@function
c_ushr:                                 # @c_ushr
# %bb.0:
	srl	a0, a0, a1
	ret
.Lfunc_end59:
	.size	c_ushr, .Lfunc_end59-c_ushr
                                        # -- End function
	.globl	c_udiv                          # -- Begin function c_udiv
	.p2align	2
	.type	c_udiv,@function
c_udiv:                                 # @c_udiv
# %bb.0:
	divu	a0, a0, a1
	ret
.Lfunc_end60:
	.size	c_udiv, .Lfunc_end60-c_udiv
                                        # -- End function
	.globl	c_shlq                          # -- Begin function c_shlq
	.p2align	2
	.type	c_shlq,@function
c_shlq:                                 # @c_shlq
# %bb.0:
	addi	a4, a2, -32
	sll	a3, a0, a2
	bltz	a4, .LBB61_2
# %bb.1:
	mv	a1, a3
	j	.LBB61_3
.LBB61_2:
	not	a5, a2
	srli	a0, a0, 1
	sll	a1, a1, a2
	srl	a0, a0, a5
	or	a1, a1, a0
.LBB61_3:
	srai	a0, a4, 31
	and	a0, a0, a3
	ret
.Lfunc_end61:
	.size	c_shlq, .Lfunc_end61-c_shlq
                                        # -- End function
	.globl	c_applyq                        # -- Begin function c_applyq
	.p2align	2
	.type	c_applyq,@function
c_applyq:                               # @c_applyq
# %bb.0:
	addi	sp, sp, -16
	sw	ra, 12(sp)                      # 4-byte Folded Spill
	mv	a4, a0
	mv	a0, a1
	mv	a1, a2
	mv	a2, a3
	jalr	a4
	addi	a0, a0, 1
	seqz	a2, a0
	add	a1, a1, a2
	lw	ra, 12(sp)                      # 4-byte Folded Reload
	addi	sp, sp, 16
	ret
.Lfunc_end62:
	.size	c_applyq, .Lfunc_end62-c_applyq
                                        # -- End function
	.globl	c_calls_merlin                  # -- Begin function c_calls_merlin
	.p2align	2
	.type	c_calls_merlin,@function
c_calls_merlin:                         # @c_calls_merlin
# %bb.0:
	addi	sp, sp, -224
	sw	ra, 220(sp)                     # 4-byte Folded Spill
	sw	s0, 216(sp)                     # 4-byte Folded Spill
	sw	s1, 212(sp)                     # 4-byte Folded Spill
	sw	s2, 208(sp)                     # 4-byte Folded Spill
	sw	s3, 204(sp)                     # 4-byte Folded Spill
	sw	s4, 200(sp)                     # 4-byte Folded Spill
	sw	s5, 196(sp)                     # 4-byte Folded Spill
	sw	s6, 192(sp)                     # 4-byte Folded Spill
	sw	s7, 188(sp)                     # 4-byte Folded Spill
	sw	s8, 184(sp)                     # 4-byte Folded Spill
	sh	zero, 144(sp)
	sb	zero, 146(sp)
	sh	zero, 140(sp)
	sb	zero, 142(sp)
	sw	zero, 132(sp)
	sh	zero, 136(sp)
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
	beq	a0, s0, .LBB63_3
# %bb.1:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_3
# %bb.2:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_3:
	addi	a0, sp, 80
	li	a1, 4
	call	m_make12
	lw	a0, 4(s3)
	lw	a1, 88(sp)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	beq	a1, s0, .LBB63_6
# %bb.4:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_6
# %bb.5:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_6:
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
	beqz	a1, .LBB63_9
# %bb.7:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_9
# %bb.8:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_9:
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
	beqz	a1, .LBB63_12
# %bb.10:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_12
# %bb.11:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_12:
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
	beq	a2, s2, .LBB63_15
# %bb.13:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_15
# %bb.14:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_15:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 80(sp)
	sw	a1, 84(sp)
	sw	a2, 88(sp)
	lui	a3, 32
	li	a0, 1
	li	a4, 3
	addi	a3, a3, 1
	addi	a5, sp, 80
	li	a6, 2
	li	a7, 3
	mv	a1, s0
	mv	a2, s1
	call	m_many
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 107
	sw	a1, 4(s3)
	beq	a0, a2, .LBB63_18
# %bb.16:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_18
# %bb.17:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_18:
	addi	a0, sp, 80
	li	a1, 3
	call	m_big
	addi	a0, sp, 148
	addi	a1, sp, 80
	li	a2, 36
	call	memcpy
	lw	s4, 116(sp)
	li	s2, 12
	bne	s4, s2, .LBB63_20
# %bb.19:
	addi	a0, sp, 80
	addi	a1, sp, 148
	li	a2, 36
	call	memcpy
	sw	s2, 116(sp)
	addi	a0, sp, 80
	call	m_sumbig
	addi	a0, a0, -75
	seqz	a1, a0
	j	.LBB63_21
.LBB63_20:
	li	a1, 0
.LBB63_21:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_24
# %bb.22:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_24
# %bb.23:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_24:
	li	a0, 97
	call	m_inc1
	lw	a1, 4(s3)
	zext.b	a2, a0
	addi	a0, a1, 1
	li	a1, 98
	sw	a0, 4(s3)
	beq	a2, a1, .LBB63_27
# %bb.25:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_27
# %bb.26:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_27:
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
	beqz	a1, .LBB63_30
# %bb.28:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_30
# %bb.29:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_30:
	li	a0, 41
	li	a1, 0
	call	m_union
	lw	a0, 4(s3)
	addi	a0, a0, 1
	li	a2, 42
	sw	a0, 4(s3)
	beq	a1, a2, .LBB63_33
# %bb.31:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_33
# %bb.32:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_33:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 80(sp)
	sw	a1, 84(sp)
	sw	a2, 88(sp)
	addi	a0, sp, 80
	call	m_modify12
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 1020
	sw	a1, 4(s3)
	beq	a0, a2, .LBB63_36
# %bb.34:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_36
# %bb.35:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_36:
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
	beq	a0, s5, .LBB63_39
# %bb.37:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_39
# %bb.38:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_39:
	lui	a0, 32
	addi	a0, a0, -3
	call	m_trunc_sc
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beq	a0, s2, .LBB63_42
# %bb.40:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_42
# %bb.41:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_42:
	lui	a0, 24
	call	m_trunc_s
	lw	a1, 4(s3)
	addi	a1, a1, 1
	lui	a2, 1048568
	sw	a1, 4(s3)
	beq	a0, a2, .LBB63_45
# %bb.43:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_45
# %bb.44:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_45:
	lui	a0, 32
	addi	a0, a0, -3
	call	m_trunc_us
	lw	a1, 4(s3)
	lui	a2, 16
	addi	a1, a1, 1
	addi	a2, a2, -3
	sw	a1, 4(s3)
	beq	a0, a2, .LBB63_48
# %bb.46:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_48
# %bb.47:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_48:
	li	a0, 256
	call	m_tobool
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	bnez	a0, .LBB63_51
# %bb.49:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_51
# %bb.50:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_51:
	li	a0, 0
	call	m_vsum
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beqz	a0, .LBB63_54
# %bb.52:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_54
# %bb.53:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_54:
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
	beq	a0, a2, .LBB63_57
# %bb.55:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_57
# %bb.56:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_57:
	li	s5, 20
	li	s6, 30
	sw	s2, 48(sp)
	sw	s5, 52(sp)
	sw	s6, 56(sp)
	addi	a0, sp, 80
	addi	a1, sp, 148
	li	a2, 36
	addi	s7, sp, 80
	call	memcpy
	sw	s4, 116(sp)
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
	beq	a0, s8, .LBB63_60
# %bb.58:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_60
# %bb.59:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_60:
	addi	a0, sp, 80
	addi	a1, sp, 148
	li	a2, 36
	call	memcpy
	li	a0, 10
	sw	s4, 116(sp)
	sw	a0, 48(sp)
	sw	s5, 52(sp)
	sw	s6, 56(sp)
	lui	a0, %hi(.L.str.1)
	addi	a0, a0, %lo(.L.str.1)
	addi	a1, sp, 80
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
	beq	a0, s8, .LBB63_63
# %bb.61:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_63
# %bb.62:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_63:
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
	beq	a0, a2, .LBB63_66
# %bb.64:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_66
# %bb.65:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_66:
	li	a0, 10
	li	a1, 20
	li	a2, 30
	sw	a0, 80(sp)
	sw	a1, 84(sp)
	sw	a2, 88(sp)
	lui	a0, 32
	li	t0, -5
	addi	a0, a0, 1
	li	t1, 97
	sw	a0, 16(sp)
	sw	s2, 20(sp)
	addi	a0, sp, 80
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
	beq	a0, a3, .LBB63_69
# %bb.67:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_69
# %bb.68:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a2, %lo(.L_MergedGlobals)(a0)
.LBB63_69:
	addi	a1, a1, 2
	sw	a1, 4(s3)
	li	t0, 9
	li	t1, 8
	addi	a0, sp, 80
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
	lw	a0, 80(sp)
	lw	a1, 84(sp)
	lw	a2, 88(sp)
	lw	a3, 4(s3)
	xori	a0, a0, 6
	xori	a1, a1, 15
	or	a0, a0, a1
	xori	a1, a2, 65
	or	a1, a0, a1
	addi	a0, a3, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_72
# %bb.70:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_72
# %bb.71:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_72:
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
	beq	a0, a2, .LBB63_75
# %bb.73:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_75
# %bb.74:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_75:
	li	a0, -1
	call	m_umax
	beqz	a0, .LBB63_77
# %bb.76:
	lui	a0, 524288
	addi	a0, a0, -1
	call	m_umax
	seqz	a1, a0
	j	.LBB63_78
.LBB63_77:
	li	a1, 0
.LBB63_78:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_81
# %bb.79:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_81
# %bb.80:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_81:
	li	a0, -5
	li	s2, -5
	call	m_sext
	bne	a0, s2, .LBB63_83
# %bb.82:
	lui	s2, 524288
	addi	s2, s2, -1
	mv	a0, s2
	call	m_sext
	xor	a0, a0, s2
	seqz	a1, a0
	j	.LBB63_84
.LBB63_83:
	li	a1, 0
.LBB63_84:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_87
# %bb.85:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_87
# %bb.86:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_87:
	li	a0, -1
	li	s2, -1
	call	m_zext
	beq	a0, s2, .LBB63_89
# %bb.88:
	li	a1, 0
	j	.LBB63_90
.LBB63_89:
	lui	a0, 524288
	call	m_zext
	snez	a1, a0
.LBB63_90:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_93
# %bb.91:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_93
# %bb.92:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_93:
	lui	s4, 524288
	addi	s2, s4, 5
	mv	a0, s2
	call	m_narrow
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beq	a0, s2, .LBB63_96
# %bb.94:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_96
# %bb.95:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_96:
	lui	a0, 524288
	call	m_unarrow
	bne	a0, s4, .LBB63_98
# %bb.97:
	lui	a0, 524288
	call	m_unarrow
	snez	a1, a0
	j	.LBB63_99
.LBB63_98:
	li	a1, 0
.LBB63_99:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_102
# %bb.100:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_102
# %bb.101:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_102:
	lui	a0, 65536
	lui	s2, 65536
	call	m_incl1
	lw	a1, 4(s3)
	addi	a1, a1, 1
	addi	a2, s2, 1
	sw	a1, 4(s3)
	beq	a0, a2, .LBB63_105
# %bb.103:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_105
# %bb.104:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_105:
	lui	a0, 65536
	li	a1, -7
	call	m_swapl2
	lw	a2, 4(s3)
	xor	a1, a1, s2
	xori	a0, a0, -7
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_108
# %bb.106:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_108
# %bb.107:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_108:
	lui	a0, 65536
	li	a1, -3
	lui	s2, 65536
	call	m_li
	lw	a2, 4(s3)
	addi	s2, s2, -3
	xor	a0, a0, s2
	xori	a1, a1, 3
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_111
# %bb.109:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_111
# %bb.110:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_111:
	li	a0, 97
	lui	a1, 65536
	lui	s2, 65536
	call	m_cl
	lw	a2, 4(s3)
	zext.b	a3, a0
	lui	a0, 983040
	xori	a3, a3, 98
	xor	a1, a1, a0
	or	a3, a3, a1
	addi	a1, a2, 1
	sw	a1, 4(s3)
	beqz	a3, .LBB63_114
# %bb.112:
	lw	a2, 0(s3)
	bnez	a2, .LBB63_114
# %bb.113:
	lui	a2, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a2)
.LBB63_114:
	li	a1, 2
	sw	s2, 48(sp)
	sw	a1, 52(sp)
	sw	a0, 56(sp)
	sw	s2, 68(sp)
	sw	a1, 72(sp)
	sw	a0, 76(sp)
	addi	a0, sp, 80
	addi	a1, sp, 48
	addi	a2, sp, 68
	addi	s6, sp, 80
	call	m_addl3
	lw	s2, 80(sp)
	lw	s4, 84(sp)
	lw	s5, 88(sp)
	lui	a0, 131072
	lw	a1, 4(s3)
	xor	a0, s2, a0
	xori	a2, s4, 4
	lui	a3, 917504
	or	a0, a0, a2
	xor	a2, s5, a3
	or	a2, a0, a2
	addi	a0, a1, 1
	sw	a0, 4(s3)
	beqz	a2, .LBB63_117
# %bb.115:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_117
# %bb.116:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_117:
	sw	s2, 80(sp)
	sw	s4, 84(sp)
	sw	s5, 88(sp)
	li	a0, 5
	li	a1, 6
	sw	a0, 0(sp)
	sw	a1, 4(sp)
	sw	s6, 8(sp)
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
	li	s7, -1
	lui	s8, 65536
	call	m_vmixw
	lw	a1, 4(s3)
	lui	a2, 163840
	addi	a1, a1, 1
	addi	s6, a2, 214
	sw	a1, 4(s3)
	beq	a0, s6, .LBB63_120
# %bb.118:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_120
# %bb.119:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_120:
	sw	s2, 80(sp)
	sw	s4, 84(sp)
	sw	s5, 88(sp)
	sw	s8, 0(sp)
	sw	s7, 4(sp)
	sw	s7, 8(sp)
	lui	a0, %hi(.L.str.5)
	addi	a0, a0, %lo(.L.str.5)
	lui	a6, %hi(.L.str.4)
	addi	a6, a6, %lo(.L.str.4)
	addi	a1, sp, 80
	li	a2, 5
	li	a3, 6
	li	a4, 3
	li	a5, 4
	lui	a7, 524288
	li	s2, 3
	li	s4, 4
	call	m_vmixw
	lw	a1, 4(s3)
	addi	a1, a1, 1
	sw	a1, 4(s3)
	beq	a0, s6, .LBB63_123
# %bb.121:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_123
# %bb.122:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_123:
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
	sw	s2, 0(sp)
	sw	s4, 4(sp)
	sw	t1, 8(sp)
	sw	t0, 12(sp)
	call	m_stackw
	lw	a1, 4(s3)
	lui	a2, 131072
	addi	a1, a1, 1
	addi	a2, a2, 311
	sw	a1, 4(s3)
	beq	a0, a2, .LBB63_126
# %bb.124:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_126
# %bb.125:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_126:
	li	a1, 1
	li	a2, -1
	li	a4, 1
	li	a0, 0
	li	a3, 0
	call	m_qadd
	xori	a1, a1, 1
	not	a0, a0
	or	a0, a0, a1
	li	a1, 0
	bnez	a0, .LBB63_128
# %bb.127:
	li	a0, -1
	li	a1, -1
	li	a2, -1
	li	a3, -1
	li	a4, -1
	call	m_qadd
	not	a1, a1
	xori	a0, a0, -3
	or	a0, a0, a1
	seqz	a1, a0
.LBB63_128:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_131
# %bb.129:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_131
# %bb.130:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_131:
	lui	a0, 144470
	lui	a1, 703711
	addi	a0, a0, 1929
	addi	s2, a1, -255
	mv	a1, s2
	call	m_qhigh
	lw	a2, 4(s3)
	xor	a0, a0, s2
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_134
# %bb.132:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_134
# %bb.133:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_134:
	li	a7, 0
	li	t0, 1
	sw	zero, 24(sp)
	sw	t0, 28(sp)
	li	t1, 9
	li	t2, -1
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	sw	t0, 0(sp)
	sw	zero, 8(sp)
	sw	t2, 12(sp)
	sw	t1, 16(sp)
	call	m_qlate
	lw	a2, 4(s3)
	xori	a1, a1, 4
	xori	a0, a0, 37
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_137
# %bb.135:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_137
# %bb.136:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_137:
	lui	s2, 172127
	addi	s2, s2, 512
	li	a1, 1
	mv	a0, s2
	call	m_qtwice
	lw	a2, 4(s3)
	lui	a3, 344254
	addi	a3, a3, 1024
	xor	a0, a0, a3
	xori	a1, a1, 2
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_140
# %bb.138:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_140
# %bb.139:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_140:
	li	a0, 7
	li	s4, -1
	lui	a1, 316154
	addi	a1, a1, 512
	sw	a0, 80(sp)
	sw	a1, 88(sp)
	sw	s4, 92(sp)
	addi	a0, sp, 80
	li	a2, 1
	mv	a1, s2
	call	m_qsum
	lw	a2, 4(s3)
	lui	a3, 488281
	addi	a3, a3, 1031
	xor	a0, a0, a3
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_143
# %bb.141:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_143
# %bb.142:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_143:
	li	a2, 0
	sw	zero, 0(sp)
	lui	s5, 172127
	lui	a0, %hi(.L.str.6)
	addi	a0, a0, %lo(.L.str.6)
	addi	a6, s5, 512
	li	a1, 1
	li	a3, 1
	li	a4, 2
	li	a7, 1
	sw	s4, 4(sp)
	li	s2, 1
	call	m_vq
	lw	a2, 4(s3)
	addi	a3, s5, 515
	xor	a0, a0, a3
	xori	a1, a1, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_146
# %bb.144:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_146
# %bb.145:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_146:
	lui	a0, %hi(.L.str.7)
	addi	a0, a0, %lo(.L.str.7)
	li	a1, 1
	li	a2, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a6, 6
	sw	zero, 0(sp)
	sw	s2, 4(sp)
	call	m_vq
	lw	a2, 4(s3)
	xori	a1, a1, 1
	xori	a0, a0, 21
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_149
# %bb.147:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_149
# %bb.148:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_149:
	li	a7, 1
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	sw	zero, 0(sp)
	sw	a7, 4(sp)
	call	m_vqlate
	lw	a2, 4(s3)
	xori	a1, a1, 1
	xori	a0, a0, 28
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_152
# %bb.150:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_152
# %bb.151:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_152:
	li	a6, 0
	li	s4, 3
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a7, 1
	li	s2, 4
	li	s5, 1
	call	m_q67
	lw	a2, 4(s3)
	xori	a1, a1, 3
	xori	a0, a0, 21
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_155
# %bb.153:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_155
# %bb.154:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_155:
	sw	s4, 32(sp)
	lui	a0, 32
	lui	a1, 172127
	li	a2, -2
	addi	a1, a1, 512
	addi	a0, a0, 1
	sw	a1, 16(sp)
	sw	s5, 20(sp)
	sw	a2, 24(sp)
	sw	a0, 28(sp)
	li	t0, 9
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	li	a7, 8
	sw	t0, 0(sp)
	sw	s0, 4(sp)
	sw	s1, 8(sp)
	sw	s2, 12(sp)
	call	m_oddstack
	lw	a2, 4(s3)
	lui	a3, 172128
	addi	a3, a3, 1251
	xor	a0, a0, a3
	xori	a1, a1, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_158
# %bb.156:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_158
# %bb.157:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_158:
	lui	a0, %hi(.L.str.8)
	addi	a0, a0, %lo(.L.str.8)
	li	a3, 1
	li	a2, 0
	call	m_vq
	xori	a1, a1, 1
	or	a0, a0, a1
	li	a1, 0
	bnez	a0, .LBB63_160
# %bb.159:
	lui	a0, %hi(.L.str.9)
	addi	a0, a0, %lo(.L.str.9)
	lui	s0, 172127
	addi	s0, s0, 512
	li	a3, 1
	mv	a2, s0
	call	m_vq
	xor	a0, a0, s0
	xori	a1, a1, 1
	or	a0, a0, a1
	seqz	a1, a0
.LBB63_160:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_163
# %bb.161:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_163
# %bb.162:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_163:
	lui	a0, %hi(.L.str.10)
	addi	a0, a0, %lo(.L.str.10)
	lui	s1, 172127
	li	a1, 1
	addi	s0, s1, 512
	li	a2, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a7, 1
	mv	a6, s0
	li	s2, 1
	call	m_vq
	lw	a2, 4(s3)
	addi	a3, s1, 527
	xor	a0, a0, a3
	xori	a1, a1, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_166
# %bb.164:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_166
# %bb.165:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_166:
	lui	a0, %hi(.L.str.11)
	addi	a0, a0, %lo(.L.str.11)
	li	a1, 1
	li	a2, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a6, 6
	sw	s0, 0(sp)
	sw	s2, 4(sp)
	call	m_vq
	lw	a2, 4(s3)
	addi	a3, s1, 533
	xor	a0, a0, a3
	xori	a1, a1, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_169
# %bb.167:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_169
# %bb.168:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_169:
	li	a4, 0
	li	a3, 1
	lui	s0, 172127
	addi	a7, s0, 512
	li	a0, -1
	sw	zero, 0(sp)
	sw	a0, 4(sp)
	lui	a2, %hi(.L.str.12)
	addi	a2, a2, %lo(.L.str.12)
	li	a0, 1
	li	a1, 2
	li	a5, 1
	li	a6, 3
	sw	a7, 8(sp)
	sw	a3, 12(sp)
	call	m_vq3
	lw	a2, 4(s3)
	addi	a3, s0, 518
	xor	a0, a0, a3
	xori	a1, a1, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_172
# %bb.170:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_172
# %bb.171:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_172:
	lui	a2, %hi(.L.str.13)
	addi	a2, a2, %lo(.L.str.13)
	li	a0, 1
	li	a1, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a7, 1
	li	a6, 0
	li	s0, 1
	call	m_vq3
	lw	a2, 4(s3)
	xori	a1, a1, 1
	xori	a0, a0, 15
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_175
# %bb.173:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_175
# %bb.174:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_175:
	lui	s1, 172127
	lui	a2, %hi(.L.str.14)
	addi	a2, a2, %lo(.L.str.14)
	li	a0, 1
	addi	a7, s1, 512
	li	a1, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a6, 6
	sw	a7, 0(sp)
	sw	s0, 4(sp)
	call	m_vq3
	lw	a2, 4(s3)
	addi	a3, s1, 533
	xor	a0, a0, a3
	xori	a1, a1, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_178
# %bb.176:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_178
# %bb.177:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_178:
	lbu	a0, 144(sp)
	lbu	a1, 145(sp)
	lbu	a2, 146(sp)
	li	a3, 1
	lbu	a4, 140(sp)
	lbu	a5, 141(sp)
	lbu	a6, 142(sp)
	sb	a3, 48(sp)
	sb	a0, 49(sp)
	sb	a1, 50(sp)
	sb	a2, 51(sp)
	li	a0, 9
	sb	zero, 52(sp)
	sb	a4, 53(sp)
	sb	a5, 54(sp)
	sb	a6, 55(sp)
	sb	a0, 56(sp)
	addi	a0, sp, 80
	addi	a1, sp, 48
	call	m_rev9
	lbu	a0, 82(sp)
	lbu	s0, 80(sp)
	lbu	a1, 81(sp)
	lbu	a2, 83(sp)
	lbu	s1, 88(sp)
	lbu	s2, 84(sp)
	lbu	a3, 85(sp)
	lbu	a4, 86(sp)
	slli	a0, a0, 8
	xori	a5, s0, 9
	or	a0, a0, a1
	xori	a1, s1, 1
	sh	a0, 144(sp)
	sb	a2, 146(sp)
	lw	a0, 4(s3)
	or	a1, a5, a1
	lbu	a2, 87(sp)
	slli	a4, a4, 8
	or	a1, a1, s2
	or	a3, a4, a3
	zext.b	a1, a1
	addi	a0, a0, 1
	sh	a3, 140(sp)
	sb	a2, 142(sp)
	sw	a0, 4(s3)
	beqz	a1, .LBB63_181
# %bb.179:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_181
# %bb.180:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_181:
	lh	a0, 132(sp)
	lh	a1, 134(sp)
	lh	a2, 136(sp)
	li	a3, 2
	li	a4, -3
	sh	a3, 48(sp)
	sh	a0, 50(sp)
	sh	a1, 52(sp)
	sh	a2, 54(sp)
	sh	a4, 56(sp)
	addi	a0, sp, 80
	addi	a1, sp, 48
	addi	s6, sp, 80
	call	m_neg10
	lhu	s4, 80(sp)
	lhu	a0, 84(sp)
	lhu	a1, 82(sp)
	lhu	s5, 88(sp)
	lw	a2, 4(s3)
	lh	a3, 86(sp)
	slli	a0, a0, 16
	xori	a4, s4, -2
	or	a1, a0, a1
	xori	a0, s5, 3
	or	a4, a4, a0
	addi	a0, a2, 1
	slli	a4, a4, 16
	sw	a1, 132(sp)
	sh	a3, 136(sp)
	sw	a0, 4(s3)
	beqz	a4, .LBB63_184
# %bb.182:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_184
# %bb.183:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_184:
	lbu	a0, 144(sp)
	lbu	a1, 145(sp)
	lbu	a2, 146(sp)
	lbu	a3, 140(sp)
	lbu	a4, 141(sp)
	lbu	a5, 142(sp)
	sb	s0, 48(sp)
	sb	a0, 49(sp)
	sb	a1, 50(sp)
	sb	a2, 51(sp)
	lh	a0, 132(sp)
	lh	a1, 134(sp)
	lh	a2, 136(sp)
	sb	s2, 52(sp)
	sb	a3, 53(sp)
	sb	a4, 54(sp)
	sb	a5, 55(sp)
	sb	s1, 56(sp)
	sh	s4, 80(sp)
	sh	a0, 82(sp)
	sh	a1, 84(sp)
	sh	a2, 86(sp)
	sh	s5, 88(sp)
	li	a0, 1
	li	a1, 2
	li	a2, 3
	li	a3, 4
	li	a4, 5
	li	a5, 6
	li	a6, 7
	addi	a7, sp, 48
	sw	s6, 0(sp)
	call	m_late910
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	a2, 345
	sw	a1, 4(s3)
	beq	a0, a2, .LBB63_187
# %bb.185:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_187
# %bb.186:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_187:
	lbu	a0, 144(sp)
	lbu	a1, 145(sp)
	lbu	a2, 146(sp)
	lbu	a3, 140(sp)
	lbu	a4, 141(sp)
	lbu	a5, 142(sp)
	sb	s0, 68(sp)
	sb	a0, 69(sp)
	sb	a1, 70(sp)
	sb	a2, 71(sp)
	lh	a0, 132(sp)
	lh	a1, 134(sp)
	lh	a2, 136(sp)
	sb	s2, 72(sp)
	sb	a3, 73(sp)
	sb	a4, 74(sp)
	sb	a5, 75(sp)
	sh	s4, 48(sp)
	sh	a0, 50(sp)
	sh	a1, 52(sp)
	sh	a2, 54(sp)
	li	a0, 97
	lbu	a1, 124(sp)
	lbu	a2, 125(sp)
	lbu	a3, 126(sp)
	lbu	a4, 127(sp)
	lbu	a5, 128(sp)
	lbu	a6, 129(sp)
	lbu	a7, 130(sp)
	sb	a0, 80(sp)
	sb	a1, 81(sp)
	sb	a2, 82(sp)
	sb	a3, 83(sp)
	sb	s1, 76(sp)
	sh	s5, 56(sp)
	sb	a4, 84(sp)
	sb	a5, 85(sp)
	sb	a6, 86(sp)
	sb	a7, 87(sp)
	li	a0, -1
	sw	zero, 88(sp)
	sw	a0, 92(sp)
	lui	a0, %hi(.L.str.15)
	addi	a0, a0, %lo(.L.str.15)
	addi	a1, sp, 68
	addi	a2, sp, 48
	addi	a3, sp, 80
	li	a4, 5
	call	m_v910
	lw	a2, 4(s3)
	not	a1, a1
	xori	a0, a0, 419
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_190
# %bb.188:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_190
# %bb.189:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_190:
	lbu	a0, 144(sp)
	lbu	a1, 145(sp)
	lbu	a2, 146(sp)
	lbu	a3, 140(sp)
	lbu	a4, 141(sp)
	lbu	a5, 142(sp)
	sb	s0, 80(sp)
	sb	a0, 81(sp)
	sb	a1, 82(sp)
	sb	a2, 83(sp)
	sb	s2, 84(sp)
	sb	a3, 85(sp)
	sb	a4, 86(sp)
	sb	a5, 87(sp)
	sb	s1, 88(sp)
	lui	a0, %hi(.L.str.16)
	addi	a0, a0, %lo(.L.str.16)
	li	a1, 1
	li	a2, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a6, 6
	addi	a7, sp, 80
	addi	s1, sp, 80
	call	m_v910
	lw	a2, 4(s3)
	xori	a0, a0, 40
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_193
# %bb.191:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_193
# %bb.192:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_193:
	lh	a0, 132(sp)
	lh	a1, 134(sp)
	lh	a2, 136(sp)
	sh	s5, 56(sp)
	li	s0, 97
	sh	s4, 48(sp)
	sh	a0, 50(sp)
	sh	a1, 52(sp)
	sh	a2, 54(sp)
	lbu	a0, 124(sp)
	lbu	a1, 125(sp)
	lbu	a2, 126(sp)
	lbu	a3, 127(sp)
	lbu	a4, 128(sp)
	lbu	a5, 129(sp)
	lbu	a6, 130(sp)
	sb	s0, 80(sp)
	sb	a0, 81(sp)
	sb	a1, 82(sp)
	sb	a2, 83(sp)
	sb	a3, 84(sp)
	sb	a4, 85(sp)
	sb	a5, 86(sp)
	sb	a6, 87(sp)
	li	s2, -1
	sw	zero, 88(sp)
	sw	s2, 92(sp)
	addi	t0, sp, 48
	lui	a0, %hi(.L.str.17)
	addi	a0, a0, %lo(.L.str.17)
	li	a1, 1
	li	a2, 2
	li	a3, 3
	li	a4, 4
	li	a5, 5
	li	a6, 6
	li	a7, 7
	sw	t0, 0(sp)
	sw	s1, 4(sp)
	call	m_v910
	lw	a2, 4(s3)
	not	a1, a1
	xori	a0, a0, 423
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_196
# %bb.194:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_196
# %bb.195:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_196:
	lbu	a0, 124(sp)
	lbu	a1, 125(sp)
	lbu	a2, 126(sp)
	lbu	a3, 127(sp)
	lbu	a4, 128(sp)
	lbu	a5, 129(sp)
	lbu	a6, 130(sp)
	sb	s0, 48(sp)
	sb	a0, 49(sp)
	sb	a1, 50(sp)
	sb	a2, 51(sp)
	sb	a3, 52(sp)
	sb	a4, 53(sp)
	sb	a5, 54(sp)
	sb	a6, 55(sp)
	sw	zero, 56(sp)
	sw	s2, 60(sp)
	addi	a0, sp, 80
	addi	a1, sp, 48
	call	m_cq
	lbu	a3, 85(sp)
	lbu	a1, 86(sp)
	lbu	a0, 80(sp)
	lbu	a4, 81(sp)
	lbu	a5, 82(sp)
	lbu	a6, 83(sp)
	lbu	a7, 84(sp)
	lbu	t0, 87(sp)
	slli	t1, a1, 8
	lw	a1, 88(sp)
	lw	a2, 92(sp)
	or	t1, t1, a3
	slli	a5, a5, 8
	slli	a6, a6, 16
	slli	a7, a7, 24
	lw	a3, 4(s3)
	or	a4, a5, a4
	or	a5, a7, a6
	or	a4, a5, a4
	xori	a5, a2, 1
	xori	a6, a0, 98
	or	a5, a1, a5
	or	a5, a6, a5
	addi	a3, a3, 1
	sw	a4, 124(sp)
	sh	t1, 128(sp)
	sb	t0, 130(sp)
	sw	a3, 4(s3)
	beqz	a5, .LBB63_199
# %bb.197:
	lw	a4, 0(s3)
	bnez	a4, .LBB63_199
# %bb.198:
	lui	a4, %hi(.L_MergedGlobals)
	sw	a3, %lo(.L_MergedGlobals)(a4)
.LBB63_199:
	lbu	a3, 128(sp)
	lbu	a4, 129(sp)
	lbu	a5, 130(sp)
	lbu	a6, 124(sp)
	lbu	a7, 124(sp)
	lbu	t0, 125(sp)
	lbu	t1, 125(sp)
	lbu	t2, 126(sp)
	lbu	t3, 126(sp)
	lbu	t4, 127(sp)
	lbu	t5, 127(sp)
	sb	a0, 80(sp)
	sb	a6, 81(sp)
	sb	t0, 82(sp)
	sb	t2, 83(sp)
	lbu	a6, 128(sp)
	lbu	t0, 129(sp)
	lbu	t2, 130(sp)
	sb	t4, 84(sp)
	sb	a3, 85(sp)
	sb	a4, 86(sp)
	sb	a5, 87(sp)
	sb	a0, 48(sp)
	sb	a7, 49(sp)
	sb	t1, 50(sp)
	sb	t3, 51(sp)
	sw	a1, 88(sp)
	sw	a2, 92(sp)
	sb	t5, 52(sp)
	sb	a6, 53(sp)
	sb	t0, 54(sp)
	sb	t2, 55(sp)
	sw	a1, 56(sp)
	sw	a2, 60(sp)
	li	a0, 3
	addi	a1, sp, 80
	addi	a2, sp, 48
	call	m_cqsum
	lw	a2, 4(s3)
	xori	a1, a1, 2
	xori	a0, a0, 1081
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_202
# %bb.200:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_202
# %bb.201:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_202:
	lui	a0, 524288
	li	a1, -1
	call	m_ucmp
	li	a1, 15
	bne	a0, a1, .LBB63_204
# %bb.203:
	li	a0, 5
	li	a1, 3
	call	m_ucmp
	seqz	a1, a0
	j	.LBB63_205
.LBB63_204:
	li	a1, 0
.LBB63_205:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_208
# %bb.206:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_208
# %bb.207:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_208:
	lui	s0, 524288
	addi	a0, s0, -1
	li	a1, 1
	call	m_uadd
	bne	a0, s0, .LBB63_210
# %bb.209:
	li	a0, -1
	li	a1, 2
	call	m_uadd
	addi	a0, a0, -1
	seqz	a1, a0
	j	.LBB63_211
.LBB63_210:
	li	a1, 0
.LBB63_211:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_214
# %bb.212:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_214
# %bb.213:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_214:
	lui	a0, 524288
	li	a1, 0
	lui	s0, 524288
	call	m_ushr
	li	a1, 0
	bne	a0, s0, .LBB63_216
# %bb.215:
	li	a0, -1
	li	a1, 4
	call	m_ushr
	lui	a1, 65536
	addi	a1, a1, -1
	xor	a0, a0, a1
	seqz	a1, a0
.LBB63_216:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_219
# %bb.217:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_219
# %bb.218:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_219:
	li	a0, -2
	li	a1, 1
	li	s0, -2
	call	m_udiv
	bne	a0, s0, .LBB63_221
# %bb.220:
	li	a0, -1
	li	a1, 2
	call	m_udiv
	lui	a1, 524288
	addi	a1, a1, -1
	xor	a0, a0, a1
	seqz	a1, a0
	j	.LBB63_222
.LBB63_221:
	li	a1, 0
.LBB63_222:
	lw	a0, 4(s3)
	addi	a0, a0, 1
	sw	a0, 4(s3)
	bnez	a1, .LBB63_225
# %bb.223:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_225
# %bb.224:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_225:
	lui	s1, 524288
	addi	a0, s1, -1
	li	a1, 1
	call	m_uadd
	mv	s0, a0
	li	a0, -1
	li	a1, 1
	call	m_udiv
	mv	a1, a0
	mv	a0, s0
	call	m_ucmp
	lw	a1, 4(s3)
	addi	a1, a1, 1
	li	s2, 15
	sw	a1, 4(s3)
	beq	a0, s2, .LBB63_228
# %bb.226:
	lw	a0, 0(s3)
	bnez	a0, .LBB63_228
# %bb.227:
	lui	a0, %hi(.L_MergedGlobals)
	sw	a1, %lo(.L_MergedGlobals)(a0)
.LBB63_228:
	lui	a0, 524288
	li	a1, 0
	call	m_ushr
	mv	s0, a0
	li	a0, -2
	li	a1, 1
	call	m_uadd
	xor	s1, s0, s1
	srli	a1, s0, 28
	sltu	a2, s0, a0
	andi	a1, a1, 8
	or	a1, a1, a2
	lw	a2, 4(s3)
	seqz	a3, s1
	addi	a0, a0, 1
	slli	a3, a3, 1
	seqz	a0, a0
	slli	a0, a0, 2
	or	a1, a1, a3
	or	a1, a1, a0
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beq	a1, s2, .LBB63_231
# %bb.229:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_231
# %bb.230:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_231:
	lui	a0, %hi(m_shlq)
	addi	a0, a0, %lo(m_shlq)
	li	a2, 1
	li	a3, 3
	li	a1, 0
	call	m_applyq
	lw	a2, 4(s3)
	xori	a1, a1, 8
	xori	a0, a0, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_234
# %bb.232:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_234
# %bb.233:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_234:
	lui	a0, %hi(c_shlq)
	addi	a0, a0, %lo(c_shlq)
	li	a1, 5
	li	a3, 40
	li	a2, 0
	call	m_applyq
	lw	a2, 4(s3)
	xori	a1, a1, 1280
	xori	a0, a0, 1
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_237
# %bb.235:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_237
# %bb.236:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_237:
	li	a0, 3
	li	a2, 33
	li	a1, 0
	call	m_shlq
	lw	a2, 4(s3)
	xori	a1, a1, 6
	or	a1, a0, a1
	addi	a0, a2, 1
	sw	a0, 4(s3)
	beqz	a1, .LBB63_240
# %bb.238:
	lw	a1, 0(s3)
	bnez	a1, .LBB63_240
# %bb.239:
	lui	a1, %hi(.L_MergedGlobals)
	sw	a0, %lo(.L_MergedGlobals)(a1)
.LBB63_240:
	lui	a0, %hi(.L_MergedGlobals)
	lw	a0, %lo(.L_MergedGlobals)(a0)
	lw	ra, 220(sp)                     # 4-byte Folded Reload
	lw	s0, 216(sp)                     # 4-byte Folded Reload
	lw	s1, 212(sp)                     # 4-byte Folded Reload
	lw	s2, 208(sp)                     # 4-byte Folded Reload
	lw	s3, 204(sp)                     # 4-byte Folded Reload
	lw	s4, 200(sp)                     # 4-byte Folded Reload
	lw	s5, 196(sp)                     # 4-byte Folded Reload
	lw	s6, 192(sp)                     # 4-byte Folded Reload
	lw	s7, 188(sp)                     # 4-byte Folded Reload
	lw	s8, 184(sp)                     # 4-byte Folded Reload
	addi	sp, sp, 224
	ret
.Lfunc_end63:
	.size	c_calls_merlin, .Lfunc_end63-c_calls_merlin
                                        # -- End function
	.type	.L__const.c_calls_merlin.s9,@object # @__const.c_calls_merlin.s9
	.section	.rodata,"a",@progbits
.L__const.c_calls_merlin.s9:
	.ascii	"\001\000\000\000\000\000\000\000\t"
	.size	.L__const.c_calls_merlin.s9, 9

	.type	.L__const.c_calls_merlin.s10,@object # @__const.c_calls_merlin.s10
	.p2align	1, 0x0
.L__const.c_calls_merlin.s10:
	.half	2                               # 0x2
	.half	0                               # 0x0
	.half	0                               # 0x0
	.half	0                               # 0x0
	.half	65533                           # 0xfffd
	.size	.L__const.c_calls_merlin.s10, 10

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

	.type	.L.str.6,@object                # @.str.6
.L.str.6:
	.asciz	"iqisq"
	.size	.L.str.6, 6

	.type	.L.str.7,@object                # @.str.7
.L.str.7:
	.asciz	"iiiiiiq"
	.size	.L.str.7, 8

	.type	.L.str.8,@object                # @.str.8
.L.str.8:
	.asciz	"q"
	.size	.L.str.8, 2

	.type	.L.str.9,@object                # @.str.9
.L.str.9:
	.asciz	"s"
	.size	.L.str.9, 2

	.type	.L.str.10,@object               # @.str.10
.L.str.10:
	.asciz	"iiiiis"
	.size	.L.str.10, 7

	.type	.L.str.11,@object               # @.str.11
.L.str.11:
	.asciz	"iiiiiis"
	.size	.L.str.11, 8

	.type	.L.str.12,@object               # @.str.12
.L.str.12:
	.asciz	"qiqs"
	.size	.L.str.12, 5

	.type	.L.str.13,@object               # @.str.13
.L.str.13:
	.asciz	"iiiq"
	.size	.L.str.13, 5

	.type	.L.str.14,@object               # @.str.14
.L.str.14:
	.asciz	"iiiis"
	.size	.L.str.14, 6

	.type	.L.str.15,@object               # @.str.15
.L.str.15:
	.asciz	"90ci"
	.size	.L.str.15, 5

	.type	.L.str.16,@object               # @.str.16
.L.str.16:
	.asciz	"iiiiii9"
	.size	.L.str.16, 8

	.type	.L.str.17,@object               # @.str.17
.L.str.17:
	.asciz	"iiiiiii0c"
	.size	.L.str.17, 10

	.type	.L_MergedGlobals,@object        # @_MergedGlobals
	.local	.L_MergedGlobals
	.comm	.L_MergedGlobals,8,4
first_failure = .L_MergedGlobals
	.size	first_failure, 4
checks = .L_MergedGlobals+4
	.size	checks, 4
	.ident	"clang version 23.1.4 (https://github.com/llvm/llvm-project 7a941dfb2877a0b8cf30323fefa85ad019e0b0f5)"
	.section	".note.GNU-stack","",@progbits
