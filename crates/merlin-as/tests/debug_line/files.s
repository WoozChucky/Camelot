# Numbered .file directives and no .loc: clang writes the line table all
# the same, its header alone (the directories and the files, an MD5 column
# only if every file has one) and no sequence.
	.file	0 "/src" "u.c"
	.file	1 "/src" "v.c"
	.file	2 "inc/w.h" md5 0x00112233445566778899aabbccddeeff
	.text
	nop
	.data
	.word	1
