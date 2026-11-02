# hello: prints one line from U-mode and exits with status 0.

    .section .text.start
    .globl _start
_start:
    la    a0, msg
    call  puts
    li    a0, 0
    call  exit

    .section .rodata
msg:
    .asciz "hello from user mode\n"
