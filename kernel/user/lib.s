# The runtime every user program in kernel/user is linked with (the
# programs are listed in kernel/README.md).
#
# Calls into the kernel use the provisional ecalls (spec section 10, "Kernel
# processes"; kernel/include/syscall.h): the number in a7, the arguments in
# a0-a1, the result in a0. The system call ABI replaces them later (#21).

    .equ ECALL_EXIT, 1
    .equ ECALL_DEBUG_WRITE, 2

    .text

# void exit(int status): ends the process. Never returns.
    .globl exit
exit:
    li    a7, ECALL_EXIT
    ecall
1:  j     1b

# int write(const char *buf, u32 len): prints len bytes on the console.
# Returns len, or -1 if any of the bytes is not readable user memory.
    .globl write
write:
    li    a7, ECALL_DEBUG_WRITE
    ecall
    ret

# u32 strlen(const char *s)
    .globl strlen
strlen:
    mv    t0, a0
1:  lbu   t1, 0(t0)
    beqz  t1, 2f
    addi  t0, t0, 1
    j     1b
2:  sub   a0, t0, a0
    ret

# int puts(const char *s): write(s, strlen(s)).
    .globl puts
puts:
    addi  sp, sp, -16
    sw    ra, 12(sp)
    sw    a0, 8(sp)
    call  strlen
    mv    a1, a0
    lw    a0, 8(sp)
    call  write
    lw    ra, 12(sp)
    addi  sp, sp, 16
    ret

# void delay(u32 n): spins for n iterations of two instructions each.
    .globl delay
delay:
1:  addi  a0, a0, -1
    bnez  a0, 1b
    ret

# void count(const char *name, u32 len): prints "<name> 1" to "<name> 4", one
# line per call to write, with DELAY iterations (about half a timer tick)
# after each, then exits with status 0. The line is built in a buffer on the
# stack; len must be at most 32.
    .equ COUNT, 4
    .equ DELAY, 250000
    .equ BUF, 48

    .globl count
count:
    addi  sp, sp, -BUF
    mv    s0, a0                # name
    mv    s1, a1                # its length
    li    t0, 0
1:  bgeu  t0, s1, 2f            # copy the name to the buffer
    add   t1, s0, t0
    lbu   t2, 0(t1)
    add   t1, sp, t0
    sb    t2, 0(t1)
    addi  t0, t0, 1
    j     1b
2:  add   s3, sp, s1            # then " N\n" after it
    li    t0, 32                # ' '
    sb    t0, 0(s3)
    li    t0, 10                # '\n'
    sb    t0, 2(s3)
    li    s2, 1
3:  addi  t0, s2, 48            # '0' + N
    sb    t0, 1(s3)
    mv    a0, sp
    addi  a1, s1, 3
    call  write
    li    a0, DELAY
    call  delay
    addi  s2, s2, 1
    li    t0, COUNT
    bleu  s2, t0, 3b
    li    a0, 0
    call  exit
