# The system call stubs (docs/camelot-syscalls.md). Each one puts its call
# number in a7 and executes ecall; the arguments are already in a0-a5, where
# the C calling convention put them, and the result comes back in a0, a
# negative error number on failure. The kernel preserves every other
# register, so the stubs need no stack frame.
#
# The numbers are kernel/include/syscall.h's SYS_*: a test checks they agree.

    .equ SYS_exit, 1
    .equ SYS_write, 2
    .equ SYS_read, 3
    .equ SYS_getpid, 4
    .equ SYS_yield, 5
    .equ SYS_sleep, 6
    .equ SYS_spawn, 7
    .equ SYS_wait, 8
    .equ SYS_sbrk, 9
    .equ SYS_uptime, 10

    .text

# void exit(int status): never returns.
    .globl exit
exit:
    li    a7, SYS_exit
    ecall
1:  j     1b

# int write(int fd, const void *buf, uint len)
    .globl write
write:
    li    a7, SYS_write
    ecall
    ret

# int read(int fd, void *buf, uint len)
    .globl read
read:
    li    a7, SYS_read
    ecall
    ret

# int getpid(void)
    .globl getpid
getpid:
    li    a7, SYS_getpid
    ecall
    ret

# int yield(void)
    .globl yield
yield:
    li    a7, SYS_yield
    ecall
    ret

# int sleep(uint ticks)
    .globl sleep
sleep:
    li    a7, SYS_sleep
    ecall
    ret

# int spawn(const char *name, char *const argv[])
    .globl spawn
spawn:
    li    a7, SYS_spawn
    ecall
    ret

# int wait(int pid, int *status)
    .globl wait
wait:
    li    a7, SYS_wait
    ecall
    ret

# int sbrk(int increment)
    .globl sbrk
sbrk:
    li    a7, SYS_sbrk
    ecall
    ret

# uint uptime(void)
    .globl uptime
uptime:
    li    a7, SYS_uptime
    ecall
    ret

# int syscall(int number, uint a0, uint a1, uint a2): any call by number, for
# the tests that make calls the stubs above can't (an unknown number).
    .globl syscall
syscall:
    mv    a7, a0
    mv    a0, a1
    mv    a1, a2
    mv    a2, a3
    ecall
    ret
