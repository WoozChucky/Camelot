# Camelot system calls

The interface between Camelot's user programs and its kernel: how a program
starts, how it calls the kernel, what each call does and how it fails. The
kernel side is `kernel/syscall.c` and `kernel/sysfile.c` (with
`kernel/proc.c`, `kernel/exec.c`, `kernel/file.c`, `kernel/pipe.c`,
`kernel/console.c`, `kernel/fs.c` and `kernel/vm.c`; the network's calls
in `kernel/net/net.c`, `docs/camelot-net.md`), the numbers are in
`kernel/include/syscall.h`; the user side is `kernel/user/usys.s` (one stub
per call), `kernel/user/user.h` (their C declarations, the error numbers,
`open`'s flags and `struct stat`) and `kernel/user/crt0.s` (the start-up
code). How processes, address spaces, the scheduler and open files work
inside the kernel is in `docs/excalibur-spec.md`, section 10, "Kernel
processes" and "Kernel files"; the filesystem's format is in
`docs/camelot-fs.md`.

The numbers in the tables below are checked against `syscall.h`, `usys.s`
and `user.h` by `cargo test` (`crates/xtask/tests/kernel.rs`): change all of
them together.

## Calling convention

The Linux RISC-V convention, which is also the C calling convention's, so a
stub is two instructions:

- The program executes `ecall` in U-mode with the call number in `a7` and
  the arguments in `a0`-`a5` (no call takes more than three).
- The result comes back in `a0`. A value from -4095 to -1 is an error: minus
  one of the error numbers below. Every other value is a success.
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
W to write to it) and then copies through the physical pages. A range that
fails the check, such as a null pointer, a kernel address, the guard page
under the stack, memory past the end of the heap, read-only memory for a
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

## Calls

| Number | Name | Arguments | Returns |
|---|---|---|---|
| 1 | `exit` | `int status` | does not return |
| 2 | `write` | `int fd, const void *buf, uint len` | bytes written; `-EBADF`, `-EFAULT`, `-EPIPE`, `-ENOSPC`, `-EFBIG`, `-EINVAL`, `-ENOMEM` |
| 3 | `read` | `int fd, void *buf, uint len` | bytes read, 0 at the end of the input; `-EBADF`, `-EFAULT`, `-ENOMEM` |
| 4 | `getpid` | | the caller's pid |
| 5 | `yield` | | 0 |
| 6 | `sleep` | `uint ticks` | 0 |
| 7 | `spawn` | `const char *path, char *const argv[]` | the child's pid; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EACCES`, `-E2BIG`, `-ENOEXEC`, `-EAGAIN`, `-ENOMEM`, `-EFAULT` |
| 8 | `wait` | `int pid, int *status` | the pid of the child collected; `-ECHILD`, `-EFAULT` |
| 9 | `sbrk` | `int increment` | the old end of the heap; `-ENOMEM`, `-EINVAL` |
| 10 | `uptime` | | timer ticks since boot |
| 11 | `open` | `const char *path, int flags` | a new descriptor; `-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`, `-EISDIR`, `-ENODEV`, `-EROFS`, `-ENOSPC`, `-EINVAL`, `-EMFILE`, `-ENFILE`, `-EFAULT` |
| 12 | `close` | `int fd` | 0; `-EBADF` |
| 13 | `lseek` | `int fd, int offset, int whence` | the new offset; `-EBADF`, `-ESPIPE`, `-EINVAL` |
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
| 27 | `tcptest` | `uint op, uint a, uint b, uint c, struct tcptest_report *report` | 0, or a listener's handle; `-ECONNREFUSED`, `-ECONNRESET`, `-ETIMEDOUT`, `-ENETUNREACH`, `-EADDRINUSE`, `-ENOBUFS`, `-EINTR`, `-ENODEV`, `-EINVAL`, `-EFAULT` |

**exit(status)** ends the caller. Its exit status is kept for its parent's
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
can. Returning from `main` calls `exit` with the return value (`crt0.s`).

A process that faults in U-mode (any exception but `ecall`: a page fault, an
illegal instruction such as `xcal.halt`, a misaligned access) is killed
instead: the kernel prints a report on the console (section 10, "Kernel
processes") and the process ends as if it had called `exit(-1)`.

**write(fd, buf, len)** writes the `len` bytes at `buf` to the open file
behind `fd` ("File descriptors" below) and returns how many it wrote: `len`,
unless the disk filled up or a pipe lost its readers part way, when it is
the bytes written until then. `fd` must be open for writing, or it is
`-EBADF`; all of `buf` must be readable, or nothing is written (`-EFAULT`).
`len` = 0 writes nothing and returns 0. To a file on the disk, the bytes go
at the descriptor's offset, which moves past them, and the file grows if they
run past its end; a write that can write nothing fails with what stopped it:
`-ENOSPC` (the disk is full), `-EFBIG` (the file would pass the largest a
file can be, 274 432 bytes), `-EINVAL` (the offset is past the end of the
file, which another descriptor has emptied meanwhile), `-ENOMEM` (no page
free for copying through). To the console, the bytes are printed. To a
pipe, they wait in it for a reader ("Pipes"); `-EPIPE` if no read end is
open any more.

**read(fd, buf, len)** reads up to `len` bytes from the open file behind
`fd` into `buf` and returns how many it read; 0 means the end of the input.
`fd` must be open for reading, or it is `-EBADF`; all of `buf` must be
writable, or it is `-EFAULT` and nothing is read. `len` = 0 returns 0 without
waiting. From a file on the disk, the bytes come from the descriptor's
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
open files, offsets included; a closed one stays closed) and no other, and
the caller's working directory. Its name, in the kernel's messages, is the
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
  they are collected);
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
and returns the old end, so `sbrk(0)` returns the current one. The heap
starts empty, on the page after the program's last segment, and grows up
towards the stack: new pages are mapped R+W and read zero, including bytes
given back by an earlier shrink and grown again. A negative `increment`
shrinks it, unmapping and freeing the pages no longer used. Errors:
`-ENOMEM` when the heap would reach the guard page under the stack or memory
runs out (nothing changes), `-EINVAL` when it would shrink below its start.

**uptime()** returns the timer ticks since boot (10 a second at the nominal
clock), as an unsigned number.

**open(path, flags)** opens the file at `path` and returns a new descriptor
for it, the lowest one free. `flags` is one access mode, `O_RDONLY` (0),
`O_WRONLY` (1) or `O_RDWR` (2), plus any of `O_CREAT` (0x40: make the file,
empty, if `path` names nothing; open it as it is if it exists) and
`O_TRUNC` (0x200: empty a file opened for writing; ignored for reading and
for devices); anything else is `-EINVAL`. The offset starts at 0. What it
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

Errors: the path's (`-ENOENT`, `-ENOTDIR`, `-ENAMETOOLONG`), those above,
`-ENOSPC` (no free inode or block to make the file with), `-EMFILE` (all 16
of the caller's descriptors are taken), `-ENFILE` (all 64 of the kernel's
open files are), `-EFAULT`. Without a free descriptor and open file nothing
is made. Making the file, or emptying it, is one transaction: a crash leaves
it done or not.

**close(fd)** closes descriptor `fd`, which is free again. The open file
behind it goes with the last descriptor that shares it: a pipe end is closed
then ("Pipes"), and a file whose last name was removed meanwhile is freed.
`-EBADF` if `fd` isn't open.

**lseek(fd, offset, whence)** moves the offset of the file behind `fd` to
`offset` bytes from the start (`whence` = `SEEK_SET`, 0), from the current
offset (`SEEK_CUR`, 1) or from the end (`SEEK_END`, 2), and returns it.
Errors: `-EBADF`; `-ESPIPE` for a pipe or a device, which have no offset;
`-EINVAL` for another `whence`, or a new offset below 0 or past the end of
the file (files have no holes: they only grow by writing at their end).

**fstat(fd, st)** stores what the open file behind `fd` is in the `struct
stat` at `st` (16 bytes, `user.h`): `ino` (its inode, 0 for a pipe and for
the console the kernel gave the shell), `type` (`T_DIR` 1, `T_FILE` 2,
`T_DEVICE` 3, `T_PIPE` 4), `nlink` (the names it has; 0 for a file whose
last name was removed while it is open), `size` (bytes; for a pipe, the
bytes waiting in it; 0 for a device), `major` and `minor` (a device's
numbers, 1 and 0 for the console). `-EBADF`, or `-EFAULT` if `st` is not
writable.

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
(`-ENOENT` also for a removed directory to put `new` in), `-EPERM` if `old`
is a directory (a directory has one name, in its parent), `-EEXIST` if
`new` is taken (`/` included), `-ENOSPC`, `-EROFS`, `-EFAULT`.

**chdir(path)** makes the directory `path` the caller's working directory,
where every path that doesn't start with `/` is looked up from. `-ENOTDIR`
if it is not a directory; the path's errors; `-EFAULT`.

**dup(fd)** returns a new descriptor, the lowest one free, for the open file
behind `fd`: both share it, offset included, and it stays open until both
are closed. `-EBADF`, or `-EMFILE` if every descriptor is taken.

**pipe(fds)** makes a pipe and stores two new descriptors in `fds[0]` (the
read end) and `fds[1]` (the write end), the lowest two free ("Pipes").
Errors: `-EMFILE`, `-ENFILE`, `-ENOMEM` (no page for its buffer), or
`-EFAULT` if `fds` is not 8 writable bytes; nothing is opened then.

**exec(path, argv)** replaces the caller's program with the one at `path`:
its address space goes, and the new program starts as one started by
`spawn` does ("Starting a program"), with `argv`, in the same process. Its
pid, parent, descriptors (all of them) and working directory stay; its name
becomes the last name of `path`. On success it doesn't return; on an error,
nothing changed and the old program carries on with the error, `spawn`'s
but `-EAGAIN` (the kernel prints `exec: PATH: REASON` for `-ENOEXEC`).

**kill(pid)** ends the process `pid`: it exits with status -1, as a
process killed for a fault does (its parent's `wait` gets -1), but
silently, and not at once. The kernel marks it killed and, if it is asleep,
wakes it; it ends when it would next go back to U-mode, before it runs any
more of its own code (a process killed before its first turn runs none of
it). A system call it is in the middle of carries on to
its end, so a change to the disk is never left half done, except that the
calls that may wait for long give up at once: `sleep`, `wait`, a `read` of
the console or of a pipe, a `write` to a full pipe (what they return is
never seen). A process spinning in U-mode ends at the next timer tick. A
process may kill itself; `kill` then returns 0 and the process ends on its
way back from the call. Killing a zombie, which has already ended, does
nothing and returns 0. Errors: `-ESRCH` when no process has that pid (one
whose parent has collected it is gone), `-EPERM` for one of the kernel's
own processes or for the first process, the shell, whose end would be
Camelot's (it ends only by its own `exit`).

**procs(buf, max)** describes the processes, in pid order, in the array of
`max` `struct procinfo` (32 bytes each, `user.h`) at `buf`, and returns how
many processes there are, which may be more than `max`: only the first
`max` are stored then, and `procs(NULL, 0)` just counts them. Each entry
holds `pid`; `ppid`, its parent's pid, or 0 for a process without one (the
shell, an orphan, a kernel process); `state`: `PROC_RUNNABLE` (1, waiting
for its turn), `PROC_RUNNING` (2, the caller itself), `PROC_ZOMBIE` (3,
ended, its status not collected yet) or `PROC_SLEEPING` (4); `size`, the
end of its heap (its program, data and heap, from address 0, without the
stack page), 0 for a kernel process; and `name`, NUL-terminated (the last
name of the path it was started from, at most 15 characters). A process
that `spawn` is still loading is not listed. All of `buf`, `max` entries,
must be writable, however many are stored, or it is `-EFAULT` and nothing
is.

The network's calls are what the shell's `ifconfig` and `ping` built-ins
use until #85 brings sockets and network programs; `docs/camelot-net.md`
describes the stack behind them. Addresses are `uint`s in host order
(10.0.2.15 is 0x0a00020f). Without a network card each fails with
`-ENODEV` (after the pointer check).

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
to `addr`, with the caller's pid as its identifier and `seq` (0 to 65535)
as its sequence number, and waits up to one second (10 ticks) for the
matching echo reply; then stores a `struct ping_reply` at `reply` (who
answered, the reply's TTL, the ICMP message's length, 64, and the round
trip in microseconds of `mtime`) and returns 0. 127.0.0.1 and the
interface's own address answer through loopback. Errors: `-ETIMEDOUT` (no
reply in time), `-EHOSTUNREACH` (ARP got no answer from the next hop within
the last three seconds), `-ENETUNREACH` (the interface has no address, or
`addr` is off the subnet with no gateway), `-ENOBUFS` (no packet buffer
free), `-EINVAL` (`addr` 0, or `seq` over 65535) and `-EFAULT` (`reply` is
not 16 writable bytes; checked before anything is sent).

**tcptest(op, a, b, c, report)** is temporary: it drives the kernel's TCP
from a process until #85's socket calls replace it (`docs/camelot-net.md`,
"Testing TCP until #85"; the `tcptest` program is its user). `op` is one of:

- `TCPTEST_CONNECT` (`a` address, `b` port, `c` bytes): connects, sends `c`
  bytes of a fixed pattern (byte `i` is `i ^ i >> 8 ^ i >> 16`), ends its
  side and reads until the other side ends its own, then closes;
- `TCPTEST_LISTEN` (`a` port, `b` backlog, taken as 1 to 8): a listener, held by the
  caller until `TCPTEST_CLOSE` or its exit; returns its handle (0 to 3);
- `TCPTEST_ACCEPT` (`a` handle): waits for a connection, sends back what it
  sends until it ends its side, then closes it;
- `TCPTEST_CLOSE` (`a` handle): closes the listener, resetting the
  connections still waiting in its backlog.

`CONNECT` and `ACCEPT` store a `struct tcptest_report` (`user.h`, 28
bytes) at `report`, even when they fail: the connection's two ends (all 0
if it never had them), the bytes sent and received, and the FNV-1a hash of
what was received. The other two ignore `report`. Errors: what ended the
connection (`-ECONNREFUSED`, `-ECONNRESET`, `-ETIMEDOUT`), `-ENETUNREACH`
(no route), `-EADDRINUSE` (a port with a listener), `-ENOBUFS` (no socket,
handle or memory left), `-EINTR` (the process was killed while it waited),
`-EINVAL` (an unknown `op`, a handle the caller doesn't hold, a port of 0 or
over 65535, an address no connection can go to) and `-EFAULT` (`report` is not 28
writable bytes, for `CONNECT` and `ACCEPT`; checked first).

## Error numbers

Linux's values for the same conditions.

| Number | Name | Meaning |
|---|---|---|
| 1 | `EPERM` | link: not for a directory; kill: a kernel process, or the shell |
| 2 | `ENOENT` | no such file or directory |
| 3 | `ESRCH` | kill: no such process |
| 4 | `EINTR` | a wait ended because the process was killed |
| 7 | `E2BIG` | spawn, exec: argument list too long |
| 8 | `ENOEXEC` | spawn, exec: the file is not a valid executable |
| 9 | `EBADF` | not an open descriptor, or not open for that |
| 10 | `ECHILD` | wait: no such child |
| 11 | `EAGAIN` | spawn: every process slot is taken |
| 12 | `ENOMEM` | out of memory, or the heap would reach the stack |
| 13 | `EACCES` | spawn, exec: not a regular file |
| 14 | `EFAULT` | a pointer argument is not valid user memory |
| 17 | `EEXIST` | the name is taken |
| 19 | `ENODEV` | open: a device node with no driver |
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
| 90 | `EMSGSIZE` | a datagram too big for one packet (the kernel's UDP API) |
| 98 | `EADDRINUSE` | a UDP port that is taken (the kernel's UDP API) |
| 101 | `ENETUNREACH` | no route: the interface has no address, or no gateway |
| 104 | `ECONNRESET` | TCP: the peer reset the connection |
| 105 | `ENOBUFS` | the network's buffers or sockets are all in use |
| 106 | `EISCONN` | TCP: the socket is connected already (the kernel's TCP API) |
| 107 | `ENOTCONN` | TCP: the socket is not connected (the kernel's TCP API) |
| 110 | `ETIMEDOUT` | ping: no reply in time; TCP: the peer stopped answering |
| 111 | `ECONNREFUSED` | TCP: nothing listens on that port |
| 113 | `EHOSTUNREACH` | ARP got no answer from the address |

`strerror` in `kernel/user/lib/strerror.c` has a short message for each.

## File descriptors

A descriptor is a small number, 0 to 15, that stands for an open file: a
file or directory of the filesystem, a device (the console), or one end of a
pipe. Each process has its own 16; they point into the kernel's table of 64
open files, which is where the access mode (reading, writing or both) and,
for a file on the disk, the offset live. `dup`, and `spawn` for descriptors
0 to 2, make a second descriptor for the same open file, which then shares
its offset; `open` makes a new open file every time, with its own. The open
file goes once the last descriptor for it is closed.

The first process, the shell, starts with the console as 0 (stdin, for
reading), 1 (stdout) and 2 (stderr), both for writing. Every other process
gets 0, 1 and 2 from its parent at `spawn`, whatever they are by then: the
shell redirects a program's input or output by putting a file or a pipe at
0 or 1 (`close`, then `dup` or `open`, which take the lowest free number),
spawning it, and putting its own back from copies it keeps above 2, which
no child inherits. So does `exec`, which keeps every descriptor.

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

A program is an ELF32 executable linked with `kernel/user/user.ld`: code from
0x1000 (page 0 is never mapped, so a null pointer faults), each output
section on a page boundary so the kernel can map each with its own
permissions, and a heap after the last one. User space is the low 4 MiB:

```
0x0000_0000  unmapped
0x0000_1000  .text (R+X), .rodata (R), .data and .bss (R+W)
             heap (R+W), grown by sbrk
             ...
0x003F_E000  guard page, unmapped
0x003F_F000  stack page (R+W)
0x0040_0000  USER_TOP: nothing above is accessible from U-mode
```

Whether from `spawn` or `exec`, the kernel enters the program at its ELF
entry point, crt0's `_start`, with every register zero except:

- `a0` = `argc`, the number of arguments;
- `a1` = `argv`, the address of the argument array;
- `sp`, 16-byte aligned, just below the arguments.

The arguments sit at the top of the stack page:

```
USER_TOP     the strings, argv[0]'s first, each NUL-terminated
             padding to a multiple of 4
argv         argv[0] ... argv[argc - 1], then a null pointer
             padding to a multiple of 16
sp           the program's first stack frame grows down from here
```

Every page the program gets (its segments beyond their file bytes, `.bss`,
the stack, the heap) reads zero. `_start` calls `main(argc, argv)` and passes
its return value to `exit`.

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
error number included), their constants and structures (`O_*`, `SEEK_*`,
`struct stat`, `struct dirent`, `T_*`, `struct procinfo`, `PROC_*`,
`struct netinfo`, `struct net_stats`, `NETINFO_*`, `struct ping_reply`,
`TCPTEST_*`, `struct tcptest_report`, `MAXPATH`, `DIRSIZ`) and the C library in `kernel/user/lib/`, described in
its `README.md`: strings and memory (`strlen`, `strcmp`, `strncmp`,
`strcpy`, `strcat`, `strchr`, `strrchr`, `strstr`, `memset`, `memcpy`,
`memmove`, `memcmp`), numbers (`atoi`, `strtol`), the heap (`malloc`,
`free`, `calloc`, `realloc`, on top of `sbrk`), formatted output (`printf`,
`fprintf` to a descriptor, `snprintf`; `%d %u %x %p %s %c %%` with `-`, `0`
and a width), `puts`, line input (`getline`, one read of the console
without the newline; readers, `reader_getline`, for files and pipes),
`strerror`, `stat` (`open`, `fstat`, `close`) and `spin` (a busy loop).
`syscall(number, a0, a1, a2)` makes any call by number, for tests.

## Building programs

Each `kernel/user/X.c` or `X.s` is a program called `X`, linked with the
runtime, `crt0.s` and `usys.s`, and with the members of the library
(`kernel/user/lib/*.c`) it needs: like an archive's, a member goes in only
if it defines something the program (or another member that went in)
refers to and nothing else defines (`crates/merlin-ld/src/library.rs`,
merlin-ld's `--start-lib`/`--end-lib`). Our own tools build them, with no
LLVM: merlin compiles each C file (with `-Ikernel/user`), merlin-as
assembles each source, merlin-ld links each program with `user.ld`, and the
executables go to `/bin/X` on the root image (`cargo xtask fs`,
`crates/xtask/src/fs.rs`) and are embedded in the kernel, for when there is
no disk, through the generated `kernel/userprogs.s`
(`crates/xtask/src/userprogs.rs`).

After changing anything in `kernel/user/`, run

```
cargo xtask kernel
```

which regenerates `kernel/userprogs.s` and rebuilds `kernel/kernel.elf`;
commit both (and run `cargo xtask fs` for a root image with the new
programs). `cargo test` checks, without LLVM, that both are what the
current sources and tools give. `cargo xtask kernel --cc clang` builds the
reference kernel instead, with the programs' C compiled by clang `-O2`
(`-S`, with the flags in `userprogs.rs`), in `target/kernel/clang`.
