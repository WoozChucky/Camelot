# The user C library

The C library of Camelot's user programs: what `kernel/user/user.h`
declares besides the system calls. It is small, written in the C that
merlin compiles (so the same sources can one day be compiled on Camelot
itself), and has no `FILE`, no locale and no floating point. Every
function's contract is in `user.h`; this file says how the library is put
together.

| File | What it has |
|---|---|
| `string.c` | `strlen`, `strcmp`, `strncmp`, `strcpy`, `strcat`, `strchr`, `strrchr`, `strstr`, `memset`, `memcpy`, `memmove`, `memcmp` |
| `stdlib.c` | `atoi`, `strtol` (bases 2 to 36, or 0 for C's prefixes; out of range gives `LONG_MAX` or `LONG_MIN`) |
| `environ.c` | `environ`, the program's environment: `crt0.S` sets it to `main`'s `envp` before calling `main`, as glibc's `__libc_start_main` does, so every program has this member, and only this one of the environment's (as glibc's `posix/environ.c` is a file of its own) |
| `getenv.c` | `getenv`, `setenv`, `unsetenv` over `environ`, as POSIX has them, but for returning a negative error number (`-EINVAL` for an empty name or one with a `=`, `-ENOMEM`): `setenv` adds a variable by growing the array it made last with `realloc` (none the first time), copying `environ`'s pointers into it when `environ` is another array, as glibc's `__add_to_environ` does, `unsetenv` takes entries out of whichever array it is in place; neither frees a string, as glibc's keep them, so a value `getenv` returned stays good; the two hold a mutex of their own, glibc's `envlock`, so they may run in several threads at once, but not alongside a `getenv` or anything else reading `environ` |

| `malloc.c` | `malloc`, `free`, `calloc`, `realloc` on top of `sbrk`, under a mutex, and blocks of 128 KiB or more in mappings of their own (`mmap`) |
| `printf.c` | `printf`, `fprintf` (to a descriptor), `snprintf`, `puts` |
| `getline.c` | `getline` (a console line from stdin), `reader_init` and `reader_getline` (lines from any descriptor) |
| `strerror.c` | `strerror`: a short message for each error number |
| `stat.c` | `stat`: `fstat` of a path |
| `spin.c` | `spin`: a busy loop, for tests that need CPU time |
| `proc.c` | `kill_descendants`: kills what a process started, and theirs (the shell and `shd` hang a session up with it) |
| `inet.c` | `htons`, `ntohs`, `htonl`, `ntohl`, `inet_aton`, `inet_addr`, `inet_ntoa`, `sockaddr_set`: addresses and byte order |
| `resolve.c` | `resolve`: a host's address, from its text or by a DNS query (RFC 1035) to the server the network gave; `resolve_error` |
| `atomic.s` | `atomic_swap`, `atomic_add`, `atomic_cas`, `atomic_load`, `atomic_store`; `spin_lock`, `spin_unlock` |
| `mutex.c` | `mutex_lock`, `mutex_trylock`, `mutex_unlock`: a mutex that sleeps on a futex |
| `thread.S` | `thread_start`: a C function in a new thread (preprocessed for `<asm.h>`: its two stack slots are pointers) |

## How programs get it

Every `kernel/user/X.c` (or `X.s`) is a program, linked with the runtime
(`crt0.S`, which sets `environ` to `envp`, calls `main` and then `exit`, and
`usys.s`, the system call stubs) and with the library
(`crates/xtask/src/userprogs.rs`). The library
is linked the way a C library is linked from an archive (`libc.a`): each
file here is a *member*, compiled and assembled once for all the programs,
and a member goes into a program only if it defines a symbol that the
program, or a member already in, refers to and nothing else defines. So
`hello`, which calls `printf`, gets `printf.c` and, through it, `string.c`
(for `strlen`), and `environ.c` through crt0, but not `malloc.c`; `grep`, which calls `realloc`, gets
`malloc.c` too. A program carries only the code and the static data it can
use, and the members' `static` variables (malloc's free list) exist only in
the programs that need them.

merlin-ld does the choosing (`crates/merlin-ld/src/library.rs`): given the
program's objects and the library's, it pulls in members until nothing
undefined is left that a member defines, then links them after the
program's objects, in file name order. On its command line the members go
between `--start-lib` and `--end-lib`, as with ld.lld:

```
merlin-ld -T user.ld -o cat cat.c.o crt0.S.o usys.s.o \
          --start-lib lib/getline.c.o lib/malloc.c.o lib/printf.c.o lib/spin.c.o \
          lib/stat.c.o lib/stdlib.c.o lib/strerror.c.o lib/string.c.o --end-lib
```

The members include `../user.h`; merlin compiles them like the programs.
After changing anything here, run `cargo xtask kernel` (the programs are
embedded in the kernel) and commit `kernel/kernel.elf` and
`kernel/kernel-disk.elf`; `cargo test` checks them. Adding a function: declare it
in `user.h`, define it in the member it belongs with (or a new `.c` file
here, which is picked up by name), and check it in `libtest.c`.

## Conventions

- Errors come back as they come from the kernel: a negative error number
  (`-ENOENT`), which `strerror` turns into words. There is no `errno`.
  `getline` and `malloc` return -1 and `NULL`, as in C.
- Programs report an error as `NAME: WHAT: REASON` on stderr and exit with a
  non-zero status; the shell prints `NAME: exit STATUS` after them.
- No output buffering survives a call: `printf` formats into a 64-byte
  buffer on the caller's stack and writes it out before it returns (a line
  is usually one `write`), so output from several processes, or from a
  program and its children, never comes out of order.

## Formatted output

`printf.c` has one formatter, `vformat`, and a `struct out` that says
where its characters go: to a descriptor, through the buffer, for `printf`
and `fprintf`; into the caller's string for `snprintf`, which keeps
counting past the end so that it can return the length the whole text
needs. The conversions are `%d %u %x %p %s %c %%`, with the flags `-` (pad
on the right) and `0` (pad with zeros, after a minus sign) and a width.
`d`, `u` and `x` take an `int` (or `unsigned int`), or with a length before
them a wider integer, as C's `printf` does: `l` a `long`, as wide as a
register (64 bits), `ll` a `long long` (`int64_t`, 64 bits too), `z` a
`size_t` (`ssize_t` with `d`). `%p` prints every digit of a pointer, 16.
An unknown conversion is printed as written. The digits are divided off a
64-bit value with `do_div`, as Linux's `vsprintf.c` does. It is the
kernel's `kprintf` (`kernel/kprintf.c`) with what programs need added:
`kprintf` has no `-` flag and no `z` length, and takes `0` only right
after the `%`; and a conversion cut off by the end of the format it prints
as written, where `printf` prints a lone `%`.

## Line input

Two ways, for two kinds of input:

- `getline(buf, size)` is for the console, which returns one line per
  `read`: it makes exactly one `read` and so never takes anything that
  follows the line. The shell reads its commands with it, and the programs
  it starts read the same console after it, so read-ahead would steal their
  input.
- A `struct reader` (`reader_init(&r, fd)`, then `reader_getline(&r, line,
  size)`) reads a file or a pipe `READER_SIZE` (512) bytes at a time and
  hands out a line per call, with its newline. A line longer than the
  buffer the caller gives comes back in pieces, each but the last without a
  newline; `grep` grows its buffer with `realloc` until the whole line fits.
  The reader lives in the caller's memory, so a program can read several
  descriptors at once.

## The heap

`malloc.c` is the allocator of K&R's *The C Programming Language*, section
8.7. Each block starts with a header of two words, a pointer's size each
(16 bytes): its size in header-sized units, a `size_t`, and, while it is
free, the next free block. So every block is aligned to the header's size,
which is the ABI's largest alignment, 16 bytes, as glibc's `malloc` gives. When a program has moved the end of the heap
with `sbrk` itself, by a number of bytes that is not a multiple of the
header's size, `malloc` skips to the next multiple before it puts a block
there, and takes the bytes it skipped from `sbrk` too, as glibc corrects a
misaligned break. A request of more than `LONG_MAX` bytes (what
`sbrk` takes) fails with `NULL` at once. The
free blocks form a ring in address order, through a dummy block of size 0;
`malloc` walks it from where the last search stopped and takes the first
block big enough, cutting the request off its end. The threads of a
process share the heap, so `malloc` and `free` hold a mutex while they walk
and change the ring (`realloc` and `calloc` go through them), which costs
one atomic instruction each way when no other thread wants it. `free` puts a block back
in its place in the ring and merges it with a free neighbour on either side,
so freeing everything gives the heap back as one block. When no block is
big enough, `malloc` grows the heap with `sbrk`, by at least a page, and
frees the new memory into the ring. The heap is never given back to the
kernel: it goes when the program exits.

A request of 128 KiB or more (glibc's default `M_MMAP_THRESHOLD`) gets a
mapping of its own instead, anonymous and private (`mmap`, whole pages with
the header, the block at the header's size into the first), and `free`
gives it back to the kernel at once (`munmap`), so a big buffer doesn't
keep the heap big after it is gone. Its header's link, unused while a block
is handed out, says which it is: 1 for a mapping (no free block can be at
address 1), `NULL` for the heap. When the kernel has no room for the
mapping, `malloc` takes the block from the heap after all, as glibc falls
back.

## Threads

A program starts more threads with `thread_start(fn, arg, stack, size)`
(`thread.S`): `fn(arg)` runs in a new thread on the `size` bytes at
`stack`, which the caller provides (static, or from `malloc`) and must keep
until the thread has ended, and what `fn` returns is the status
`thread_join` collects. The `thread_create` system call itself starts a
thread at an address with every register but `sp` and `a0` zero, `ra`
included, so the code it starts must never return; `thread_start` writes
`fn` and `arg` at the top of the new stack and starts the thread at
`thread_entry`, which loads them, calls `fn` and passes its result to
`thread_exit`.

Threads that share data need the atomics of `atomic.s`. Each is one
indivisible step on an aligned word (`amoswap.w`, `amoadd.w`, or an
`lr.w`/`sc.w` loop for the compare-and-swap) with both `aq` and `rl` set,
or fences around a plain load, so they are also ordered against the
caller's other loads and stores: what a thread wrote before an atomic is
seen by a thread that sees the atomic's effect (RVWMO, the ISA's memory
model, would otherwise let another hart see them in a different order).
merlin has no inline assembly, so they are functions.

Two locks are built on them:

- `struct spinlock` (`spin_lock`, `spin_unlock`): `amoswap.w.aq` of a 1
  until a 0 comes back, reading the word in between so that waiting
  threads don't keep writing it; `amoswap.w.rl` of 0 to unlock. A thread
  waiting for a spinlock uses its whole time slice, so it suits only
  stretches of a few instructions.
- `struct mutex` (`mutex.c`): Ulrich Drepper's mutex from "Futexes Are
  Tricky", with three states, 0 (free), 1 (taken) and 2 (taken, and someone
  may be waiting). Taking a free mutex is one compare-and-swap and freeing
  one nobody waits for is one atomic add, with no system call; a thread
  that finds it taken sets 2 and sleeps in `futex_wait` while it stays 2,
  and the unlock that sees 2 wakes one sleeper with `futex_wake`. The
  kernel checks the word and goes to sleep as one step against
  `futex_wake`, so an unlock between the thread's check and its sleep is
  never lost.

Both start unlocked when zeroed, so a static one needs no setting up, and
neither may be taken twice by the same thread. `threads` (in `kernel/user`)
uses both, and `threadtest` checks them along with the calls.
