# Alignment with a fill and a maximum, in code and in data: an explicit fill
# pads with that byte (never nops), a gap longer than the maximum is left
# unpadded (the section's alignment is raised all the same), and padding
# makes no mapping symbol. scripts/gen-mapping-golden.sh, tests/mapping.rs.
# An explicit fill in code follows data here: among instructions, bytes
# that decode to none list differently in merlin-objdump (spec section 10).
	.text
	nop
	.byte	1
	.p2align 2, 0
	nop
	.byte	2
	.p2align 3, 0x55
	nop
	.byte	3
	.p2align 3,,4
	nop
	.byte	4
	.p2align 3,,8
	nop
	.byte	5
	.balign	8, 0x77, 3
	nop
	.byte	7
	.p2align 4, 0x1ff
	nop
	.byte	6
	.balign	16, 0, 4
	.word	1
	.balign	16
	nop
	.byte	8
	.align	3, 0x11
	.byte	1, 2, 3
	.p2align 2, -1
	nop
	.data
	.byte	1
	.p2align 2
	.byte	2
	.p2align 2, 0x55
	.byte	3
	.balign	8, 0x66, 2
	.word	3
	.p2align 5, 0, 31
	.word	4
	.p2align 2, -1
	.byte	1
	.p2align 2, -129
	.section .data.big,"aw",@progbits
	.byte	1
	.balign	64, 0, 2
	.byte	2
	.bss
	.byte	0
	.p2align 3, 0
	.zero	1
	.balign	16,, 2
