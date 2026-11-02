# sysregs: checks that a system call changes a0 and nothing else
# (docs/camelot-syscalls.md, "Calling convention"). C programs can't tell:
# a1-a7 and the temporaries are caller-saved there, so a kernel that
# clobbered them would go unnoticed. For each case in the table below, sets
# a7 and a0-a2 from the case and every other register to its number in each
# byte (t6 excepted, see below), makes the call, then checks every register
# but a0 against what it was set to, and a0 against the case's result when
# it has one. The cases cover a call that only answers (getpid), ones that
# switch to another process or the scheduler (yield, sleep), ones that fail
# (a bad file descriptor, an unknown call number) and a write, which prints
# `sysregs: write`. Then prints `sysregs: every register but a0 kept` and
# exits 0; or, at the first register that changed, exits with 32 times the
# case's index plus the register's number (a0's for a wrong result).

    .equ SYS_write, 2
    .equ SYS_read, 3
    .equ SYS_getpid, 4
    .equ SYS_yield, 5
    .equ SYS_sleep, 6
    .equ SYS_sbrk, 9
    .equ SYS_uptime, 10
    .equ SYS_UNKNOWN, 99
    .equ NEG_EBADF, -9
    .equ NEG_ENOSYS, -38
    .equ STDOUT, 1
    # The bytes of each case in the table, and of the two messages.
    .equ CASE, 20
    .equ MSG_LEN, 15
    .equ OK_LEN, 36
    # A result that isn't checked (getpid's, sbrk's, uptime's).
    .equ ANY, 0x80000000
    # t6 points at the case while the arguments are loaded, and is the first
    # register checked after the call, with none to spare: it holds a value
    # one addi takes back to zero.
    .equ T6, -0x31f

    .text
    .globl main
main:
    la    t0, cases
    la    t1, cursor
    sw    t0, 0(t1)

next:
    la    t6, cursor
    lw    t6, 0(t6)             # the case
    lw    a7, 0(t6)             # its call number, or -1 after the last one
    bltz  a7, done
    lw    a0, 4(t6)
    lw    a1, 8(t6)
    lw    a2, 12(t6)
    li    ra, 0x01010101
    li    sp, 0x02020202        # nothing here uses the stack
    li    gp, 0x03030303
    li    tp, 0x04040404
    li    t0, 0x05050505
    li    t1, 0x06060606
    li    t2, 0x07070707
    li    s0, 0x08080808
    li    s1, 0x09090909
    li    a3, 0x0d0d0d0d
    li    a4, 0x0e0e0e0e
    li    a5, 0x0f0f0f0f
    li    a6, 0x10101010
    li    s2, 0x12121212
    li    s3, 0x13131313
    li    s4, 0x14141414
    li    s5, 0x15151515
    li    s6, 0x16161616
    li    s7, 0x17171717
    li    s8, 0x18181818
    li    s9, 0x19191919
    li    s10, 0x1a1a1a1a
    li    s11, 0x1b1b1b1b
    li    t3, 0x1c1c1c1c
    li    t4, 0x1d1d1d1d
    li    t5, 0x1e1e1e1e
    li    t6, T6
    ecall

# t6 first, on its own; then sp and ra with t6 holding each expected value.
# Once they are checked, sp points at the case again and ra holds the number
# of the register being checked, for the exit status.
    addi  t6, t6, -T6
    bnez  t6, bad_t6
    li    t6, 0x02020202
    bne   sp, t6, bad_sp
    li    t6, 0x01010101
    bne   ra, t6, bad_ra
    la    sp, cursor
    lw    sp, 0(sp)
    # a0: the result, unless the case has none to check.
    lw    t6, 16(sp)
    li    ra, ANY
    beq   t6, ra, 1f
    li    ra, 10
    bne   a0, t6, bad
1:  li    ra, 3
    li    t6, 0x03030303
    bne   gp, t6, bad
    li    ra, 4
    li    t6, 0x04040404
    bne   tp, t6, bad
    li    ra, 5
    li    t6, 0x05050505
    bne   t0, t6, bad
    li    ra, 6
    li    t6, 0x06060606
    bne   t1, t6, bad
    li    ra, 7
    li    t6, 0x07070707
    bne   t2, t6, bad
    li    ra, 8
    li    t6, 0x08080808
    bne   s0, t6, bad
    li    ra, 9
    li    t6, 0x09090909
    bne   s1, t6, bad
    li    ra, 11
    lw    t6, 8(sp)
    bne   a1, t6, bad
    li    ra, 12
    lw    t6, 12(sp)
    bne   a2, t6, bad
    li    ra, 13
    li    t6, 0x0d0d0d0d
    bne   a3, t6, bad
    li    ra, 14
    li    t6, 0x0e0e0e0e
    bne   a4, t6, bad
    li    ra, 15
    li    t6, 0x0f0f0f0f
    bne   a5, t6, bad
    li    ra, 16
    li    t6, 0x10101010
    bne   a6, t6, bad
    li    ra, 17
    lw    t6, 0(sp)
    bne   a7, t6, bad
    li    ra, 18
    li    t6, 0x12121212
    bne   s2, t6, bad
    li    ra, 19
    li    t6, 0x13131313
    bne   s3, t6, bad
    li    ra, 20
    li    t6, 0x14141414
    bne   s4, t6, bad
    li    ra, 21
    li    t6, 0x15151515
    bne   s5, t6, bad
    li    ra, 22
    li    t6, 0x16161616
    bne   s6, t6, bad
    li    ra, 23
    li    t6, 0x17171717
    bne   s7, t6, bad
    li    ra, 24
    li    t6, 0x18181818
    bne   s8, t6, bad
    li    ra, 25
    li    t6, 0x19191919
    bne   s9, t6, bad
    li    ra, 26
    li    t6, 0x1a1a1a1a
    bne   s10, t6, bad
    li    ra, 27
    li    t6, 0x1b1b1b1b
    bne   s11, t6, bad
    li    ra, 28
    li    t6, 0x1c1c1c1c
    bne   t3, t6, bad
    li    ra, 29
    li    t6, 0x1d1d1d1d
    bne   t4, t6, bad
    li    ra, 30
    li    t6, 0x1e1e1e1e
    bne   t5, t6, bad
    # On to the next case.
    addi  sp, sp, CASE
    la    t6, cursor
    sw    sp, 0(t6)
    j     next

done:
    li    a0, STDOUT
    la    a1, ok
    li    a2, OK_LEN
    call  write
    li    a0, 0
    call  exit

bad_t6:
    li    ra, 31
    j     bad_case
bad_sp:
    li    ra, 2
    j     bad_case
bad_ra:
    li    ra, 1
bad_case:
    la    sp, cursor
    lw    sp, 0(sp)
# ra is the register's number, sp the case: exit with 32 * index + number.
bad:
    la    t6, cases
    sub   sp, sp, t6            # index * CASE (20)
    li    t6, CASE
    divu  sp, sp, t6
    slli  sp, sp, 5
    add   a0, sp, ra
    call  exit

    .section .rodata
# Each case: the call number, a0, a1, a2, then the result a0 must hold (or
# ANY). The arguments are patterns too where the call ignores them.
cases:
    .word SYS_getpid, 0x0a0a0a0a, 0x0b0b0b0b, 0x0c0c0c0c, ANY
    .word SYS_yield, 0x0a0a0a0a, 0x0b0b0b0b, 0x0c0c0c0c, 0
    .word SYS_sleep, 1, 0x0b0b0b0b, 0x0c0c0c0c, 0
    .word SYS_read, 5, 0x0b0b0b0b, 0x0c0c0c0c, NEG_EBADF
    .word SYS_UNKNOWN, 0x0a0a0a0a, 0x0b0b0b0b, 0x0c0c0c0c, NEG_ENOSYS
    .word SYS_sbrk, 0, 0x0b0b0b0b, 0x0c0c0c0c, ANY
    .word SYS_uptime, 0x0a0a0a0a, 0x0b0b0b0b, 0x0c0c0c0c, ANY
    .word SYS_write, STDOUT, msg, MSG_LEN, MSG_LEN
    .word -1
msg:
    .ascii "sysregs: write\n"
ok:
    .ascii "sysregs: every register but a0 kept\n"

    .data
# The case being run: in memory, since every register is in use.
cursor:
    .word 0
