# The start code of the archive fixtures: _start calls main, then exit with
# what main returns. In link.ld and first.ld an object of the command line;
# in entry.ld the only member of libcrt.a, which only the entry symbol
# extracts.
    .section .text.start
    .globl _start
_start:
    la    sp, __stack_top
    call  main
    call  exit
1:  j     1b
