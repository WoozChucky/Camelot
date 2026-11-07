# Camelot kernel

The Camelot OS kernel, in C with a little assembly. It runs in S-mode on an
SBI, the RISC-V Supervisor Binary Interface that Linux uses and OpenSBI
provides on boards: Camelot's own on Grail (`firmware/sbi.S`, in M-mode: the
kernel runs on Camelot's firmware, `grail --bios default`, Grail's default,
and has no M-mode code) sets up
trap delegation and forwards the timer and IPIs, starts the other harts and
stops the machine, and the kernel asks for those only through SBI calls
(`sbi.c`; spec section 10, "Kernel traps"). It prints a banner, what the SBI
has and a `kprintf` self-test over the UART, puts every free page of RAM on
the page allocator's free list, turns on Sv48 paging (Sv39 on a hart without
Sv48) with a kernel page table in the upper half, where it runs, linked at
0xffff_ffff_8000_0000,
runs the memory self-tests, lists the disks, takes ten timer ticks, tests the
first disk if there is one and mounts the filesystem on it, gives the first
network card an address (DHCP, or a static one), wakes the other harts if
the machine has several (the SBI's `hart_start`), and starts the shell, a user program, as the first
process. User programs run in U-mode, each in its own page table, switching on
every timer tick, on every hart at once; they call the kernel through system calls
(`docs/camelot-syscalls.md`): files, directories, the console and pipes
through file descriptors, starting and waiting for programs (from `/bin` on
the disk, or built into the kernel without one), sleeping, growing the heap,
threads that share a process and the futexes their locks sleep on. A program that faults is killed and
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
sbi: v2.0, implementation 0x434d4c54 version 0x1
sbi: extensions TIME IPI HSM SRST DBCN
kprintf: d=-42 u=4294967295 x=cafef00d p=0x0000000080000000 s=excalibur c=K %=%
kprintf self-test: 33/33 passed
[dbg] pc=ffffffff80004fe8 a0 = 0x0000000000000000 (0)
ram: 128 MiB at 0x80000000
paging: on (Sv48)
vm self-test: 26/26 passed
fb: 640x480, 6 modes, irq 16
timer: 10 Hz tick, 10000000 Hz timebase
rtc: 2026-11-06 09:41 UTC
disk: none
tick 1
tick 2
...
tick 10
kstack self-test: 3/3 passed
kstack: 880 of 16384 bytes used at most
Type help for the list of commands.
# hello
hello from user mode
# echo a  b   c
a b c
# exit
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
fb: 640x480, 6 modes, irq 16
timer: 10 Hz tick, 10000000 Hz timebase
rtc: 2026-11-06 09:41 UTC
disk 0: 8 sectors (4 KiB), irq 3
tick 1
...
tick 10
blk self-test: 14/14 passed
fs: no filesystem on disk 0
kstack self-test: 3/3 passed
...
Type help for the list of commands.
```

Disk 0 holding a Camelot filesystem (`docs/camelot-fs.md`) is mounted as the
root: the kernel replays its log if a crash left a transaction there, lists
`/` and runs its filesystem self-test (spec section 10, "Kernel
filesystem"). `cargo xtask fs` builds the root image with the user programs
in `/bin` and the files of `fs/` at `/`, no LLVM needed. The image is
bootable: it holds the bootloader (`boot/boot.S`) in its boot block and
the disk kernel, `kernel-disk.elf`, after the filesystem, so Grail can boot
it with no program, through Camelot's firmware (spec section 7, "Boot from
disk"; `docs/camelot-fs.md`, "Booting"), in 6 MiB of RAM (the firmware's
2 MiB and the default 4). The bootloader prints one line,
`boot: kernel at sector 0x...`, then the kernel starts as if Grail had
loaded its ELF.

The two kernels are Linux's split. `kernel-disk.elf` is the kernel alone,
about 380 KB: like Linux's, its programs are the files of the root
filesystem's `/bin`, and with no `/bin/sh` (or no filesystem) it panics
(`panic: no shell in /bin, and no programs built into the kernel`).
`kernel.elf` is the same kernel with a copy of every program built in (1.7
MB), which play the part of an initramfs: they are what it runs without a
disk. So the whole system fits in one image (the kernel about 300 blocks,
the filesystem the rest; its size is in the spec, section 10, "Root image");
`cargo xtask fs --size N` makes an image of N blocks.

```
cargo xtask fs
cargo run --release -p grail -- --ram 6 --disk target/camelot-fs.img
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
login: root
Password:
Type help for the list of commands.
# echo hello world > hello.txt
# cat hello.txt | wc
1 2 12
# mkdir notes
# cd notes
# ls /bin | wc > count
# cat < count
40 200 943
```

The root image's init runs `login` on the console: root's password is
`root`, and `user`'s (uid 1000, home `/home/user`) is `user`, unless `cargo
xtask fs --root-password`/`--user-password` gave others. Root's prompt is
`# `, anyone else's `$ ` (excalibur-spec section 10, "Users").

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
# ping 10.0.2.2 1
PING 10.0.2.2: 56 data bytes
64 bytes from 10.0.2.2: seq=1 ttl=64 time=1.211 ms
1 sent, 1 received
```

On a machine of several harts (`--harts N`, up to 8; spec section 10,
"Kernel SMP"), hart 0 boots the kernel alone, while the SBI holds the others,
then starts them with `hart_start`, and they join the scheduler: any hart runs any runnable thread, so up to N programs,
or N threads of one program, run at the same time. `harts` shows what each one is doing, and `primes`
keeps them all busy:

```
cargo run --release -p grail -- --harts 4 kernel/kernel.elf
```

```
tick 10
smp: 4 harts running
Type help for the list of commands.
# primes 4 40000
primes [0, 10000): 1229
primes [10000, 20000): 1033
primes [20000, 30000): 983
primes [30000, 40000): 958
primes: 4203 below 40000, 4 workers
# harts
HART   PID    TICKS     IDLE SWITCHES
   0     1       20        9        9
   1     7       10        2       14
   2     -       10        8        6
   3     -       10        9        5
```

The workers' lines come in whatever order they finish. Each hart has its own
timer, and takes its ticks from when it started; a thread runs on one hart
at a time, but may move from one to another between its turns (here the
shell, pid 1, is still on hart 0 on its way to wait for `harts`).

A process may have several threads (spec section 10, "Kernel threads";
`thread_create`, `thread_join` and the futex calls in
`docs/camelot-syscalls.md`), which share its memory and its descriptors.
`threads` starts some that add to shared counters under a mutex and a
spinlock from the C library, and checks the totals:

```
# threads 4 500
threads: 4 threads, 500 rounds: counter 2000, spun 2000: ok
```

The shell (`user/sh.c`) runs programs by name, with arguments: `PROG
[ARG...]` runs `/bin/PROG`, and a word with a `/` in it is a path to the
program. `CMD < FILE` and `CMD > FILE` redirect its input and output, `CMD |
CMD` sends the first one's output to the second, and `CMD & CMD` runs the
first in the background (the shell prints its pid) and the last in the
foreground, where Ctrl-C ends it; `wait` collects the background ones,
`cd [DIR]` changes the working directory, `exec PROG [ARG...]` replaces the shell with PROG (and
`exec < FILE` or `exec > FILE` redirects the shell itself), `help` lists the
built-ins, and `exit [N]` (or Ctrl-D on an empty line) ends the shell, after
which the kernel stops the machine with its status as the exit code (the
console's shell of the server's root image, through init, which ends the
machine when its `once` services have ended). Lines are edited with backspace and end with Enter. In a terminal,
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
# ding & dong & badload
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
# wait
ding 3
ding 4
dong 3
dong 4
[2] ding: exit 0
[3] dong: exit 0
```

The programs live in `user/`, one `.c` or `.s` file each, all linked with the
start code (`user/crt0.S`, which sets `environ` to `envp`, calls `main(argc, argv, envp)` and then `exit`), the C library of `user/lib/`, `libc.a` (strings, numbers, `malloc`,
`printf`, a line reader, `getenv`, and `usys.s`, the system call stubs;
declared in `user/user.h`, described in `user/lib/README.md`, an archive, so a
program gets only the parts it uses) and `user/user.ld`. `user/README.md` has each program's usage:

| Program | What it does |
|---|---|
| `init` | the first process: starts `/etc/rc`'s services, collects orphans |
| `sh` | the shell |
| `hello` | prints a line and exits |
| `echo` | prints its arguments (`-n`: without the newline) |
| `ls` | lists directories (the working directory without arguments); `-l` with modes and owners |
| `login`, `su`, `passwd` | log in, run a shell as another user, change a password |
| `id`, `whoami` | print the user and group ids, or the user's name |
| `chmod`, `chown` | change files' permission bits and owner |
| `cat` | copies files, or its input, to its output |
| `wc` | counts the lines, words and bytes of files, or of its input (`-l`, `-w`, `-c`) |
| `mkdir`, `rm`, `ln` | make directories, remove names, link a second name to a file |
| `cp`, `mv` | copy or rename files, or copy or move them into a directory |
| `grep` | prints the lines that contain a string (`-v`, `-c`, `-n`) |
| `ps` | lists the processes |
| `kill` | kills processes by pid |
| `env` | runs a program with the environment changed, or prints it |
| `printenv` | prints variables' values, or the whole environment |
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
| `permtest` | checks users and permissions on a writable root filesystem (`permtest: N checks, 0 failed`) |
| `stress` | many processes at once: pages, the console, a shared pipe and files under timer preemption (`stress: W workers, R rounds, N bytes: ok`) |
| `threads` | threads of one process adding to shared counters under a mutex and a spinlock (`threads: T threads, R rounds: counter N, spun N: ok`) |
| `threadtest` | checks the thread and futex calls, and processes with threads that exit, exec, fault or are killed (`threadtest: N checks, 0 failed`); `threadtest fs` does the same with threads in file calls, on the root filesystem |
| `maptest` | checks `mmap`, `munmap`, `mprotect`, `memfd_create` and `ftruncate` (`maptest: N checks, 0 failed`); `maptest threads` maps and unmaps from four threads at once, `maptest fb` draws into `/dev/fb0` through a mapping and waits for a line |
| `fbtest` | checks `/dev/fb0`'s fbdev requests: modes in VRAM and RAM, `-EBUSY` while mapped, a double-height buffer flipped, the vertical blank, blanking, the errors (`fbtest: N checks, 0 failed`); `fbtest show` shows 800x600 from a flip and waits for a line, `fbtest ansi` writes the framebuffer console's escape sequences |
| `inputtest` | checks `/dev/input/event0` and `event1`'s evdev requests and `EVIOCGRAB` (`inputtest: N checks, 0 failed`); `inputtest keys N` and `pointer N` print the next N events, `inputtest grab` two readers and a grab, `inputtest overflow` a reader that falls behind (`SYN_DROPPED`, `EVIOCGKEY`) |
| `libtest` | checks the C library (`libtest: N checks, 0 failed`) |
| `nettest`, `socktest` | move known bytes through TCP, for the TCP tests; check the socket calls (`socktest: N checks, 0 failed`) |
| `polltest` | checks `poll` on every kind of file (`polltest: N checks, 0 failed`); `polltest stress R` passes bytes between threads waiting in `poll` on several harts, which a lost wakeup would stop |
| `ptytest` | checks the terminals: pseudo-terminal pairs, canonical and raw modes, `VMIN`/`VTIME`, `termios`, Ctrl-C and the foreground, hanging up, the console's `termios`; and runs `sh` on a slave, typed at from the master (`ptytest: N checks, 0 failed`) |
| `unixtest` | checks local sockets, `SCM_RIGHTS`, and `sendmsg`/`recvmsg` on every socket, and shares a memfd with a second process (`unixtest: N checks, 0 failed`) |
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
shell takes about 3 million instructions with 4 MiB, and 16 million with
128 MiB, where most of them go to freeing (and, in the self-test, allocating)
every page one at a time. The kernel page table maps every whole 4 MiB of RAM
and devices with one megapage: page by page, the 64 MiB PLIC alone was 13
million instructions of every boot.

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
unmapped page, a W-only mapping, a superpage, `kfree` of a bad page), so the
tests see each of its checks panic, e.g. `panic: map_pages: remap`. Values
12 and 13 misuse a spinlock: 12 acquires one twice, 13 releases one it
doesn't hold, and each panics naming the lock and the hart, e.g.
`panic: acquire: lock "boot_fault" already held by hart 0`. 14 has a kernel
process yield while it holds a spinlock (`panic: sched: holding a lock
other than proc_lock`), 15 calls `pop_off()` with no `push_off()` and 16
turns interrupts on under a `push_off()` before its `pop_off()`.

## A higher-half kernel (#154)

The kernel is RV64 code, merlin's LP64, and runs as Linux's riscv64 kernel
does (spec section 10, "The kernel" and "Processes"):

```
grail --ram 8 kernel/kernel.elf        # Sv48, the default hart
grail --ram 8 --paging sv39 --harts 4 kernel/kernel.elf
```

- It runs in S-mode only, on Camelot's firmware (`--bios default`, Grail's
  default; spec section 7, "Firmware"), which keeps the first 2 MiB of RAM:
  8 MiB in all for the image and its programs (6 MiB for the disk kernel,
  from a disk).
- The image is linked at 0xffff_ffff_8000_0000 and loaded at 0x8020_0000
  (`AT` in `kernel.lds.S`). `entry.S` starts at the physical address in
  S-mode with
  paging off; `setup_vm()` (`vm.c`) probes `satp` for Sv48, falling back to
  Sv39, and builds the early page table, which maps the image at both
  addresses; every hart turns paging on and jumps to the high address;
  `kvminit()` builds the final table, which drops the identity mapping.
- All of RAM is in a direct map at `PAGE_OFFSET` (0xffff_af80_0000_0000 in
  Sv48), but the firmware's 2 MiB, which the device tree marks `no-map`
  and which Linux's linear map leaves out too; the device tree's pages are
  never handed out, as Linux reserves them; the kernel stacks at the start of the vmalloc area, then the
  devices, mapped as their drivers ask (`ioremap()`, which finds them room,
  as Linux's does); sysinfo and the UART in the fixmap for the boot. `pa()` and `va()` convert
  between kernel pointers and physical addresses, as Linux's `__pa` and
  `__va`: `kalloc()` returns direct-map pointers, and page tables, `satp`
  and the devices' DMA registers take physical ones.
- Kernel stacks are 16 KiB with an unmapped guard page below, and a stack
  that overflows is reported as `panic: kernel stack overflow in <name>`.
  A freed stack stays mapped in its hart's cache (two at most, Linux's
  `NR_CACHED_STACKS`); the others are unmapped and their pages freed where
  no lock is held (`kstack_reclaim()`).
- The boot runs every self-test, on 1 to 8 harts, loaded by Grail or from a
  disk (through `boot/boot.S`'s bootloader), then init (`user/init.c`, a
  64-bit program), as the first process, pid 1, Linux's init: it starts
  the services of the image's `/etc/rc` (the root image's profiles,
  `profiles/`: `login` on the console and `shd`) and collects orphans;
  without `/bin/init` the shell is the first process, and init without
  `/etc/rc` runs `login` again and again if there are accounts
  (`/etc/passwd`), and execs the shell if not.
- Users and permissions (#115): every process has real and effective user
  and group ids, every inode an owner and a mode (CMFS version 3), and
  `open`, `exec`, path lookup and the calls that change a directory check
  them (`fs.c`'s `inode_permission`); root passes but for running a file
  no one may run. `chmod`, `chown`, the set-ID bits, `kill`'s and `bind`'s
  rules: `docs/camelot-syscalls.md`, "Users and permissions".
- User space is the lower half (#155): below `USER_TOP`, 2^47 in Sv48 and
  2^38 in Sv39, Linux's `TASK_SIZE`, with the stack under it and the
  program at 0x1000. Every process's root table copies the kernel's
  entries 256-511; user leaves have A, and D if writable, from the start.
  The programs are ELF64; the system calls the LP64 ABI at Linux's widths
  (`docs/camelot-syscalls.md`): 64-bit user addresses, `size_t`, `ssize_t`
  and `off_t`, a `long` result sign-extended in `a0`.

```
...
paging: on (Sv48)
vm self-test: 26/26 passed
...
kstack self-test: 3/3 passed
smp: 4 harts running
kstack: 880 of 16384 bytes used at most
Type help for the list of commands.
# ps
  PID  PPID STATE      SIZE NAME
    1     0 sleep     40960 sh
   34     1 run       20480 ps
```

(The pids after the shell's went to the kernel stacks' self-test's kernel
processes.) Its tests are `crates/xtask/tests/kernel.rs`, and its
processes' `crates/xtask/tests/processes.rs`.

## Locking

Every structure that more than one thread, a thread and an interrupt
handler, or more than one hart can reach is protected by a
lock (spec section 10, "Kernel locking"). Spinlocks (`spinlock.S`,
`include/spinlock.h`) are xv6's: `acquire()` turns interrupts off on the
hart and spins on an `amoswap.w`; `release()` turns them back on once the
last lock is gone. Code that must wait sleeps with `sleep(chan, lk)`, which
lets go of the lock it holds while it sleeps; buffers and inodes, held
across disk reads, and each process's heap, held across `sbrk()`, have sleep
locks (`sleeplock.c`). A process's descriptors and working directory, which
all its threads use, have a spinlock of their own (`files`), and the user
copies one for everyone (`uvm`), which `unmap_pages()` takes before freeing
pages another thread may be copying through. Per-hart state is
`cpus[tp]`: `tp` holds the hart's id in the kernel, and `user_vec` puts it
back after a trap from U-mode.

## Several harts

`smp.c` says how the kernel runs on several harts (spec section 10, "Kernel
SMP"). Hart 0 boots alone, while the SBI holds every other hart STOPPED in
M-mode (its coldboot woke them out of the boot ROM's park loop, and each set
up its own M-mode trap vector and `mscratch` block); `smp_boot()` gives each
a kernel stack and starts it with the SBI's `hart_start` at
`secondary_entry`, which turns paging on and goes into `hart_main()`: its
tick, then the scheduler. A hart that fails to start, or doesn't come up
within a second, is reported and left out. The thread table is shared: any hart runs any
runnable thread, under `proc_lock`, and a hart with nothing to run waits in
`wfi` until its next tick, or until another hart makes a thread runnable
and wakes it with an IPI (the SBI's `send_ipi`). Only hart 0 counts the clock (`ticks`, `sleep`,
the network's timer) and takes the devices' interrupts. Taking away a
mapping (`unmap_pages()`) shoots it down on the harts that may hold it
(`tlb_shootdown()`) before the page is reused. A panic on one hart stops
the others with an IPI, and at their next byte to the UART (`uart_putc()`
checks), so its report comes out whole.

Locks are always taken in this order, so no two holders can wait for each
other:

1. sleep locks: `begin_op()` (the log), then inode locks (a parent
   directory before its child), then buffer locks;
2. one of `net`, `console`, `pipe`, `disk`, `log`, `ticks`, `fstest`
   (never two of them at once);
3. a process's `files`;
4. `itable`;
5. `ftable`, `bcache`, or a sleep lock's own spinlock;
6. `proc` (taken by `sleep()`, `wakeup()`, the scheduler and the futexes);
7. `kmem`, `uart` or `uvm` (leaves: nothing is taken while they are held).

A process's `heap` sleep lock stands apart from the other sleep locks:
`sbrk()` takes none of them under it. A thread may hold only the lock it
passes to `sleep()` when it sleeps, and `sched()` panics if it holds any
other. `stress` (`user/stress.c`) runs many processes through all of it at
once under timer preemption, and `threads` and `threadtest` many threads of
one process.

## Files

| File | What it does |
|---|---|
| `entry.S` | `_start`: in S-mode at 0x8020_0000, where the firmware starts the boot hart (a0 = its id, a1 = the device tree), as Linux's `_start_kernel`: `sie` and `sip` cleared, boot stack, zero `.bss` and the stack, the device tree's address in `dtb_early_pa`. `supervisor_entry` (S-mode, a0 = the hart id, a1 = the device tree): `tp`, `s0` = 0 (the outermost frame), `stvec`, `kmain`, then `machine_power_off` with its return value. `secondary_entry` (where `hart_start` starts the other harts, a1 = their stack): as Linux's `secondary_start_sbi`, `sie` and `sip` cleared, a trap vector that needs no stack (`early_s_trap`), `tp` and `sp`, then paging (`early_s_trap`'s high address in `stvec` before `kernel_satp` goes in `satp`), `kernel_vec`, `hart_main`. Also `xcal_dbg` for C and the early trap handler, `early_s_trap` (prints `scause`, `sepc`, `stval`, exits 0xff through the SBI). With paging off: `setup_vm()` on hart 0, `relocate_enable_mmu` (`satp` = the early table, return to the link address, adding `va_kernel_pa_offset`), the high stack, `stvec`. The early page tables and the harts' overflow stacks |
| `sbi.c` | the kernel's SBI calls, as Linux's `sbi.c`: `sbi_init()` (the version and the extensions, checked at boot), `sbi_set_timer()`, `sbi_send_ipi()` (a failure reported as Linux's `__sbi_send_ipi_v02()` reports it, the error mapped to an errno), `sbi_hart_start()`, `sbi_hart_stop()`, `sbi_hart_get_status()` and `sbi_hart_state_name()`, `sbi_system_reset()`, `machine_power_off()` (an exit code as a shutdown's reason, with a warning for each refused one, Linux's `sbi_srst_reset()` warning), `sbi_strerror()`; `sbi.h` has `sbi_err_map_linux_errno()` inline, as Linux's header (an SBI error as minus an errno, as Linux maps them) |
| `sbi_ecall.S` | `sbi_ecall()`: an SBI call, the arguments moved to a0-a5, a6 and a7, `struct sbiret` back in a0 and a1 |
| `kernelvec.S` | `kernel_vec`, the trap vector for traps from S-mode: saves x1-x31 and the trap CSRs as a `struct trap_frame` on the stack, calls `kernel_trap`, restores, `sret`. Also `illegal_instruction()` for the panic tests. It checks first that the frame fits on the stack (one bit of `sp`, the stacks being 32 KiB aligned) and sends a kernel stack overflow to the hart's overflow stack and `kstack_overflow()` |
| `uservec.S` | `user_vec`, the trap vector for traps from U-mode: saves the user registers in the thread's trap frame (`sscratch`), calls `user_trap` on the thread's kernel stack; `user_return`: frame to registers, `sret` to U-mode |
| `trap.c` | `kernel_trap()`: a timer tick (STIP) sets the next deadline and counts the tick (hart 0's wake sleepers), an IPI (SSIP) serves TLB shootdowns, claims/completes PLIC interrupts and runs the console's or a disk's handler, resumes faulting memory probes, panics on anything else (cause name, `sepc`, `stval`, the PTE for a page fault, register dump); `user_trap()`: a tick yields, an ecall goes to `syscall()`, a fault kills the process; `user_trap_return()`, `trap_init()`, `panic()` |
| `proc.c` | processes and threads: `proc_create()` (slots, kernel stack, page table, ELF, stack with the arguments, descriptors 0-2 but those that close on exec, and the working directory, inherited), `proc_exec_load()` and `proc_exec_commit()` (a new address space for the same process, loaded first; then the other threads ended, the old space freed and the close-on-exec descriptors closed), `kproc_create()` (a kernel process running a C function), `proc_run_init()` (the shell, with the console as descriptors 0-2), the round-robin `scheduler()` over the threads on every hart (idling in `wfi` while nothing is runnable, woken by an IPI when something is), `yield()`, `sleep()`/`wakeup()`, `proc_exit()` (ending every thread first), `proc_kill()`, `proc_wait()`, `proc_sbrk()`, `thread_create()`/`thread_exit()`/`thread_join()`, `futex_wait()`/`futex_wake()`, and the descriptor table under its lock (`fd_get()`, `fd_install()`, `fd_take()`, `proc_cwd()`) |
| `swtch.S` | `swtch()`: saves `ra`, `sp`, `s0`-`s11` and loads another thread's |
| `spinlock.S` | `acquire()`/`release()` (an `amoswap.w` loop; panics on a lock acquired twice or released unheld), `holding()`, `push_off()`/`pop_off()` (nested interrupt disabling), `mycpu()` (`&cpus[tp]`) |
| `spinlock.c` | `cpus[]`, the per-hart state; `initlock()`, `cpu_init()`, `cpuid()`, and the panic messages that name the lock and the hart |
| `smp.c` | the other harts: `smp_boot()` (stacks, `hart_start`, the wait for them, the TLB shootdown self-test), `hart_main()` (which stops a hart that comes too late, with `hart_stop`), `tlb_shootdown()`/`tlb_serve()` |
| `sleeplock.c` | sleep locks (xv6's), held across sleeps: `acquiresleep()`/`releasesleep()`, for the buffers, the inodes and each process's heap |
| `elf.c` | `elf_load()`: checks an ELF64 executable (an ELF32 one, RV32 code, is refused), read from memory or from a file through a `struct elf_file`, and maps its `PT_LOAD` segments into user space |
| `exec.c` | `exec_spawn()`, `exec_replace()`, `exec_init()`: find the program a path names (a file of the root filesystem, or an embedded program without one) and start it |
| `syscall.c` | `syscall()`: the system calls (`docs/camelot-syscalls.md`), with `spawn`'s and `exec`'s argument copying; the file ones go to `sysfile.c` |
| `sysfile.c` | the file system calls: `read`, `write`, `open`, `close`, `lseek`, `fstat`, `dup`, `fcntl`, `pipe`, `mkdir`, `unlink`, `link`, `chdir`, each change to the disk in one transaction, and `memfd_create`, `ftruncate` |
| `file.c` | open files (xv6's): the table of 64, `filealloc()`/`filedup()`/`fileclose()`, reading and writing an inode (writes 3 KiB per transaction), a device (`devsw`, Linux's `file_operations`: open, close, read, write, ioctl, mmap and the mapping counts; the console, the framebuffer, the input devices) or a pipe, `filelseek()`, `filestat()`, `filemmap()` (what backs a mapping of a descriptor: a shared-memory object or a device's memory) |
| `mmap.c` | memory mappings (Linux's VMAs, `struct vm_area`): `mm_mmap()`, `mm_munmap()`, `mm_mprotect()`, the page faults that map their pages on demand (`mm_fault()`), and `mm_release()` when a process ends or execs |
| `shm.c` | shared-memory objects (`memfd_create`): the table of 32, their references, `shm_truncate()`, their pages made on demand in a two-level table |
| `fb.c` | the framebuffer device (`/dev/fb0`, major 2), Linux's fbdev: the display controller's modes, the buffer (VRAM, or contiguous pages of RAM for another mode or two frames), `fb_mmap()`, `fb_ioctl()` (`FBIOGET/PUT_VSCREENINFO`, `FBIOGET_FSCREENINFO`, `FBIOPAN_DISPLAY`, `FBIO_WAITFORVSYNC`, `FBIOBLANK`), the vertical blank's interrupt, and the display back to the console when the last user goes |
| `fbcon.c` | the framebuffer console: 80x30 cells of Spleen 8x16 at 640x480 that `kprintf` and the console write to too; CR, LF, BS, TAB and ANSI's SGR, CUP, cursor moves, ED and EL; cells drawn as they change, scrolls at hart 0's clock tick (`fbcon_flush()`), suspended while a program has `/dev/fb0` |
| `fbblit.S` | the framebuffer console's pixel loops: a glyph, a fill, a copy, a row's first changed cell |
| `font.c` | Spleen 8x16 as PSF2, a C array, generated from `fonts/spleen-8x16.psfu` by `cargo xtask kernel` (`crates/xtask/src/font.rs`) |
| `fonts/` | the console's font as Spleen 2.2.0 ships it, its BSD-2-Clause licence and where it came from |
| `input.c` | the input core and evdev (`/dev/input/event*`, major 3): the devices, a reader per open file with a ring of 128 events, `SYN_DROPPED` on overflow, `EVIOCGRAB` and evdev's other requests |
| `keyboard.c` | the keyboard driver: raw mode's HID usages as Linux's key codes while a reader has it, ASCII mode's characters to the console otherwise, or as key codes on a US layout on a keyboard without raw mode |
| `pointer.c` | the pointer driver: absolute position as `ABS_X`/`ABS_Y`, three buttons, the wheel as `REL_WHEEL`, catching up after an overflow |
| `pipe.c` | pipes (xv6's): a 512-byte buffer in a page, readers and writers sleeping on each other, end of input when the writers are gone, `-EPIPE` when the readers are |
| `poll.c` | `poll()` (Linux's `poll_table`): every file's poll hook names its wait channels in the caller's table and says what it is ready for; a wakeup of any of them wakes the thread, or has it scan again, so none is lost (`proc.c`'s `poll_begin`, `poll_wait`, `poll_sleep`) |
| `user/` | the user programs (`*.c`, `*.s`), their start code (`crt0.S`) and header (`user.h`), the C library (`lib/`, `libc.a`, the system call stubs `usys.s` among it) and linker script `user.ld`, built with merlin, merlin-as and merlin-ld and embedded in `kernel.elf` through `userprogs.s`, which the build generates (not a file here: see "Building") |
| `kalloc.c` | physical page allocator: `kinit()` reads RAM's base and size from sysinfo, and the firmware's reserved range, and frees every page of RAM but the image's and the firmware's; `kalloc()`, `kzalloc()`, `kfree()`, `kfree_pages()`; `kalloc_contig()`/`kfree_contig()`, a run of contiguous pages (the framebuffer's buffers) |
| `vm.c` | page tables (Sv48 or Sv39, with the levels the boot chose): `walk()`, `map_pages()`, `unmap_pages()`, `pagetable_create()`/`pagetable_free()`; `kvminit()` builds the kernel page table, `kvminithart()` turns paging on; user address spaces: `uvm_create()`/`uvm_free()`, `uvm_range_ok()`, `copyin()`, `copyout()`, `copyinstr()`; `ioremap()`; the kernel stacks (`kstack_alloc()`: 16 KiB in the vmalloc area with a guard page); `setup_vm()`, the early page table and the `satp` probe |
| `vmtest.c` | the memory self-tests: allocate and free every page, `kzalloc()` and `pagetable_create()` clear a reused dirty page, a second mapping of one page, store page faults on text and rodata, building and freeing a page table |
| `probe.S` | `probe_load()`/`probe_store()`: a word access that returns the trap cause instead of panicking (`kernel_trap` resumes them at `probe_fault`) |
| `sysinfo.c` | reads Grail's sysinfo block: registers and the device table |
| `timer.c` | `timebase_hz()` (sysinfo's `SYS_TIMEBASE_HZ`, read once), `timer_interval()`, each hart's next deadline: `timer_start()`, `timer_next()` (past any missed ones, which it counts), through `sbi_set_timer()` |
| `plic.c` | `plic_init()` (UART source enabled for hart 0's S-mode context, which takes every device interrupt), `plic_enable()` (another source: a disk's, the display's, the keyboard's, the pointer's), `plic_claim()`, `plic_complete()` |
| `blk.c` | disk driver: `blk_init()` finds the disks in the sysinfo device table and enables their completion interrupts; `blk_read()`/`blk_write()`/`blk_flush()` issue one DMA request at a time and sleep until `blk_intr()`, the interrupt handler, wakes them with the `BLK_ERROR` code |
| `bio.c` | buffer cache (xv6's): 30 one-KiB block buffers in LRU order, `bread()`/`bwrite()`/`brelse()`, each buffer sleep-locked while in use |
| `blktest.c` | the disk self-test, in kernel processes: three readers at once, write and read back a scratch block (restored afterwards), errors, LRU reuse |
| `log.c` | the filesystem's write-ahead log (xv6's): `begin_op()`/`end_op()` around each operation, `log_write()` pinning changed buffers, group commit (slots, flush, header, flush, install, flush, empty header, flush), and the replay at mount |
| `fs.c` | the filesystem (xv6's): `fs_mount()`, the bitmap allocator, the inode cache (`iget`/`ilock`/`iput`, `readi`/`writei`), directories, path lookup (`namei`/`nameiparent`, from the working directory for a relative path), `fs_create()`/`fs_unlink()`/`fs_link()`, `itrunc()` |
| `fstest.c` | `fs_boot()`: mounts disk 0 in a kernel process, lists `/`, runs the filesystem self-test (and, with `fs_test` patched to 2, its transactions; to 3, filling the disk) |
| `net/` | the network stack (`docs/camelot-net.md`), a file per layer: `nic.c` (the card driver: descriptor rings, `nic_intr()`, the transmit queue), `pbuf.c` (the buffer pool), `ether.c`, `arp.c` (the cache), `ip.c` (checks, routing, loopback), `icmp.c` (echo, `icmp_ping()`), `udp.c` (the kernel-internal socket API, the echo service), `tcp.c` (connections, retransmission, the kernel-internal socket API), `dhcp.c` (the client), `net.c` (configuration, `net_boot()`, the counters, `net_timer()`, the `netinfo`/`netconf`/`ping` system calls), `socket.c` (the socket system calls: sockets behind file descriptors), `unix.c` (local sockets, `AF_UNIX` streams: names on the disk, connections, descriptors passed), `msg.c` (`sendmsg` and `recvmsg`'s buffers and `SCM_RIGHTS`) |
| `fs/` | the files `cargo xtask fs` copies to `/` of the root image (a README); the user programs go to `/bin`, and `/dev/console`, `/dev/fb0`, `/dev/input/event0` and `/dev/input/event1`, `/dev/ptmx` and `/dev/pts/0` to `15` are made too |
| `boot/` | the bootloader of a bootable disk (`boot.S`, linked by `link.ld` into the committed `boot.elf` with `cargo xtask boot`), the firmware's next stage: the boot block, which loads the kernel ELF stored after the filesystem; `camelot-fs mkfs --boot` installs it |
| `riscv.S` | S-mode CSR accessors for C (`r_sstatus`, `w_sie`, `w_satp`, ...), `sfence_vma()`, `intr_off()`/`intr_on()`, `wfi()`, the atomics `atomic_swap()`/`atomic_add()`/`atomic_or()`, and `fence_rw()` |
| `main.c` | `kmain()`: banner, `sbi_init()`, `kprintf` self-test, memory setup and self-tests, the disk list, the network cards (`net_init`), ten ticks, the optional deliberate fault or memory API misuse, the disk self-test, the filesystem (`fs_boot`), the network's address (`net_boot`), the other harts (`smp_boot`), then the user-space shell as the first process |
| `uart.c` | 16550 driver: receive interrupt handler `uart_intr()` filling a ring buffer (`uart_rx_pop()`), polling `uart_putc`/`uart_puts` |
| `console.c` | the console device (`read`, `write` and `ioctl` on it), a terminal of `tty.c`'s line discipline with the defaults it has always had: `console_read()` (`tty_read()`), `console_write()`, `console_intr()` (the UART interrupt: the ring's bytes into the terminal's input, Ctrl-C at once); `console_input()`, the keyboard's characters while no process reads it; the output and the echo go to the framebuffer console too |
| `tty.c` | the terminals' line discipline (Linux's `n_tty`), the console's and the pseudo-terminals': input mapped and its Ctrl-C acted on as it comes (`tty_receive()`), canonical lines edited and echoed as they are read, non-canonical reads with `VMIN` and `VTIME`, `ONLCR`, and the `termios`, window size, foreground and input requests |
| `pty.c` | pseudo-terminals: `/dev/ptmx` makes a pair (16 at most) and gives its master, `/dev/pts/N` its slave, a `tty.c` terminal; the master's writes are the slave's input, the slave's writes a 2 KiB ring the master reads; `TIOCGPTN`, `TIOCSPTLCK`, and hanging up either way |
| `kprintf.c` | `kprintf`/`ksnprintf`: `%d %u %x %p %s %c %%`, `0` flag, width; `kprintf` writes to the UART and the framebuffer console |
| `memset.S`, `memcpy.S`, `memmove.S`, `memcmp.S` | `memset memcpy memmove memcmp` (clang may call them by itself; `string.h`), in assembly, a register at a time as Linux's and the C libraries' are: whole registers where both pointers are on a register boundary, bytes around them, eight registers a round for `memset` and `memcpy`; `memmove` copies pointers that are differently aligned a register at a time too, from two aligned loads shifted, and `memcpy` hands it those. The user library's are these files (`user/lib/mem*.S` include them) |
| `strlen.S`, `strcmp.S` | `strlen strcmp`, in assembly, two bytes a round; the user library's too |
| `kernel.lds.S` | the layout, preprocessed first (`merlin -E -P`, as Linux's `vmlinux.lds.S`): `.text` (`_start` first), then page-aligned `.rodata` and `.data` (so each gets its own page permissions), `.bss`, 16 KiB stack, `__kernel_end`; linked at 0xffff_ffff_8000_0000 and loaded at 0x8020_0000 (`AT`), after the firmware's 2 MiB, the boot stack 32 KiB aligned |
| `include/` | `types.h` (fixed-width types), `mmio.h` (`volatile` accessors), `varargs.h`, `riscv.h` (CSR bits, cause numbers), `memlayout.h` (page size, linker symbols), `sysinfo.h` (register offsets, device types), `plic.h`, `vm.h` (also the user address space layout), `proc.h` (`struct proc`, `struct thread`, `struct user_program`, sleep and wakeup, the thread and futex calls), `context.h` (`struct context`, `swtch`), `spinlock.h` (`struct spinlock`, `struct cpu`, `NCPU`), `smp.h`, `sleeplock.h`, `elf.h`, `blk.h` (disk registers, `BLK_ERR_` codes), `bio.h` (`struct buf`, `BSIZE`, `NBUF`), `fs.h` (the on-disk structures, `struct inode`, the filesystem's API), `log.h` (transactions, `LOGSIZE`, `MAXOPBLOCKS`), `file.h` (`struct file`, `NFILE`, `NOFILE`, `devsw`), `mmap.h` (`struct vm_area`, `PROT_`, `MAP_`), `shm.h` (`struct shm_object`, `MFD_`), `fb.h`, `pipe.h`, `exec.h`, `sysfile.h`, `errno.h` (`syscall.h`'s error numbers and the kernel's own: `EIO`, `ERANGE`, `ENOTSUPP`), `syscall.h` (the system call and error numbers, `open`'s flags, `struct stat`, `struct netinfo`), `net.h` (the network stack: `struct pbuf`, every layer's functions and limits), `nic.h` (the card's registers and descriptors), and the headers for the above |
| `kernel.elf` | the prebuilt image, with the user programs built in, committed so `cargo test` and Grail need no build |
| `kernel-disk.elf` | the prebuilt disk kernel: the same objects without the user programs, which a bootable root image holds (`cargo xtask fs`) |

## Building

Our own toolchain builds the kernel, with no LLVM:

```
cargo xtask kernel          # builds kernel/kernel.elf and kernel/kernel-disk.elf
cargo xtask kernel --run    # ... and runs it on Grail, with this terminal as its console
cargo xtask kernel -v       # shows the merlin/merlin-as/merlin-ld command lines
```

`cargo xtask` is a Rust program (`crates/xtask`), so there is no Makefile or
shell script, and merlin, merlin-as and merlin-ld run inside it as
libraries. It first builds the user programs: merlin compiles each
`user/*.c` (with `-I user`), merlin-as assembles every program and the
runtime, and merlin-ld links each with `user/user.ld`, all in memory; the
executables go into `userprogs.s` as data: the user programs' ELF images and
the `user_programs` table (`crates/xtask/src/userprogs.rs`). That file is
generated by every build and is not a source here (it used to be committed;
a stale copy in this directory is now an error): `cargo xtask kernel` writes
it to `target/kernel/obj` with the objects, and it is a source like the
others, linked in its name order. Then it compiles every `*.c` and
assembles every `*.S` in this directory, in name order (a `.S` file goes
through the C preprocessor first, `merlin -E`, with `#include <asm.h>`
from the repository's `include/`, #193),
and links with `kernel.lds.S`, the linker script, preprocessed the same way
but without line markers (`-P`), which a linker script can't have. By hand, from this directory, with the tools' command lines
(`cargo run -p merlin --`, and so on), the kernel build is:

```
merlin -g -fdebug-compilation-dir=kernel -I include main.c -o main.c.s   # and each other .c
merlin-as main.c.s -o main.c.o
merlin -E -I ../include -I include entry.S -o entry.S.s   # and each other .S
merlin-as entry.S.s -o entry.S.o
merlin-as userprogs.s -o userprogs.s.o       # the generated one (in target/kernel/obj)
merlin -g -fdebug-compilation-dir=kernel -I include net/arp.c -o net/arp.c.s   # the network stack's too
merlin -E -P -I ../include -I include kernel.lds.S -o kernel.lds
merlin-ld -T kernel.lds -o kernel.elf bio.c.o blk.c.o blktest.c.o console.c.o elf.c.o entry.S.o \
       exec.c.o fb.c.o fbblit.S.o fbcon.c.o file.c.o font.c.o fs.c.o fstest.c.o input.c.o \
       kalloc.c.o kernelvec.S.o keyboard.c.o kprintf.c.o log.c.o main.c.o memcmp.S.o memcpy.S.o \
       memmove.S.o memset.S.o mmap.c.o net/arp.c.o net/dhcp.c.o net/ether.c.o net/icmp.c.o \
       net/ip.c.o net/msg.c.o net/net.c.o net/nic.c.o net/pbuf.c.o net/socket.c.o net/tcp.c.o \
       net/udp.c.o net/unix.c.o pipe.c.o plic.c.o pointer.c.o poll.c.o probe.S.o proc.c.o pty.c.o \
       riscv.S.o sbi.c.o sbi_ecall.S.o shm.c.o \
       sleeplock.c.o smp.c.o spinlock.c.o spinlock.S.o strcmp.S.o strlen.S.o swtch.S.o syscall.c.o \
       sysfile.c.o sysinfo.c.o timer.c.o trap.c.o tty.c.o uart.c.o \
       userprogs.s.o uservec.S.o vm.c.o vmtest.c.o
```

(`-g` writes line tables for merlin-dbg, which are not loaded; the
compilation directory makes their paths `kernel/main.c`, relative to the
repository root. `cargo xtask kernel` keeps the `.s` and `.o` files in `target/kernel/obj`,
`net/`'s in `target/kernel/obj/net`.) The disk kernel is the same link with
`userprogs.s.o` replaced by the object of a `userprogs.s` with no programs
(`user_program_count` 0), written to `kernel-disk.elf`.
The build is reproducible. **After changing anything here (the user
programs included), or merlin, merlin-as or merlin-ld, run `cargo xtask
kernel` and commit `kernel.elf` and `kernel-disk.elf` with the sources.**
`cargo test` rebuilds both in memory and fails if a committed one is stale.

### The clang reference build

`cargo xtask kernel --cc clang` builds the kernel with clang and ld.lld
instead (e.g. from the official Windows LLVM installer, found on `PATH` or
in `C:\Program Files\LLVM\bin`), into `target/kernel/clang/kernel.elf`, to
compare against. clang compiles the user programs' C to assembly in
`target/kernel/clang/user` (each output gets a two-line header naming its
source), which merlin-as and merlin-ld turn into the programs as above, and
then the kernel:

```
clang --target=riscv64-unknown-elf -march=rv64ima_zicsr -mabi=lp64 -mno-relax -mcmodel=medany \
      -std=c11 -O2 -ffreestanding -fno-builtin -nostdinc -nostdlib -fno-pic -fno-common -fno-ident \
      -fno-stack-protector -fno-asynchronous-unwind-tables -fno-unwind-tables \
      -Wall -Wextra -Werror -S sh.c -o .../user/sh.s    # in user/, and each other .c
clang --target=riscv64-unknown-elf -march=rv64ima_zicsr -mabi=lp64 -mno-relax -I../include -Iinclude \
      -c entry.S -o entry.S.o                                      # and each other .S
clang --target=riscv64-unknown-elf -march=rv64ima_zicsr -mabi=lp64 -mno-relax -mcmodel=medany \
      -std=c11 -O2 -ffreestanding -fno-builtin -nostdinc -nostdlib -fno-pic -fno-common -fno-ident \
      -fno-stack-protector -fno-asynchronous-unwind-tables -fno-unwind-tables \
      -Wall -Wextra -Werror -Iinclude -c main.c -o main.c.o        # and each other .c
ld.lld -T kernel.lds -o kernel.elf bio.c.o blk.c.o ... vmtest.c.o    # the same objects and script as above
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
