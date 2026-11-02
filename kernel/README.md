# Camelot kernel

The Camelot OS kernel, in C with a little assembly. A small M-mode layer sets
up trap delegation and the timer, then the kernel runs in S-mode: it prints a
banner and a `kprintf` self-test over the UART, puts every free page of RAM on
the page allocator's free list, turns on Sv32 paging with a kernel page table,
runs the memory self-tests, lists the disks, takes ten timer ticks, tests the
first disk if there is one and mounts the filesystem on it, and starts the
shell, a user program, as the first process. User programs run in U-mode, each in its own page table, switching on
every timer tick; they call the kernel through system calls
(`docs/camelot-syscalls.md`): console input and output, starting and waiting
for programs, sleeping, growing the heap. A program that faults is killed and
the kernel says why; a bad pointer passed to a system call is an error, not a
crash. Faults in the kernel itself panic with the cause, `sepc`, `stval` and a
register dump.

```
cargo run --release -p grail -- kernel/kernel.elf
cargo run --release -p grail -- --clock 10MHz kernel/kernel.elf    # ticks in real time
```

```
Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=80000054 a0 = 0x00000000 (0)
ram: 128 MiB at 0x80000000
paging: on (Sv32)
vm self-test: 26/26 passed
timer: 10 Hz tick, 10000000 Hz timebase
disk: none
tick 1
tick 2
...
tick 10
Type help for the list of commands.
$ hello
hello from user mode
$ echo a  b   c
a b c
$ exit
```

With disks attached (`--disk IMG`, `--disk-ro IMG`, spec section 9.6), the
kernel lists each one and, after the ticks, runs its disk self-test on disk 0
(read, write and read back a scratch block through the buffer cache, then put
it back, so the image ends as it was; on a read-only disk the write must be
refused):

```
cargo run --release -p grail -- --disk examples/disk/disk.img kernel/kernel.elf
```

```
timer: 10 Hz tick, 10000000 Hz timebase
disk 0: 8 sectors (4 KiB), irq 3
tick 1
...
tick 10
blk self-test: 14/14 passed
fs: no filesystem on disk 0
Type help for the list of commands.
```

Disk 0 holding a Camelot filesystem (`docs/camelot-fs.md`) is mounted as the
root: the kernel replays its log if a crash left a transaction there, lists
`/` and runs its filesystem self-test (spec section 10, "Kernel
filesystem"). `cargo xtask fs` builds the root image with the user programs
in `/bin` and the files of `fs/` at `/`, no LLVM needed:

```
cargo xtask fs
cargo run --release -p grail -- --disk target/camelot-fs.img kernel/kernel.elf
```

```
blk self-test: 16/16 passed
fs: disk 0 mounted: 2048 blocks (1780 free), 256 inodes (238 free), 21 log blocks
fs: ls /
d    1  2      64 .
d    1  2      64 ..
f    2  1     355 README
d    3  1     256 bin
fs: /bin holds 14 of the kernel's 14 programs
fs self-test: 12/12 passed
Type help for the list of commands.
```

`cargo run -p camelot-fs -- ls target/camelot-fs.img /bin` (and `cat`,
`fsck`, `mkfs`) reads and builds images on the host. Running programs from
`/bin` and the file system calls come with #67; until then the filesystem is
the kernel-internal API in `include/fs.h` and `include/log.h`.

The shell (`user/sh.c`) runs programs by name, with arguments: `PROG
[ARG...]`. `CMD & CMD` runs the first in the background (the shell prints its
pid) and the last in the foreground; `wait` collects the background ones,
`help` lists the built-ins, and `exit [N]` (or Ctrl-D on an empty line) ends
the shell, after which the kernel stops the machine with its status as the
exit code. Lines are edited with backspace and end with Enter. In a terminal,
Grail passes each key to the kernel as it is typed and the kernel does the
echoing; piped input works too:

```
printf 'ticks\nexit\n' | cargo run --release -p grail -- kernel/kernel.elf
```

Piped or redirected input must end with `exit`, or give Grail `--max-steps`:
when the input runs out the shell keeps waiting for a line, and Grail keeps
running.

Programs side by side, and one the kernel kills:

```
$ ping & pong & badload
[2]
[3]
ping 1
ping 2
pong 1
pong 2
pid 4 (badload) killed: load page fault
  scause 0x0000000d  sepc 0x00001018  stval 0x80000000
  va 0x80000000 maps to pa 0x80000000, pte 0x2000004b -A--X-RV
badload: exit -1
$ wait
ping 3
ping 4
pong 3
pong 4
[2] ping: exit 0
[3] pong: exit 0
```

The programs live in `user/`, one `.c` or `.s` file each, all linked with the
runtime (`user/crt0.s`, which calls `main` and then `exit`; `user/usys.s`, the
system call stubs; `user/ulib.c`, a small C library with `printf`, declared in
`user/user.h`) and `user/user.ld`:

| Program | What it does |
|---|---|
| `sh` | the shell |
| `hello` | prints a line and exits |
| `echo` | prints its arguments |
| `ticks` | prints the timer ticks since boot |
| `ping`, `pong` | print four numbered lines half a tick apart: run together, they take turns |
| `regs` | fills every register, spins for three ticks and exits with 0 only if none changed |
| `sysregs` | fills every register, makes system calls and exits with 0 only if none but a0 changed |
| `data` | exits with 0 only if its stack and `.bss` start zeroed and its `.data` holds its initial value and takes stores |
| `badptr` | hands the system calls bad pointers, which must all fail with `-EFAULT` |
| `systest` | checks every system call, its results and its errors (`systest: N checks, 0 failed`) |
| `badload` | reads kernel memory: killed by a load page fault |
| `badstore` | writes to its own read-only data: killed by a store page fault |
| `badhalt` | executes `xcal.halt`, which is M-mode only: killed by an illegal instruction |

The system calls, the error numbers and how a program starts are specified in
`docs/camelot-syscalls.md`. The memory layout, the trap path, the scheduler
and the ELF loader are described in `docs/excalibur-spec.md` section 10,
"Kernel processes".

Without `--clock`, Grail runs at full speed and skips every `wfi` to the next
timer tick, so while the shell waits for a key the tick count races ahead;
with `--clock 10MHz` it counts ten a second.

The RAM size comes from Grail's sysinfo device, so `--ram` changes what the
allocator manages (`cargo run --release -p grail -- --ram 16 kernel/kernel.elf`
prints `ram: 16 MiB at 0x80000000`). Booting takes about 4.5 million
instructions with 128 MiB, nearly all of them spent setting up memory one page
at a time.

The exit code is `kmain()`'s return value: the shell's exit status once it
and every program it left running have ended (the self-test failure count
instead, if any failed), 0xfe after a kernel panic, or 0xff after an
unexpected M-mode trap.
The traps, the M-mode layer and the panic format are described in
`docs/excalibur-spec.md` section 10, "Kernel traps"; the allocator, the page
tables and the kernel address space in "Kernel memory"; the PLIC setup, the
UART driver and the console in "Kernel console"; processes, the system calls,
the user programs and the shell in "Kernel processes"; the disk driver and the
buffer cache in "Kernel disks"; the log, the filesystem and mounting in
"Kernel filesystem" (the on-disk format in `docs/camelot-fs.md`). A failed
disk or filesystem self-test check counts in the exit code like the others.

A panic looks like this (from the deliberate illegal instruction the tests
trigger by patching `boot_fault` in the image to 2):

```
panic: illegal instruction
  scause 0x00000002  sepc 0x800002bc  stval 0xc0001073  sstatus 0x00000120
   ra 01010101   sp 80007400   gp 03030303   tp 04040404
   t0 05050505   t1 06060606   t2 07070707   s0 08080808
   ...
   t4 1d1d1d1d   t5 1e1e1e1e   t6 1f1f1f1f
```

A page fault adds a line on how the kernel page table maps `stval`. With
`boot_fault` = 4 the kernel stores to its own code, which is mapped read and
execute only (3 loads from the unmapped address 0x100 instead, which reports
`va 0x00000100 is not mapped`):

```
panic: store page fault
  scause 0x0000000f  sepc 0x80002974  stval 0x8000199c  sstatus 0x00000120
  va 0x8000199c maps to pa 0x8000199c, pte 0x2000044b -A--X-RV
   ra 80002918   sp 8000a060   gp 00000000   tp 00000000
   ...
```

The PTE flags are printed bit 7 to bit 0 (`DAGUXWRV`), `-` where clear.

Values 5 to 11 misuse the memory API instead (a remap, unmapping an
unmapped page, a W-only mapping, a megapage, `kfree` of a bad page), so the
tests see each of its checks panic, e.g. `panic: map_pages: remap`.

## Files

| File | What it does |
|---|---|
| `entry.s` | `_start` (M-mode): early trap vector, stack, zero `.bss`, `call start`, `mret` to S-mode. `supervisor_entry`: `call kmain`, then `machine_halt` with its return value. Also `xcal_dbg`/`xcal_halt` for C and the early trap handler (prints `mcause`, `mepc`, `mtval`, exits 0xff) |
| `start.c` | `start()`: M-mode setup: `medeleg`/`mideleg`, `stvec`, `mtvec`, `mscratch`, the first timer interrupt, `mstatus.MPP = S`, `mepc` |
| `machine.s` | `machine_vec`, the M-mode trap handler: forwards the machine timer to S-mode as SSIP, serves machine calls (`MCALL_HALT`). `machine_halt()` makes that call from S-mode |
| `kernelvec.s` | `kernel_vec`, the trap vector for traps from S-mode: saves x1-x31 and the trap CSRs as a `struct trap_frame` on the stack, calls `kernel_trap`, restores, `sret`. Also `illegal_instruction()` for the panic tests |
| `uservec.s` | `user_vec`, the trap vector for traps from U-mode: saves the user registers in the process's trap frame (`sscratch`), calls `user_trap` on the process's kernel stack; `user_return`: frame to registers, `sret` to U-mode |
| `trap.c` | `kernel_trap()`: counts timer ticks (waking sleepers), claims/completes PLIC interrupts and runs the console's or a disk's handler, resumes faulting memory probes, panics on anything else (cause name, `sepc`, `stval`, the PTE for a page fault, register dump); `user_trap()`: a tick yields, an ecall goes to `syscall()`, a fault kills the process; `user_trap_return()`, `trap_init()`, `panic()` |
| `proc.c` | processes: `proc_create()` (slot, kernel stack, page table, ELF, stack with the arguments), `kproc_create()` (a kernel process running a C function), `proc_run_init()` (the shell), the round-robin `scheduler()` (idling in `wfi` while everyone sleeps), `yield()`, `sleep()`/`wakeup()`, `proc_exit()`, `proc_kill()`, `proc_wait()`, `proc_sbrk()` |
| `swtch.s` | `swtch()`: saves `ra`, `sp`, `s0`-`s11` and loads another thread's |
| `elf.c` | `elf_load()`: checks an ELF32 executable and maps its `PT_LOAD` segments into user space |
| `syscall.c` | `syscall()`: the system calls (`docs/camelot-syscalls.md`), with `spawn`'s argument copying |
| `userprogs.s` | generated by `cargo xtask kernel` from `user/`: the user programs' ELF images and the `user_programs` table (do not edit) |
| `user/` | the user programs (`*.c`, `*.s`), their runtime (`crt0.s`, `usys.s`, `ulib.c`, `user.h`) and linker script `user.ld`, built with merlin-as and merlin-ld; `user/clang/` holds clang's assembly for each `.c` (generated, committed) |
| `kalloc.c` | physical page allocator: `kinit()` reads RAM's base and size from sysinfo and frees every page from `__kernel_end` to the end of RAM; `kalloc()`, `kzalloc()`, `kfree()`, `kfree_pages()` |
| `vm.c` | Sv32 page tables: `walk()`, `map_pages()`, `unmap_pages()`, `pagetable_create()`/`pagetable_free()`; `kvminit()` builds the kernel page table, `kvminithart()` turns paging on; user address spaces: `uvm_create()`/`uvm_free()`, `uvm_range_ok()`, `copyin()`, `copyout()`, `copyinstr()` |
| `vmtest.c` | the memory self-tests: allocate and free every page, `kzalloc()` and `pagetable_create()` clear a reused dirty page, a second mapping of one page, store page faults on text and rodata, building and freeing a page table |
| `probe.s` | `probe_load()`/`probe_store()`: a word access that returns the trap cause instead of panicking (`kernel_trap` resumes them at `probe_fault`) |
| `sysinfo.c` | reads Grail's sysinfo block: registers and the device table |
| `timer.c` | `timebase_hz()` (sysinfo's `SYS_TIMEBASE_HZ`, read once), `timer_interval()`, `timer_init()` |
| `plic.c` | `plic_init()` (UART source enabled for the S-mode context), `plic_enable()` (another source, such as a disk's), `plic_claim()`, `plic_complete()` |
| `blk.c` | disk driver: `blk_init()` finds the disks in the sysinfo device table and enables their completion interrupts; `blk_read()`/`blk_write()`/`blk_flush()` issue one DMA request at a time and sleep until `blk_intr()`, the interrupt handler, wakes them with the `BLK_ERROR` code |
| `bio.c` | buffer cache (xv6's): 30 one-KiB block buffers in LRU order, `bread()`/`bwrite()`/`brelse()`, each buffer sleep-locked while in use |
| `blktest.c` | the disk self-test, in kernel processes: three readers at once, write and read back a scratch block (restored afterwards), errors, LRU reuse |
| `log.c` | the filesystem's write-ahead log (xv6's): `begin_op()`/`end_op()` around each operation, `log_write()` pinning changed buffers, group commit (slots, flush, header, flush, install, flush, empty header, flush), and the replay at mount |
| `fs.c` | the filesystem (xv6's): `fs_mount()`, the bitmap allocator, the inode cache (`iget`/`ilock`/`iput`, `readi`/`writei`), directories, path lookup (`namei`/`nameiparent`), `fs_create()`/`fs_unlink()` |
| `fstest.c` | `fs_boot()`: mounts disk 0 in a kernel process, lists `/`, runs the filesystem self-test (and, with `fs_test` patched to 2, its transactions) |
| `fs/` | the files `cargo xtask fs` copies to `/` of the root image (a README); the user programs go to `/bin` |
| `riscv.s` | CSR accessors for C (`r_sstatus`, `w_mepc`, `w_satp`, ...), `sfence_vma()`, `intr_off()`/`intr_on()` and `wfi()` |
| `main.c` | `kmain()`: banner, `kprintf` self-test, memory setup and self-tests, the disk list, ten ticks, the optional deliberate fault or memory API misuse, the disk self-test, the filesystem (`fs_boot`), then the user-space shell as the first process |
| `uart.c` | 16550 driver: receive interrupt handler `uart_intr()` filling a ring buffer (`uart_rx_pop()`), polling `uart_putc`/`uart_puts` |
| `console.c` | the `read` and `write` system calls: `console_read()` (line editing and echo: backspace, CR/LF/CR LF, Ctrl-D; sleeps until a line is complete), `console_write()`, `console_intr()` (the UART interrupt, waking readers) |
| `kprintf.c` | `kprintf`/`ksnprintf`: `%d %u %x %p %s %c %%`, `0` flag, width |
| `string.c` | `memset memcpy memmove memcmp strlen strcmp` (clang may call the `mem*` ones by itself) |
| `kernel.ld` | layout: `.text` at 0x8000_0000 (`_start` first), then page-aligned `.rodata` and `.data` (so each gets its own page permissions), `.bss`, 16 KiB stack, `__kernel_end` |
| `include/` | `types.h` (fixed-width types), `mmio.h` (`volatile` accessors), `varargs.h`, `riscv.h` (CSR bits, cause numbers), `clint.h`, `memlayout.h` (page size, linker symbols), `sysinfo.h` (register offsets, device types), `plic.h`, `vm.h` (also the user address space layout), `proc.h` (`struct proc`, `struct context`, `struct user_program`), `elf.h`, `blk.h` (disk registers, `BLK_ERR_` codes), `bio.h` (`struct buf`, `BSIZE`, `NBUF`), `fs.h` (the on-disk structures, `struct inode`, the filesystem's API and error numbers), `log.h` (transactions, `LOGSIZE`, `MAXOPBLOCKS`), `syscall.h` (the system call and error numbers), and the headers for the above |
| `kernel.elf` | the prebuilt image, committed so `cargo test` and Grail need no LLVM |
| `kernel.elf.clang-version` | the clang that built `kernel.elf` |

## Building

Needs clang and ld.lld, e.g. from the official Windows LLVM installer (found on
`PATH` or in `C:\Program Files\LLVM\bin`):

```
cargo xtask kernel          # builds kernel/kernel.elf
cargo xtask kernel --run    # ... and runs it on Grail, with this terminal as its console
cargo xtask kernel -v       # shows the clang/ld.lld command lines
```

`cargo xtask` is a Rust program (`crates/xtask`), so there is no Makefile or
shell script. It first builds the user programs. clang compiles each
`user/*.c` to assembly in `user/clang/` (until Merlin, #15, can), with the
kernel's target and freestanding flags plus `-O2 -S`:

```
clang --target=riscv32-unknown-elf -march=rv32im_zicsr -mabi=ilp32 -mno-relax -mcmodel=medany \
      -std=c11 -O2 -ffreestanding -fno-builtin -nostdinc -nostdlib -fno-pic -fno-common -fno-ident \
      -fno-stack-protector -fno-asynchronous-unwind-tables -fno-unwind-tables \
      -Wall -Wextra -Werror -S sh.c -o clang/sh.s    # in user/, and each other .c
```

(each output gets a two-line header naming its source). Then merlin-as
assembles every program with the runtime and merlin-ld links each with
`user/user.ld`, all in memory (our own tools, no LLVM); the executables go
into `userprogs.s` as data. Files are rewritten only when they changed. Then it
compiles every `*.c` and assembles every `*.s` in this directory, in name
order, and links with `kernel.ld`. By hand, from this directory, the kernel
build is:

```
clang --target=riscv32-unknown-elf -march=rv32im_zicsr -mabi=ilp32 -mno-relax -c entry.s -o entry.s.o   # and each other .s
clang --target=riscv32-unknown-elf -march=rv32im_zicsr -mabi=ilp32 -mno-relax -mcmodel=medany \
      -std=c11 -O2 -ffreestanding -fno-builtin -nostdinc -nostdlib -fno-pic -fno-common -fno-ident \
      -fno-stack-protector -fno-asynchronous-unwind-tables -fno-unwind-tables \
      -Wall -Wextra -Werror -Iinclude -c main.c -o main.c.o        # and each other .c
ld.lld -T kernel.ld -o kernel.elf bio.c.o blk.c.o blktest.c.o console.c.o elf.c.o entry.s.o \
       fs.c.o fstest.c.o kalloc.c.o kernelvec.s.o kprintf.c.o log.c.o machine.s.o main.c.o plic.c.o \
       probe.s.o proc.c.o riscv.s.o start.c.o string.c.o swtch.s.o syscall.c.o sysinfo.c.o timer.c.o \
       trap.c.o uart.c.o userprogs.s.o uservec.s.o vm.c.o vmtest.c.o
```

The build is reproducible: the same clang gives byte-identical output wherever
the repository is checked out. **After changing anything here, run `cargo xtask
kernel` and commit `kernel.elf` (and `userprogs.s` and `user/clang/`, if
`user/` changed) with the sources.** `cargo test` checks, without LLVM, that
every `user/*.c` has its committed assembly and that `userprogs.s` is what the
current `user/` and tools build. When the installed clang matches
`kernel.elf.clang-version`, it also rebuilds the kernel and the user
programs' assembly and fails if anything committed is stale. With any clang it checks that a fresh build boots
with the same output.

`cargo xtask kernel --as merlin` assembles the `.s` files with merlin-as
instead (bootstrapping step 1, spec section 10). It works once the merlin-as
encoder covers what the `.s` files use; a test runs it automatically from then on.

## Rules for kernel code

The kernel and the user programs will eventually be compiled by Merlin (#15),
so they stick to plain C:

- No libc: headers come from `include/` (`-nostdinc`; `user/user.h` for the
  programs), no `<stdint.h>`/`<stdarg.h>`.
- No floating point.
- No inline `asm()`: CSR access and custom instructions live in `.s` files.
- Assembly uses only what merlin-as understands (no `.insn`, no preprocessor);
  Xcal instructions are `.word` constants. A test checks this.
- Varargs (`kprintf`) use clang's `__builtin_va_*` through `include/varargs.h`;
  Merlin has to provide the same builtins.
