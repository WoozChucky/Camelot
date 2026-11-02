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
    .equ SYS_open, 11
    .equ SYS_close, 12
    .equ SYS_lseek, 13
    .equ SYS_fstat, 14
    .equ SYS_mkdir, 15
    .equ SYS_unlink, 16
    .equ SYS_link, 17
    .equ SYS_chdir, 18
    .equ SYS_dup, 19
    .equ SYS_pipe, 20
    .equ SYS_exec, 21
    .equ SYS_kill, 22
    .equ SYS_procs, 23
    .equ SYS_netinfo, 24
    .equ SYS_netconf, 25
    .equ SYS_ping, 26

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

# int spawn(const char *path, char *const argv[])
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

# int open(const char *path, int flags)
    .globl open
open:
    li    a7, SYS_open
    ecall
    ret

# int close(int fd)
    .globl close
close:
    li    a7, SYS_close
    ecall
    ret

# int lseek(int fd, int offset, int whence)
    .globl lseek
lseek:
    li    a7, SYS_lseek
    ecall
    ret

# int fstat(int fd, struct stat *st)
    .globl fstat
fstat:
    li    a7, SYS_fstat
    ecall
    ret

# int mkdir(const char *path)
    .globl mkdir
mkdir:
    li    a7, SYS_mkdir
    ecall
    ret

# int unlink(const char *path)
    .globl unlink
unlink:
    li    a7, SYS_unlink
    ecall
    ret

# int link(const char *old, const char *new)
    .globl link
link:
    li    a7, SYS_link
    ecall
    ret

# int chdir(const char *path)
    .globl chdir
chdir:
    li    a7, SYS_chdir
    ecall
    ret

# int dup(int fd)
    .globl dup
dup:
    li    a7, SYS_dup
    ecall
    ret

# int pipe(int *fds)
    .globl pipe
pipe:
    li    a7, SYS_pipe
    ecall
    ret

# int exec(const char *path, char *const argv[]): returns only on an error
    .globl exec
exec:
    li    a7, SYS_exec
    ecall
    ret

# int kill(int pid)
    .globl kill
kill:
    li    a7, SYS_kill
    ecall
    ret

# int procs(struct procinfo *buf, uint max)
    .globl procs
procs:
    li    a7, SYS_procs
    ecall
    ret

# int netinfo(struct netinfo *info)
    .globl netinfo
netinfo:
    li    a7, SYS_netinfo
    ecall
    ret

# int netconf(uint addr, uint mask, uint gateway)
    .globl netconf
netconf:
    li    a7, SYS_netconf
    ecall
    ret

# int ping(uint addr, uint seq, struct ping_reply *reply)
    .globl ping
ping:
    li    a7, SYS_ping
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
