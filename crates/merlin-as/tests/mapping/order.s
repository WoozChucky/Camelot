# The symbol table's order, clang's: a symbol has its place where the source
# first names it, `.local`, `.type` and `.globl` included, but `.size` names
# none (obj2, z); `.local` keeps an undefined symbol local (u).
# scripts/gen-mapping-golden.sh, tests/mapping.rs.
	.data
	.word	1
	.type	obj,@object
	.size	obj2, 4
	.local	loc1
	.local	u
	.size	z, 4
	.type	w,@object
	.text
	nop
	.data
obj:	.word	2
obj2:	.word	3
	.text
tgt:	nop
	lui	a0, %hi(late)
	addi	a0, a0, %lo(late)
	call	u
	.section .rodata
late:	.word	0
late2:
loc1:	.byte	1
	.text
	.word	0
	.type	f,@function
f:	ret
	.size	f, .-f
	.globl	h
h:	.word	0
