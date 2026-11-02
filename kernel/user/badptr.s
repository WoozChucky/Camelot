# badptr: hands the kernel buffers it must refuse. Each write below must
# return -1 without printing anything, and an ecall the kernel doesn't know
# must return -1 too. Then a good write must return its length. Exits with
# status 0 if all of that holds, else with the number of the case that
# failed.

    .equ USER_TOP, 0x00400000

    .section .text.start
    .globl _start
_start:
    li    s0, 1                 # kernel memory: mapped, but not for U-mode
    li    a0, 0x80000000
    li    a1, 4
    call  write
    li    t0, -1
    bne   a0, t0, fail

    li    s0, 2                 # page 0: never mapped
    li    a0, 0
    li    a1, 4
    call  write
    li    t0, -1
    bne   a0, t0, fail

    li    s0, 3                 # the last two bytes of the stack, then past
    li    a0, USER_TOP - 2      # the end of user space
    li    a1, 4
    call  write
    li    t0, -1
    bne   a0, t0, fail

    li    s0, 4                 # a length that wraps around the address space
    la    a0, msg
    li    a1, -1
    call  write
    li    t0, -1
    bne   a0, t0, fail

    li    s0, 5                 # the guard page under the stack
    li    a0, USER_TOP - 0x1000 - 4
    li    a1, 4
    call  write
    li    t0, -1
    bne   a0, t0, fail

    li    s0, 6                 # an ecall number nobody defined
    li    a7, 99
    ecall
    li    t0, -1
    bne   a0, t0, fail

    li    s0, 7                 # and a good buffer
    la    a0, msg
    call  strlen
    mv    s1, a0
    la    a0, msg
    mv    a1, s1
    call  write
    bne   a0, s1, fail

    li    a0, 0
    call  exit

fail:
    mv    a0, s0
    call  exit

    .section .rodata
msg:
    .asciz "badptr: every bad buffer refused\n"
