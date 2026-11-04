# Mapping symbols ($x<isa> where code starts, $d where data starts) and the
# padding of code. scripts/gen-mapping-golden.sh assembles this with clang
# (mapping.o, mapping.d.txt; rv64/ for RV64); tests/mapping.rs compares.
	.option nopic
	.text
	.globl	_start
	.type	_start,@function
_start:
	.local	buf
	.comm	buf, 16, 4
	nop
	.word	0x12345678
	addi	a0, a0, 1
	.byte	1
	.p2align 2
	nop
	.p2align 4
	.half	7
	.balign	8
	.insn	r 0x33, 0, 0, a0, a1, a2
lbl:
	.zero	4
	j	fwd
	la	a1, other
	.zero	0
	nop
	.ascii	""
	nop
	.word	fwd
	.option	push
	.option	norelax
fwd:
	ret
	.option	pop
	.size	_start, .-_start
	.data
	.word	1
	.lcomm	lb2, 8
	nop
	.text
	nop
	.section .text.b,"ax",@progbits
	.string	"hi"
	.section .rodata
	.ascii	"x"
	.bss
	.zero	8
	.section .empty
	.p2align 3
	.section .text.c,"ax",@progbits
	.p2align 3
	nop
	.section .text.d,"ax",@progbits
	.p2align 3
	.word	5
	.section .text.f,"ax",@progbits
	.zero	0
	.section .text.g,"ax",@progbits
	.ascii	""
	.section .text.h,"ax",@progbits
	.byte
	.section .text.i,"ax",@progbits
	.byte	1
	.p2align 2
	.section .text.j,"ax",@progbits
	.byte	1
	.p2align 3
	.byte	2, 3
	.p2align 3
	.byte	4, 5, 6
	.p2align 4
	nop
	.section .text.k,"ax",@progbits
	.word	0, 1, 2
	.dword	3
	.quad	4
	.section .text.l,"ax",@progbits
	.dword	other
	nop
	.word	other
	nop
	.section .data.m,"aw",@progbits
	nop
	.word	7
	.text
	.word	8
