# `.` kept as a label by a relocation: the label comes before the mapping
# symbol of its line, as clang makes it when it reads the operand. Only the
# symbols are compared (tests/mapping.rs): clang writes the label as `.L0 `.
	.text
	nop
	.word	.
	lui	a0, %hi(.)
	.word	0, .
	.section .text.b,"ax",@progbits
	.word	.
	.section .text.c,"ax",@progbits
	lui	a0, %hi(.)
