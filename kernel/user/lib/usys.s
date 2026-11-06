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
    .equ SYS_socket, 27
    .equ SYS_bind, 28
    .equ SYS_listen, 29
    .equ SYS_accept, 30
    .equ SYS_connect, 31
    .equ SYS_send, 32
    .equ SYS_recv, 33
    .equ SYS_sendto, 34
    .equ SYS_recvfrom, 35
    .equ SYS_shutdown, 36
    .equ SYS_getsockname, 37
    .equ SYS_getpeername, 38
    .equ SYS_setsockopt, 39
    .equ SYS_harts, 40
    .equ SYS_thread_create, 41
    .equ SYS_thread_exit, 42
    .equ SYS_thread_join, 43
    .equ SYS_gettid, 44
    .equ SYS_futex_wait, 45
    .equ SYS_futex_wake, 46
    .equ SYS_fcntl, 47
    .equ SYS_mmap, 48
    .equ SYS_munmap, 49
    .equ SYS_mprotect, 50
    .equ SYS_memfd_create, 51
    .equ SYS_ftruncate, 52
    .equ SYS_ioctl, 53
    .equ SYS_poll, 54
    .equ SYS_socketpair, 55
    .equ SYS_sendmsg, 56
    .equ SYS_recvmsg, 57
    .equ SYS_stat, 58
    .equ SYS_getsockopt, 59

    .text

# void exit(int status): never returns.
    .globl exit
exit:
    li    a7, SYS_exit
    ecall
1:  j     1b

# ssize_t write(int fd, const void *buf, size_t len)
    .globl write
write:
    li    a7, SYS_write
    ecall
    ret

# ssize_t read(int fd, void *buf, size_t len)
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

# int spawn(const char *path, char *const argv[], char *const envp[])
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

# void *sbrk(long increment)
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

# off_t lseek(int fd, off_t offset, int whence)
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

# int exec(const char *path, char *const argv[], char *const envp[]): returns only on an error
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

# int socket(int domain, int type, int protocol)
    .globl socket
socket:
    li    a7, SYS_socket
    ecall
    ret

# int bind(int fd, const struct sockaddr_in *addr)
    .globl bind
bind:
    li    a7, SYS_bind
    ecall
    ret

# int listen(int fd, int backlog)
    .globl listen
listen:
    li    a7, SYS_listen
    ecall
    ret

# int accept(int fd, struct sockaddr_in *addr)
    .globl accept
accept:
    li    a7, SYS_accept
    ecall
    ret

# int connect(int fd, const struct sockaddr_in *addr)
    .globl connect
connect:
    li    a7, SYS_connect
    ecall
    ret

# ssize_t send(int fd, const void *buf, size_t len)
    .globl send
send:
    li    a7, SYS_send
    ecall
    ret

# ssize_t recv(int fd, void *buf, size_t len)
    .globl recv
recv:
    li    a7, SYS_recv
    ecall
    ret

# ssize_t sendto(int fd, const void *buf, size_t len, const struct sockaddr_in *to)
    .globl sendto
sendto:
    li    a7, SYS_sendto
    ecall
    ret

# ssize_t recvfrom(int fd, void *buf, size_t len, struct sockaddr_in *from)
    .globl recvfrom
recvfrom:
    li    a7, SYS_recvfrom
    ecall
    ret

# int shutdown(int fd, int how)
    .globl shutdown
shutdown:
    li    a7, SYS_shutdown
    ecall
    ret

# int getsockname(int fd, struct sockaddr_in *addr)
    .globl getsockname
getsockname:
    li    a7, SYS_getsockname
    ecall
    ret

# int getpeername(int fd, struct sockaddr_in *addr)
    .globl getpeername
getpeername:
    li    a7, SYS_getpeername
    ecall
    ret

# int setsockopt(int fd, int level, int option, const void *value, uint len)
    .globl setsockopt
setsockopt:
    li    a7, SYS_setsockopt
    ecall
    ret

# int getsockopt(int fd, int level, int option, void *value, uint *len)
    .globl getsockopt
getsockopt:
    li    a7, SYS_getsockopt
    ecall
    ret

# int harts(struct hartinfo *buf, uint max)
    .globl harts
harts:
    li    a7, SYS_harts
    ecall
    ret

# int thread_create(void (*entry)(void *), void *arg, void *stack): the raw
# call; the new thread starts at entry with a0 = arg, sp = stack and every
# other register zero, ra included, so entry must end with thread_exit.
# The library's thread_start (lib/thread.S) wraps it for C functions.
    .globl thread_create
thread_create:
    li    a7, SYS_thread_create
    ecall
    ret

# void thread_exit(int status)
    .globl thread_exit
thread_exit:
    li    a7, SYS_thread_exit
    ecall
    ret

# int thread_join(int tid, int *status)
    .globl thread_join
thread_join:
    li    a7, SYS_thread_join
    ecall
    ret

# int gettid(void)
    .globl gettid
gettid:
    li    a7, SYS_gettid
    ecall
    ret

# int futex_wait(int *addr, int val)
    .globl futex_wait
futex_wait:
    li    a7, SYS_futex_wait
    ecall
    ret

# int futex_wake(int *addr, int n)
    .globl futex_wake
futex_wake:
    li    a7, SYS_futex_wake
    ecall
    ret

# int fcntl(int fd, int cmd, int arg)
    .globl fcntl
fcntl:
    li    a7, SYS_fcntl
    ecall
    ret

# void *mmap(void *addr, size_t len, int prot, int flags, int fd, off_t offset)
    .globl mmap
mmap:
    li    a7, SYS_mmap
    ecall
    ret

# int munmap(void *addr, size_t len)
    .globl munmap
munmap:
    li    a7, SYS_munmap
    ecall
    ret

# int mprotect(void *addr, size_t len, int prot)
    .globl mprotect
mprotect:
    li    a7, SYS_mprotect
    ecall
    ret

# int memfd_create(const char *name, uint flags)
    .globl memfd_create
memfd_create:
    li    a7, SYS_memfd_create
    ecall
    ret

# int ftruncate(int fd, off_t length)
    .globl ftruncate
ftruncate:
    li    a7, SYS_ftruncate
    ecall
    ret

# int ioctl(int fd, unsigned long request, ...): the third argument, a
# pointer or a value, in a2 as a variadic call passes it.
    .globl ioctl
ioctl:
    li    a7, SYS_ioctl
    ecall
    ret

# int poll(struct pollfd *fds, unsigned long nfds, int timeout)
    .globl poll
poll:
    li    a7, SYS_poll
    ecall
    ret

# int socketpair(int domain, int type, int protocol, int *fds)
    .globl socketpair
socketpair:
    li    a7, SYS_socketpair
    ecall
    ret

# ssize_t sendmsg(int fd, const struct msghdr *msg, int flags)
    .globl sendmsg
sendmsg:
    li    a7, SYS_sendmsg
    ecall
    ret

# ssize_t recvmsg(int fd, struct msghdr *msg, int flags)
    .globl recvmsg
recvmsg:
    li    a7, SYS_recvmsg
    ecall
    ret

# int stat(const char *path, struct stat *st)
    .globl stat
stat:
    li    a7, SYS_stat
    ecall
    ret

# long syscall(long number, long a0, long a1, long a2): any call by number, for
# the tests that make calls the stubs above can't (an unknown number).
    .globl syscall
syscall:
    mv    a7, a0
    mv    a0, a1
    mv    a1, a2
    mv    a2, a3
    ecall
    ret
