# Camelot system calls

The interface between Camelot's user programs and its kernel: how a program
starts, how it calls the kernel, what each call does and how it fails. The
kernel side is `kernel/syscall.c` and `kernel/sysfile.c` (with
`kernel/proc.c`, `kernel/exec.c`, `kernel/file.c`, `kernel/pipe.c`,
`kernel/console.c`, `kernel/fs.c` and `kernel/vm.c`; the network's calls
in `kernel/net/net.c` and the sockets' in `kernel/net/socket.c`,
`docs/camelot-net.md`), the numbers are in
`kernel/include/syscall.h`; the user side is `kernel/user/usys.s` (one stub
per call), `kernel/user/user.h` (their C declarations, the error numbers,
`open`'s flags and `struct stat`) and `kernel/user/crt0.S` (the start-up
code). How processes, address spaces, the scheduler and open files work
inside the kernel is in `docs/excalibur-spec.md`, section 10, "Kernel
processes" and "Kernel files"; the filesystem's format is in
`docs/camelot-fs.md`.

The numbers in the tables below are checked against `syscall.h`, `usys.s`
and `user.h` by `cargo test` (`crates/xtask/tests/kernel.rs`): change all of
them together.

Camelot runs on RV32 and RV64 while the switch to 64 bits happens (#141):
the same calls, with Linux's widths for each but for RV32's `off_t` and
`time_t` (below). An RV32 program is ILP32
(`int`, `long` and pointers 32 bits), an RV64 one LP64 (`int` 32 bits,
`long` and pointers 64), and every type below is the C type it says, at
that width: `size_t`, `ssize_t`, `off_t` and `time_t` are a `long` wide, so
64-bit on RV64, as on Linux. On RV32 that leaves `off_t` and `time_t` 32-bit,
which riscv32 Linux's are not (its glibc has only 64-bit ones): a stopgap
until RV32 goes (#159), which limits RV32's files to 2^31 - 1 bytes
(`open`). "Structures" gives what the widths make of each structure's
size.

## Calling convention

The Linux RISC-V convention, which is also the C calling convention's, so a
stub is two instructions:

- The program executes `ecall` in U-mode with the call number in `a7` and
  the arguments in `a0`-`a5` (`setsockopt` takes five, the most). An
  argument is a register, read at its C type: an `int` is the register's
  low 32 bits (which the calling convention keeps sign-extended on RV64), a
  pointer, a `size_t` or an `off_t` all of it. The number is all of `a7`, a
  `long`: on RV64, 2^32 + 4 is no call, not `getpid`.
- The result comes back in `a0`: a `long`, sign-extended to the register's
  width, so an error is negative to a 64-bit program too (-14 is
  0xffff_ffff_ffff_fff2, not 0x0000_0000_ffff_fff2). A value from -4095 to
  -1 is an error: minus one of the error numbers below. Every other value
  is a success: a count, a descriptor, or for `sbrk` an address (user
  addresses are below `USER_TOP`, so never in that range).
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
declare them, in bytes. The ones made of fixed-width fields are the same on
both widths, as their Linux counterparts are; the three with a `long`, a
`size_t` or an `off_t` grow on RV64, each such field 8-byte aligned. The
test that checks the tables compiles both headers for both widths and
checks each size.

| Structure | RV32 | RV64 | What |
|-----------|------|------|------|
| `struct stat` | 16 | 24 | `fstat`'s: `size` is an `off_t` |
| `struct dirent` | 16 | 16 | a directory's entries, as `read` gives them |
| `struct procinfo` | 32 | 40 | `procs`': `size` is a `size_t` |
| `struct hartinfo` | 16 | 16 | `harts`' |
| `struct net_stats` | 108 | 108 | the network's counters |
| `struct netinfo` | 132 | 132 | `netinfo`'s |
| `struct ping_reply` | 16 | 16 | `ping`'s |
| `struct timeval` | 8 | 16 | `SO_RCVTIMEO`'s: two `long`s, `tv_sec` a `time_t` |
| `struct sockaddr_in` | 16 | 16 | an IPv4 address and port |

## Calls

| Number | Name | Arguments | Returns |
|---|---|---|---|
| 1 | `exit` | `int status` | does not return |
| 2 | `write` | `int fd, const void *buf, size_t len` | bytes written; `-EBADF`, `-EFAULT`, `-EPIPE`, `-ENOSPC`, `-EFBIG`, `-EINVAL`, `-ENOMEM` |
| 3 | `read` | `int fd, void *buf, size_t len` | bytes read, 0 at the end of the input; `-EBADF`, `-EFAULT`, `-ENOMEM` |
| 4 | `getpid` | | the caller's pid |
| 5 | `yield` | | 0 |
| 6 | `sleep` | `uint ticks` | 0 |
| 7 | `spawn` | `const char *path, char *const argv[]` | the child's pid; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EACCES`, `-E2BIG`, `-ENOEXEC`, `-EAGAIN`, `-ENOMEM`, `-EFAULT` |
| 8 | `wait` | `int pid, int *status` | the pid of the child collected; `-ECHILD`, `-EFAULT` |
| 9 | `sbrk` | `long increment` | the old end of the heap, a pointer; `-ENOMEM`, `-EINVAL` |
| 10 | `uptime` | | timer ticks since boot |
| 11 | `open` | `const char *path, int flags` | a new descriptor; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EISDIR`, `-ENODEV`, `-EROFS`, `-EOVERFLOW`, `-ENOSPC`, `-EINVAL`, `-EMFILE`, `-ENFILE`, `-EFAULT` |
| 12 | `close` | `int fd` | 0; `-EBADF` |
| 13 | `lseek` | `int fd, off_t offset, int whence` | the new offset; `-EBADF`, `-ESPIPE`, `-EINVAL` |
| 14 | `fstat` | `int fd, struct stat *st` | 0; `-EBADF`, `-EFAULT` |
| 15 | `mkdir` | `const char *path` | 0; `-EEXIST`, `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-ENOSPC`, `-EROFS`, `-EFAULT` |
| 16 | `unlink` | `const char *path` | 0; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EINVAL`, `-ENOTEMPTY`, `-EROFS`, `-EFAULT` |
| 17 | `link` | `const char *old, const char *new` | 0; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EPERM`, `-EEXIST`, `-ENOSPC`, `-EROFS`, `-EFAULT` |
| 18 | `chdir` | `const char *path` | 0; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EFAULT` |
| 19 | `dup` | `int fd` | the new descriptor; `-EBADF`, `-EMFILE` |
| 20 | `pipe` | `int *fds` | 0; `-EMFILE`, `-ENFILE`, `-ENOMEM`, `-EFAULT` |
| 21 | `exec` | `const char *path, char *const argv[]` | does not return; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EACCES`, `-E2BIG`, `-ENOEXEC`, `-ENOMEM`, `-EFAULT` |
| 22 | `kill` | `int pid` | 0; `-ESRCH`, `-EPERM` |
| 23 | `procs` | `struct procinfo *buf, uint max` | the number of processes; `-EFAULT` |
| 24 | `netinfo` | `struct netinfo *info` | 0; `-ENODEV`, `-EFAULT` |
| 25 | `netconf` | `uint addr, uint mask, uint gateway` | 0; `-ENODEV`, `-EINVAL` |
| 26 | `ping` | `uint addr, uint seq, struct ping_reply *reply` | 0; `-ETIMEDOUT`, `-EHOSTUNREACH`, `-ENETUNREACH`, `-ENOBUFS`, `-ENODEV`, `-EINVAL`, `-EFAULT` |
| 27 | `socket` | `int domain, int type, int protocol` | a new descriptor; `-EAFNOSUPPORT`, `-EPROTONOSUPPORT`, `-EINVAL`, `-ENODEV`, `-EMFILE`, `-ENFILE`, `-ENOBUFS` |
| 28 | `bind` | `int fd, const struct sockaddr_in *addr` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EAFNOSUPPORT`, `-EADDRNOTAVAIL`, `-EADDRINUSE`, `-EINVAL`, `-ENOBUFS` |
| 29 | `listen` | `int fd, int backlog` | 0; `-EBADF`, `-ENOTSOCK`, `-EOPNOTSUPP`, `-EINVAL` |
| 30 | `accept` | `int fd, struct sockaddr_in *addr` | a new descriptor; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EOPNOTSUPP`, `-EINVAL`, `-EMFILE`, `-ENFILE`, `-EAGAIN`, `-EINTR` |
| 31 | `connect` | `int fd, const struct sockaddr_in *addr` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EAFNOSUPPORT`, `-EINVAL`, `-EISCONN`, `-ENETUNREACH`, `-ECONNREFUSED`, `-ECONNRESET`, `-ETIMEDOUT`, `-ENOBUFS`, `-EADDRINUSE`, `-EINTR` |
| 32 | `send` | `int fd, const void *buf, size_t len` | bytes sent; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EPIPE`, `-ENOTCONN`, `-ECONNRESET`, `-ETIMEDOUT`, `-EDESTADDRREQ`, `-EMSGSIZE`, `-ENETUNREACH`, `-ENOBUFS`, `-EINTR` |
| 33 | `recv` | `int fd, void *buf, size_t len` | bytes received, 0 at the end; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-ENOTCONN`, `-ECONNRESET`, `-ETIMEDOUT`, `-EINVAL`, `-EAGAIN`, `-EINTR` |
| 34 | `sendto` | `int fd, const void *buf, size_t len, const struct sockaddr_in *to` | bytes sent; `send`'s errors, `-EFAULT` also for `to`, `-EAFNOSUPPORT`, `-EINVAL`, `-EADDRINUSE` |
| 35 | `recvfrom` | `int fd, void *buf, size_t len, struct sockaddr_in *from` | bytes received; `recv`'s errors (`-EFAULT` also for `from`) |
| 36 | `shutdown` | `int fd, int how` | 0; `-EBADF`, `-ENOTSOCK`, `-EINVAL`, `-ENOTCONN`, `-ECONNREFUSED`, `-ECONNRESET`, `-ETIMEDOUT` |
| 37 | `getsockname` | `int fd, struct sockaddr_in *addr` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT` |
| 38 | `getpeername` | `int fd, struct sockaddr_in *addr` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-ENOTCONN` |
| 39 | `setsockopt` | `int fd, int level, int option, const void *value, uint len` | 0; `-EBADF`, `-ENOTSOCK`, `-EFAULT`, `-EINVAL`, `-ENOPROTOOPT` |
| 40 | `harts` | `struct hartinfo *buf, uint max` | the number of harts; `-EFAULT` |
| 41 | `thread_create` | `void (*entry)(void *), void *arg, void *stack` | the new thread's tid; `-EFAULT`, `-EINVAL`, `-EAGAIN`, `-ENOMEM` |
| 42 | `thread_exit` | `int status` | does not return |
| 43 | `thread_join` | `int tid, int *status` | 0; `-ESRCH`, `-EINVAL`, `-EINTR`, `-EFAULT` |
| 44 | `gettid` | | the caller's thread id |
| 45 | `futex_wait` | `int *addr, int val` | 0; `-EAGAIN`, `-EINVAL`, `-EINTR`, `-EFAULT` |
| 46 | `futex_wake` | `int *addr, int n` | the number of threads woken; `-EINVAL`, `-EFAULT` |
| 47 | `fcntl` | `int fd, int cmd, int arg` | the descriptor's flags, a new descriptor, or 0; `-EBADF`, `-EINVAL`, `-EMFILE` |

**exit(status)** ends the caller, every one of its threads ("Threads"
below): the others are stopped wherever they are, as by `kill`, and the
process ends once they have. Its exit status is kept for its parent's
`wait`; the slot and every page the process held are freed once the parent
has collected it (or at once, if it has no parent any more). Its descriptors
are closed and its working directory let go of first, as `close` would
(which may free a file it held open after its last name was removed). Its
children are orphaned: they keep running, and nobody collects them. When the
first process, the shell, exits, the kernel runs until every remaining
process has ended and then stops the machine with the shell's status as the
exit code (its low 8 bits, so `exit -2` gives 0xFE). That includes a process
blocked reading the console, which keeps the machine running until it gets a
line or the end of the input (`sh &`, then `exit` in the first shell, leaves
the second one reading). Note that 0xFE is also the code of a kernel panic:
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
kernel writes: on RV64 ext4's, 2^32 - 1 blocks; on RV32 2^31 - 1 bytes,
what its `off_t` reaches), `-EINVAL` (the offset is past the end of the
file, which another descriptor has emptied meanwhile), `-ENOMEM` (no page
free for copying through). To the console, the bytes are printed. To a
pipe, they wait in it for a reader ("Pipes"); `-EPIPE` if no read end is
open any more.

**read(fd, buf, len)** reads up to `len` bytes from the open file behind
`fd` into `buf` and returns how many it read; 0 means the end of the input.
`fd` must be open for reading, or it is `-EBADF`; all of `buf` must be
writable, or it is `-EFAULT` and nothing is read. `len` = 0 returns 0 without
waiting (except from a socket, where `read` is `recv`: on UDP it takes the
next datagram). From a file on the disk, the bytes come from the descriptor's
offset, which moves past them: `len` of them, fewer at the end of the file,
0 at or past it. A directory reads the same way, as its array of 16-byte
entries (`struct dirent` in `user.h`, the format in `docs/camelot-fs.md`).
From the console, a read waits for a whole line ("Console input"). From a
pipe, it waits while the pipe is empty and a write end is open, then returns
what is there, up to `len` bytes; 0 once it is empty and no write end is
left.

**getpid()** returns the caller's pid. Pids count from 1 (the shell) and are
never reused.

**yield()** gives the CPU to the other runnable processes, if any, and
returns 0 when the caller's turn comes again. A process is also preempted at
every timer tick (10 Hz), so yielding is only ever a courtesy.

**sleep(ticks)** returns 0 after at least `ticks` timer ticks (10 a second at
the nominal clock) have passed, without using the CPU meanwhile. `sleep(0)`
returns at once.

**spawn(path, argv)** starts the program at `path` ("Running programs"
below) as a new process, a child of the caller, and returns its pid. From
then on both take turns on the CPU with the other runnable processes. The
child gets copies of the caller's descriptors 0, 1 and 2 (sharing their
open files, offsets included; a closed one stays closed, and so does one
that closes on exec, since `spawn` stands for a fork and an exec, as
`posix_spawn` does) and no other, and the caller's working directory. Its name, in the kernel's messages, is the
last name of `path`. `argv` is the child's argument list, an array of string
pointers ending with a null pointer, `argv[0]` conventionally the program's
name; a null `argv` stands for `{ path, NULL }`. The kernel copies the
strings onto the child's stack ("Starting a program" below), so the caller's
array can go as soon as `spawn` returns. Errors:

- `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`: the path names nothing ("Paths");
- `-EACCES`: it names a directory or a device;
- `-E2BIG`: more than `MAXARG` (16) arguments, or their strings, with their
  NULs, take more than `ARG_MAX` (512) bytes;
- `-ENOEXEC`: the file is not a valid executable (the kernel also prints why
  on the console: `spawn: PATH: REASON`);
- `-EAGAIN`: every one of the 8 process slots is taken (zombies count until
  they are collected), or every one of the 16 thread slots;
- `-ENOMEM`: not enough free memory;
- `-EFAULT`: `path`, `argv` or one of the strings is not readable (a path
  longer than `MAXPATH` is still read to its NUL).

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
(`USER_LOAD_TOP`: 0x0B7F_F000 on RV32, 0x7FFF_FF7F_F000 in Sv48), when it
would grow by more pages than the whole of RAM has (refused at once, as
Linux's default overcommit check refuses it), or when memory runs out
(nothing changes), `-EINVAL` when it would shrink below its start.
A program whose segments end above 4 GiB has its heap there, and `sbrk`
returns those addresses whole.

**uptime()** returns the timer ticks since boot (10 a second at the nominal
clock), as an unsigned number.

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
- a device node (`/dev/console`), which gives the device itself: the
  console's reads and writes, in the access mode asked for, also on a
  read-only filesystem; a node whose major number has no driver is
  `-ENODEV`.

On RV32, a file bigger than 2^31 - 1 bytes is `-EOVERFLOW`: its `off_t` is
32-bit, so `lseek` could not reach all of it. That `off_t` is this
kernel's stopgap until RV32 goes (#159), not riscv32 Linux's, which is
64-bit and so opens such a file. The filesystem allows
files far bigger (docs/camelot-fs.md, "Inodes"), which an RV64 kernel,
whose `off_t` is 64-bit, opens, reads and writes (or mkfs makes).

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
stat` at `st` (16 bytes on RV32, 24 on RV64, `user.h`): `ino` (its inode, 0 for a pipe and for
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
does, for its sessions).
`-EBADF`, or `-EFAULT` if `st` is not writable.

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

**fcntl(fd, cmd, arg)** is Linux's for a descriptor's flags and for copies
of it, with Linux's numbers. Each descriptor has one flag, `FD_CLOEXEC`
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
  returns 0.

Errors: `-EBADF` if `fd` is not open, checked before `cmd` (so an unknown
`cmd` on a closed descriptor is `-EBADF`, as on Linux); `-EINVAL` for
another `cmd`, or for an `arg` of `F_DUPFD` or `F_DUPFD_CLOEXEC` that is
not a descriptor, 0 to 15 (Linux's answer for one at or past
`RLIMIT_NOFILE`); `-EMFILE` when no descriptor from `arg` up is free.

**pipe(fds)** makes a pipe and stores two new descriptors in `fds[0]` (the
read end) and `fds[1]` (the write end), the lowest two free ("Pipes").
Errors: `-EMFILE`, `-ENFILE`, `-ENOMEM` (no page for its buffer), or
`-EFAULT` if `fds` is not 8 writable bytes; nothing is opened then.

**exec(path, argv)** replaces the caller's program with the one at `path`:
its address space goes, and the new program starts as one started by
`spawn` does ("Starting a program"), with `argv`, in the same process. Its
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
own processes or for the first process, the shell, whose end would be
Camelot's (it ends only by its own `exit`).

**procs(buf, max)** describes the processes, in pid order, in the array of
`max` `struct procinfo` (32 bytes each on RV32, 40 on RV64, `user.h`) at `buf`, and returns how
many processes there are, which may be more than `max`: only the first
`max` are stored then, and `procs(NULL, 0)` just counts them. Each entry
holds `pid`; `ppid`, its parent's pid, or 0 for a process without one (the
shell, an orphan, a kernel process); `state`: `PROC_RUNNABLE` (1, waiting
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
IPv4 only (`AF_INET`), TCP (`SOCK_STREAM`) and UDP (`SOCK_DGRAM`). A socket
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
(`INADDR_ANY`) means any of this host's.

The checks come in a fixed order, as for the other calls: the descriptor
first (`-EBADF` if it is not open, `-ENOTSOCK` if it is not a socket),
then the pointers (`-EFAULT`; an address must be 16 readable or writable
bytes), then everything else. Without a network card, `socket` fails with
`-ENODEV`, so no other call ever sees a socket.

A call that waits (`accept` for a connection, `connect` for the handshake,
`send` for room in a full buffer, `recv` for data) sleeps without using the
CPU, and ends with `-EINTR` as soon as the process is killed; what the
socket had done stays done. `recv` and `accept` wait for ever, unless the
socket has a receive timeout (`setsockopt`): then they return `-EAGAIN`
once it has run out.

**socket(domain, type, protocol)** makes a socket and returns a new
descriptor for it, the lowest one free. `domain` is `AF_INET`; `type` is
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
`tv_usec`, two `long`s: 8 bytes on RV32, 16 on RV64, `len`), rounded up to
whole timer ticks, 0 for no limit (the default). Errors: `-EINVAL` (`len`
is not the structure's size, or a negative time, or
`tv_usec` of a second or more), `-EFAULT` (`value` is not readable),
`-ENOPROTOOPT` (another level or option).

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
process's threads, as no memory is shared between processes. Errors, for
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
starts, but another thread's `sbrk` may unmap part of the buffer while
the call is under way (a `write` to a full pipe, say). The call then
stops where the copy fails: `read` and `write` return the bytes moved
until then, or `-EFAULT` if none, and the bytes they couldn't copy are
neither written nor taken from the pipe; the calls that fill in a struct
(`fstat`, `procs`, `harts`, `pipe`, `getsockname`, `getpeername`) return
`-EFAULT`, and `pipe` closes the descriptors it made.

## Error numbers

Linux's values for the same conditions.

| Number | Name | Meaning |
|---|---|---|
| 1 | `EPERM` | link: not for a directory; kill: a kernel process, or the shell |
| 2 | `ENOENT` | no such file or directory |
| 3 | `ESRCH` | kill: no such process; thread_join: no such thread |
| 4 | `EINTR` | a wait ended because the process was killed |
| 7 | `E2BIG` | spawn, exec: argument list too long |
| 8 | `ENOEXEC` | spawn, exec: the file is not a valid executable |
| 9 | `EBADF` | not an open descriptor, or not open for that |
| 10 | `ECHILD` | wait: no such child |
| 11 | `EAGAIN` | spawn, thread_create: every slot is taken; futex_wait: the word is not the value; a socket's receive timeout ran out |
| 12 | `ENOMEM` | out of memory, or the heap would reach the stack |
| 13 | `EACCES` | spawn, exec: not a regular file |
| 14 | `EFAULT` | a pointer argument is not valid user memory |
| 17 | `EEXIST` | the name is taken |
| 19 | `ENODEV` | open: a device node with no driver; the network: no network card |
| 20 | `ENOTDIR` | a name on the way is not a directory |
| 21 | `EISDIR` | open: a directory, for writing |
| 22 | `EINVAL` | an argument is out of range |
| 23 | `ENFILE` | every one of the kernel's open files is taken |
| 24 | `EMFILE` | every descriptor of the process is taken |
| 27 | `EFBIG` | write: past the largest file |
| 28 | `ENOSPC` | no free block or inode on the disk |
| 29 | `ESPIPE` | lseek: a pipe or a device |
| 30 | `EROFS` | the filesystem is read-only |
| 32 | `EPIPE` | write: a pipe nobody reads any more |
| 36 | `ENAMETOOLONG` | a name longer than 14 bytes, or a path longer than `MAXPATH` |
| 38 | `ENOSYS` | no such system call |
| 39 | `ENOTEMPTY` | unlink: a directory with entries |
| 75 | `EOVERFLOW` | open: a file bigger than the kernel's offsets reach |
| 88 | `ENOTSOCK` | a socket call on a descriptor that is not a socket |
| 89 | `EDESTADDRREQ` | send: a UDP socket with no peer, and no address given |
| 90 | `EMSGSIZE` | a datagram too big for one packet |
| 92 | `ENOPROTOOPT` | setsockopt: an option sockets don't have |
| 93 | `EPROTONOSUPPORT` | socket: a protocol the type doesn't have |
| 95 | `EOPNOTSUPP` | listen, accept: not on a UDP socket |
| 97 | `EAFNOSUPPORT` | an address family other than `AF_INET` |
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
pipe, or a socket. Each process has its own 16, which all its threads share; they point into the kernel's table of 64
open files, which is where the access mode (reading, writing or both) and,
for a file on the disk, the offset live. `dup`, `fcntl`'s `F_DUPFD`, and
`spawn` for descriptors 0 to 2, make a second descriptor for the same open
file, which then shares its offset; `open` makes a new open file every
time, with its own. The open file goes once the last descriptor for it is
closed. A descriptor's one flag, `FD_CLOEXEC` (`fcntl`, `O_CLOEXEC`), is
its own: `exec` closes the descriptors that have it, and `spawn` passes
none of them on.

The first process, the shell, starts with the console as 0 (stdin, for
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

A program is an ELF executable of the kernel's own width, ELF32 for the
RV32 kernel and ELF64 for the RV64 one (an RV64 kernel runs no RV32
program: there is no compat mode, #141; anything else is `-ENOEXEC`),
linked with `kernel/user/user.ld`: code from
0x1000 (page 0 is never mapped, so a null pointer faults), each output
section on a page boundary so the kernel can map each with its own
permissions, and a heap after the last one. On RV32, user space is the low 192 MiB:

```
0x0000_0000  unmapped
0x0000_1000  .text (R+X), .rodata (R), .data and .bss (R+W)
             heap (R+W), grown by sbrk
             ...
0x0B7F_F000  guard page, unmapped (USER_LOAD_TOP)
0x0B80_0000  the stack's limit (USER_STACK_LIMIT): 8 MiB of stack at most
             ...
             the stack (R+W), grown down on demand
0x0BFF_F000  the stack's first page, mapped from the start
0x0C00_0000  USER_TOP: nothing above is accessible from U-mode
```

On RV64, user space is the lower half of the address space, as Linux's
riscv64 `TASK_SIZE`: 128 TiB in Sv48, 256 GiB in Sv39 (on a hart without
Sv48), with the same order and the same 8 MiB stack limit. In Sv48:

```
0x0000_0000_0000_0000  unmapped
0x0000_0000_0000_1000  .text (R+X), .rodata (R), .data and .bss (R+W)
                       heap (R+W), grown by sbrk
                       ...
0x0000_7FFF_FF7F_F000  guard page, unmapped (USER_LOAD_TOP)
0x0000_7FFF_FF80_0000  the stack's limit (USER_STACK_LIMIT)
                       ...
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
RISC-V psABI describes it ("Process Initialization"), at both widths: at
its ELF entry point, crt0's `_start`, with every register zero except
`sp`, which is 16-byte aligned and points at `argc`. `a0` is zero too: the
psABI's address of a function for the program to register with `atexit`
(a dynamic linker's), and there is none. Everything is at the top of the
stack's first page, each word a `long`, a pointer's size (4 bytes on RV32,
8 on RV64):

```
USER_TOP     a word of zeros, the end marker
             the path the program was started with (AT_EXECFN),
             NUL-terminated
             the argument strings, argv[0]'s first, each NUL-terminated
             (an environment's strings would go between the two: spawn
             and exec pass none)
             padding to a multiple of 16
             16 random bytes (AT_RANDOM)
             padding
             the auxiliary vector: pairs of words, a type and a value,
             the last pair AT_NULL
envp         a null pointer: the environment, empty
argv         argv[0] ... argv[argc - 1], then a null pointer
sp           argc
```

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
| 4 | `AT_PHENT` | a program header's size: 32 (ELF32) or 56 (ELF64) |
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
finds `argv` right above it and `envp` past `argv`'s null pointer, calls
`main(argc, argv, envp)` and passes its return value to `exit`. A program
that wants the auxiliary vector walks past `envp`'s null pointer to it.

## Console input

`read` on stdin returns whole lines, edited as they are typed, like a
terminal in canonical mode. The kernel echoes the input as a program reads
it, so input typed ahead appears after the prompt that asks for it rather
than in the middle of other output:

- printable characters (0x20-0x7E) are kept and echoed, up to 127 a line;
  the rest of a longer line is dropped, neither echoed nor kept;
- BS (0x08) and DEL (0x7F) erase the last character, echoing BS, space, BS;
- CR, LF or CR LF ends the line, which is returned with a LF at the end (a
  LF right after a CR is skipped, so raw consoles and piped text both work);
- Ctrl-D (0x04) ends the line without a newline; on an empty line, the read
  returns 0, the end of the input;
- other control characters are ignored.

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
`strerror`, `stat` (`open`, `fstat`, `close`), `spin` (a busy loop),
addresses and names (`htons`, `ntohs`, `htonl`, `ntohl`, `inet_aton`,
`inet_addr`, `inet_ntoa`, `sockaddr_set`; `resolve`, a DNS client, and
`resolve_error`), and threads (`thread_start`; the atomics `atomic_swap`,
`atomic_add`, `atomic_cas`, `atomic_load`, `atomic_store`; `struct
spinlock` with `spin_lock` and `spin_unlock`; `struct mutex` with
`mutex_lock`, `mutex_trylock` and `mutex_unlock`, on the futex calls).
`malloc` and its kin may be called from several threads at once; so may
the rest, but `resolve` (which keeps its DNS messages in static buffers)
and `inet_ntoa` (which returns one), and a `struct reader` must not be
shared without a lock.
`syscall(number, a0, a1, a2)` makes any call by number, for tests, and
returns its `long` result.

## Building programs

Each `kernel/user/X.c`, `X.s` or `X.S` (assembly that goes through the C
preprocessor first, for each width: `#if __riscv_xlen == 64`) is a program called `X`, linked with the
runtime, `crt0.S` and `usys.s`, and with the members of the library
(`kernel/user/lib/*.c`) it needs: like an archive's, a member goes in only
if it defines something the program (or another member that went in)
refers to and nothing else defines (`crates/merlin-ld/src/library.rs`,
merlin-ld's `--start-lib`/`--end-lib`). Our own tools build them, with no
LLVM: merlin compiles each C file (with `-Ikernel/user`), merlin-as
assembles each source, merlin-ld links each program with `user.ld`, and the
executables go to `/bin/X` on the root image (`cargo xtask fs`,
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
