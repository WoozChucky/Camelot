# How a name with no directory is split into a directory and a name: by
# LLVM's sys::path::parent_path and filename (POSIX style). A root directory
# is a directory (/q.h is q.h in /), a trailing / makes the name . (a/ is .
# in a), separators in a row are one (a//b.h is b.h in a), //net/ is a root,
# a name with no parent stays whole (/ alone too), and an empty name is
# <stdin>. A directory that is the compilation directory is none: ./z.h is
# split as if it had none, and the . directory of ./x.h is found again for
# it. Merlin writes its .file lines split this way already, as clang does
# (y.h, h.h).
	.file	0 "/src" "m.c"
	.file	1 "/q.h"
	.file	2 "a/"
	.file	3 "./x.h"
	.file	4 "." "y.h"
	.file	5 "/src" "./z.h"
	.file	6 ""
	.file	7 "/src" ""
	.file	8 "//net/x.h"
	.file	9 "a//b.h"
	.file	10 "/"
	.file	11 "a/b/"
	.file	12 "./inc" "h.h"
	.text
	.loc 1 1 0
	nop
	.loc 12 2 0
	nop
