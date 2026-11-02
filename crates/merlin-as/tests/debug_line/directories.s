# How file names and directories become the directory table: the compilation
# directory, names split at their last '/', directories found again. Two .loc
# lines in a row both get rows.
	.file	0 "/src" "main.c"
	.file	1 "kernel/a.h"
	.file	2 "/other" "x.h"
	.file	3 "/src" "y.h"
	.file	4 "/src/kernel" "z.h"
	.file	5 "/abs/q.h"
	.file	6 "/src" "kernel/b.h"
	.text
	.loc 0 1 0
	nop
	.loc 1 2 0
	.loc 2 3 0
	nop
	.loc 3 4 0
	nop
	.loc 4 5 0
	nop
	.loc 5 6 0
	nop
	.loc 6 7 0
	nop
