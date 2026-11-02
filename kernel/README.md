# Camelot kernel

The Camelot OS kernel, in C with a little assembly. A small M-mode layer sets
up trap delegation and the timer, then the kernel runs in S-mode: it prints a
banner and a `kprintf` self-test over the UART, puts every free page of RAM on
the page allocator's free list, turns on Sv32 paging with a kernel page table,
runs the memory self-tests, lists the disks, takes ten timer ticks, tests the
first disk if there is one and mounts the filesystem on it, gives the first
network card an address (DHCP, or a static one), wakes the other harts if
the machine has several, and starts the shell, a user program, as the first
process. User programs run in U-mode, each in its own page table, switching on
every timer tick, on every hart at once; they call the kernel through system calls
(`docs/camelot-syscalls.md`): files, directories, the console and pipes
through file descriptors, starting and waiting for programs (from `/bin` on
the disk, or built into the kernel without one), sleeping, growing the heap. A program that faults is killed and
the kernel says why; a bad pointer passed to a system call is an error, not a
crash. Faults in the kernel itself panic with the cause, `sepc`, `stval` and a
register dump.

```
cargo run --release -p grail -- kernel/kernel.elf
cargo run --release -p grail -- --clock 10MHz kernel/kernel.elf    # ticks in real time
```

```

    ____      _      __  __   _____   _        ___    _____
   / ___|    / \    |  \/  | | ____| | |      / _ \  |_   _|
  | |       / _ \   | |\/| | |  _|   | |     | | | |   | |
  | |___   / ___ \  | |  | | | |___  | |___  | |_| |   | |
   \____| /_/   \_\ |_|  |_| |_____| |_____|  \___/    |_|

Camelot v0.1.0 booting...
kprintf: d=-42 u=4294967295 x=cafef00d p=0x80000000 s=excalibur c=K %=%
kprintf self-test: 26/26 passed
[dbg] pc=80000054 a0 = 0x00000000 (0)
ram: 4 MiB at 0x80000000
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
in `/bin` and the files of `fs/` at `/`, no LLVM needed. The image is
bootable: it holds the bootloader (`boot/boot.s`) in its boot block and
the disk kernel, `kernel-disk.elf`, after the filesystem, so Grail can boot
it with no program (spec section 7, "Boot from disk"; `docs/camelot-fs.md`,
"Booting"), in the default 4 MiB of RAM. The bootloader prints one line,
`boot: kernel at sector 0x...`, then the kernel starts as if Grail had
loaded its ELF.

The two kernels are Linux's split. `kernel-disk.elf` is the kernel alone,
about 340 KB: like Linux's, its programs are the files of the root
filesystem's `/bin`, and with no `/bin/sh` (or no filesystem) it panics
(`panic: no shell in /bin, and no programs built into the kernel`).
`kernel.elf` is the same kernel with a copy of every program built in (1.4
MB), which play the part of an initramfs: they are what it runs without a
disk. So the whole system fits in a 2 MiB image (the kernel about 280
blocks, the filesystem the rest, a third of it free); `cargo xtask fs --size
N` makes an image of N blocks.

```
cargo xtask fs
cargo run --release -p grail -- --disk target/camelot-fs.img
cargo run --release -p grail -- --disk target/camelot-fs.img kernel/kernel.elf
```

```
blk self-test: 16/16 passed
fs: disk 0 mounted: 1763 blocks (482 free), 256 inodes (212 free), 21 log blocks
fs: ls /
d    1  3      80 .
d    1  3      80 ..
f    2  1     410 README
d    3  1      48 dev
d    5  1     640 bin
fs: /bin holds 38 programs, the kernel none
fs self-test: 12/12 passed
Type help for the list of commands.
$ echo hello world > hello.txt
$ cat hello.txt | wc
1 2 12
$ mkdir notes
$ cd notes
$ ls /bin | wc > count
$ cat < count
40 200 943
```

With `kernel.elf` given as the program instead, the kernel checks that each
program in `/bin` is byte for byte its own copy (`fs: /bin holds 38 of the
kernel's 38 programs`).

The shell then runs its programs from `/bin`, and what they write stays on
the image. `cargo run -p camelot-fs -- ls target/camelot-fs.img /bin` (and
`cat`, `fsck`, `mkfs`) reads and builds images on the host. Without a disk,
the same programs, built into the kernel, stand in for `/bin`, and there are
no other files.

With a network card (`--net user`, `--net hub`, spec section 9.8), the
kernel brings up the first one: it lists the card, gets an address by DHCP
(or falls back to a static one) after the ticks, and the shell's `ifconfig`
and `ping` use it (`docs/camelot-net.md`):

```
cargo run --release -p grail -- --net user kernel/kernel.elf
```

```
disk: none
net 0: mac 52:54:00:12:34:56, irq 12, link up
tick 1
...
tick 10
net: 10.0.2.15/24, gateway 10.0.2.2, dns 10.0.2.3 (dhcp)
Type help for the list of commands.
$ ping 10.0.2.2 1
PING 10.0.2.2: 56 data bytes
64 bytes from 10.0.2.2: seq=1 ttl=64 time=1.211 ms
1 sent, 1 received
```

On a machine of several harts (`--harts N`, up to 8; spec section 10,
"Kernel SMP"), hart 0 boots the kernel alone, then wakes the others, which
join the scheduler: any hart runs any runnable process, so up to N programs
run at the same time. `harts` shows what each one is doing, and `primes`
keeps them all busy:

```
cargo run --release -p grail -- --harts 4 kernel/kernel.elf
```

```
tick 10
smp: 4 harts running
Type help for the list of commands.
$ primes 4 40000
primes [0, 10000): 1229
primes [10000, 20000): 1033
primes [20000, 30000): 983
primes [30000, 40000): 958
primes: 4203 below 40000, 4 workers
$ harts
HART   PID    TICKS SWITCHES
   0     1       20        9
   1     7       10       14
   2     -       10        6
   3     -       10        5
```

The workers' lines come in whatever order they finish. Each hart has its own
timer, and takes its ticks from when it started; a process runs on one hart
at a time, but may move from one to another between its turns (here the
shell, pid 1, is still on hart 0 on its way to wait for `harts`).

The shell (`user/sh.c`) runs programs by name, with arguments: `PROG
[ARG...]` runs `/bin/PROG`, and a word with a `/` in it is a path to the
program. `CMD < FILE` and `CMD > FILE` redirect its input and output, `CMD |
CMD` sends the first one's output to the second, and `CMD & CMD` runs the
first in the background (the shell prints its pid) and the last in the
foreground; `wait` collects the background ones, `cd [DIR]` changes the
working directory, `help` lists the built-ins, and `exit [N]` (or Ctrl-D on
an empty line) ends the shell, after which the kernel stops the machine with
its status as the exit code. Lines are edited with backspace and end with Enter. In a terminal,
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
$ ding & dong & badload
[2]
[3]
ding 1
ding 2
dong 1
dong 2
pid 4 (badload) killed: load page fault
  scause 0x0000000d  sepc 0x00001018  stval 0x80000000
  va 0x80000000 maps to pa 0x80000000, pte 0x2000004b -A--X-RV
badload: exit -1
$ wait
ding 3
ding 4
dong 3
dong 4
[2] ding: exit 0
[3] dong: exit 0
```

The programs live in `user/`, one `.c` or `.s` file each, all linked with the
runtime (`user/crt0.s`, which calls `main` and then `exit`; `user/usys.s`, the
system call stubs), the C library of `user/lib/` (strings, numbers, `malloc`,
`printf`, a line reader; declared in `user/user.h`, described in
`user/lib/README.md`, and linked like an archive, so a program gets only the
parts it uses) and `user/user.ld`. `user/README.md` has each program's usage:

| Program | What it does |
|---|---|
| `sh` | the shell |
| `hello` | prints a line and exits |
| `echo` | prints its arguments (`-n`: without the newline) |
| `ls` | lists directories (the working directory without arguments) |
| `cat` | copies files, or its input, to its output |
| `wc` | counts the lines, words and bytes of files, or of its input (`-l`, `-w`, `-c`) |
| `mkdir`, `rm`, `ln` | make directories, remove names, link a second name to a file |
| `cp`, `mv` | copy or rename files, or copy or move them into a directory |
| `grep` | prints the lines that contain a string (`-v`, `-c`, `-n`) |
| `ps` | lists the processes |
| `kill` | kills processes by pid |
| `sleep` | waits for some seconds (`0.5` for half a second) |
| `ticks` | prints the timer ticks since boot |
| `ifconfig`, `ping` | show and set the network interface; ICMP echo to a host (an address or a name) |
| `nc`, `fetch`, `httpd` | netcat over TCP or UDP; an HTTP download; a web server for a directory |
| `ding`, `dong` | print four numbered lines half a tick apart: run together, they take turns |
| `regs` | fills every register, spins for three ticks and exits with 0 only if none changed |
| `sysregs` | fills every register, makes system calls and exits with 0 only if none but a0 changed |
| `data` | exits with 0 only if its stack and `.bss` start zeroed and its `.data` holds its initial value and takes stores |
| `badptr` | hands the system calls bad pointers, which must all fail with `-EFAULT` |
| `systest` | checks every system call, its results and its errors (`systest: N checks, 0 failed`) |
| `filetest` | checks the file system calls on a writable root filesystem (`filetest: N checks, 0 failed`) |
| `stress` | many processes at once: pages, the console, a shared pipe and files under timer preemption (`stress: W workers, R rounds, N bytes: ok`) |
| `libtest` | checks the C library (`libtest: N checks, 0 failed`) |
| `nettest`, `socktest` | move known bytes through TCP, for the TCP tests; check the socket calls (`socktest: N checks, 0 failed`) |
| `badload` | reads kernel memory: killed by a load page fault |
| `badstore` | writes to its own read-only data: killed by a store page fault |
| `badhalt` | executes `xcal.halt`, which is M-mode only: killed by an illegal instruction |

The system calls, the error numbers and how a program starts are specified in
`docs/camelot-syscalls.md`. The memory layout, the trap path, the scheduler
and the ELF loader are described in `docs/excalibur-spec.md` section 10,
"Kernel processes"; open files, pipes and running programs from the disk in
"Kernel files".

Without `--clock`, Grail runs at full speed and skips every `wfi` to the next
timer tick, so while the shell waits for a key the tick count races ahead;
with `--clock 10MHz` it counts ten a second.

The RAM size comes from Grail's sysinfo device, so `--ram` changes what the
allocator manages (`cargo run --release -p grail -- --ram 16 kernel/kernel.elf`
prints `ram: 16 MiB at 0x80000000`; the default is 4 MiB). Booting to the
shell takes about 16 million instructions with 4 MiB, and 36 million with
128 MiB, where nearly all of them go to setting up memory one page at a time.

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
"Kernel filesystem" (the on-disk format in `docs/camelot-fs.md`); file
descriptors, devices, pipes and the file system calls in "Kernel files"; the
network stack in "Kernel network" and `docs/camelot-net.md`. A failed
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
tests see each of its checks panic, e.g. `panic: map_pages: remap`. Values
12 and 13 misuse a spinlock: 12 acquires one twice, 13 releases one it
doesn't hold, and each panics naming the lock and the hart, e.g.
`panic: acquire: lock "boot_fault" already held by hart 0`. 14 has a kernel
process yield while it holds a spinlock (`panic: sched: holding a lock
other than proc_lock`), 15 calls `pop_off()` with no `push_off()` and 16
turns interrupts on under a `push_off()` before its `pop_off()`.

## Locking

Every structure that more than one process, a process and an interrupt
handler, or more than one hart can reach is protected by a
lock (spec section 10, "Kernel locking"). Spinlocks (`spinlock.s`,
`include/spinlock.h`) are xv6's: `acquire()` turns interrupts off on the
hart and spins on an `amoswap.w`; `release()` turns them back on once the
last lock is gone. Code that must wait sleeps with `sleep(chan, lk)`, which
lets go of the lock it holds while it sleeps; buffers and inodes, held
across disk reads, have sleep locks (`sleeplock.c`). Per-hart state is
`cpus[tp]`: `tp` holds the hart's id in the kernel, and `user_vec` puts it
back after a trap from U-mode.

## Several harts

`smp.c` says how the kernel runs on several harts (spec section 10, "Kernel
SMP"). Hart 0 boots alone; `smp_boot()` gives each other hart a stack and
wakes them with an IPI, and each goes through `_start` and `start()` (its
own M-mode trap vector, `mscratch` block and timer) into `hart_main()`,
which runs the scheduler. The process table is shared: any hart runs any
runnable process, under `proc_lock`, and a hart with nothing to run waits in
`wfi` until its next tick, or until another hart makes a process runnable
and wakes it with an IPI. Only hart 0 counts the clock (`ticks`, `sleep`,
the network's timer) and takes the devices' interrupts. Taking away a
mapping (`unmap_pages()`) shoots it down on the harts that may hold it
(`tlb_shootdown()`) before the page is reused. A panic on one hart stops
the others at their next byte to the UART (`uart_putc()` checks), so its
report comes out whole.

Locks are always taken in this order, so no two holders can wait for each
other:

1. sleep locks: `begin_op()` (the log), then inode locks (a parent
   directory before its child), then buffer locks;
2. one of `net`, `console`, `pipe`, `disk`, `log`, `itable`, `ticks`,
   `fstest` (never two of them at once);
3. `ftable`, `bcache`, or a sleep lock's own spinlock;
4. `proc` (taken by `sleep()`, `wakeup()` and the scheduler);
5. `kmem` or `uart` (leaves: nothing is taken while they are held).

A process may hold only the lock it passes to `sleep()` when it sleeps, and
`sched()` panics if it holds any other. `stress` (`user/stress.c`) runs many
processes through all of it at once under timer preemption.

## Files

| File | What it does |
|---|---|
| `entry.s` | `_start` (M-mode): early trap vector, stack (hart 0's, or the one `smp_boot()` gave another hart), zero `.bss` (hart 0 only), `call start`, `mret` to S-mode. `supervisor_entry`: hart 0 calls `kmain`, then `machine_halt` with its return value; the others call `hart_main`. Also `xcal_dbg`/`xcal_halt` for C and the early trap handler (prints `mcause`, `mepc`, `mtval`, exits 0xff) |
| `start.c` | `start()`, on each hart: M-mode setup: `medeleg`/`mideleg`, `stvec`, `mtvec`, the hart's `mscratch` block, its first timer interrupt, `mstatus.MPP = S`, `mepc` |
| `machine.s` | `machine_vec`, the M-mode trap handler: forwards the machine timer (marking a tick) and IPIs (the machine software interrupt) to S-mode as SSIP, serves machine calls (`MCALL_HALT`, `MCALL_IPI`). `machine_halt()` and `machine_ipi()` make them from S-mode |
| `kernelvec.s` | `kernel_vec`, the trap vector for traps from S-mode: saves x1-x31 and the trap CSRs as a `struct trap_frame` on the stack, calls `kernel_trap`, restores, `sret`. Also `illegal_instruction()` for the panic tests |
| `uservec.s` | `user_vec`, the trap vector for traps from U-mode: saves the user registers in the process's trap frame (`sscratch`), calls `user_trap` on the process's kernel stack; `user_return`: frame to registers, `sret` to U-mode |
| `trap.c` | `kernel_trap()`: counts timer ticks (hart 0's wake sleepers), serves TLB shootdowns, claims/completes PLIC interrupts and runs the console's or a disk's handler, resumes faulting memory probes, panics on anything else (cause name, `sepc`, `stval`, the PTE for a page fault, register dump); `user_trap()`: a tick yields, an ecall goes to `syscall()`, a fault kills the process; `user_trap_return()`, `trap_init()`, `panic()` |
| `proc.c` | processes: `proc_create()` (slot, kernel stack, page table, ELF, stack with the arguments, descriptors 0-2 and working directory inherited), `proc_exec()` (a new address space for the same process), `kproc_create()` (a kernel process running a C function), `proc_run_init()` (the shell, with the console as descriptors 0-2), the round-robin `scheduler()` on every hart (idling in `wfi` while nothing is runnable, woken by an IPI when something is), `yield()`, `sleep()`/`wakeup()`, `proc_exit()`, `proc_kill()`, `proc_wait()`, `proc_sbrk()` |
| `swtch.s` | `swtch()`: saves `ra`, `sp`, `s0`-`s11` and loads another thread's |
| `spinlock.s` | `acquire()`/`release()` (an `amoswap.w` loop; panics on a lock acquired twice or released unheld), `holding()`, `push_off()`/`pop_off()` (nested interrupt disabling), `mycpu()` (`&cpus[tp]`) |
| `spinlock.c` | `cpus[]`, the per-hart state; `initlock()`, `cpu_init()`, `cpuid()`, and the panic messages that name the lock and the hart |
| `smp.c` | the other harts: `smp_boot()` (stacks, the IPI that wakes them, the TLB shootdown self-test), `hart_main()`, `tlb_shootdown()`/`tlb_serve()` |
| `sleeplock.c` | sleep locks (xv6's), held across sleeps: `acquiresleep()`/`releasesleep()`, for the buffers and the inodes |
| `elf.c` | `elf_load()`: checks an ELF32 executable, read from memory or from a file through a `struct elf_file`, and maps its `PT_LOAD` segments into user space |
| `exec.c` | `exec_spawn()`, `exec_replace()`, `exec_init()`: find the program a path names (a file of the root filesystem, or an embedded program without one) and start it |
| `syscall.c` | `syscall()`: the system calls (`docs/camelot-syscalls.md`), with `spawn`'s and `exec`'s argument copying; the file ones go to `sysfile.c` |
| `sysfile.c` | the file system calls: `read`, `write`, `open`, `close`, `lseek`, `fstat`, `dup`, `pipe`, `mkdir`, `unlink`, `link`, `chdir`, each change to the disk in one transaction |
| `file.c` | open files (xv6's): the table of 64, `filealloc()`/`filedup()`/`fileclose()`, reading and writing an inode (writes 3 KiB per transaction), a device (`devsw`: the console) or a pipe, `filelseek()`, `filestat()` |
| `pipe.c` | pipes (xv6's): a 512-byte buffer in a page, readers and writers sleeping on each other, end of input when the writers are gone, `-EPIPE` when the readers are |
| `userprogs.s` | generated by `cargo xtask kernel` from `user/`: the user programs' ELF images and the `user_programs` table (do not edit); the disk kernel is linked with an empty table instead |
| `user/` | the user programs (`*.c`, `*.s`), their runtime (`crt0.s`, `usys.s`, `user.h`), the C library (`lib/`) and linker script `user.ld`, built with merlin, merlin-as and merlin-ld |
| `kalloc.c` | physical page allocator: `kinit()` reads RAM's base and size from sysinfo and frees every page from `__kernel_end` to the end of RAM; `kalloc()`, `kzalloc()`, `kfree()`, `kfree_pages()` |
| `vm.c` | Sv32 page tables: `walk()`, `map_pages()`, `unmap_pages()`, `pagetable_create()`/`pagetable_free()`; `kvminit()` builds the kernel page table, `kvminithart()` turns paging on; user address spaces: `uvm_create()`/`uvm_free()`, `uvm_range_ok()`, `copyin()`, `copyout()`, `copyinstr()` |
| `vmtest.c` | the memory self-tests: allocate and free every page, `kzalloc()` and `pagetable_create()` clear a reused dirty page, a second mapping of one page, store page faults on text and rodata, building and freeing a page table |
| `probe.s` | `probe_load()`/`probe_store()`: a word access that returns the trap cause instead of panicking (`kernel_trap` resumes them at `probe_fault`) |
| `sysinfo.c` | reads Grail's sysinfo block: registers and the device table |
| `timer.c` | `timebase_hz()` (sysinfo's `SYS_TIMEBASE_HZ`, read once), `timer_interval()`, `timer_init()` |
| `plic.c` | `plic_init()` (UART source enabled for hart 0's S-mode context, which takes every device interrupt), `plic_enable()` (another source, such as a disk's), `plic_claim()`, `plic_complete()` |
| `blk.c` | disk driver: `blk_init()` finds the disks in the sysinfo device table and enables their completion interrupts; `blk_read()`/`blk_write()`/`blk_flush()` issue one DMA request at a time and sleep until `blk_intr()`, the interrupt handler, wakes them with the `BLK_ERROR` code |
| `bio.c` | buffer cache (xv6's): 30 one-KiB block buffers in LRU order, `bread()`/`bwrite()`/`brelse()`, each buffer sleep-locked while in use |
| `blktest.c` | the disk self-test, in kernel processes: three readers at once, write and read back a scratch block (restored afterwards), errors, LRU reuse |
| `log.c` | the filesystem's write-ahead log (xv6's): `begin_op()`/`end_op()` around each operation, `log_write()` pinning changed buffers, group commit (slots, flush, header, flush, install, flush, empty header, flush), and the replay at mount |
| `fs.c` | the filesystem (xv6's): `fs_mount()`, the bitmap allocator, the inode cache (`iget`/`ilock`/`iput`, `readi`/`writei`), directories, path lookup (`namei`/`nameiparent`, from the working directory for a relative path), `fs_create()`/`fs_unlink()`/`fs_link()`, `itrunc()` |
| `fstest.c` | `fs_boot()`: mounts disk 0 in a kernel process, lists `/`, runs the filesystem self-test (and, with `fs_test` patched to 2, its transactions; to 3, filling the disk) |
| `net/` | the network stack (`docs/camelot-net.md`), a file per layer: `nic.c` (the card driver: descriptor rings, `nic_intr()`, the transmit queue), `pbuf.c` (the buffer pool), `ether.c`, `arp.c` (the cache), `ip.c` (checks, routing, loopback), `icmp.c` (echo, `icmp_ping()`), `udp.c` (the kernel-internal socket API, the echo service), `tcp.c` (connections, retransmission, the kernel-internal socket API), `dhcp.c` (the client), `net.c` (configuration, `net_boot()`, the counters, `net_timer()`, the `netinfo`/`netconf`/`ping` system calls), `socket.c` (the socket system calls: sockets behind file descriptors) |
| `fs/` | the files `cargo xtask fs` copies to `/` of the root image (a README); the user programs go to `/bin`, and `/dev/console` is made too |
| `boot/` | the bootloader of a bootable disk (`boot.s`, linked by `link.ld` into the committed `boot.elf` with `cargo xtask boot`): the boot block, which loads the kernel ELF stored after the filesystem; `camelot-fs mkfs --boot` installs it |
| `riscv.s` | CSR accessors for C (`r_sstatus`, `w_mepc`, `w_satp`, ...), `sfence_vma()`, `intr_off()`/`intr_on()`, `wfi()`, the atomics `atomic_swap()`/`atomic_add()`/`atomic_or()`, and `fence_rw()` |
| `main.c` | `kmain()`: banner, `kprintf` self-test, memory setup and self-tests, the disk list, the network cards (`net_init`), ten ticks, the optional deliberate fault or memory API misuse, the disk self-test, the filesystem (`fs_boot`), the network's address (`net_boot`), the other harts (`smp_boot`), then the user-space shell as the first process |
| `uart.c` | 16550 driver: receive interrupt handler `uart_intr()` filling a ring buffer (`uart_rx_pop()`), polling `uart_putc`/`uart_puts` |
| `console.c` | the console device (`read` and `write` on it): `console_read()` (line editing and echo: backspace, CR/LF/CR LF, Ctrl-D; sleeps until a line is complete), `console_write()`, `console_intr()` (the UART interrupt, waking readers) |
| `kprintf.c` | `kprintf`/`ksnprintf`: `%d %u %x %p %s %c %%`, `0` flag, width |
| `string.c` | `memset memcpy memmove memcmp strlen strcmp` (clang may call the `mem*` ones by itself) |
| `kernel.ld` | layout: `.text` at 0x8000_0000 (`_start` first), then page-aligned `.rodata` and `.data` (so each gets its own page permissions), `.bss`, 16 KiB stack, `__kernel_end` |
| `include/` | `types.h` (fixed-width types), `mmio.h` (`volatile` accessors), `varargs.h`, `riscv.h` (CSR bits, cause numbers), `clint.h`, `memlayout.h` (page size, linker symbols), `sysinfo.h` (register offsets, device types), `plic.h`, `vm.h` (also the user address space layout), `proc.h` (`struct proc`, `struct user_program`, sleep and wakeup), `context.h` (`struct context`, `swtch`), `spinlock.h` (`struct spinlock`, `struct cpu`, `NCPU`), `smp.h`, `sleeplock.h`, `elf.h`, `blk.h` (disk registers, `BLK_ERR_` codes), `bio.h` (`struct buf`, `BSIZE`, `NBUF`), `fs.h` (the on-disk structures, `struct inode`, the filesystem's API), `log.h` (transactions, `LOGSIZE`, `MAXOPBLOCKS`), `file.h` (`struct file`, `NFILE`, `NOFILE`, `devsw`), `pipe.h`, `exec.h`, `sysfile.h`, `syscall.h` (the system call and error numbers, `open`'s flags, `struct stat`, `struct netinfo`), `net.h` (the network stack: `struct pbuf`, every layer's functions and limits), `nic.h` (the card's registers and descriptors), and the headers for the above |
| `kernel.elf` | the prebuilt image, with the user programs built in, committed so `cargo test` and Grail need no build |
| `kernel-disk.elf` | the prebuilt disk kernel: the same objects without the user programs, which a bootable root image holds (`cargo xtask fs`) |

## Building

Our own toolchain builds the kernel, with no LLVM:

```
cargo xtask kernel          # builds kernel/kernel.elf and kernel/kernel-disk.elf (and kernel/userprogs.s)
cargo xtask kernel --run    # ... and runs it on Grail, with this terminal as its console
cargo xtask kernel -v       # shows the merlin/merlin-as/merlin-ld command lines
```

`cargo xtask` is a Rust program (`crates/xtask`), so there is no Makefile or
shell script, and merlin, merlin-as and merlin-ld run inside it as
libraries. It first builds the user programs: merlin compiles each
`user/*.c` (with `-I user`), merlin-as assembles every program and the
runtime, and merlin-ld links each with `user/user.ld`, all in memory; the
executables go into `userprogs.s` as data (rewritten only when it changed).
Then it compiles every `*.c` and assembles every `*.s` in this directory, in
name order, and links with `kernel.ld`. By hand, from this directory, with
the tools' command lines (`cargo run -p merlin --`, and so on), the kernel
build is:

```
merlin -I include main.c -o main.c.s         # and each other .c
merlin-as main.c.s -o main.c.o
merlin-as entry.s -o entry.s.o               # and each other .s
merlin -I include net/arp.c -o net/arp.c.s   # the network stack's files too
merlin-ld -T kernel.ld -o kernel.elf bio.c.o blk.c.o blktest.c.o console.c.o elf.c.o entry.s.o \
       exec.c.o file.c.o fs.c.o fstest.c.o kalloc.c.o kernelvec.s.o kprintf.c.o log.c.o machine.s.o \
       main.c.o net/arp.c.o net/dhcp.c.o net/ether.c.o net/icmp.c.o net/ip.c.o net/net.c.o \
       net/nic.c.o net/pbuf.c.o net/tcp.c.o net/udp.c.o pipe.c.o plic.c.o probe.s.o proc.c.o riscv.s.o \
       sleeplock.c.o smp.c.o spinlock.c.o spinlock.s.o start.c.o \
       string.c.o swtch.s.o syscall.c.o sysfile.c.o sysinfo.c.o timer.c.o trap.c.o uart.c.o \
       userprogs.s.o uservec.s.o vm.c.o vmtest.c.o
```

(`cargo xtask kernel` keeps the `.s` and `.o` files in `target/kernel/obj`,
`net/`'s in `target/kernel/obj/net`.) The disk kernel is the same link with
`userprogs.s.o` replaced by the object of a `userprogs.s` with no programs
(`user_program_count` 0), written to `kernel-disk.elf`.
The build is reproducible. **After changing anything here, or merlin,
merlin-as or merlin-ld, run `cargo xtask kernel` and commit `kernel.elf`,
`kernel-disk.elf` and `userprogs.s` with the sources.** `cargo test` rebuilds
all three in memory and fails if a committed one is stale.

### The clang reference build

`cargo xtask kernel --cc clang` builds the kernel with clang and ld.lld
instead (e.g. from the official Windows LLVM installer, found on `PATH` or
in `C:\Program Files\LLVM\bin`), into `target/kernel/clang/kernel.elf`, to
compare against. clang compiles the user programs' C to assembly in
`target/kernel/clang/user` (each output gets a two-line header naming its
source), which merlin-as and merlin-ld turn into the programs as above, and
then the kernel:

```
clang --target=riscv32-unknown-elf -march=rv32ima_zicsr -mabi=ilp32 -mno-relax -mcmodel=medany \
      -std=c11 -O2 -ffreestanding -fno-builtin -nostdinc -nostdlib -fno-pic -fno-common -fno-ident \
      -fno-stack-protector -fno-asynchronous-unwind-tables -fno-unwind-tables \
      -Wall -Wextra -Werror -S sh.c -o .../user/sh.s    # in user/, and each other .c
clang --target=riscv32-unknown-elf -march=rv32ima_zicsr -mabi=ilp32 -mno-relax -c entry.s -o entry.s.o   # and each other .s
clang --target=riscv32-unknown-elf -march=rv32ima_zicsr -mabi=ilp32 -mno-relax -mcmodel=medany \
      -std=c11 -O2 -ffreestanding -fno-builtin -nostdinc -nostdlib -fno-pic -fno-common -fno-ident \
      -fno-stack-protector -fno-asynchronous-unwind-tables -fno-unwind-tables \
      -Wall -Wextra -Werror -Iinclude -c main.c -o main.c.o        # and each other .c
ld.lld -T kernel.ld -o kernel.elf bio.c.o blk.c.o ... vmtest.c.o     # the same objects as above
```

`--as merlin` assembles the `.s` files with merlin-as instead (bootstrapping
step 1, spec section 10). Tests that need LLVM, ignored otherwise
(`cargo test -p xtask --test kernel -- --ignored`), check that the clang
build behaves like the committed one, and that merlin-as and merlin-ld can
take clang's and ld.lld's place in it. merlin's unoptimized code makes the
kernel about 40% bigger and 5 to 6 times slower than clang `-O2`'s (spec
section 10, "Kernel build", has the numbers).

## Rules for kernel code

merlin builds the kernel and the user programs, and clang must still build
them for the reference build, so they stick to plain C:

- No libc: headers come from `include/` (`-nostdinc`; `user/user.h` for the
  programs), no `<stdint.h>`/`<stdarg.h>`.
- No floating point.
- No inline `asm()`: CSR access and custom instructions live in `.s` files.
- Assembly uses only what merlin-as understands (no `.insn`, no preprocessor);
  Xcal instructions are `.word` constants. A test checks this.
- Varargs (`kprintf`) use clang's `__builtin_va_*` through `include/varargs.h`;
  Merlin provides the same builtins, with the same calling convention.
