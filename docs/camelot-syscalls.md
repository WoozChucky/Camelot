# Camelot system calls

The interface between Camelot's user programs and its kernel: how a program
starts, how it calls the kernel, what each call does and how it fails. The
kernel side is `kernel/syscall.c` (with `kernel/proc.c`, `kernel/console.c`
and `kernel/vm.c`), the numbers are in `kernel/include/syscall.h`; the user
side is `kernel/user/usys.s` (one stub per call), `kernel/user/user.h` (their
C declarations and the error numbers) and `kernel/user/crt0.s` (the start-up
code). How processes, address spaces and the scheduler work inside the kernel
is in `docs/excalibur-spec.md`, section 10, "Kernel processes".

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
written to the console, no input is consumed, no process is started. A bad
pointer never faults the kernel or kills the caller. An empty range (a
length of 0) is never a fault, whatever the pointer.

## Calls

| Number | Name | Arguments | Returns |
|---|---|---|---|
| 1 | `exit` | `int status` | does not return |
| 2 | `write` | `int fd, const void *buf, uint len` | `len`; `-EBADF`, `-EFAULT` |
| 3 | `read` | `int fd, void *buf, uint len` | bytes read, 0 at the end of the input; `-EBADF`, `-EFAULT` |
| 4 | `getpid` | | the caller's pid |
| 5 | `yield` | | 0 |
| 6 | `sleep` | `uint ticks` | 0 |
| 7 | `spawn` | `const char *name, char *const argv[]` | the child's pid; `-ENOENT`, `-E2BIG`, `-ENOEXEC`, `-EAGAIN`, `-ENOMEM`, `-EFAULT` |
| 8 | `wait` | `int pid, int *status` | the pid of the child collected; `-ECHILD`, `-EFAULT` |
| 9 | `sbrk` | `int increment` | the old end of the heap; `-ENOMEM`, `-EINVAL` |
| 10 | `uptime` | | timer ticks since boot |

**exit(status)** ends the caller. Its exit status is kept for its parent's
`wait`; the slot and every page the process held are freed once the parent
has collected it (or at once, if it has no parent any more). Its children
are orphaned: they keep running, and nobody collects them. When the first
process, the shell, exits, the kernel runs until every remaining process has
ended and then stops the machine with the shell's status as the exit code
(its low 8 bits, so `exit -2` gives 0xFE). Returning from `main` calls
`exit` with the return value (`crt0.s`).

A process that faults in U-mode (any exception but `ecall`: a page fault, an
illegal instruction such as `xcal.halt`, a misaligned access) is killed
instead: the kernel prints a report on the console (section 10, "Kernel
processes") and the process ends as if it had called `exit(-1)`.

**write(fd, buf, len)** prints the `len` bytes at `buf` on the console and
returns `len`. `fd` must be 1 (stdout) or 2 (stderr), which both go to the
console; anything else is `-EBADF`. All of `buf` must be readable, or
nothing is printed (`-EFAULT`). `len` = 0 prints nothing and returns 0.

**read(fd, buf, len)** reads console input into `buf`. `fd` must be 0
(stdin), or it is `-EBADF`; all of `buf` must be writable, or it is
`-EFAULT` and no input is taken. Input is line-buffered and edited by the
kernel ("Console input" below): a read waits, sleeping, until a whole line
has been typed, then returns up to `len` bytes of it. What it leaves of the
line goes to the next read, so a line longer than `len` takes several reads,
and a read never returns bytes from two lines. 0 means the end of the input
(Ctrl-D on an empty line). `len` = 0 returns 0 without waiting.

**getpid()** returns the caller's pid. Pids count from 1 (the shell) and are
never reused.

**yield()** gives the CPU to the other runnable processes, if any, and
returns 0 when the caller's turn comes again. A process is also preempted at
every timer tick (10 Hz), so yielding is only ever a courtesy.

**sleep(ticks)** returns 0 after at least `ticks` timer ticks (10 a second at
the nominal clock) have passed, without using the CPU meanwhile. `sleep(0)`
returns at once.

**spawn(name, argv)** starts the program called `name` (one of those built
into the kernel, `kernel/user/`) as a new process, a child of the caller,
and returns its pid. From then on both take turns on the CPU with the other
runnable processes. `argv` is the child's argument list, an array of string
pointers ending with a null pointer, `argv[0]` conventionally the program's
name; a null `argv` stands for `{ name, NULL }`. The kernel copies the
strings onto the child's stack ("Starting a program" below), so the caller's
array can go as soon as `spawn` returns. Errors:

- `-ENOENT`: no program is called `name`;
- `-E2BIG`: more than `MAXARG` (16) arguments, or their strings, with their
  NULs, take more than `ARG_MAX` (512) bytes;
- `-ENOEXEC`: the program's image is not a valid executable (the kernel also
  prints why on the console: `spawn: NAME: REASON`);
- `-EAGAIN`: every one of the 8 process slots is taken (zombies count until
  they are collected);
- `-ENOMEM`: not enough free memory;
- `-EFAULT`: `name`, `argv` or one of the strings is not readable.

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

## Error numbers

Linux's values for the same conditions.

| Number | Name | Meaning |
|---|---|---|
| 2 | `ENOENT` | spawn: no program by that name |
| 7 | `E2BIG` | spawn: argument list too long |
| 8 | `ENOEXEC` | spawn: the program's image is not a valid executable |
| 9 | `EBADF` | read, write: not an open file descriptor |
| 10 | `ECHILD` | wait: no such child |
| 11 | `EAGAIN` | spawn: every process slot is taken |
| 12 | `ENOMEM` | spawn, sbrk: out of memory, or the heap would reach the stack |
| 14 | `EFAULT` | a pointer argument is not valid user memory |
| 22 | `EINVAL` | an argument is out of range |
| 38 | `ENOSYS` | no such system call |

`strerror` in `kernel/user/ulib.c` has a short message for each.

## File descriptors

There is no file system yet, only the console, and every process has the
same three descriptors, which can't be closed or duplicated: 0 (stdin) for
`read`, 1 (stdout) and 2 (stderr) for `write`. Using one for the other
direction is `-EBADF`.

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

The kernel enters the program at its ELF entry point, crt0's `_start`, with
every register zero except:

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
error number included) and a small C library (`ulib.c`): `strlen`, `strcmp`,
`strchr`, `memset`, `memcpy`, `memmove`, `memcmp`, `atoi`, `printf` and
`fprintf` (`%d %u %x %p %s %c %%` with `-`, `0` and a width), `puts`,
`getline` (one read, without the newline), `strerror` and `spin` (a busy
loop). `syscall(number, a0, a1, a2)` makes any call by number, for tests.

## Building programs

Each `kernel/user/X.c` or `X.s` is a program called `X`, linked with the
runtime: `crt0.s`, `usys.s` and `ulib.c`. Our own tools build them, with no
LLVM: merlin-as assembles each source, merlin-ld links each program with
`user.ld`, and the executables are embedded in the kernel through the
generated `kernel/userprogs.s` (`crates/xtask/src/userprogs.rs`). Until
Merlin (#15) compiles this much C, the assembly for each C file comes from
clang, `-S` with the flags in `userprogs.rs`, and is committed in
`kernel/user/clang/`.

After changing anything in `kernel/user/`, run

```
cargo xtask kernel
```

which recompiles the C files with clang into `kernel/user/clang/`
(rewriting only the files that changed, deleting any whose C file is gone),
regenerates `kernel/userprogs.s` with merlin-as and merlin-ld, and rebuilds
`kernel/kernel.elf`; commit all three. `cargo test` checks, without LLVM,
that every C file has its committed assembly and that `userprogs.s` is what
the current sources and tools give; with the clang recorded in
`kernel/kernel.elf.clang-version` installed, it also checks that the
committed assembly is what that clang makes of the C files.
