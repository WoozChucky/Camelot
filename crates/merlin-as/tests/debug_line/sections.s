# Two code sections, each its own sequence; a header file whose name has a
# directory (split off into the directory table); is_stmt carrying over from
# one .loc to the next and into the second sequence.
	.file	"t.c"
	.file	0 "/src" "kernel/t.c"
	.file	1 "/src" "kernel/t.h"
	.text
	.globl	add
	.type	add,@function
add:
	.loc	0 2 0
	addi	sp, sp, -16
	.loc	0 3 13 prologue_end
	add	a0, a0, a1
	.loc	1 40 2
	nop
	.loc	0 5 5 epilogue_begin is_stmt 0
	addi	sp, sp, 16
	ret
	.size	add, .-add
	.section .text.other,"ax",@progbits
f:
	.loc 0 9 1
	nop
	.loc 0 1000 1
	nop
	ret
