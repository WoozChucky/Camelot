# sysregs: checks that a system call changes a0 and nothing else
# (docs/camelot-syscalls.md, "Calling convention"). C programs can't tell:
# a1-a7 and the temporaries are caller-saved there, so a kernel that
# clobbered them would go unnoticed. For each case in the table below, sets
# a7 and a0-a2 from the case and every other register to a pattern of its
# own (PATTERN, regs.S's), makes the call, then checks every register but a0
# against what it was set to, and a0 against the case's result when it has
# one. The cases cover a call that only answers (getpid), ones that switch to
# another process or the scheduler (yield, sleep), ones that fail (a bad file
# descriptor, an unknown call number) and a write, which prints
# `sysregs: write`. Then prints `sysregs: every register but a0 kept` and
# exits 0; or, at the first register that changed, exits with 32 times the
# case's index plus the register's number (a0's for a wrong result).

# Both widths: every field of a case is a register wide (RISCV_PTR, asm.h),
# loaded whole (REG_L), and so is the cursor, a pointer.
#include <asm.h>

# Register xN's pattern, as in regs.S: N in each byte, and on RV64 N + 0x80
# in each byte of the upper half, so that a register the kernel saved or
# restored as 32 bits comes back different.
#if __riscv_xlen == 64
#define PATTERN(n, high) 0x##high##high##high##high##n##n##n##n
#else
#define PATTERN(n, high) 0x##n##n##n##n
#endif

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
    # The bytes of each case in the table (five fields), and of the two
    # messages.
    .equ CASE, 5 * SZREG
    .equ MSG_LEN, 15
    .equ OK_LEN, 36
    # A result that isn't checked (getpid's, sbrk's, uptime's): one that no
    # call here returns, and that li makes as the table holds it.
    .equ ANY, -0x80000000
    # t6 points at the case while the arguments are loaded, and is the first
    # register checked after the call, with none to spare: it is set to sp's
    # pattern plus T6, so that its difference with sp, less T6, is zero, and
    # then sp is checked on its own.
    .equ T6, 0x31f

    .text
    .globl main
main:
    la    t0, cases
    la    t1, cursor
    REG_S t0, 0(t1)

next:
    la    t6, cursor
    REG_L t6, 0(t6)             # the case
    REG_L a7, 0(t6)             # its call number, or -1 after the last one
    bltz  a7, done
    REG_L a0, 1*SZREG(t6)
    REG_L a1, 2*SZREG(t6)
    REG_L a2, 3*SZREG(t6)
    li    ra,  PATTERN(01, 81)
    li    sp,  PATTERN(02, 82)  # nothing here uses the stack
    li    gp,  PATTERN(03, 83)
    li    tp,  PATTERN(04, 84)
    li    t0,  PATTERN(05, 85)
    li    t1,  PATTERN(06, 86)
    li    t2,  PATTERN(07, 87)
    li    s0,  PATTERN(08, 88)
    li    s1,  PATTERN(09, 89)
    li    a3,  PATTERN(0d, 8d)
    li    a4,  PATTERN(0e, 8e)
    li    a5,  PATTERN(0f, 8f)
    li    a6,  PATTERN(10, 90)
    li    s2,  PATTERN(12, 92)
    li    s3,  PATTERN(13, 93)
    li    s4,  PATTERN(14, 94)
    li    s5,  PATTERN(15, 95)
    li    s6,  PATTERN(16, 96)
    li    s7,  PATTERN(17, 97)
    li    s8,  PATTERN(18, 98)
    li    s9,  PATTERN(19, 99)
    li    s10, PATTERN(1a, 9a)
    li    s11, PATTERN(1b, 9b)
    li    t3,  PATTERN(1c, 9c)
    li    t4,  PATTERN(1d, 9d)
    li    t5,  PATTERN(1e, 9e)
    addi  t6, sp, T6
    ecall

# t6 first: less sp and T6 it is zero if the call kept both (if not,
# bad_t6_or_sp finds which); then sp and ra, with t6 holding each expected
# value. Once they are checked, sp points at the case again and ra holds the
# number of the register being checked, for the exit status.
    sub   t6, t6, sp
    addi  t6, t6, -T6
    bnez  t6, bad_t6_or_sp
    li    t6, PATTERN(02, 82)
    bne   sp, t6, bad_sp
    li    t6, PATTERN(01, 81)
    bne   ra, t6, bad_ra
    la    sp, cursor
    REG_L sp, 0(sp)
    # a0: the result, unless the case has none to check.
    REG_L t6, 4*SZREG(sp)
    li    ra, ANY
    beq   t6, ra, 1f
    li    ra, 10
    bne   a0, t6, bad
1:  li    ra, 3
    li    t6, PATTERN(03, 83)
    bne   gp, t6, bad
    li    ra, 4
    li    t6, PATTERN(04, 84)
    bne   tp, t6, bad
    li    ra, 5
    li    t6, PATTERN(05, 85)
    bne   t0, t6, bad
    li    ra, 6
    li    t6, PATTERN(06, 86)
    bne   t1, t6, bad
    li    ra, 7
    li    t6, PATTERN(07, 87)
    bne   t2, t6, bad
    li    ra, 8
    li    t6, PATTERN(08, 88)
    bne   s0, t6, bad
    li    ra, 9
    li    t6, PATTERN(09, 89)
    bne   s1, t6, bad
    li    ra, 11
    REG_L t6, 2*SZREG(sp)
    bne   a1, t6, bad
    li    ra, 12
    REG_L t6, 3*SZREG(sp)
    bne   a2, t6, bad
    li    ra, 13
    li    t6, PATTERN(0d, 8d)
    bne   a3, t6, bad
    li    ra, 14
    li    t6, PATTERN(0e, 8e)
    bne   a4, t6, bad
    li    ra, 15
    li    t6, PATTERN(0f, 8f)
    bne   a5, t6, bad
    li    ra, 16
    li    t6, PATTERN(10, 90)
    bne   a6, t6, bad
    li    ra, 17
    REG_L t6, 0(sp)
    bne   a7, t6, bad
    li    ra, 18
    li    t6, PATTERN(12, 92)
    bne   s2, t6, bad
    li    ra, 19
    li    t6, PATTERN(13, 93)
    bne   s3, t6, bad
    li    ra, 20
    li    t6, PATTERN(14, 94)
    bne   s4, t6, bad
    li    ra, 21
    li    t6, PATTERN(15, 95)
    bne   s5, t6, bad
    li    ra, 22
    li    t6, PATTERN(16, 96)
    bne   s6, t6, bad
    li    ra, 23
    li    t6, PATTERN(17, 97)
    bne   s7, t6, bad
    li    ra, 24
    li    t6, PATTERN(18, 98)
    bne   s8, t6, bad
    li    ra, 25
    li    t6, PATTERN(19, 99)
    bne   s9, t6, bad
    li    ra, 26
    li    t6, PATTERN(1a, 9a)
    bne   s10, t6, bad
    li    ra, 27
    li    t6, PATTERN(1b, 9b)
    bne   s11, t6, bad
    li    ra, 28
    li    t6, PATTERN(1c, 9c)
    bne   t3, t6, bad
    li    ra, 29
    li    t6, PATTERN(1d, 9d)
    bne   t4, t6, bad
    li    ra, 30
    li    t6, PATTERN(1e, 9e)
    bne   t5, t6, bad
    # On to the next case.
    addi  sp, sp, CASE
    la    t6, cursor
    REG_S sp, 0(t6)
    j     next

done:
    li    a0, STDOUT
    la    a1, ok
    li    a2, OK_LEN
    call  write
    li    a0, 0
    call  exit

# t6 and sp disagree: sp is the one that changed if it isn't its pattern.
bad_t6_or_sp:
    li    t6, PATTERN(02, 82)
    bne   sp, t6, bad_sp
    li    ra, 31
    j     bad_case
bad_sp:
    li    ra, 2
    j     bad_case
bad_ra:
    li    ra, 1
bad_case:
    la    sp, cursor
    REG_L sp, 0(sp)
# ra is the register's number, sp the case: exit with 32 * index + number.
bad:
    la    t6, cases
    sub   sp, sp, t6            # index * CASE
    li    t6, CASE
    divu  sp, sp, t6
    slli  sp, sp, 5
    add   a0, sp, ra
    call  exit

    .section .rodata
# Each case: the call number, a0, a1, a2, then the result a0 must hold (or
# ANY). The arguments are patterns too where the call ignores them.
    .align RISCV_LGPTR
cases:
    RISCV_PTR SYS_getpid, PATTERN(0a, 8a), PATTERN(0b, 8b), PATTERN(0c, 8c), ANY
    RISCV_PTR SYS_yield, PATTERN(0a, 8a), PATTERN(0b, 8b), PATTERN(0c, 8c), 0
    RISCV_PTR SYS_sleep, 1, PATTERN(0b, 8b), PATTERN(0c, 8c), 0
    RISCV_PTR SYS_read, 5, PATTERN(0b, 8b), PATTERN(0c, 8c), NEG_EBADF
    RISCV_PTR SYS_UNKNOWN, PATTERN(0a, 8a), PATTERN(0b, 8b), PATTERN(0c, 8c), NEG_ENOSYS
    RISCV_PTR SYS_sbrk, 0, PATTERN(0b, 8b), PATTERN(0c, 8c), ANY
    RISCV_PTR SYS_uptime, PATTERN(0a, 8a), PATTERN(0b, 8b), PATTERN(0c, 8c), ANY
    RISCV_PTR SYS_write, STDOUT, msg, MSG_LEN, MSG_LEN
    RISCV_PTR -1
msg:
    .ascii "sysregs: write\n"
ok:
    .ascii "sysregs: every register but a0 kept\n"

    .data
# The case being run: in memory, since every register is in use.
    .align RISCV_LGPTR
cursor:
    RISCV_PTR 0
