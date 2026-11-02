# MD5 checksums (a column of the file table when every file has one), a name
# used twice (one string), every .loc option, and address and line advances
# too big for a special opcode. .space makes no row; the instruction after it does.
	.file	0 "/src" "main.c" md5 0x0c0c0a567318e515c95354828410bedf
	.file	1 "/other" "main.c" md5 0x00112233445566778899aabbccddeeff
	.text
	.loc 0 10 3 is_stmt 0
	nop
	.loc 0 11 4
	nop
	.loc 1 12 0 discriminator 3 isa 2 basic_block
	nop
	.loc 0 9 0
	.space 400
	nop
	.loc 0 9 0
	.space 64
	nop
	.loc 0 300 0
	.space 40000
	nop
	.loc 0 299 0 is_stmt 1
	.space 64
	nop
	.loc 0 2 0
	.space 20
	nop
	.loc 0 3 0
	.space 13
	.balign 4
	nop
	.loc 0 3 0
	nop
	.space 17
