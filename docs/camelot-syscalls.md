# Camelot system calls

The interface between Camelot's user programs and its kernel: how a program
starts, how it calls the kernel, what each call does and how it fails. The
kernel side is `kernel/syscall.c` and `kernel/sysfile.c` (with
`kernel/proc.c`, `kernel/exec.c`, `kernel/file.c`, `kernel/pipe.c`,
`kernel/console.c`, `kernel/fs.c` and `kernel/vm.c`; the network's calls
in `kernel/net/net.c` and the sockets' in `kernel/net/socket.c`,
`docs/camelot-net.md`), the numbers are in
`kernel/include/syscall.h`; the user side is `kernel/user/lib/usys.s` (one stub
per call), `kernel/user/user.h` (their C declarations, the error numbers,
`open`'s flags and `struct stat`) and `kernel/user/crt0.S` (the start-up
code). How processes, address spaces, the scheduler and open files work
inside the kernel is in `docs/excalibur-spec.md`, section 10, "Kernel
processes" and "Kernel files"; the filesystem's format is in
`docs/camelot-fs.md`.

The numbers in the tables below are checked against `syscall.h`, `usys.s`
and `user.h` by `cargo test` (`crates/xtask/tests/kernel.rs`): change all of
them together.

Camelot is RV64, and a program LP64 (`int` 32 bits, `long` and pointers
64), as riscv64 Linux's are; every type below is the C type it says:
`size_t`, `ssize_t`, `off_t` and `time_t` are a `long` wide, 64 bits.
"Structures" gives each structure's size.

## Calling convention

The Linux RISC-V convention, which is also the C calling convention's, so a
stub is two instructions:

- The program executes `ecall` in U-mode with the call number in `a7` and
  the arguments in `a0`-`a5` (`mmap` takes six, the most). An
  argument is a register, read at its C type: an `int` is the register's
  low 32 bits (which the calling convention keeps sign-extended on RV64), a
  pointer, a `size_t` or an `off_t` all of it. The number is all of `a7`, a
  `long`: on RV64, 2^32 + 4 is no call, not `getpid`.
- The result comes back in `a0`: a `long`, sign-extended to the register's
  width, so an error is negative to a 64-bit program too (-14 is
  0xffff_ffff_ffff_fff2, not 0x0000_0000_ffff_fff2). A value from -4095 to
  -1 is an error: minus one of the error numbers below. Every other value
  is a success: a count, a descriptor, or for `sbrk` and `mmap` an address
  (user addresses are below `USER_TOP`, so never in that range).
- Every register but `a0` is preserved, `a1`-`a7` and the temporaries
  included, and `pc` continues after the `ecall`.

```
    li    a7, 2        # SYS_write
    ecall              # a0 = fd, a1 = buf, a2 = len
    bltz  a0, error    # a0 = bytes written, or -errno
```

An unknown call number returns `-ENOSYS` and changes nothing else.

**Pointers.** A pointer argument is an address in the caller's own address
space. The kernel never dereferences it: it checks every byte of the range
against the process's page table (each page must be below `USER_TOP`,
mapped, and mapped with U and the access the call needs: R to read from it,
W to write to it) and then copies through the physical pages, which the
kernel reaches through its own mapping of them (on RV64 the direct map, as
Linux's kernel reaches user pages). A range that
fails the check, such as a null pointer, a kernel address (on RV64 any
address from `USER_TOP` up: the kernel's half of the address space, and the
addresses between the halves, which are not canonical), the guard page
under the stack, the stack below `sp` (not grown to yet: see "Starting a
program"), memory past the end of the heap, read-only memory for a
buffer the kernel writes, or a length that runs past `USER_TOP` or wraps
around, makes the call return `-EFAULT` having done nothing: no bytes are
written to the console, a file or a pipe, no input is consumed, no file is
made or opened, no process is started. A bad pointer never faults the
kernel or kills the caller. An empty range (a length of 0) is never a
fault, whatever the pointer. A string (a path, an argument) must be readable
up to its NUL. The checks come in a fixed order: a bad descriptor
(`-EBADF`) first, then a bad pointer (`-EFAULT`), then everything else, so
a call that takes a path fails with `-EFAULT` for a bad one whether or not
there is a filesystem to look it up in.

## Structures

What the calls store and read, as `user.h` and `kernel/include/syscall.h`
declare them, in bytes, LP64's: a `long`, a `size_t` or an `off_t` is 8
bytes, 8-byte aligned, and the fixed-width fields are as their Linux
counterparts have them. The test that checks the table compiles both
headers and checks each size.

| Structure | Bytes | What |
|-----------|-------|------|
| `struct stat` | 24 | `fstat`'s: `size` is an `off_t` |
| `struct dirent` | 16 | a directory's entries, as `read` gives them |
| `struct procinfo` | 40 | `procs`': `size` is a `size_t` |
| `struct hartinfo` | 16 | `harts`' |
| `struct net_stats` | 108 | the network's counters |
| `struct netinfo` | 132 | `netinfo`'s |
| `struct ping_reply` | 16 | `ping`'s |
| `struct timeval` | 16 | `SO_RCVTIMEO`'s: two `long`s, `tv_sec` a `time_t` |
| `struct timespec` | 16 | `clock_gettime`'s: two `long`s, `tv_sec` a `time_t`, then `tv_nsec` |
| `struct sysinfo` | 112 | `sysinfo`'s, Linux's: `long`s and `unsigned long`s, two `unsigned short`s and an `unsigned int` |
| `struct sockaddr_in` | 16 | an IPv4 address and port |
| `struct fb_var_screeninfo` | 160 | the framebuffer's mode and the row shown ("The framebuffer") |
| `struct fb_fix_screeninfo` | 80 | the framebuffer's buffer |
| `struct fb_bitfield` | 12 | where a colour's bits are in a pixel |
| `struct input_event` | 24 | an input device's event ("Input devices"): a `struct timeval`, then the type, code and value |
| `struct input_id` | 8 | `EVIOCGID`'s |
| `struct input_absinfo` | 24 | `EVIOCGABS`'s |
| `struct pollfd` | 8 | `poll`'s ("Polling"): an `int` descriptor, then two `short`s, `events` and `revents` |
| `struct sockaddr_un` | 110 | a local socket's address ("Local sockets"): the family, then a path of 108 bytes |
| `struct iovec` | 16 | a buffer ("Messages"): a pointer and a `size_t` |
| `struct msghdr` | 56 | `sendmsg`'s and `recvmsg`'s message: an address, buffers, control messages and flags |
| `struct cmsghdr` | 16 | a control message's header: a `size_t` length, then two `int`s, the level and the type |
| `struct termios` | 36 | a terminal's modes ("Terminals"): four `unsigned int` flags, the line discipline's number, then 19 control characters, Linux's for `TCGETS` |
| `struct winsize` | 8 | a terminal's size: four `unsigned short`s, rows, columns and two in pixels |
| `struct ucred` | 12 | `SO_PEERCRED`'s: the peer's pid, then its uid and gid, `unsigned int`s, 0 |

## Calls

| Number | Name | Arguments | Returns |
|---|---|---|---|
| 1 | `exit` | `int status` | does not return |
| 2 | `write` | `int fd, const void *buf, size_t len` | bytes written; `-EBADF`, `-EFAULT`, `-EPIPE`, `-ENOSPC`, `-EFBIG`, `-EINVAL`, `-ENOMEM`, `-EIO`, `-EINTR` |
| 3 | `read` | `int fd, void *buf, size_t len` | bytes read, 0 at the end of the input; `-EBADF`, `-EFAULT`, `-ENOMEM`, `-EIO`, `-EINTR` |
| 4 | `getpid` | | the caller's pid |
| 5 | `yield` | | 0 |
| 6 | `sleep` | `uint ticks` | 0 |
| 7 | `spawn` | `const char *path, char *const argv[], char *const envp[]` | the child's pid; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EACCES`, `-E2BIG`, `-ENOEXEC`, `-EAGAIN`, `-ENOMEM`, `-EFAULT` |
| 8 | `wait` | `int pid, int *status` | the pid of the child collected; `-ECHILD`, `-EFAULT` |
| 9 | `sbrk` | `long increment` | the old end of the heap, a pointer; `-ENOMEM`, `-EINVAL` |
| 10 | `uptime` | | timer ticks since boot |
| 11 | `open` | `const char *path, int flags` | a new descriptor; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EISDIR`, `-ENODEV`, `-ENXIO`, `-EIO`, `-EROFS`, `-EOVERFLOW`, `-ENOSPC`, `-ENOMEM`, `-EINVAL`, `-EMFILE`, `-ENFILE`, `-EFAULT` |
| 12 | `close` | `int fd` | 0; `-EBADF` |
| 13 | `lseek` | `int fd, off_t offset, int whence` | the new offset; `-EBADF`, `-ESPIPE`, `-EINVAL` |
| 14 | `fstat` | `int fd, struct stat *st` | 0; `-EBADF`, `-EFAULT` |
| 15 | `mkdir` | `const char *path` | 0; `-EEXIST`, `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-ENOSPC`, `-EROFS`, `-EFAULT` |
| 16 | `unlink` | `const char *path` | 0; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EINVAL`, `-ENOTEMPTY`, `-EROFS`, `-EFAULT` |
| 17 | `link` | `const char *old, const char *new` | 0; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EPERM`, `-EEXIST`, `-ENOSPC`, `-EROFS`, `-EFAULT` |
| 18 | `chdir` | `const char *path` | 0; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EFAULT` |
| 19 | `dup` | `int fd` | the new descriptor; `-EBADF`, `-EMFILE` |
| 20 | `pipe` | `int *fds` | 0; `-EMFILE`, `-ENFILE`, `-ENOMEM`, `-EFAULT` |
| 21 | `exec` | `const char *path, char *const argv[], char *const envp[]` | does not return; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EACCES`, `-E2BIG`, `-ENOEXEC`, `-ENOMEM`, `-EFAULT` |
| 22 | `kill` | `int pid` | 0; `-ESRCH`, `-EPERM` |
| 23 | `procs` | `struct procinfo *buf, uint max` | the number of processes; `-EFAULT` |
| 24 | `netinfo` | `struct netinfo *info` | 0; `-ENODEV`, `-EFAULT` |
| 25 | `netconf` | `uint addr, uint mask, uint gateway` | 0; `-ENODEV`, `-EINVAL` |
| 26 | `ping` | `uint addr, uint seq, struct ping_reply *reply` | 0; `-ETIMEDOUT`, `-EHOSTUNREACH`, `-ENETUNREACH`, `-ENOBUFS`, `-ENODEV`, `-EINVAL`, `-EFAULT` |
| 27 | `socket` | `int domain, int type, int protocol` | a new descriptor; `-EAFNOSUPPORT`, `-EPROTONOSUPPORT`, `-ESOCKTNOSUPPORT`, `-EINVAL`, `-ENODEV`, `-EMFILE`, `-ENFILE`, `-ENOBUFS`, `-ENOMEM` |
| 28 | `bind` | `int fd, const void *addr` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EAFNOSUPPORT`, `-EADDRNOTAVAIL`, `-EADDRINUSE`, `-EINVAL`, `-ENOBUFS`, `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-ENOSPC`, `-EROFS` |
| 29 | `listen` | `int fd, int backlog` | 0; `-EBADF`, `-ENOTSOCK`, `-EOPNOTSUPP`, `-EINVAL` |
| 30 | `accept` | `int fd, void *addr` | a new descriptor; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EOPNOTSUPP`, `-EINVAL`, `-EMFILE`, `-ENFILE`, `-EAGAIN`, `-EINTR` |
| 31 | `connect` | `int fd, const void *addr` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EAFNOSUPPORT`, `-EINVAL`, `-EISCONN`, `-ENETUNREACH`, `-ECONNREFUSED`, `-ECONNRESET`, `-ETIMEDOUT`, `-ENOBUFS`, `-EADDRINUSE`, `-EINTR`, `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-ENOMEM` |
| 32 | `send` | `int fd, const void *buf, size_t len` | bytes sent; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EPIPE`, `-ENOTCONN`, `-ECONNRESET`, `-ETIMEDOUT`, `-EDESTADDRREQ`, `-EMSGSIZE`, `-ENETUNREACH`, `-ENOBUFS`, `-EINTR` |
| 33 | `recv` | `int fd, void *buf, size_t len` | bytes received, 0 at the end; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-ENOTCONN`, `-ECONNRESET`, `-ETIMEDOUT`, `-EINVAL`, `-EAGAIN`, `-EINTR` |
| 34 | `sendto` | `int fd, const void *buf, size_t len, const void *to` | bytes sent; `send`'s errors, `-EFAULT` also for `to`, `-EAFNOSUPPORT`, `-EINVAL`, `-EADDRINUSE`, `-EISCONN`, `-EOPNOTSUPP` |
| 35 | `recvfrom` | `int fd, void *buf, size_t len, void *from` | bytes received; `recv`'s errors (`-EFAULT` also for `from`) |
| 36 | `shutdown` | `int fd, int how` | 0; `-EBADF`, `-ENOTSOCK`, `-EINVAL`, `-ENOTCONN`, `-ECONNREFUSED`, `-ECONNRESET`, `-ETIMEDOUT` |
| 37 | `getsockname` | `int fd, void *addr` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT` |
| 38 | `getpeername` | `int fd, void *addr` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-ENOTCONN` |
| 39 | `setsockopt` | `int fd, int level, int option, const void *value, uint len` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EINVAL`, `-ENOPROTOOPT` |
| 40 | `harts` | `struct hartinfo *buf, uint max` | the number of harts; `-EFAULT` |
| 41 | `thread_create` | `void (*entry)(void *), void *arg, void *stack` | the new thread's tid; `-EFAULT`, `-EINVAL`, `-EAGAIN`, `-ENOMEM` |
| 42 | `thread_exit` | `int status` | does not return |
| 43 | `thread_join` | `int tid, int *status` | 0; `-ESRCH`, `-EINVAL`, `-EINTR`, `-EFAULT` |
| 44 | `gettid` | | the caller's thread id |
| 45 | `futex_wait` | `int *addr, int val` | 0; `-EAGAIN`, `-EINVAL`, `-EINTR`, `-EFAULT` |
| 46 | `futex_wake` | `int *addr, int n` | the number of threads woken; `-EINVAL`, `-EFAULT` |
| 47 | `fcntl` | `int fd, int cmd, int arg` | the descriptor's flags, a new descriptor, the seals, or 0; `-EBADF`, `-EINVAL`, `-EMFILE`, `-EPERM` |
| 48 | `mmap` | `void *addr, size_t len, int prot, int flags, int fd, off_t offset` | the mapping's address, a pointer; `-EINVAL`, `-ENOMEM`, `-EEXIST`, `-EBADF`, `-EACCES`, `-ENODEV`, `-EOVERFLOW`, `-EOPNOTSUPP` |
| 49 | `munmap` | `void *addr, size_t len` | 0; `-EINVAL`, `-ENOMEM` |
| 50 | `mprotect` | `void *addr, size_t len, int prot` | 0; `-EINVAL`, `-ENOMEM`, `-EACCES` |
| 51 | `memfd_create` | `const char *name, uint flags` | a new descriptor; `-EINVAL`, `-EFAULT`, `-EMFILE`, `-ENFILE` |
| 52 | `ftruncate` | `int fd, off_t length` | 0; `-EBADF`, `-EINVAL`, `-EFBIG`, `-EPERM` |
| 53 | `ioctl` | `int fd, unsigned long request, ...` | the request's result; `-EBADF`, `-ENOTTY`, `-EINVAL`, `-EFAULT`, `-EBUSY`, `-ENOMEM`, `-ENODEV`, `-EINTR` |
| 54 | `poll` | `struct pollfd *fds, unsigned long nfds, int timeout` | the descriptors with events, 0 if the time ran out; `-EINVAL`, `-EFAULT`, `-EINTR` |
| 55 | `socketpair` | `int domain, int type, int protocol, int *fds` | 0; `-EAFNOSUPPORT`, `-EOPNOTSUPP`, `-EPROTONOSUPPORT`, `-ESOCKTNOSUPPORT`, `-EMFILE`, `-ENFILE`, `-ENOMEM`, `-EFAULT` |
| 56 | `sendmsg` | `int fd, const struct msghdr *msg, int flags` | bytes sent; `send`'s and `sendto`'s errors, `-EMSGSIZE`, `-EINVAL`, `-EBADF` (a descriptor passed), `-EOPNOTSUPP`, `-EAGAIN`, `-ENOBUFS`, `-EFAULT` |
| 57 | `recvmsg` | `int fd, struct msghdr *msg, int flags` | bytes received, 0 at the end; `recv`'s errors, `-EMSGSIZE`, `-EOPNOTSUPP`, `-ENOBUFS`, `-EFAULT` |
| 58 | `stat` | `const char *path, struct stat *st` | 0; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EFAULT` |
| 59 | `getsockopt` | `int fd, int level, int option, void *value, uint *len` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-ENOPROTOOPT` |
| 60 | `clock_gettime` | `int clock, struct timespec *ts` | 0; `-EINVAL`, `-EFAULT` |
| 61 | `sysinfo` | `struct sysinfo *info` | 0; `-EFAULT` |

**exit(status)** ends the caller, every one of its threads ("Threads"
below): the others are stopped wherever they are, as by `kill`, and the
process ends once they have. Its exit status is kept for its parent's
`wait`; the slot and every page the process held are freed once the parent
has collected it (or at once, if it has no parent any more). Its descriptors
are closed and its working directory let go of first, as `close` would
(which may free a file it held open after its last name was removed). Its
children are orphaned: they keep running, and become init's children, as
Linux gives them to its init, which collects them as they end (its `wait`
gets them); when the first process is not init (the shell, on an image
without `/etc/rc`, or init once it has `exec`'d another program), nobody
collects them, and the kernel frees them as they end. When the first
process (init, or the shell) exits, the kernel runs until every remaining
process has ended and then stops the machine with its status as the
exit code (its low 8 bits, so `exit -2` gives 0xFE). Init kills every other
process first (its `/etc/rc`, excalibur-spec section 10, "Init and the
shell"); a shell that is the first process does not, and a process blocked
reading the console keeps the machine running until it gets a line or the
end of the input (`sh &`, then `exit` in the first shell, leaves the second
one reading). Note that 0xFE is also the code of a kernel panic:
the exit code alone can't tell `exit -2` from a panic, the console output
can. Returning from `main` calls `exit` with the return value (`crt0.S`).

A process that faults in U-mode (any exception but `ecall`: a page fault, an
illegal instruction such as `xcal.halt`, a misaligned access) is killed
instead: the kernel prints a report on the console (section 10, "Kernel
processes") and the process ends as if it had called `exit(-1)`.

**write(fd, buf, len)** writes the `len` bytes at `buf` to the open file
behind `fd` ("File descriptors" below) and returns how many it wrote: `len`,
unless the disk filled up or a pipe lost its readers part way, when it is
the bytes written until then. `fd` must be open for writing, or it is
`-EBADF`; all of `buf` must be readable, or nothing is written (`-EFAULT`).
`len` = 0 writes nothing and returns 0 (except to a socket, where `write`
is `send`: on UDP that is an empty datagram). To a file on the disk, the bytes go
at the descriptor's offset, which moves past them, and the file grows if they
run past its end; a write that can write nothing fails with what stopped it:
`-ENOSPC` (the disk is full), `-EFBIG` (the file would pass the largest the
kernel writes: ext4's, 2^32 - 1 blocks), `-EINVAL` (the offset is past the end of the
file, which another descriptor has emptied meanwhile), `-ENOMEM` (no page
free for copying through). To the console, the bytes are printed. To a
pseudo-terminal, they go to the other end ("Terminals"), a slave's waiting
for room; `-EIO` once that end is gone, `-EINTR` if the process is killed
while it waits. To a pipe, they wait in it for a reader ("Pipes");
`-EPIPE` if no read end is open any more.

**read(fd, buf, len)** reads up to `len` bytes from the open file behind
`fd` into `buf` and returns how many it read; 0 means the end of the input.
`fd` must be open for reading, or it is `-EBADF`; all of `buf` must be
writable, or it is `-EFAULT` and nothing is read. `len` = 0 returns 0 without
waiting (except from a socket, where `read` is `recv`: on UDP it takes the
next datagram). From a file on the disk, the bytes come from the descriptor's
offset, which moves past them: `len` of them, fewer at the end of the file,
0 at or past it. A directory reads the same way, as its array of 16-byte
entries (`struct dirent` in `user.h`, the format in `docs/camelot-fs.md`).
From a terminal, the console or a pseudo-terminal's slave, a read waits as
the terminal's mode says, for a whole line by default ("Terminals"); from a
pseudo-terminal's master, for what the slave wrote, `-EIO` once the slave is
closed; `-EINTR` if the process is killed meanwhile. From a pipe, it waits
while the pipe is empty and a write end is open, then returns what is
there, up to `len` bytes; 0 once it is empty and no write end is left.

**getpid()** returns the caller's pid. Pids count from 1 (the shell) and are
never reused.

**yield()** gives the CPU to the other runnable processes, if any, and
returns 0 when the caller's turn comes again. A process is also preempted at
every timer tick (10 Hz), so yielding is only ever a courtesy.

**sleep(ticks)** returns 0 after at least `ticks` timer ticks (10 a second at
the nominal clock) have passed, without using the CPU meanwhile. `sleep(0)`
returns at once.

**spawn(path, argv, envp)** starts the program at `path` ("Running programs"
below) as a new process, a child of the caller, and returns its pid. From
then on both take turns on the CPU with the other runnable processes. The
child gets copies of the caller's descriptors 0, 1 and 2 (sharing their
open files, offsets included; a closed one stays closed, and so does one
that closes on exec, since `spawn` stands for a fork and an exec, as
`posix_spawn` does) and no other, and the caller's working directory. Its name, in the kernel's messages, is the
last name of `path`. `argv` is the child's argument list, an array of string
pointers ending with a null pointer, `argv[0]` conventionally the program's
name; a null `argv` stands for `{ path, NULL }`. `envp` is its environment,
as `execve` takes it: an array of pointers to `NAME=value` strings ending
with a null pointer (the kernel copies any string, `=` or not: what a
`NAME=value` is, is the C library's business, `getenv`'s), which programs
usually pass on from their own (`environ`, "The user library"); a null
`envp` is an empty environment, as Linux takes it. The kernel copies the
strings of both onto the child's stack ("Starting a program" below), so the
caller's arrays can go as soon as `spawn` returns. Both are read as
Linux's `execve` reads them (`do_execveat_common` in `fs/exec.c`), in two
passes: first it counts argv's pointers, then envp's (`count`: an
unreadable pointer is `-EFAULT`, more than `MAXARG` of them `-E2BIG`),
before any string is looked at; then it copies the strings, envp's
before argv's, each array's from its last string to its first
(`copy_strings`). Of two errors, the call's is the first one found in
that order: an unreadable argv string with 17 environment strings is
`-E2BIG`, an unreadable argv string with an environment string too long
to fit is `-E2BIG` too, and arguments over `ARG_MAX` with an unreadable
envp are `-EFAULT`. Errors:

- `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`: the path names nothing ("Paths");
- `-EACCES`: it names a directory or a device;
- `-E2BIG`: more than `MAXARG` (16) arguments, or more than `MAXARG`
  environment strings (Linux's `MAX_ARG_STRINGS` bounds each array alike),
  or the strings of both together, with their NULs, take more than
  `ARG_MAX` (1024) bytes (POSIX's `ARG_MAX` counts the environment in; so
  no one string is longer than that, Linux's `MAX_ARG_STRLEN`; Linux also
  counts the pointers against its limit, where Camelot bounds them with
  `MAXARG`);
- `-ENOEXEC`: the file is not a valid executable (the kernel also prints why
  on the console: `spawn: PATH: REASON`);
- `-EAGAIN`: every one of the 8 process slots is taken (zombies count until
  they are collected; init, on a root image, holds one), or every one of
  the 16 thread slots;
- `-ENOMEM`: not enough free memory;
- `-EFAULT`: `path`, `argv`, `envp` or one of their strings is not
  readable (a path longer than `MAXPATH` is still read to its NUL).

**wait(pid, status)** collects a child that has ended and returns its pid:
the child `pid`, or any child if `pid` is -1. If none has ended yet, it
sleeps until one does. Unless `status` is null, the child's exit status is
stored there (-1 for a killed one). A collected child is gone: its slot is
free and a second `wait` for it is `-ECHILD`. Errors: `-ECHILD` when the
caller has no child (no child `pid`) left to wait for, at once rather than
after sleeping; `-EFAULT` when `status` is not writable, checked before
anything else.

**sbrk(increment)** moves the end of the caller's heap by `increment` bytes
(a `long`) and returns the old end, a pointer, so `sbrk(0)` returns the
current one; an error is minus its number as a pointer (`(long)p < 0`). The heap
starts empty, on the page after the program's last segment, and grows up
towards the stack: new pages are mapped R+W and read zero, including bytes
given back by an earlier shrink and grown again. A negative `increment`
shrinks it, unmapping and freeing the pages no longer used. Errors:
`-ENOMEM` when the heap would reach the guard page under the stack's limit
(`USER_LOAD_TOP`: 0x7FFF_FF7F_F000 in Sv48, 0x3F_FF7F_F000 in Sv39), when it
would map more new pages than the whole of RAM has (those from the old
end's page boundary to the new end's: refused at once, as Linux's default
overcommit check refuses it), or when memory runs out
(nothing changes), `-EINVAL` when it would shrink below its start.
A program whose segments end above 4 GiB has its heap there, and `sbrk`
returns those addresses whole.

**uptime()** returns the timer ticks since boot (10 a second at the nominal
clock), as an unsigned number.

**clock_gettime(clock, ts)** stores the time of `clock` at `ts`, a
`struct timespec` (16 bytes: seconds, then nanoseconds, below 10^9), as
Linux's: `CLOCK_MONOTONIC` (1) the time since boot, counted by `time`
(the CLINT's `mtime`) at the timebase, to the nanosecond the timebase
allows, and `CLOCK_REALTIME` (0) the date, the time since 1970-01-01
00:00:00 UTC. The kernel reads the real-time clock once, at boot
(`docs/excalibur-spec.md` section 9.9; it prints `rtc: 2026-01-01 00:00
UTC`), and the date runs on the monotonic clock from there, as Linux
keeps its system time; without a real-time clock (`rtc: none`) the date
is the time since boot, from 1970. Grail starts the clock at the host's
time, or a test's machine at a fixed date (`--rtc`). Another clock is
`-EINVAL`; `ts` not 16 writable bytes `-EFAULT`. There is no
`clock_settime`. The C library's `time` and `gmtime_r` (UTC only: there
are no time zones) are built on it, and `date` prints it.

**sysinfo(info)** stores Linux's `struct sysinfo` (112 bytes, `user.h`) at
`info`, with what Camelot has: `uptime`, the seconds since boot;
`totalram`, the memory the kernel's page allocator manages (RAM less the
kernel's image, the firmware's and the device tree's), and `freeram`, what
of it is free, both in bytes (`mem_unit` is 1); and `procs`, the number of
processes, as `procs(NULL, 0)` counts them. The load averages, `sharedram`,
`bufferram`, the swap and the high memory are 0. `-EFAULT` if `info` is
not 112 writable bytes.

**open(path, flags)** opens the file at `path` and returns a new descriptor
for it, the lowest one free. `flags` is one access mode, `O_RDONLY` (0),
`O_WRONLY` (1) or `O_RDWR` (2), plus any of `O_CREAT` (0x40: make the file,
empty, if `path` names nothing; open it as it is if it exists),
`O_TRUNC` (0x200: empty a file opened for writing; ignored for reading and
for devices) and `O_CLOEXEC` (0x80000: the new descriptor closes on
`exec`, its `FD_CLOEXEC` set; see `fcntl`); anything else is `-EINVAL`. The offset starts at 0. What it
opens:

- a file, in the access mode asked for; on a read-only filesystem, writing
  (or creating) is `-EROFS`;
- a directory, for reading only: anything else is `-EISDIR`, and so is
  `O_CREAT` on a missing name with a trailing slash (`newf/` names a
  directory, and `open` makes only files);
- a device node (`/dev/console`, `/dev/fb0`, `/dev/input/event0` and
  `event1`, `/dev/ptmx`, `/dev/pts/N`), which gives the device itself, in
  the access mode asked for, also on a read-only filesystem: the console's
  reads and writes, the framebuffer's memory to map and its requests ("The
  framebuffer"; its `read` and `write` are `-EINVAL`), an input device's
  events ("Input devices"), a new pseudo-terminal's master or a slave
  ("Terminals": `-ENOSPC` when all 16 are in use, `-ENOMEM`, and a slave
  `-ENXIO` without its pair, `-EIO` locked or without its master); a node
  whose major number has no driver, or whose device the machine hasn't (a
  framebuffer, a keyboard, a pointer), is `-ENODEV`.

A local socket's name (an inode of type `T_SOCKET`, which `bind` makes) is
not opened: `-ENXIO`, as on Linux; `connect` reaches the socket behind it
("Local sockets"), and `stat` says what it is.

A file bigger than the kernel reads or writes, 2^32 - 1 blocks (ext4's
`s_maxbytes`), is `-EOVERFLOW`: only a damaged inode can be one, as the
format's largest file is one block more (docs/camelot-fs.md, "Inodes"), and
no filesystem holds anything near it.

Errors: the path's (`-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`), those above,
`-ENOSPC` (no free inode or block to make the file with), `-EMFILE` (all 16
of the caller's descriptors are taken), `-ENFILE` (all 64 of the kernel's
open files are), `-EFAULT`. Without a free descriptor and open file nothing
is made. Making the file is one transaction: a crash leaves it done or
not. Emptying it takes as many as freeing its blocks does, after the file
is open (docs/camelot-fs.md, "Truncation"): from the first one on the file
reads as empty, writes to it wait until it is done, and a crash part way
leaves the rest to the next mount.

**close(fd)** closes descriptor `fd`, which is free again. The open file
behind it goes with the last descriptor that shares it: a pipe end is closed
then ("Pipes"), and a file whose last name was removed meanwhile is freed.
`-EBADF` if `fd` isn't open.

**lseek(fd, offset, whence)** moves the offset of the file behind `fd` to
`offset` bytes from the start (`whence` = `SEEK_SET`, 0), from the current
offset (`SEEK_CUR`, 1) or from the end (`SEEK_END`, 2), and returns it.
`offset` and the result are an `off_t`. Errors: `-EBADF`; `-ESPIPE` for a
pipe or a device, which have no offset; `-EINVAL` for another `whence`, or
a new offset below 0 or past the end of the file (files have no holes: they
only grow by writing at their end), which every offset past the largest
file is too: the whole `off_t` counts, so on RV64 2^32 + 100 is past the
end of a small file, not 100.

**fstat(fd, st)** stores what the open file behind `fd` is in the `struct
stat` at `st` (24 bytes, `user.h`): `ino` (its inode, 0 for a pipe and for
the console the kernel gave the shell), `type` (`T_DIR` 1, `T_FILE` 2,
`T_DEVICE` 3, `T_PIPE` 4), `nlink` (the names it has; 0 for a file whose
last name was removed while it is open), `size` (bytes; for a pipe, the
bytes waiting in it; 0 for a device), `major` and `minor` (a device's
numbers, 1 and 0 for the console). A socket is `T_SOCKET` (5), with the
bytes waiting to be received as its size (for UDP, the datagrams' bytes),
and an `nlink` of 1, or 0 once nothing more will arrive: the other end
ended its side (its FIN came), the connection was reset or timed out, or
the socket was shut down for reading. With a size of 0 too, a read
returns at once, the end or the error; so a program that holds a
connection without reading it can tell the other end hung up (`shd`
does, for its sessions). A local socket is `T_SOCKET` too ("Local
sockets"), and the same: its `ino` 0, its size the bytes waiting in it,
its `nlink` 0 once nothing more will arrive: its peer has closed or shut
its sending down, or the socket was shut down for reading.
`-EBADF`, or `-EFAULT` if `st` is not writable.

**stat(path, st)** stores in `st` what `fstat` would of the file at `path`
opened, without opening it, as Linux's `stat` (its `newfstatat`): so it
looks at what `open` refuses as well, a local socket's name (`T_SOCKET`,
size 0) or a device node without a driver. Errors: the path's (`-ENOENT`,
`-ENOTDIR`, `-ENAMETOOLONG`), `-EFAULT`.

**mkdir(path)** makes the directory `path`, with its `.` and `..`. Errors:
`-EEXIST` (the name is taken, `/` included), the path's, `-ENOENT` also for
a directory to make it in that has been removed (a working directory, say),
`-ENOSPC`, `-EROFS`, `-EFAULT`.

**unlink(path)** removes the name `path`. A file loses a link; once it has
none, it is freed as soon as no descriptor has it open (until then it can
still be read and written through them). A directory must be empty (but for
`.` and `..`), or it is `-ENOTEMPTY`; one that is some process's working
directory is removed even so: nothing can be made in it any more, and
nothing but `.` is found in it (looking up its `..` is `-ENOENT`: the parent
may be gone too, its inode freed or reused).
`-EINVAL` for `.`, `..` and `/`; the path's errors; `-EROFS`; `-EFAULT`.

**link(old, new)** gives the file `old` another name, `new`: a hard link,
the same file under both, with one more link. Errors: the paths'
(`-ENOENT` also for a removed directory to put `new` in, and for a file
whose last name another process removed meanwhile, which cannot be linked
back, as on Linux), `-EPERM` if `old`
is a directory (a directory has one name, in its parent), `-EEXIST` if
`new` is taken (`/` included), `-ENOSPC`, `-EROFS`, `-EFAULT`.

**chdir(path)** makes the directory `path` the caller's working directory,
where every path that doesn't start with `/` is looked up from. `-ENOTDIR`
if it is not a directory; the path's errors; `-EFAULT`.

**dup(fd)** returns a new descriptor, the lowest one free, for the open file
behind `fd`: both share it, offset included, and it stays open until both
are closed. The new descriptor stays open across `exec`, whatever `fd`'s
flag: a descriptor's `FD_CLOEXEC` is its own, not the open file's. `-EBADF`,
or `-EMFILE` if every descriptor is taken.

**fcntl(fd, cmd, arg)** is Linux's for a descriptor's flags, for copies
of it and for a shared-memory object's seals, with Linux's numbers. Each descriptor has one flag, `FD_CLOEXEC`
(1): it closes when the process `exec`s a new program (and `spawn` passes
it on to no child). `open` sets it with `O_CLOEXEC`; every other descriptor
starts without it, and `close` clears it with the descriptor. `cmd` is:

- `F_DUPFD` (0): a new descriptor for the open file behind `fd`, as `dup`
  makes, but the lowest free one from `arg` up, without `FD_CLOEXEC`;
- `F_DUPFD_CLOEXEC` (1030): the same, with `FD_CLOEXEC` set (a shell keeps
  its own descriptors this way while it puts others at 0 and 1 for a
  program it `exec`s);
- `F_GETFD` (1): `fd`'s flags, `FD_CLOEXEC` or 0;
- `F_SETFD` (2): sets them to `arg`, of which only `FD_CLOEXEC` counts;
  returns 0;
- `F_ADD_SEALS` (1033): adds the seals in `arg` to the shared-memory
  object `fd` is open on ("Memory mappings", `memfd_create`): `F_SEAL_SEAL`
  (1: no more seals), `F_SEAL_SHRINK` (2: `ftruncate` can't make it
  smaller) and `F_SEAL_GROW` (4: nor bigger); returns 0. A seal is never
  taken off. Linux's `F_SEAL_WRITE` (8) and `F_SEAL_FUTURE_WRITE` (16) are
  not there;
- `F_GET_SEALS` (1034): the object's seals.

Errors: `-EBADF` if `fd` is not open, checked before `cmd` (so an unknown
`cmd` on a closed descriptor is `-EBADF`, as on Linux); `-EINVAL` for
another `cmd`, for an `arg` of `F_DUPFD` or `F_DUPFD_CLOEXEC` that is
not a descriptor, 0 to 31 (Linux's answer for one at or past
`RLIMIT_NOFILE`), for the seals' commands on anything but a shared-memory
object, and for a seal Camelot hasn't; `-EMFILE` when no descriptor from
`arg` up is free; `-EPERM` to add seals to an object sealed with
`F_SEAL_SEAL`, or through a descriptor not open for writing.

**ioctl(fd, request, ...)** is Linux's: a request to the device behind
`fd`, with one argument, a number or a pointer as the request says (its
number says which, as Linux's `_IOR` and `_IOW` encode it, but the driver
decides). The devices' requests are in "The framebuffer" and "Input
devices", and the terminals' in "Terminals", at Linux's numbers, with
Linux's structures. Returns what the request returns, 0 for most. Errors:
`-EBADF` if `fd` is not open; `-ENOTTY` if it is not a device (a file, a
pipe, a socket) or its driver doesn't know the request; the request's own,
`-EFAULT` for a pointer to memory it can't read or write among them.

**pipe(fds)** makes a pipe and stores two new descriptors in `fds[0]` (the
read end) and `fds[1]` (the write end), the lowest two free ("Pipes").
Errors: `-EMFILE`, `-ENFILE`, `-ENOMEM` (no page for its buffer), or
`-EFAULT` if `fds` is not 8 writable bytes; nothing is opened then.

**exec(path, argv, envp)** replaces the caller's program with the one at
`path`: its address space goes, and the new program starts as one started
by `spawn` does ("Starting a program"), with `argv` and the environment
`envp` (not the old program's: an `exec` that should keep it passes
`environ`), in the same process. Its
pid, parent, descriptors and working directory stay, but for the
descriptors with `FD_CLOEXEC` set (`fcntl`), which are closed once the new
program has replaced the old one; its name becomes the last name of `path`. Its other threads, if it has any, end
first ("Threads" below): the new program runs in the caller's thread
alone. On success it doesn't return; on an error,
nothing changed (the descriptors that close on exec are still open) and
the old program carries on with the error, `spawn`'s but `-EAGAIN` (the
kernel prints `exec: PATH: REASON` for `-ENOEXEC`).

**kill(pid)** ends the process `pid`, every one of its threads: it exits with status -1, as a
process killed for a fault does (its parent's `wait` gets -1), but
silently, and not at once. The kernel marks it killed and, if it is asleep,
wakes it; it ends when it would next go back to U-mode, before it runs any
more of its own code (a process killed before its first turn runs none of
it). A system call it is in the middle of carries on to
its end, so a change to the disk is never left half done, except that the
calls that may wait for long give up at once: `sleep`, `wait`, a `read` of
the console or of a pipe, a `write` to a full pipe, `thread_join`,
`futex_wait` (what they return is never seen). A process spinning in U-mode ends at the next timer tick. A
process may kill itself; `kill` then returns 0 and the process ends on its
way back from the call. Killing a zombie, which has already ended, does
nothing and returns 0. Errors: `-ESRCH` when no process has that pid (one
whose parent has collected it is gone), `-EPERM` for one of the kernel's
own processes or for the first process (init, or the shell without one),
whose end would be Camelot's (it ends only by its own `exit`).

**procs(buf, max)** describes the processes, in pid order, in the array of
`max` `struct procinfo` (40 bytes each, `user.h`) at `buf`, and returns how
many processes there are, which may be more than `max`: only the first
`max` are stored then, and `procs(NULL, 0)` just counts them. Each entry
holds `pid`; `ppid`, its parent's pid (1 for an orphan init was given), or
0 for a process without one (the first process, an orphan when the first
process is not init, a kernel process); `state`: `PROC_RUNNABLE` (1, waiting
for its turn), `PROC_RUNNING` (2, the caller itself), `PROC_ZOMBIE` (3,
ended, its status not collected yet) or `PROC_SLEEPING` (4); for a process
of several threads, running if one of them is, else runnable if one is,
else sleeping; `size`, the
end of its heap (its program, data and heap, from address 0, without the
stack page), 0 for a kernel process; and `name`, NUL-terminated (the last
name of the path it was started from, at most 15 characters). A process
that `spawn` is still loading is not listed. All of `buf`, `max` entries,
must be writable, however many are stored, or it is `-EFAULT` and nothing
is.

**harts(buf, max)** describes the harts the kernel runs on, from hart 0
up, in the array of `max` `struct hartinfo` (16 bytes each, `user.h`) at
`buf`, and returns how many there are: 1 on a one-hart machine, else every
hart Grail has (`--harts`; `docs/excalibur-spec.md` section 10, "Kernel
SMP"). As for `procs`, only the first `max` are stored, `harts(NULL, 0)`
just counts them, and all of `buf`, `max` entries, must be writable, or it
is `-EFAULT`. Each entry holds `hart`, its id (0 to the count - 1); `pid`,
the process it is running, which on the caller's own hart is the caller,
or 0 while it idles; `ticks`, the timer ticks it has taken since it
started (each hart has its own timer: hart 0's are `uptime`'s); and
`switches`, the times it has switched to a process since it started. A
process runs on one hart at a time, but not always the same one.

The network's calls are what the `ifconfig` and `ping` programs use;
`docs/camelot-net.md` describes the stack behind them. Addresses are
`uint`s in host order (10.0.2.15 is 0x0a00020f), unlike the sockets'.
Without a network card each fails with `-ENODEV` (after the pointer check).

**netinfo(info)** stores a `struct netinfo` (`user.h`, 132 bytes) at
`info`: the interface's MAC address, `flags` (`NETINFO_LINK` the card's
link is up, `NETINFO_UP` it has an address, `NETINFO_DHCP` from DHCP),
its address, netmask, default gateway and DNS server (0 when it has none),
and `stats`, a `struct net_stats` of 27 counters: frames and bytes received
and sent, the frames and packets dropped, by reason, and what TCP's
connections went through (`docs/camelot-net.md`, "Counters"). `-EFAULT` if
`info` is not 132 writable bytes.

**netconf(addr, mask, gateway)** gives the interface a static address,
replacing what DHCP or the boot configuration gave it: `addr` with netmask
`mask`, and `gateway` as the default route (0 for none, so only the subnet
is reachable). The ARP cache is emptied. `-EINVAL`, changing nothing, if
`mask` is not ones then zeros (at least one of each), if `addr` is not an
ordinary unicast address (0.0.0.0/8, 127.0.0.0/8 and 224.0.0.0 up are not)
or is its subnet's network or broadcast address, or if `gateway` is not
another such address on the subnet.

**ping(addr, seq, reply)** sends an ICMP echo request with 56 bytes of data
to `addr`, with the caller's tid as its identifier (its pid, in a
single-threaded process) and `seq` (0 to 65535) as its sequence number, and waits up to one second (10 ticks) for the
matching echo reply; then stores a `struct ping_reply` at `reply` (who
answered, the reply's TTL, the ICMP message's length, 64, and the round
trip in microseconds of `mtime`) and returns 0. 127.0.0.1 and the
interface's own address answer through loopback. Errors: `-ETIMEDOUT` (no
reply in time), `-EHOSTUNREACH` (ARP got no answer from the next hop within
the last three seconds), `-ENETUNREACH` (the interface has no address, or
`addr` is off the subnet with no gateway), `-ENOBUFS` (no packet buffer
free), `-EINVAL` (`addr` 0, or `seq` over 65535) and `-EFAULT` (`reply` is
not 16 writable bytes; checked before anything is sent).

## Sockets

The socket calls are a subset of BSD's, with Linux's numbers and layouts:
IPv4 (`AF_INET`), TCP (`SOCK_STREAM`) and UDP (`SOCK_DGRAM`), described
here, and local stream sockets (`AF_UNIX`), described in "Local sockets"
below; `sendmsg` and `recvmsg`, every socket's, in "Messages". A socket
is an open file behind a descriptor, like a pipe end: `read` and `write`
on it are `recv` and `send`, `dup` and `spawn` share it, `close` ends it
with its last descriptor, `lseek` is `-ESPIPE` and `fstat` says `T_SOCKET`.
So a program spawned with a socket as its output writes to the network
without knowing it. `docs/camelot-net.md` ("Sockets") describes the stack
behind them; the user library's `resolve` finds a name's address by DNS.

Addresses are a `struct sockaddr_in` (16 bytes, `user.h`): `sin_family`
(`AF_INET`, 2), then the port and the address in network byte order
(big-endian, as on the wire: `htons`, `htonl`), then 8 bytes of zeros. A
family other than `AF_INET` is `-EAFNOSUPPORT`. The address 0.0.0.0
(`INADDR_ANY`) means any of this host's. The calls take an address as a
`void *`, since a local socket's is a `struct sockaddr_un`: Camelot's
calls have no address length, so each family's is its structure's size.

The checks come in a fixed order, as for the other calls: the descriptor
first (`-EBADF` if it is not open, `-ENOTSOCK` if it is not a socket),
then the pointers (`-EFAULT`; an address must be 16 readable or writable
bytes), then everything else. Without a network card, `socket` fails with
`-ENODEV` for `AF_INET`, so no other call ever sees a TCP or UDP socket.

A call that waits (`accept` for a connection, `connect` for the handshake,
`send` for room in a full buffer, `recv` for data) sleeps without using the
CPU, and ends with `-EINTR` as soon as the process is killed; what the
socket had done stays done. `recv` and `accept` wait for ever, unless the
socket has a receive timeout (`setsockopt`): then they return `-EAGAIN`
once it has run out.

**socket(domain, type, protocol)** makes a socket and returns a new
descriptor for it, the lowest one free. `domain` is `AF_INET`, or
`AF_UNIX` ("Local sockets"), anything else `-EAFNOSUPPORT`; for `AF_INET`, `type` is
`SOCK_STREAM` (1, TCP) or `SOCK_DGRAM` (2, UDP), anything else is
`-EINVAL`; `protocol` is 0 or the type's own (`IPPROTO_TCP` 6, `IPPROTO_UDP`
17), anything else `-EPROTONOSUPPORT`. Errors also: `-ENODEV` (no network
card), `-EMFILE`, `-ENFILE`, `-ENOBUFS` (every one of the kernel's 16 TCP
sockets is taken; UDP sockets are taken at `bind`, 8 of them).

**bind(fd, addr)** gives the socket the port of `addr`, or the next free
ephemeral one (49152 up) for port 0. The address must be 0.0.0.0, the
interface's own or one of 127.0.0.0/8 (`-EADDRNOTAVAIL` otherwise), but the
socket gets the port on every address of the host either way. Errors:
`-EADDRINUSE` (a port another socket of the same kind has: a TCP listener,
or a TCP socket bound and not connected yet, or any UDP socket), `-EINVAL`
(the socket is bound or connected already), `-ENOBUFS` (no UDP socket
left).

**listen(fd, backlog)** makes a TCP socket a listener: the connections that
come to its port wait for `accept`, at most `backlog` of them (taken as 1
to 8) that are not accepted yet. One more is dropped: its SYN gets no
answer, so the peer sends it again, and gets in once there is room. A socket that is not
bound gets an ephemeral port first. `-EOPNOTSUPP` for a UDP socket,
`-EINVAL` for one that is connected or listening already.

**accept(fd, addr)** waits for a connection on the listener `fd` and returns
a new descriptor for it, the oldest one first; unless `addr` is null, the
other end's address is stored there. The descriptor and the open file are
taken before the connection, so `-EMFILE` or `-ENFILE` leave it waiting.
Errors: `-EOPNOTSUPP` (UDP), `-EINVAL` (not a listener), `-EAGAIN` (the
receive timeout ran out), `-EINTR`.

**connect(fd, addr)** connects a TCP socket to `addr` and waits until the
connection is up, or not: `-ECONNREFUSED` (a reset: nothing listens there),
`-ETIMEDOUT` (no answer to the SYN and its five retries), `-ECONNRESET`,
`-ENETUNREACH` (no route), `-EINTR`. A socket that is not bound gets an
ephemeral port. `-EINVAL` for port 0, address 0.0.0.0, a broadcast or
multicast address, or a listener; `-EISCONN` if it has connected before
(a socket connects once). For a UDP socket, it only sets the peer, at once:
`send` goes there and only its datagrams are received; it may be changed by
another `connect`. A UDP socket that is not bound gets an ephemeral port
(`-EADDRINUSE` and `-ENOBUFS` come from that).

**send(fd, buf, len)** sends the `len` bytes at `buf` and returns how many
it sent. On TCP the bytes go into the connection's 4 KiB send buffer, waiting
for room as the peer acknowledges; it returns `len`, unless the connection
ended part way (then the bytes sent until then). Errors: `-EPIPE` (this side
was shut down), `-ENOTCONN` (never connected), `-ECONNRESET` and
`-ETIMEDOUT` (what ended the connection). On UDP each call is one datagram,
to the socket's peer (`-EDESTADDRREQ` without one), of at most 1472 bytes
(`-EMSGSIZE`); it does not wait, and the datagram may still be lost on the
way. Errors also `-ENETUNREACH`, `-ENOBUFS`; `-EPIPE` after `shutdown`.

**recv(fd, buf, len)** receives up to `len` bytes into `buf` and returns how
many. On TCP it waits until there are some, then takes what is there, up
to `len`; 0 once the peer has ended its side and everything before was
read. Once a connection was reset or timed out, it returns that error
(`-ECONNRESET`, `-ETIMEDOUT`); `-ENOTCONN` for a socket never connected. On UDP it takes the next datagram (from the peer only, on a
connected socket): what does not fit in `len` is lost, and the call returns
what it stored; an empty datagram returns 0. `len` = 0 still takes the next
datagram (waiting for it), as on Linux; on TCP it returns 0 without
waiting. `-EINVAL` for a UDP socket with no port, which nothing can ever
reach. After `shutdown` for reading, it returns 0 at once. Errors also:
`-EAGAIN` (the receive timeout ran out), `-EINTR`.

**sendto(fd, buf, len, to)** is `send` to the address `to`; a null `to` is
`send`. On UDP, `to` overrides the peer (`-EINVAL` for address 0.0.0.0 or
port 0) and an unbound socket gets an ephemeral port. On TCP, `to` is
ignored, its family too (the connection says where); only the pointer is
checked.

**recvfrom(fd, buf, len, from)** is `recv`, which also stores where the
bytes came from at `from` unless it is null: the datagram's sender, or the
connection's peer.

**shutdown(fd, how)** ends one side or both of a connected socket:
`SHUT_RD` (0) stops receiving (`recv` returns 0 from then on), `SHUT_WR`
(1) sending (TCP sends its FIN once the buffer is out, and the peer's
`recv` returns 0; `send` is `-EPIPE`), `SHUT_RDWR` (2) both. The
descriptor stays open, for what the other direction still has to do.
`-EINVAL` for another `how`, `-ENOTCONN` for a socket that is not
connected (a listener, a UDP socket without a peer); for a TCP connection
that is over, whatever `how` is, what ended it (`-ECONNREFUSED`,
`-ECONNRESET`, `-ETIMEDOUT`), or `-ENOTCONN` once it closed normally.

**getsockname(fd, addr)** stores the socket's own address at `addr`: its
port (0 if it has none yet) and the address its packets leave from (0.0.0.0
until it is connected).

**getpeername(fd, addr)** stores the other end's address at `addr`; a TCP
socket keeps it once the connection is over. `-ENOTCONN` for a socket never
connected.

**setsockopt(fd, level, option, value, len)** sets an option; the one there
is is `SO_RCVTIMEO` (20) at level `SOL_SOCKET` (1), how long `recv`,
`recvfrom` and `accept` wait: `value` is a `struct timeval` (`tv_sec`,
`tv_usec`, two `long`s: 16 bytes, `len`), rounded up to
whole timer ticks, 0 for no limit (the default). Errors: `-EINVAL` (`len`
is not the structure's size, or a negative time, or
`tv_usec` of a second or more), `-EFAULT` (`value` is not readable),
`-ENOPROTOOPT` (another level or option).

**getsockopt(fd, level, option, value, len)** reads an option back, as
Linux's: `*len` is the room at `value`, and the call stores at most that
much of the option there, and in `*len` how much it stored. The one there
is is a local socket's (`-ENOPROTOOPT` on TCP and UDP, and for another
level or option): `SO_PEERCRED` (17), the process at the other end, a
`struct ucred` (`pid`, `uid`, `gid`: 12 bytes; Camelot has no users, so
the two ids are 0). Errors: `-EFAULT` (`len` or `value` not there).

## Local sockets

Local sockets are Linux's `AF_UNIX` stream sockets (`SOCK_STREAM`;
`SOCK_DGRAM` and `SOCK_SEQPACKET` are `-ESOCKTNOSUPPORT`), between the
processes of the machine (`kernel/net/unix.c`). They need no network card.
Each end of a connection is an open file like a TCP socket's: `read`,
`write`, `poll`, `fstat`, `dup`, `close`; `recv` and `send` are the same
as `read` and `write`, and `sendmsg` and `recvmsg` pass descriptors
("Messages"). What one end sends waits in a ring of 2 KiB at the other
until it is read; a writer waits for room.

Their addresses are a `struct sockaddr_un` (110 bytes): `sun_family`
(`AF_UNIX`, 1), then `sun_path`, 108 bytes holding a path and its NUL. A
path that fills them with no NUL, an empty one (Linux's unnamed address,
or one of its abstract names, which start with a NUL) and a family other
than `AF_UNIX` are `-EINVAL`.

**socket(AF_UNIX, SOCK_STREAM, protocol)** makes one, not connected:
`protocol` is 0 (or `AF_UNIX`, Linux's `PF_UNIX`), else
`-EPROTONOSUPPORT`; `-ENOMEM` if no page is free for it.

**socketpair(domain, type, protocol, fds)** makes two sockets connected to
each other, as `pipe` makes a pipe's two ends (but either end reads and
writes), and stores their descriptors in `fds[0]` and `fds[1]`. `domain` is
`AF_UNIX`: `AF_INET` is `-EOPNOTSUPP`, as on Linux, anything else
`-EAFNOSUPPORT`; `type` and `protocol` as `socket`'s. Errors also
`-EMFILE`, `-ENFILE`, `-ENOMEM`, `-EFAULT`.

**bind(fd, addr)** names the socket: makes the path an inode of type
`T_SOCKET` (5) on the filesystem (docs/camelot-fs.md, "Inodes"), which
`connect` looks up. A name that is there already, a socket's or any other
file's, is `-EADDRINUSE`; the name stays after the socket is closed, until
`unlink` removes it, so a server unlinks its path before it binds, as on
Linux. `-EINVAL` if the socket has a name already; the path's errors
(`-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`), `-ENOSPC`, `-EROFS`.

**listen(fd, backlog)** makes a named socket a listener: at most `backlog`
connections (taken as 1 to 8) wait to be accepted. `-EINVAL` for a socket
without a name, or connected.

**connect(fd, addr)** connects to the listener named `addr`, at once, as
Linux does: the connection waits in the listener's backlog, already
connected (the caller may send), until `accept` takes it; with the backlog
full, it waits for room. Errors: the path's (`-ENOENT`...),
`-ECONNREFUSED` (a file that is not a socket, or a socket's name nobody
listens on: not yet, or no more), `-EISCONN` (connected already), `-EINVAL`
(a listener), `-ENOMEM`, `-EINTR`.

**accept(fd, addr)** takes the oldest connection waiting, waiting for one,
and returns a new descriptor for it; unless `addr` is null, the client's
name is stored there, or an empty path for a client without one (most).
Errors as TCP's: `-EINVAL` (not a listener), `-EMFILE`, `-ENFILE`,
`-EAGAIN` (the receive timeout), `-EINTR`.

**send**, **write**, **recv**, **read**: a write takes what fits in the
peer's ring, waits for room for the rest, and returns once all of it is in,
or with what went in if the process is killed (else `-EINTR`). `-EPIPE` once
this end can send no more (`shutdown` for writing, or the peer closed or
shut down its reading); `-ENOTCONN` never connected. A read waits for bytes
and takes what is there, up to `len`; 0 once the peer is done sending and
everything before was read; `-EINVAL` never connected. If the peer closed
with bytes it had not read, or before a listener accepted the connection
(the listener was closed), the next read that finds nothing returns
`-ECONNRESET`, once, as on Linux. `sendto` with an address is `-EISCONN`
(connected) or `-EOPNOTSUPP`; `recvfrom` stores the peer's name.

**shutdown(fd, how)** stops this end's reading (`SHUT_RD`: the peer can't
send any more), sending (`SHUT_WR`: the peer reads the end) or both, on any
socket, connected or not, as on Linux. `-EINVAL` for another `how`.

**close**: the peer reads the end, and its writes fail with `-EPIPE`. A
listener closed resets the connections it never accepted.

**getsockname(fd, addr)** stores the socket's name: its path, the
listener's for an accepted end, or an empty path. **getpeername(fd,
addr)** stores the peer's, kept once the peer is gone; `-ENOTCONN` never
connected. **setsockopt** takes `SO_RCVTIMEO` as TCP's does: how long a
read and `accept` wait. **getsockopt(fd, SOL_SOCKET, SO_PEERCRED, ...)**
gives the pid of the process at the other end as it was when they
connected, as Linux does: an accepted end's is the process that called
`connect`, the connecting end's the one that called `listen`, each of a
`socketpair`'s the process that made it, and 0 for a socket never
connected. It stays the same once that process is gone, or the socket
passed on. A display server finds out its clients' processes this way.

## Messages

**sendmsg(fd, msg, flags)** and **recvmsg(fd, msg, flags)** are `sendto`
and `recvfrom` with several buffers, as Linux has them, on any socket, and
on a local one they pass open files. `msg` is a `struct msghdr` (56 bytes):

| Field | What |
|---|---|
| `msg_name`, `msg_namelen` | an address and its size, or NULL: sendto's `to`, recvfrom's `from` |
| `msg_iov`, `msg_iovlen` | the buffers, `struct iovec`s of a pointer and a length (at most 16: `-EMSGSIZE` above), gathered from in turn or scattered into |
| `msg_control`, `msg_controllen` | the control messages, or NULL |
| `msg_flags` | recvmsg's: `MSG_CTRUNC` (0x8), some descriptors were closed for want of room; `MSG_TRUNC` (0x20), a datagram's end did not fit |

`recvmsg` stores, besides the bytes, `msg_namelen` (the address's size: 16
for a UDP datagram's sender; a local socket's peer's name, 2 bytes and the
path with its NUL, if it has one; else 0, as for TCP), `msg_controllen`
(the control messages' bytes) and `msg_flags`.

The control messages are a `struct cmsghdr` each (16 bytes: `cmsg_len`,
the header's and the data's bytes, then `cmsg_level` and `cmsg_type`),
followed by its data, the next one 8-byte aligned (`user.h`'s `CMSG_`
macros are Linux's). The one there is is `SCM_RIGHTS` (1) at level
`SOL_SOCKET`: an array of `int` descriptors. Messages of other levels are
skipped, as on Linux; another `SOL_SOCKET` type, a length past the buffer,
or more than 16 descriptors in all are `-EINVAL`, a descriptor not open
`-EBADF`.

**Descriptors passed** (`SCM_RIGHTS`, local sockets only: `-EINVAL` on TCP
and UDP) arrive as new descriptors of the receiver for the same open files,
whatever they are: a file with its offset, a pipe's end, a socket, a
shared-memory object (`memfd_create`), a device. They come with the first
byte the `sendmsg` sent, so it must send one (`-EINVAL` for none), and a
read stops before the bytes of a `sendmsg` that passed some, so the call
that takes them takes them with that byte. `recvmsg` gives them the lowest
free descriptors and stores them as one `SCM_RIGHTS` message; those that
don't fit in the control buffer, or find no free descriptor, are closed,
and `MSG_CTRUNC` says so. `read`, `recv` and `recvfrom` have nowhere to
put them, and close them. Until they arrive they keep their files open; a
local socket passed over its own connection, or a cycle of them, stays open
until the machine stops (Linux collects those; Camelot doesn't).

The flags: `MSG_DONTWAIT` (0x40) on a local socket, `-EAGAIN` rather than
wait; `MSG_NOSIGNAL` (0x4000), accepted, with nothing to do (Camelot has no
`SIGPIPE`); `MSG_CMSG_CLOEXEC` (0x40000000, `recvmsg`), the descriptors
close on `exec`. Anything else, and `MSG_DONTWAIT` on TCP and UDP, is
`-EOPNOTSUPP`. On TCP and UDP the buffers go through a page of the
kernel's: a datagram is at most that (`-EMSGSIZE`, though UDP's limit is
smaller), a TCP `recvmsg` takes a page at most, and `-ENOBUFS` if no page
is free.

## Threads

A process starts with one thread, its main thread, and `thread_create`
adds more. The threads of a process share everything it has: its memory,
its descriptors, its working directory, its children; each has its own
registers and stack, and the scheduler runs each on its own, on any hart,
several of them at once on a machine of several harts
(`docs/excalibur-spec.md` section 10, "Kernel threads"). A thread has an
id, its tid, from the same count as the pids, so no thread has the pid of
another process as its id; the main thread's tid is the process's pid.
There are 16 thread slots for all processes together, each process's
main thread included.

**thread_create(entry, arg, stack)** starts a thread in the caller's
process at `entry`, with `a0` = `arg`, `sp` = `stack` and every other
register zero, `ra` included: the code at `entry` must end the thread
with `thread_exit` rather than return. `stack` is the top of its stack,
16-byte aligned, as the calling convention wants `sp`; the memory is the
caller's to give (static, or from `malloc`), and must stay until the
thread has ended. Returns the new thread's tid. Errors: `-EFAULT` if
`entry` is not executable user memory or the 16 bytes below `stack` are
not writable; `-EINVAL` if `stack` is not 16-byte aligned or `entry` not
4-byte aligned; `-EAGAIN` if every thread slot is taken, or the process is
ending; `-ENOMEM`. The library's `thread_start` (`kernel/user/lib/`)
starts a C function in a thread, on a stack it is given, and ends the
thread with what the function returns.

**thread_exit(status)** ends the calling thread with `status`, which
`thread_join` collects. Until then the ended thread keeps its slot (but
none of its memory). The last thread of a process to end ends the
process, with `status` as its exit status, as `exit(status)` would:
that may be the main thread or any other. `exit` and returning from
`main` end the whole process, every thread with it.

**thread_join(tid, status)** waits until thread `tid` of the caller's
process has ended (at once if it has), stores its status at `status`
unless that is null, frees its slot and returns 0. Any thread may join
any other of its process, the main thread included; each ended thread is
joined at most once. Errors: `-ESRCH` if the process has no thread `tid`
(never had, or it was joined already: a thread of another process counts
as none); `-EINVAL` for the caller's own tid; `-EFAULT` if `status` is not
writable, checked first; `-EINTR` if the caller is killed while it waits.

**gettid()** returns the caller's thread id.

**futex_wait(addr, val)** and **futex_wake(addr, n)** are what locks are
built on in user space (the library's mutex, `kernel/user/lib/mutex.c`):
a thread waits in the kernel on the word at `addr` until another thread
wakes it, with no busy loop. `futex_wait` compares the word with `val`
and returns `-EAGAIN` at once if they differ; if they are equal it sleeps
until a `futex_wake` on the same address, then returns 0. The comparison
and the sleep are one step as far as `futex_wake` is concerned, so a
thread that changes the word and then calls `futex_wake` either changed it
before the comparison (and the waiter returns `-EAGAIN`) or wakes the
waiter: no wakeup is lost between them. `futex_wake` wakes up to `n` of
the threads of the caller's process that wait on `addr` (none for `n` of 0
or less), the ones that waited first not necessarily first, and returns
how many it woke. A futex belongs to a process: it never wakes another
process's threads, even on memory the two share ("Memory mappings",
Linux's private futexes). Errors, for
both: `-EINVAL` if `addr` is not 4-byte aligned, `-EFAULT` if it is not
readable user memory; and `-EINTR` if the waiter is killed.

**Ending a process with threads.** `exit`, a fault in any of its threads
(which prints the report of "exit" above and ends the process with -1) and
`kill` end every thread of the process. The threads other than the one
ending it are stopped as `kill` stops a process: a thread in U-mode ends
at its next trap (a tick at the latest), one waiting in a long wait
(`sleep`, `wait`, `thread_join`, `futex_wait`, the console, a pipe, a
socket) gives up at once, and one in the middle of another call finishes
it first. The process ends, its descriptors closed and its memory freed,
once all of them have. `exec` from one thread ends the others the same way
before the new program starts, in the process's only thread, whose tid
stays; if another thread is ending the process meanwhile, `exec` fails
(the caller is about to end anyway). `exec` reads the whole new program
before it waits for the others, so one of them reading the same file, or
writing to any, still finishes its call.

**Buffers another thread unmaps.** A pointer is checked when the call
starts, but another thread's `sbrk`, `munmap` or `mprotect` may unmap part
of the buffer, or take an access away, while the call is under way (a
`write` to a full pipe, say). The call then
stops where the copy fails: `read` and `write` return the bytes moved
until then, or `-EFAULT` if none, and the bytes they couldn't copy are
neither written nor taken from the pipe; the calls that fill in a struct
(`fstat`, `procs`, `harts`, `pipe`, `getsockname`, `getpeername`) return
`-EFAULT`, and `pipe` closes the descriptors it made.

## Memory mappings

Besides its program, its heap and its stack, a process may have memory
mappings: ranges of its address space that `mmap` maps and `munmap` takes
away, each with its protections (`mprotect`), as Linux's: anonymous memory
of the process's own, memory shared with other processes ("Shared
memory" below), and a device's memory ("A device's memory" below). Inside
the kernel each mapping is an area (`kernel/mmap.c`, the shared objects
`kernel/shm.c`, the framebuffer `kernel/fb.c`;
`docs/excalibur-spec.md` section 10, "Kernel memory mappings").

**Where they go.** Between the end of the heap and the guard page under the
stack's limit (`USER_LOAD_TOP`, "Starting a program"), from the top down:
each new mapping goes in the highest room that fits it, as Linux's go
below the stack. The heap can't grow into a mapping (`sbrk` fails with
`-ENOMEM` then), and the program, the heap and the stack are not mappings:
`munmap` and `mprotect` leave them alone.

**Pages on demand.** A mapping's pages are mapped the first time they are
touched, by the program or by a system call that reads or writes them (a
`read` into a buffer there, say): an untouched page costs no memory, so a
program may map far more than the machine has, as long as it doesn't use
it all. Anonymous memory reads zero at first. A touch the mapping's
protections don't allow (a write to a read-only mapping, any access to a
`PROT_NONE` one) is a fault, and kills the program ("exit"), as Linux's
`SIGSEGV` would; so does a page that can't be had then. A device's
mapping too maps a page at its first touch, but the page is the device's.
A system call checks a whole buffer before it reads or writes any of it,
so every page of the buffer is mapped then, even if the call uses fewer
bytes (a `read` that finds 10 bytes waiting maps all of a 1 MiB buffer),
and a buffer bigger than the free memory is `-EFAULT`.

The values are Linux's:

| Name | Value | |
|---|---|---|
| `PROT_NONE` | 0 | no access |
| `PROT_READ` | 1 | readable |
| `PROT_WRITE` | 2 | writable, and so readable: a RISC-V page can't be writable and not readable |
| `PROT_EXEC` | 4 | executable |
| `PROT_SEM` | 8 | `mprotect` only: accepted, means nothing (every page allows atomics) |
| `MAP_SHARED` | 0x01 | shared with every other mapping of the same memory |
| `MAP_PRIVATE` | 0x02 | the process's own |
| `MAP_SHARED_VALIDATE` | 0x03 | `MAP_SHARED` of a descriptor, but a flag Linux doesn't know is `-EOPNOTSUPP` |
| `MAP_FIXED` | 0x10 | exactly at `addr`, replacing what is mapped there |
| `MAP_ANONYMOUS` | 0x20 | no file: zeroed memory |
| `MAP_NORESERVE` | 0x4000 | accepted: nothing is reserved anyway |
| `MAP_POPULATE` | 0x8000 | map every page now, rather than on first touch |
| `MAP_FIXED_NOREPLACE` | 0x100000 | exactly at `addr`, but never over a mapping |

**mmap(addr, len, prot, flags, fd, offset)** maps `len` bytes, rounded up
to whole pages, and returns where, a page-aligned address (or minus an
error, as a pointer, as `sbrk` returns one). `flags` holds one of
`MAP_SHARED`, `MAP_PRIVATE` and `MAP_SHARED_VALIDATE` and any of the
others; flags Camelot doesn't know are ignored, as Linux ignores them, but
for a descriptor's `MAP_SHARED_VALIDATE` mapping, which refuses those
Linux doesn't know either (outside its `LEGACY_MAP_MASK` and
`MAP_FIXED_NOREPLACE`). Bits of `prot` other than the three are ignored
too, as Linux's `mmap` ignores them. With `MAP_ANONYMOUS` the
mapping is anonymous memory, zeroed, and `fd` and `offset` are ignored;
`MAP_SHARED` anonymous memory is the same as `MAP_PRIVATE`, since Camelot
has no `fork` whose child could share it. Without, it maps what the
descriptor `fd` has from `offset` on, page aligned. Where it goes:
with `MAP_FIXED` or `MAP_FIXED_NOREPLACE`, at `addr` exactly, which must be
page aligned, and the whole range in the mapping room above (from the
heap's end, page aligned, to `USER_LOAD_TOP`); `MAP_FIXED` first unmaps
whatever mappings are there, as `munmap` would. Without either, `addr` is
a hint: taken, rounded up to a page, if the room is free there, and
otherwise the highest room that fits. Errors:

- `-EINVAL`: `len` is 0, `flags`' type (its low four bits) is none of
  the three (or `MAP_SHARED_VALIDATE` with `MAP_ANONYMOUS`, as on Linux),
  `addr` is not page aligned with `MAP_FIXED` or
  `MAP_FIXED_NOREPLACE`, or `offset` is negative or not page aligned for a
  descriptor;
- `-EOPNOTSUPP`: a descriptor's `MAP_SHARED_VALIDATE` mapping with a flag
  Linux doesn't know;
- `-EOVERFLOW`: a shared-memory object's mapping that reaches past the
  largest object (1 GiB), whose pages can never be;
- `-EINVAL` too: a `MAP_PRIVATE` mapping of a shared-memory object (a
  private mapping of one is a copy-on-write one on Linux, which Camelot
  doesn't have);
- `-EBADF`: `fd` is not an open descriptor (without `MAP_ANONYMOUS`);
- `-EACCES`: `fd` is not open for reading, or the mapping is writable
  and writes what `fd` is open on (`MAP_SHARED`, or any mapping of a
  device) while `fd` is not open for writing;
- `-ENODEV`: `fd` is not something that can be mapped (only shared-memory
  objects and `/dev/fb0` can be): a regular file, a directory, a pipe, a
  socket, the console;
- a device's own errors: for `/dev/fb0`, `-EINVAL` for a range past its
  memory, `-ENODEV` on a machine without a framebuffer;
- `-EEXIST`: `MAP_FIXED_NOREPLACE` over a mapping;
- `-ENOMEM`: no room for it (or, with `MAP_FIXED`, its range is not in the
  mapping room), anonymous memory bigger than the RAM (as `sbrk` fails at
  once for that), or the kernel's table of mappings (128 for every
  process together) is full.

**munmap(addr, len)** takes away every mapping, or part of one, in the
`len` bytes at `addr`, rounded up to whole pages: their pages are no
longer the program's (anonymous memory is freed), on every hart running
one of its threads, before it returns. A part of the range with no mapping
is fine. Returns 0. Errors: `-EINVAL` if `addr` is not page aligned, `len`
is 0, or the range goes past user space; `-ENOMEM` if a mapping has to be
cut in two and the kernel's table is full.

**mprotect(addr, len, prot)** gives the mappings in the `len` bytes at
`addr`, rounded up to whole pages, the protections `prot`, on every hart:
their pages keep what they hold, a `PROT_NONE` page too. Returns 0 (also
for `len` 0, whatever `prot`). Errors, in Linux's order: `-EINVAL` if
`addr` is not page aligned; `-ENOMEM` if the range goes past user space;
`-EINVAL` if `prot` has bits other than the three and `PROT_SEM`
(`PROT_GROWSDOWN` and `PROT_GROWSUP` included: no mapping grows);
`-ENOMEM` if part of the range is not a mapping (the program, the heap
and the stack included), or a mapping has to be cut and the kernel's
table is full; `-EACCES` if `prot` makes writable a mapping
that writes what a descriptor not open for writing was open on (as
`mmap` would refuse it).

A mapping's pages are given back when it is unmapped, when the program is
replaced (`exec`), and when the process ends.

**Shared memory.** `memfd_create` makes a shared-memory object, Linux's
anonymous memory file: memory that two processes map with `MAP_SHARED`
to see each other's writes, on any hart, as a GUI client and the display
server share a window's pixels. The other process gets the object's
descriptor as any other (a child's descriptors 0 to 2 from `spawn`).
Its pages are made on demand, zeroed, the first time a mapping touches
them, and belong to the object, which lives while a descriptor or a
mapping holds it: a process may close the descriptor once it has mapped
the object. Its size is `ftruncate`'s, 0 at first, and at most 1 GiB: a
mapping may go past the end, but a touch of a page wholly past it is a
fault, which kills the program (as Linux's `SIGBUS`). A shrink frees the
pages past the new end, but those a mapping has touched, which stay mapped
there until the mapping goes (Linux takes them away from the mappings
too); a later growth over them makes them read zero again, as new memory
would. The object is no regular file: `read`, `write` (`-EINVAL`) and
`lseek` (`-ESPIPE`) don't work on it, as they do on Linux's; `fstat` says
it is a regular file (`T_FILE`, no inode number, 1 link) of its size. A
futex in shared memory belongs to each process apart: one process's
`futex_wake` never wakes another's threads.

**memfd_create(name, flags)** makes an empty shared-memory object and
returns a descriptor for it, the lowest free one, open for reading and
writing. `name` only names it to someone debugging (Linux shows it in
`/proc`): it must be readable, and at most 249 bytes long. `flags` holds
any of `MFD_CLOEXEC` (1: the descriptor closes on `exec`, as `O_CLOEXEC`)
and `MFD_ALLOW_SEALING` (2: seals may be added, `fcntl`'s `F_ADD_SEALS`;
without it the object is sealed with `F_SEAL_SEAL` from the start, as on
Linux). A process that is handed an object sealed with `F_SEAL_SHRINK` knows
that its pages stay: the display server takes a window's buffers only from
one ("The display server" in the spec's section 10).
Errors: `-EFAULT` for a bad `name`, `-EINVAL` for a longer one or another
flag, `-EMFILE`, `-ENFILE` (no open file left, or none of the kernel's 32
objects).

**ftruncate(fd, length)** sets the size of the shared-memory object `fd`
is open on to `length` bytes. Returns 0. Errors: `-EBADF`; `-EINVAL` if
`fd` is no shared-memory object (`ftruncate` of a regular file is not
there yet) or not open for writing, or `length` is negative; `-EFBIG` past
1 GiB; `-EPERM` for a shrink of an object sealed with `F_SEAL_SHRINK`, or a
growth of one sealed with `F_SEAL_GROW`.

**A device's memory.** `/dev/fb0`, the framebuffer's device node on the
root image, maps the framebuffer's memory, the buffer of its mode ("The
framebuffer": 640x480 pixels of 32 bits in VRAM at first), from `offset`
on, its last page whole: a program draws by writing pixels there, and the
screen shows them.
The pages are the device's, mapped as they are touched, not the
process's: an unmap leaves them be (VRAM is never freed, a buffer in RAM
only once no descriptor or mapping of the device is left);
`MAP_PRIVATE` maps the same pages as `MAP_SHARED` (writes go to the
device either way, as Linux's framebuffer devices do), so a writable
mapping of either kind needs the descriptor open for writing. A system
call's buffer can't be in VRAM (the kernel copies only to and from RAM):
`write` from a mapping of the 640x480 framebuffer, say, fails with
`-EFAULT`; from a framebuffer in RAM (another mode, or two frames) it
works.

## The framebuffer

`/dev/fb0` is Linux's fbdev interface to the display (`docs/excalibur-spec.md`
section 9.3, the display controller; section 10, "Framebuffer driver"), the
part a desktop uses: a program maps the screen's memory to draw into
("Memory mappings", "A device's memory"), and with `ioctl` reads and sets
the mode, flips between two frames and waits for the vertical blank. The
requests and structures are Linux's (`include/uapi/linux/fb.h`), in
`user.h`:

| Request | Number | Argument | What |
|---|---|---|---|
| `FBIOGET_VSCREENINFO` | 0x4600 | `struct fb_var_screeninfo *` | the mode, the buffer's size and the row shown |
| `FBIOPUT_VSCREENINFO` | 0x4601 | `struct fb_var_screeninfo *` | sets them, and stores what was set |
| `FBIOGET_FSCREENINFO` | 0x4602 | `struct fb_fix_screeninfo *` | the buffer: where, how big, the bytes a row |
| `FBIOPAN_DISPLAY` | 0x4606 | `struct fb_var_screeninfo *` | shows the buffer from row `yoffset`, at the next vertical blank |
| `FBIOBLANK` | 0x4611 | a number | 0 shows the picture, 1 to 4 blank it |
| `FBIO_WAITFORVSYNC` | 0x40044620 | `__u32 *`, 0 | waits for the next vertical blank |

**The mode.** The display starts at 640x480 from its own VRAM, one frame.
`struct fb_var_screeninfo` gives the mode (`xres`, `yres`), the buffer
(`xres_virtual`, always `xres`, and `yres_virtual`), the row shown
(`yoffset`; `xoffset` is 0) and the pixels: 32 bits, XRGB8888 (`red` at bit
16, `green` 8, `blue` 0, 8 bits each; the top byte ignored). The size in
millimetres is unknown (`height`, `width` 0xffffffff) and the timings 0.
`FBIOPUT_VSCREENINFO` takes a mode the display has, by its size (Grail's:
640x480, 800x600, 1024x768, 1280x720, 1280x1024 and 1920x1080), a
`bits_per_pixel` of 32 or 0, and a `yres_virtual` of `yres` for one frame
or more for two, up to room for a second one; it stores the mode as set,
as `FBIOGET_VSCREENINFO` would give it. `activate`'s `FB_ACTIVATE_TEST`
checks and changes nothing. A mode other than 640x480 with one frame is a
buffer in RAM, which the kernel makes (zeroed) and the display scans out;
the old one goes. `FBIOGET_FSCREENINFO` gives the buffer (`smem_start`, its
physical address, and `smem_len`), `line_length` (`xres` times 4), `id`
`grail-fb`, `type` packed pixels, `visual` true colour, and `ypanstep`.
Errors: `-EINVAL` for a size no mode has, another pixel format, a buffer
wider than the mode or taller than allowed, or an offset as `FBIOPAN_DISPLAY`
refuses it; `-EBUSY` if the change needs another buffer while the old one
is mapped (unmap it first); `-ENOMEM` if there is no room for the buffer.

**Flipping.** A double-height buffer holds two frames: the program draws
one while the display shows the other, then flips. The display scans out
from the start of a page, so the frames start on pages: `ypanstep` is the
fewest rows whose bytes are a whole number of pages (8 at 640 pixels a row,
32 at 800), and `yoffset` must be a multiple of it. The second frame is at
row `yres`, but at 800x600, where it is row 608 (`yres_virtual` 1208);
`yres_virtual` may be up to `yres` rounded up to `ypanstep`, plus `yres`.
`FBIOPAN_DISPLAY` takes `yoffset` and shows the buffer from that row at the
next vertical blank, so no frame is shown half drawn; with `FB_ACTIVATE_VBL`
(16) in `activate` it returns once the display shows it. Errors: `-EINVAL`
for an `xoffset`, a `yoffset` past the last frame or not a multiple of
`ypanstep`; `-ENODEV` for `FB_ACTIVATE_VBL` on a display that flips but
has no vertical blank's interrupt (nothing is queued); `-EINTR` if the
process is killed while it waits.
`FBIO_WAITFORVSYNC` waits for the next vertical blank (60 a second): its
argument points to 0, the display (`-ENODEV` for another); `-EINTR` if
killed.

**Who has it.** The display is a program's while it has `/dev/fb0` open or
mapped; the kernel's console draws nothing meanwhile. When the last
descriptor and mapping go, the display goes back to 640x480 from VRAM, any
buffer in RAM is freed, the picture is shown if it was blanked, and the
console draws its text again.

## Input devices

`/dev/input/event0` is the keyboard and `/dev/input/event1` the pointer, as
Linux's evdev interface has them (`docs/excalibur-spec.md` sections 9.4 and
9.7, the devices; section 10, "Input drivers"). Open one for reading and
`read` its events: whole `struct input_event`s, 24 bytes each,

```c
struct input_event {
    struct timeval time;   /* when, since boot */
    unsigned short type;   /* EV_SYN, EV_KEY, EV_REL, EV_ABS */
    unsigned short code;   /* a key, an axis */
    int value;             /* 1 pressed, 0 released; a position; notches */
};
```

with the codes of Linux's `input-event-codes.h` (`KEY_A` 30, `BTN_LEFT`
0x110, `ABS_X` 0, `REL_WHEEL` 8, ...; `user.h` has those Camelot's devices
give). Each change the device reports is a packet of events ended by
`EV_SYN` `SYN_REPORT`. A `read` takes as many whole events as fit and are
there, and waits while there are none; a buffer smaller than an event is
`-EINVAL`, and `-EINTR` if the process is killed while it waits. Each open
descriptor gets every event from its `open` on, in a queue of its own of
128; one that falls a whole queue behind loses what it had not read and
finds `EV_SYN` `SYN_DROPPED` (3) in its place, then whole packets from
there on. `EVIOCGKEY` then tells it which keys are held, and drops the key
events still in its queue, as Linux's does. `write` is `-EINVAL` (`-EBADF` on a descriptor open for reading),
`lseek` `-ESPIPE`.

- **The keyboard** gives `EV_KEY` for each key pressed (1) and released (0),
  Linux's key codes for the keys where they are on the keyboard, as a USB
  keyboard's: what a key types is the program's to decide. While no
  descriptor has it open, the keyboard types into the console instead, the
  characters of the host's layout ("Console input"). On a keyboard that
  can only give characters (an older Grail), the codes are those of the
  keys that type them on a US layout, with `KEY_LEFTSHIFT` and
  `KEY_LEFTCTRL` pressed and released around them as needed.
- **The pointer** gives where it is on the screen, in pixels of the
  display's mode: `EV_ABS` `ABS_X` and `ABS_Y` (those that changed), the
  buttons as `EV_KEY` `BTN_LEFT`, `BTN_RIGHT` and `BTN_MIDDLE`, and the
  wheel's notches as `EV_REL` `REL_WHEEL`, positive up.

The requests, at Linux's numbers (`user.h`):

| Request | Argument | What |
|---|---|---|
| `EVIOCGVERSION` | `int *` | evdev's version, `EV_VERSION` (0x010001) |
| `EVIOCGID` | `struct input_id *` | `BUS_HOST` (0x19), vendor 0, product 1 (the keyboard) or 2 (the pointer), version 1 |
| `EVIOCGNAME(len)` | `char *` | the name, `Grail keyboard` or `Grail pointer`, cut to `len` bytes with its NUL; returns the bytes stored |
| `EVIOCGBIT(type, len)` | `unsigned char *` | for type 0 the event types the device gives, else the codes of `type` it gives, a bit each, cut to `len` bytes; returns the bytes stored; `-EINVAL` for a type evdev has not |
| `EVIOCGKEY(len)` | `unsigned char *` | the keys and buttons held, a bit for each key code (`KEY_CNT`, 768 bits, 96 bytes), cut to `len` bytes; returns the bytes stored. The `EV_KEY` events waiting for the descriptor are dropped |
| `EVIOCGABS(axis)` | `struct input_absinfo *` | the axis's value and range: `ABS_X` from 0 to the width less 1, `ABS_Y` to the height less 1; `-EINVAL` for another axis, or on the keyboard |
| `EVIOCGRAB` | a number | non-zero: only this descriptor gets the device's events, until it lets go (0) or is closed; `-EBUSY` if a descriptor has the grab already, this one included, `-EINVAL` for letting go of a grab it doesn't have |

Any other request is `-ENOTTY`, a pointer that isn't writable `-EFAULT`.

## Terminals

A terminal is the console, or a pseudo-terminal's slave: what a program
reads from it goes through the **line discipline** (`kernel/tty.c`, after
Linux's `n_tty`), which edits lines, echoes and acts on Ctrl-C, set by the
terminal's `struct termios`, Linux's. A **pseudo-terminal** (pty) is a pair:
the slave, a terminal, and the master, the other end of its line, which a
terminal emulator holds, as on Linux:

- opening `/dev/ptmx` makes a pair and gives its master; `TIOCGPTN` says
  its number N, and the slave is `/dev/pts/N` (0 to 15: the root image has
  those nodes, Camelot having no devpts), which opens once `TIOCSPTLCK`
  has unlocked it. The C library's `posix_openpt`, `grantpt`, `unlockpt`
  and `ptsname` do this (`user.h`);
- what the master writes is the slave's input, through the discipline:
  edited, echoed back to the master, its Ctrl-C acted on; a write waits
  while the slave's 256-byte input is full, but for a Ctrl-C among the
  next 256 bytes ("Ctrl-C");
- what the slave writes is what the master reads, through the output
  processing (`ONLCR`: a NL as CR NL), from a 2 KiB buffer, a write waiting
  for room; an echo that finds none is dropped;
- the pair has one `termios`, window size and foreground, which the
  requests on either end reach;
- closing the master hangs the slave up: its reads return what was left
  (a line typed part way too), then 0, its writes `-EIO`. Once every open slave descriptor is closed,
  the master reads what was left, then `-EIO`, and its writes are `-EIO`,
  until a slave is opened again. The pair goes once both ends are.

**The requests**, on a terminal or a master (Linux's,
`asm-generic/ioctls.h`; `user.h` has them):

| Request | Number | Argument | What |
|---|---|---|---|
| `TCGETS` | 0x5401 | `struct termios *` | the modes |
| `TCSETS` | 0x5402 | `const struct termios *` | sets them (`tcsetattr`'s `TCSANOW`) |
| `TCSETSW` | 0x5403 | `const struct termios *` | the same, once the output has gone (`TCSADRAIN`): on a slave, once the master has read what it wrote (the console's has always gone) |
| `TCSETSF` | 0x5404 | `const struct termios *` | the same, and the input thrown away (`TCSAFLUSH`) |
| `TCFLSH` | 0x540B | a number | throws away the input (`TCIFLUSH`, 0), the output not read yet (`TCOFLUSH`, 1), or both (2) |
| `TIOCGPGRP` | 0x540F | `int *` | the foreground process, 0 for none (as it was set, even once it has ended) |
| `TIOCSPGRP` | 0x5410 | `const int *` | sets it, 0 for none (Camelot's: Linux has no such group, `-ESRCH`): `-EINVAL` for less than 0, `-ESRCH` for no such process |
| `TIOCGWINSZ` | 0x5413 | `struct winsize *` | the size (0 by 0 until set) |
| `TIOCSWINSZ` | 0x5414 | `const struct winsize *` | sets it |
| `FIONREAD` | 0x541B | `int *` | the bytes a read would take now: a whole line's in canonical mode; the output waiting, on a master |
| `TIOCGPTN` | 0x80045430 | `unsigned int *` | a master's N, of `/dev/pts/N` |
| `TIOCSPTLCK` | 0x40045431 | `const int *` | locks (not 0) or unlocks a master's slave |

`TIOCGPTN` and `TIOCSPTLCK` are the master's (`-ENOTTY` on a slave); any
other request is `-ENOTTY`.

**The modes.** The flags are kept as set; the discipline acts on these:

- `c_iflag`: `ICRNL` (a CR typed is a NL), `INLCR` (a NL is a CR),
  `IGNCR` (a CR is dropped);
- `c_oflag`: `OPOST` with `ONLCR`, a NL written or echoed goes out as CR NL;
- `c_lflag`: `ICANON` (canonical mode), `ECHO`, `ECHOE` (erasing echoes
  BS, space, BS; and `VKILL` erases the line so, as with `ECHOKE`), `ECHOK`
  (otherwise `VKILL` echoes a newline), `ECHONL` (a NL echoed without `ECHO`), `ECHOCTL` (Ctrl-C
  echoed as `^C`), `ISIG` (Ctrl-C and Ctrl-\ kill the foreground process),
  `NOFLSH` (and keep the input);
- `c_cc`: `VINTR`, `VQUIT`, `VERASE`, `VKILL`, `VWERASE`, `VEOF`, `VEOL`,
  `VSUSP`, `VMIN`, `VTIME` (0 turns a character off).

**Canonical mode** (`ICANON`) edits a line before a read returns it, a
read taking at most the rest of the line: printable characters
(0x20-0x7E) are kept, up to 127; `VERASE` and BS (0x08, what Windows
consoles send) erase one, `VWERASE` the word before the cursor, `VKILL`
the line; NL, or `VEOL`, ends it and is kept; `VEOF` ends it without one,
so on an empty line the read returns 0; a NL right after a CR that `ICRNL`
made a NL is skipped, so CR LF is one line end; other characters,
control characters and bytes from 0x80 up, are dropped. **Non-canonical
mode** passes every byte, as Linux's does with `VMIN` and `VTIME` (tenths
of a second, a tick each): `VMIN` 0 and `VTIME` 0, what there is, maybe
nothing; `VMIN` 0, up to `VTIME` for a byte, else 0; `VTIME` 0, `VMIN`
bytes (or `len`, if fewer); both, `VMIN` bytes, or what came once `VTIME`
has passed since the last byte. Echo, in either mode, happens when a
program reads (or polls), so input typed ahead is echoed after the prompt
that asks for it.

**Ctrl-C.** With `ISIG`, `VINTR` (Ctrl-C) and `VQUIT` (Ctrl-\) kill the
terminal's foreground process as `kill` does (it exits with -1), throw
away the input unless `NOFLSH`, and echo `^C` and a newline; without a
foreground process, or once it has ended, they are dropped. They take
no room in the input, so they reach a program that doesn't read it:
when the input is full, a Ctrl-C waiting in the console's UART (128
bytes) or among the next 256 bytes of a master's write still gets in,
and the bytes ahead of it, which have no room, are dropped, as Linux
drops input that overflows (with `NOFLSH` too; without it, the Ctrl-C
throws the input away besides); further back, it waits with them. `VSUSP`
(Ctrl-Z) is dropped too: Camelot doesn't stop processes. The foreground
is a process, set with `tcsetpgrp`, until Camelot has process groups
(issue #116). The shell sets it on its terminal to each program it waits
for, so Ctrl-C ends the program, not the shell, and at its prompt makes
it none if it is the foreground itself (a shell run by another, which
Camelot has no signals for it to ignore); any other program that wants
Ctrl-C to end what it runs sets it.

**Defaults.** A new pty has Linux's (`tty_std_termios`, the pty driver's
`c_cflag`): `ICRNL`, `IXON`, `OPOST`, `ONLCR`, `B38400`, `CS8`, `CREAD`,
`ISIG`, `ICANON`, `ECHO`, `ECHOE`, `ECHOK`, `ECHOCTL`, `ECHOKE`, `IEXTEN`,
and Linux's control characters (Ctrl-C, Ctrl-\, DEL, Ctrl-U, Ctrl-D,
Ctrl-W, Ctrl-Z; `VMIN` 1, `VTIME` 0). The console has the same, with
`HUPCL` as Linux's other terminals, but for what it has always done: no
output processing (`c_oflag` 0), and no `VKILL` or `VWERASE` ("Console
input"). Its size is 0 by 0 until a
program sets it. `cfmakeraw`, in the C library, makes raw mode as glibc's
does.

## Error numbers

Linux's values for the same conditions.

| Number | Name | Meaning |
|---|---|---|
| 1 | `EPERM` | link: not for a directory; kill: a kernel process, or the shell; ftruncate, fcntl: a seal |
| 2 | `ENOENT` | no such file or directory |
| 3 | `ESRCH` | kill: no such process; thread_join: no such thread |
| 4 | `EINTR` | a wait ended because the process was killed |
| 5 | `EIO` | a pseudo-terminal whose other end is gone; a locked slave's open |
| 6 | `ENXIO` | open: a local socket's name, which only `connect` reaches |
| 7 | `E2BIG` | spawn, exec: argument list too long |
| 8 | `ENOEXEC` | spawn, exec: the file is not a valid executable |
| 9 | `EBADF` | not an open descriptor, or not open for that |
| 10 | `ECHILD` | wait: no such child |
| 11 | `EAGAIN` | spawn, thread_create: every slot is taken; futex_wait: the word is not the value; a socket's receive timeout ran out |
| 12 | `ENOMEM` | out of memory, or the heap would reach the stack or a mapping; mmap: no room; mprotect: not a mapping |
| 13 | `EACCES` | spawn, exec: not a regular file; mmap, mprotect: the descriptor isn't open for that access |
| 14 | `EFAULT` | a pointer argument is not valid user memory |
| 16 | `EBUSY` | ioctl: the framebuffer's memory is mapped, and the request would replace it; EVIOCGRAB: the device is grabbed already |
| 17 | `EEXIST` | the name is taken; mmap: MAP_FIXED_NOREPLACE over a mapping |
| 19 | `ENODEV` | open: a device node with no driver, or a device the machine hasn't; the network: no network card; mmap: a file that can't be mapped; FBIO_WAITFORVSYNC: a display other than 0 |
| 20 | `ENOTDIR` | a name on the way is not a directory |
| 21 | `EISDIR` | open: a directory, for writing |
| 22 | `EINVAL` | an argument is out of range |
| 23 | `ENFILE` | every one of the kernel's open files is taken; memfd_create: every shared-memory object |
| 24 | `EMFILE` | every descriptor of the process is taken |
| 25 | `ENOTTY` | ioctl: not a device, or a request the device doesn't know |
| 27 | `EFBIG` | write: past the largest file; ftruncate: past the largest shared-memory object |
| 28 | `ENOSPC` | no free block or inode on the disk |
| 29 | `ESPIPE` | lseek: a pipe or a device |
| 30 | `EROFS` | the filesystem is read-only |
| 32 | `EPIPE` | write: a pipe nobody reads any more |
| 36 | `ENAMETOOLONG` | a name longer than 14 bytes, or a path longer than `MAXPATH` |
| 38 | `ENOSYS` | no such system call |
| 39 | `ENOTEMPTY` | unlink: a directory with entries |
| 75 | `EOVERFLOW` | open: a file bigger than the kernel's offsets reach; mmap: past the largest object |
| 88 | `ENOTSOCK` | a socket call on a descriptor that is not a socket |
| 89 | `EDESTADDRREQ` | send: a UDP socket with no peer, and no address given |
| 90 | `EMSGSIZE` | a datagram too big for one packet |
| 92 | `ENOPROTOOPT` | setsockopt, getsockopt: an option sockets don't have |
| 93 | `EPROTONOSUPPORT` | socket: a protocol the type doesn't have |
| 94 | `ESOCKTNOSUPPORT` | socket, socketpair: a local socket of a type other than `SOCK_STREAM` |
| 95 | `EOPNOTSUPP` | listen, accept: not on a UDP socket; mmap: a flag MAP_SHARED_VALIDATE refuses |
| 97 | `EAFNOSUPPORT` | socket: a family neither `AF_INET` nor `AF_UNIX`; a TCP or UDP socket's address not `AF_INET` |
| 98 | `EADDRINUSE` | a port that is taken |
| 99 | `EADDRNOTAVAIL` | bind: an address that is not this host's |
| 101 | `ENETUNREACH` | no route: the interface has no address, or no gateway |
| 104 | `ECONNRESET` | TCP: the peer reset the connection |
| 105 | `ENOBUFS` | the network's buffers or sockets are all in use |
| 106 | `EISCONN` | connect: the socket has connected already |
| 107 | `ENOTCONN` | the socket is not connected |
| 110 | `ETIMEDOUT` | ping: no reply in time; TCP: the peer stopped answering |
| 111 | `ECONNREFUSED` | TCP: nothing listens on that port |
| 113 | `EHOSTUNREACH` | ARP got no answer from the address |

`strerror` in `kernel/user/lib/strerror.c` has a short message for each.

## File descriptors

A descriptor is a small number, 0 to 15, that stands for an open file: a
file or directory of the filesystem, a device (the console), one end of a
pipe, or a socket. Each process has its own 32, which all its threads share; they point into the kernel's table of 64
open files, which is where the access mode (reading, writing or both) and,
for a file on the disk, the offset live. `dup`, `fcntl`'s `F_DUPFD`, and
`spawn` for descriptors 0 to 2, make a second descriptor for the same open
file, which then shares its offset; `open` makes a new open file every
time, with its own. The open file goes once the last descriptor for it is
closed. A descriptor's one flag, `FD_CLOEXEC` (`fcntl`, `O_CLOEXEC`), is
its own: `exec` closes the descriptors that have it, and `spawn` passes
none of them on.

The first process (init, or the shell) starts with the console as 0 (stdin, for
reading), 1 (stdout) and 2 (stderr), both for writing. Every other process
gets 0, 1 and 2 from its parent at `spawn`, whatever they are by then: the
shell redirects a program's input or output by putting a file or a pipe at
0 or 1 (`close`, then `dup` or `open`, which take the lowest free number),
spawning it, and putting its own back from copies it keeps above 2, which
no child inherits. For `exec PROG > FILE`, where PROG keeps every
descriptor but those that close on exec, the shell makes its copies with
`fcntl`'s `F_DUPFD_CLOEXEC`: the kernel closes them as PROG starts, and if
PROG can't be started the shell puts its own back from them.

## Paths

A path names a file, a directory or a device node in the root filesystem
(`docs/camelot-fs.md`, the disk the kernel mounts at boot): names separated
by `/`, looked up from the root if it starts with `/`, else from the
caller's working directory (`chdir`; the root for the shell, and inherited
at `spawn`). Repeated slashes count as one and a trailing one is ignored;
`.` and `..` are ordinary entries of every directory (the root's `..` is
itself). Every name but the last must be a directory (`-ENOTDIR`), and each
must exist (`-ENOENT`). A name is at most 14 bytes, a path at most
`MAXPATH` (127) bytes without its NUL (`-ENAMETOOLONG` for either). The
empty path names nothing (`-ENOENT`).

Without a root filesystem (no disk, or none the kernel could mount), every
call that takes a path fails with `-ENOENT`, but `spawn` and `exec`: the
programs built into the kernel stand in for `/bin` ("Running programs").

Every change a call makes to the disk is a transaction of the filesystem's
log: `open` making or emptying a file, `mkdir`, `unlink`, `link`, and
closing (or `chdir`ing away from) something removed while in use, which
frees it. A crash, or the machine stopping, leaves all of each or none of
it. A `write` is the one exception: it goes to the disk 3 KiB per
transaction, so a crash part way through a bigger one leaves the file with
some of the bytes, a whole number of those pieces, never a torn block.

## Pipes

A pipe is a buffer of 512 bytes between its two ends, each an open file
(`pipe`). A write waits, sleeping, while the pipe is full, and returns once
all its bytes are in; a read waits while it is empty and returns what is
there. Each wakes the other as it makes room or data. When the last
descriptor for the write end is closed, a reader gets what is left, then 0,
the end of the input; when the last one for the read end is, a writer gets
`-EPIPE` (or, for a write that had put some bytes in before, their count).
`lseek` on a pipe is `-ESPIPE`, and `fstat` gives `T_PIPE` and the bytes
waiting. A pipe's buffer is a page, freed with its last end.

So the shell runs `a | b`: it makes a pipe, puts its write end at 1 and
spawns `a`, puts the read end at 0 and spawns `b`, and closes its own
copies, so that `a`'s exit closes the last write end and `b` sees the end of
its input.

## Running programs

`spawn` and `exec` take the path of an executable: a regular file of the
root filesystem (a directory or a device is `-EACCES`), which the kernel
reads as it loads it. The shell runs `NAME` as `/bin/NAME`, where `cargo
xtask fs` puts the programs of `kernel/user/`.

Without a root filesystem, the same programs are built into the kernel, and
the kernel finds them as `/bin/NAME` (or `bin/NAME`, from the root, which is
then everyone's working directory): nothing else exists. The shell itself is
started as `/bin/sh`, from the filesystem when there is one and it has a
`/bin/sh`, else the kernel's own.

## Starting a program

A program is an ELF64 executable, RV64 code (an ELF32 one, RV32 code, is
refused: there is no compat mode; anything else is `-ENOEXEC`), linked
with `kernel/user/user.ld`: code from 0x1000 (page 0 is never mapped, so a
null pointer faults), each output section on a page boundary so the kernel
can map each with its own permissions, and a heap after the last one. User
space is the lower half of the address space, as Linux's riscv64
`TASK_SIZE`: 128 TiB in Sv48, 256 GiB in Sv39 (on a hart without Sv48),
with a stack of 8 MiB at most at its top. In Sv48:

```
0x0000_0000_0000_0000  unmapped
0x0000_0000_0000_1000  .text (R+X), .rodata (R), .data and .bss (R+W)
                       heap (R+W), grown by sbrk
                       ...
0x0000_7FFF_FF7F_F000  guard page, unmapped (USER_LOAD_TOP)
0x0000_7FFF_FF80_0000  the stack's limit (USER_STACK_LIMIT): 8 MiB of stack at most
                       ...
                       the stack (R+W), grown down on demand
0x0000_7FFF_FFFF_F000  the stack's first page, mapped from the start
0x0000_8000_0000_0000  USER_TOP (Sv39: 0x0000_0040_0000_0000); above it
                       the kernel's half, from 0xFFFF_8000_0000_0000
```

The stack is mapped as it grows: a load or store just below it maps the
pages down to there, and every system call first maps it down to the
caller's `sp`, so a buffer in a stack frame the program hasn't touched yet
is good for `read`. Past 8 MiB is the guard page: a fault, which kills the
process. So is running out of RAM while it grows; the kernel says so first.

Whether from `spawn` or `exec`, the kernel starts the program as Linux
starts an ELF program (`create_elf_tables` in `fs/binfmt_elf.c`) and the
RISC-V psABI describes it ("Process Initialization"): at
its ELF entry point, crt0's `_start`, with every register zero except
`sp`, which is 16-byte aligned and points at `argc`. `a0` is zero too: the
psABI's address of a function for the program to register with `atexit`
(a dynamic linker's), and there is none. Everything is at the top of the
stack's first page, each word a `long`, a pointer's size (8 bytes):

```
USER_TOP     a word of zeros, the end marker
             the path the program was started with (AT_EXECFN),
             NUL-terminated
             the environment's strings, envp[0]'s first, each
             NUL-terminated
             the argument strings, argv[0]'s first, each NUL-terminated
             padding to a multiple of 16
             16 random bytes (AT_RANDOM)
             padding
             the auxiliary vector: pairs of words, a type and a value,
             the last pair AT_NULL
envp         envp[0] ... envp[envc - 1], then a null pointer
argv         argv[0] ... argv[argc - 1], then a null pointer
sp           argc
```

The strings are in the order Linux copies them down from the top (the
path, then the environment's, then the arguments', each array's in its
own order: `copy_string_kernel` and `copy_strings` in `fs/exec.c`), one
right after another, so argv[0]'s string is the lowest and envp's last
ends right below the path; each pointer points at its string. The first
process (init, or the shell) gets the environment Linux gives init
(`envp_init` in `init/main.c`): `HOME=/` and `TERM=linux`.

The auxiliary vector has Linux's entries, in Linux's order, with Linux's
type numbers (`AT_*` in `user.h` and `kernel/include/syscall.h`), but for
those Camelot has nothing to say with (no vDSO, no cache information, no
signals, no platform string):

| Number | Type | Value |
|---|---|---|
| 16 | `AT_HWCAP` | the ISA's single-letter extensions, bit `X - 'A'` for each: I, M and A (0x1101) |
| 6 | `AT_PAGESZ` | 4096 |
| 17 | `AT_CLKTCK` | how often `uptime` ticks a second: 10 |
| 3 | `AT_PHDR` | the address of the program headers, in the `PT_LOAD` segment whose file bytes hold them, as Linux finds them; 0 if none does (`user.ld` starts the first segment at 0x1000, past them) |
| 4 | `AT_PHENT` | a program header's size: 56 (ELF64) |
| 5 | `AT_PHNUM` | how many there are |
| 7 | `AT_BASE` | 0: no interpreter, the programs are static |
| 8 | `AT_FLAGS` | 0 |
| 9 | `AT_ENTRY` | the program's entry point |
| 11 | `AT_UID` | 0, as are the next three: Camelot has one user |
| 12 | `AT_EUID` | 0 |
| 13 | `AT_GID` | 0 |
| 14 | `AT_EGID` | 0 |
| 23 | `AT_SECURE` | 0 |
| 25 | `AT_RANDOM` | the address of the 16 random bytes: different for every program started, but not secret (Camelot has no source of entropy; they mix the time and the stack page's address) |
| 31 | `AT_EXECFN` | the address of the path |
| 0 | `AT_NULL` | 0, the end |

`MAXARG`, `ARG_MAX` and `MAXPATH` keep all of it in the first half of the
page. Every page the program gets (its segments beyond their file bytes,
`.bss`, the stack, the heap) reads zero. `_start` loads `argc` from `sp`,
finds `argv` right above it and `envp` past `argv`'s null pointer, makes
`envp` the C library's `environ` (as glibc's `__libc_start_main` does),
calls `main(argc, argv, envp)` and passes its return value to `exit`. A
program that wants the auxiliary vector walks past `envp`'s null pointer
to it.

## Polling

`poll(fds, nfds, timeout)` waits until one of `nfds` descriptors is ready,
as Linux's does: each `struct pollfd` names a descriptor (`fd`; a negative
one is skipped) and what to wait for (`events`), and the call fills in
`revents`, what the descriptor is ready for of those, plus `POLLERR`,
`POLLHUP` and `POLLNVAL`, which come whether asked for or not. It returns
the number of descriptors whose `revents` is not 0; or 0 once `timeout`
milliseconds have passed with none, counted in ticks (a tenth of a second)
as `sleep` counts them, rounded up; a negative `timeout` waits for as long
as it takes, and 0 not at all. At most 64 descriptors (`-EINVAL` above;
one may be named several times), and `-EINTR` if the process is killed
while it waits. The bits are Linux's:

| Bit | Name | Meaning |
|---|---|---|
| 0x001 | `POLLIN` | a read would not wait |
| 0x004 | `POLLOUT` | a write would not wait |
| 0x008 | `POLLERR` | an error: a pipe's reader is gone, a connection ended in one |
| 0x010 | `POLLHUP` | the other end is gone: a pipe's writer, a closed connection |
| 0x020 | `POLLNVAL` | not an open descriptor |
| 0x040 | `POLLRDNORM` | with `POLLIN` |
| 0x100 | `POLLWRNORM` | with `POLLOUT` |
| 0x2000 | `POLLRDHUP` | a stream socket's peer will send no more |

`POLLPRI`, `POLLRDBAND` and `POLLWRBAND` are never set. What each kind of
file says:

- **a pipe**: the read end is ready to read with bytes in it, and hung up
  once no write end is open; the write end ready to write with room in it,
  and in error once no read end is open;
- **a file, a directory or a shared-memory object**: always ready to read
  and write, as on Linux;
- **the console**: always ready to write; ready to read once a read would
  not wait, by default once a whole line has been typed, or Ctrl-D, which
  `poll` edits and echoes as `read` would ("Terminals"), on a descriptor
  open for reading;
- **a pseudo-terminal** ("Terminals"): the slave is ready to read as the
  console is, and to write with room for the master; hung up (and ready to
  read: the end) once the master is closed. The master is ready to read
  with output waiting, to write with room in the slave's input, and hung
  up once the slave has been closed;
- **a socket** ("Sockets"): a TCP listener is ready to read with a
  connection to accept; a connection is ready to read with bytes or the
  peer's FIN (`POLLRDHUP`) waiting, or once it ended in an error, and to
  write with room in its send buffer; one never connected is ready to write
  and hung up, as on Linux. A UDP socket is always ready to write, and to
  read with a datagram waiting;
- **a local socket** ("Local sockets"): connected, it is ready to read
  with bytes waiting, an error to read (`POLLERR` too) or its peer done
  sending (`POLLRDHUP`), hung up once neither end will send (its peer
  closed, or both shut down), and ready to write with room in its peer's
  ring or once a write would fail at once; one never connected is ready to
  write and hung up, as on Linux; a listener is ready to read with a
  connection to accept;
- **an input device** ("Input devices"): ready to read with an event
  waiting;
- **`/dev/fb0`** ("The framebuffer"): ready to write unless a flip
  `FBIOPAN_DISPLAY` queued is still waiting for the vertical blank, so a
  program that queues a flip and polls for `POLLOUT` learns when the frame
  it no longer shows may be drawn into, as DRM's page-flip event tells it;
- **any other device**: always ready to read and write.

The descriptors' files are held for the whole call, so one another thread
closes meanwhile goes on being polled. No change is missed: each file names
the wait channels of its readers and writers before it is looked at, and a
wakeup on any of them, from then on, has the call look at every file again
(spec section 10, "Waiting on several channels").

## Console input

The console is a terminal ("Terminals"), whose defaults are what it has
always done: `read` on stdin returns whole lines, edited as they are typed,
in canonical mode. The kernel echoes the input as a program reads it, so
input typed ahead appears after the prompt that asks for it rather than in
the middle of other output:

- printable characters (0x20-0x7E) are kept and echoed, up to 127 a line;
  the rest of a longer line is dropped, neither echoed nor kept;
- BS (0x08) and DEL (0x7F) erase the last character, echoing BS, space, BS;
- CR, LF or CR LF ends the line, which is returned with a LF at the end (a
  LF right after a CR is skipped, so raw consoles and piped text both work);
- Ctrl-D (0x04) ends the line without a newline; on an empty line, the read
  returns 0, the end of the input;
- Ctrl-C (0x03) kills the foreground process, if one is set (`tcsetpgrp`;
  the shell sets it to each program it waits for) and still running,
  echoing `^C` and a newline and throwing away the line; without one it is
  ignored, as are the other control characters.

A line is at most 128 bytes with its newline, so a 129-byte buffer holds any
line and its NUL.

## The user library

Programs are compiled freestanding with only `user.h`, which declares the
calls above (the stubs in `usys.s`, which return the raw result, a negative
error number included), their constants and structures (`O_*`, `F_*`,
`FD_CLOEXEC`, `SEEK_*`,
`struct stat`, `struct dirent`, `T_*`, `struct procinfo`, `PROC_*`,
`struct hartinfo`,
`struct netinfo`, `struct net_stats`, `NETINFO_*`, `struct ping_reply`,
`AF_INET`, `SOCK_*`, `IPPROTO_*`, `INADDR_*`, `SHUT_*`, `SOL_SOCKET`,
`SO_RCVTIMEO`, `struct in_addr`, `struct sockaddr_in`, `struct timeval`,
`MAXPATH`, `DIRSIZ`, `AT_*`), the integer types of `<stdint.h>` and
`<sys/types.h>` (`int8_t` to `int64_t`, `uint8_t` to `uint64_t`, the 64-bit
ones `long long`; `intptr_t`, `uintptr_t`, `size_t`, `ssize_t`, `off_t`,
`time_t`, a `long` wide) and their limits (`INT_MAX`, `UINT_MAX`,
`LONG_MAX`, `ULONG_MAX`, `LLONG_MAX`, `SIZE_MAX`, `INT64_MAX`, ...), and the C library in `kernel/user/lib/`, described in
its `README.md`: strings and memory (`strlen`, `strcmp`, `strncmp`,
`strcpy`, `strcat`, `strchr`, `strrchr`, `strstr`, `memset`, `memcpy`,
`memmove`, `memcmp`), numbers (`atoi`, `strtol`), the heap (`malloc`,
`free`, `calloc`, `realloc`, on top of `sbrk`), formatted output (`printf`,
`fprintf` to a descriptor, `snprintf`; `%d %u %x %p %s %c %%` with `-`, `0`
and a width, and `l` for a `long`, `ll` for a `long long`, `z` for a
`size_t`), `puts`, line input (`getline`, one read of the console
without the newline; readers, `reader_getline`, for files and pipes),
`strerror`, `stat` (`open`, `fstat`, `close`), `spin` (a busy loop), the
environment (`environ`, which crt0 sets to `main`'s `envp`, and `getenv`,
`setenv`, `unsetenv` over it, as POSIX has them but for returning a
negative error number),
addresses and names (`htons`, `ntohs`, `htonl`, `ntohl`, `inet_aton`,
`inet_addr`, `inet_ntoa`, `sockaddr_set`; `resolve`, a DNS client, and
`resolve_error`), and threads (`thread_start`; the atomics `atomic_swap`,
`atomic_add`, `atomic_cas`, `atomic_load`, `atomic_store`; `struct
spinlock` with `spin_lock` and `spin_unlock`; `struct mutex` with
`mutex_lock`, `mutex_trylock` and `mutex_unlock`, on the futex calls).
`malloc` and its kin may be called from several threads at once; so may
the rest, but `resolve` (which keeps its DNS messages in static buffers)
and `inet_ntoa` (which returns one), and a `struct reader` must not be
shared without a lock. `setenv` and `unsetenv` hold a lock of their own,
as glibc's do (its `envlock`), so they may run in several threads at
once, but not alongside `getenv` or anything else that reads `environ`
(a `setenv` may move the array): as in glibc, and as POSIX allows, where
none of the three need be thread-safe, a program that changes its
environment while another thread reads it keeps the two apart itself.
`syscall(number, a0, a1, a2)` makes any call by number, for tests, and
returns its `long` result.

## Building programs

Each `kernel/user/X.c`, `X.s` or `X.S` (assembly that goes through the C
preprocessor first, with `include/asm.h`) is a program called `X`, linked as
a C program is on Linux: the start code `crt0.S` first, then the program,
then the C library, `libc.a`, the files of `kernel/user/lib/` (the stubs
`usys.s` among them) in an archive, of which it gets the members it needs:
a member goes in only if it defines something the program (or another
member that went in) refers to and nothing else defines
(`crates/merlin-ld/src/library.rs`, in ld.lld's order). Our own tools build
them, with no LLVM: merlin compiles each C file (with `-Ikernel/user`),
merlin-as assembles each source, merlin-ld links each program with
`user.ld`, and the executables go to `/bin/X` on the root image (`cargo
xtask fs`, which puts `user.h` in `/usr/include` and `crt0.o`, `libc.a` and
`user.ld` in `/usr/lib` too, so that `/bin/as` and `/bin/ld` build a
program on Camelot as the host does,
`crates/xtask/src/fs.rs`) and are embedded in the kernel, for when there is
no disk, through a `userprogs.s` the kernel build generates
(`crates/xtask/src/userprogs.rs`; not committed).

After changing anything in `kernel/user/`, run

```
cargo xtask kernel
```

which rebuilds the programs and `kernel/kernel.elf` (and
`kernel/kernel-disk.elf`); commit them (and run `cargo xtask fs` for a root
image with the new programs). `cargo test` checks, without LLVM, that the
committed kernels are what the current sources and tools give. `cargo
xtask kernel --cc clang` builds the reference kernel instead, with the
programs' C compiled by clang `-O2` (`-S`, with the flags in
`userprogs.rs`), in `target/kernel/clang`.
