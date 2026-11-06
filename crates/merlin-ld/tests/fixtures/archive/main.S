# The program of the archive fixtures: it prints a line with puts, which
# libdemo.a's puts.o defines, and returns its length plus one if the weak
# `hook` is linked (it is not: a weak reference extracts no member, so the
# .dword is 0). The exit code is then the line's length: 22.
    .text
    .globl main
main:
    addi  sp, sp, -16
    sd    ra, 8(sp)
    la    a0, line
    call  puts
    la    t0, hook_ptr
    ld    t0, 0(t0)
    beqz  t0, 1f
    addi  a0, a0, 1
1:  ld    ra, 8(sp)
    addi  sp, sp, 16
    ret

    .section .rodata
line:
    .asciz "Linked from libdemo.a\n"

    .data
    .balign 8
    .weak hook
hook_ptr:
    .dword hook
