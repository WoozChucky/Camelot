# Camelot

A learning project: design an instruction set, emulate it, build the toolchain for it, and write an operating system that runs on it. Everything builds and runs natively on Windows (and Linux/macOS) with nothing but Rust's standard library.

| Piece | What it is | Status |
|---|---|---|
| **Excalibur** | The ISA: RV32IMA + Zicsr, M/S/U privilege, Sv32 paging, plus the Xcal extensions | Spec v0 in [`docs/excalibur-spec.md`](docs/excalibur-spec.md) |
| **Grail** | The emulator | Passes riscv-tests rv32ui/um/ua/mi/si; CLINT, PLIC, UART with receive interrupts; Xcal framebuffer (Win32 window or PPM dumps), keyboard and pointer (from the window, with PLIC interrupts), sysinfo and block devices (disk images or disks on a Lake server, `--disk`); configurable RAM, real-time clock, disks and number of harts (up to 8, with IPIs) |
| **Merlin** | The toolchain: `merlin-as`, `merlin-ld`, `merlin-objdump`, the `merlin` C-subset compiler and the `merlin-dbg` debugger | Builds every example and the whole Camelot kernel on its own, no LLVM (`cargo xtask examples`, `cargo xtask kernel`). merlin-as: complete; its objects match clang's ([encoder notes](crates/merlin-as/ENCODER.md)). merlin-ld: links clang objects to the same bytes as ld.lld except where ld.lld merges `SHF_MERGE` strings and constants (see [spec §10](docs/excalibur-spec.md)), no relaxation (relocations explained in [src/reloc.rs](crates/merlin-ld/src/reloc.rs)). merlin: a C99 subset (integer types, pointers, arrays, structs, globals, strings, the preprocessor, varargs, designated initializers; programs of several files) compiles to assembly that our own assembler and linker build into programs Grail runs, the kernel and its user programs among them; unoptimized, so that kernel is about 40% bigger and 5 to 6 times slower than clang `-O2`'s ([code generation notes](crates/merlin/src/codegen.rs)). merlin-objdump: matches llvm-objdump (`-l` too). merlin-dbg: a debugger over Grail's GDB remote stub, which GDB and LLDB can use too, at the machine level (breakpoints, watchpoints, steps, registers, memory, disassembly, backtraces, harts) and by source line, from the DWARF line tables `merlin -g` writes (merlin-as encodes them as clang does, byte for byte) ([docs/merlin-dbg.md](docs/merlin-dbg.md)) |
| **Camelot** | The kernel / OS | Boots on Grail in S-mode: `kprintf`, trap handling, timer ticks, page allocator and Sv32 paging, interrupt-driven UART console, user programs in C in U-mode with their own page tables, switched round robin on timer ticks, system calls ([docs/camelot-syscalls.md](docs/camelot-syscalls.md)) and a user-space shell, an interrupt-driven disk driver with a buffer cache, a journaled filesystem mounted at boot ([docs/camelot-fs.md](docs/camelot-fs.md), with a host-side mkfs and fsck in `crates/camelot-fs`), a network stack with a card driver, ARP, IPv4, ICMP, UDP, TCP and DHCP, sockets behind file descriptors and network programs (`ifconfig`, `ping`, `nc`, `fetch`, `httpd`) ([docs/camelot-net.md](docs/camelot-net.md)) ([kernel/](kernel/README.md)) |
| **Avalon** | The platform around Grail: the Avalon hypervisor server, the Round Table control CLI, the Pendragon web console, the Herald VM client and the Lake storage server | Protocol v7 in [`docs/avalon-protocol.md`](docs/avalon-protocol.md), implemented by `crates/avalon-proto`. The Avalon server runs N VMs with their own hardware profiles, a RAM budget, attachable consoles and screens, and network cards on virtual Ethernet switches, which can have a NAT gateway to the host and the internet; the Round Table CLI creates and controls them, and manages Lake's disk images; Pendragon does the same from a browser, with live consoles; the Lake storage server serves disk images to them over the network; Herald attaches a terminal to a VM's console, or a window to its screen, keyboard and mouse. `cargo xtask up` runs it all with one command ([Quick start](#quick-start)) |

## Quick start

You need [Rust](https://rustup.rs) and nothing else: no crates to download, no LLVM, no WSL. In a terminal (PowerShell, cmd or a bash) at the root of the repository:

```
cargo xtask up --demo
```

This builds the platform in release mode (a few minutes the first time), builds the Camelot root image (`target/camelot-fs.img`, the kernel and its programs on a bootable disk) and puts it in Lake's images directory, `target/platform/images`, as the image `os-base`. Then it runs Lake, Avalon and Pendragon, with their output in this terminal, each line prefixed by its server. `--demo` also creates a switch `nat` with a NAT gateway, and a VM `vm001` with the default hardware (4 MiB of RAM) that boots Camelot from `vm001-disk0`, its own copy of the image, plugged into the switch. The gateway forwards 127.0.0.1:2323 to the VM's port 23, and once the VM is at its prompt `up` starts `shd` there, Camelot's shell server (it types `shd &` on the console). Once everything is up, `up` says where it is:

```
The Avalon platform is up.

  Pendragon  http://127.0.0.1:7402/?token=3f9c...e1
             open it in a browser (the token changes at every launch)
  Avalon     127.0.0.1:7400
  Lake       127.0.0.1:7401, images in C:\...\Camelot\target\platform\images
```

Open the Pendragon URL in a browser. Its **New VM** page creates a VM: give it a name and leave *Camelot (boot from disk)* chosen, and the VM gets a disk of its own, `NAME-disk0`, copied from the image `os-base`, and boots from it; its page shows the console, where Camelot's shell waits (`ls /bin`, `echo hi > /hi`, `cat /hi`). The demo's VM is in the VM list already.

Or, from a second terminal, Round Table creates the VM and Herald attaches the terminal to its console:

```
cargo run --release -p round-table -- create --name web --camelot --start
cargo run --release -p round-table -- list
cargo run --release -p herald -- web
cargo run --release -p herald -- vm001
cargo run --release -p herald -- --tcp 127.0.0.1:2323
```

Ctrl-] leaves Herald, and the VM carries on. The console is one for the whole VM: every Herald attached to it shares its input and output, so while the shell there runs `sleep 5000`, nobody gets a prompt. The last command is different: `herald --tcp` connects straight to the demo VM's `shd` (not through Avalon), and each connection is a shell of its own, beside the console's and the other connections', so one busy session never holds up another. There Herald edits the line (Backspace, Ctrl-U, Ctrl-W) and sends it with Enter; Ctrl-D on an empty line ends the session, and so does `exit`. **`shd` is for local use only:** until Camelot has users, its shells run as root with no password, so keep the forward on 127.0.0.1 (`--shd ADDR` picks another address). Without `--demo`, a VM plugged into a NAT switch with a forward to its port 23 (`round-table switch create nat --nat --hostfwd tcp::2323-:23`) gets one by typing `shd &` on its console. **Ctrl-C** in the first terminal stops Pendragon, Avalon and Lake. The images stay in `target/platform/images`, so a VM's disk keeps what it wrote: the next `cargo xtask up` serves them again (the VMs themselves end with Avalon; `--demo` creates its VM again on its old disk). `up` replaces the image `os-base` when a build has changed it, and leaves every other image alone, so a disk copied from it before keeps the old build: `--demo` still boots `vm001-disk0` as it is (and says so). Delete `target/platform/images/vm001-disk0` while `up` is not running to have the demo copy the new image.

`cargo xtask up --help` lists the options: `--images DIR` for another images directory, `--lake ADDR`, `--avalon ADDR`, `--pendragon ADDR` and `--shd ADDR` for other addresses, `--no-build` to start what is already built. To run the servers one by one instead, see [The Avalon platform](#the-avalon-platform); for Grail on its own, [Running](#running).

## Layout

```
crates/excalibur        instruction encodings, decoder, disassembler (shared by every tool)
crates/elf32            ELF32 reader and DWARF line tables (shared by grail, merlin-objdump, merlin-ld, merlin-dbg)
crates/grail            the emulator
crates/avalon-proto     the Avalon protocol: frames, messages, blocking connection, screen diffing (docs/avalon-protocol.md)
crates/gateway          the user-mode NAT gateway behind `grail --net user` and Avalon's switches: DHCP, DNS, NAT and port forwarding over std::net
crates/avalon           the Avalon hypervisor server: runs N Grail VMs and the virtual switches between them, serves the protocol
crates/avalon-client    the client side of the platform: VM and disk operations, shared by Round Table and Pendragon
crates/round-table      the Round Table control CLI (a thin layer over avalon-client)
crates/pendragon        the Pendragon web console: a std-only HTTP server and a browser page for VMs, disks and switches (docs/pendragon.md)
crates/herald           Herald, the Avalon client that attaches a terminal to one VM's console, or a window to its screen; `--tcp` connects to a TCP port instead (a shell from shd)
crates/lake             the Lake storage server: a directory of disk images, served as block devices over the protocol
crates/camelot-fs       the Camelot filesystem on the host (docs/camelot-fs.md): mkfs, fsck and a reader, the `camelot-fs` command
crates/merlin           the C-subset compiler (C → .s), with its test programs in tests/cases
crates/merlin-as        the assembler (source → ELF32 .o)
crates/merlin-ld        the linker (.o files → ELF32 executable)
crates/merlin-objdump   the disassembler / object dumper (llvm-objdump compatible output)
crates/merlin-dbg       the debugger: a GDB remote protocol client for Grail's stub (`grail --gdb`), machine-level and by source line (docs/merlin-dbg.md)
crates/xtask            build tasks: `cargo xtask up` (the whole platform), `cargo xtask examples`, `cargo xtask kernel`, `cargo xtask fs`, `cargo xtask boot`, `cargo xtask hello`, `cargo xtask ld-fixtures`
kernel/                 the Camelot kernel (C + assembly) and its prebuilt kernel.elf (and kernel-disk.elf, without the built-in programs, for bootable images); kernel/user holds the user programs it embeds (and kernel/user/lib their C library), kernel/net the network stack, kernel/fs the root image's files, kernel/boot the bootloader of a bootable disk
examples/               bare-metal programs, built with merlin-as + merlin-ld (the .elf files are committed)
tests/riscv-tests       prebuilt riscv-tests ELFs and their objects for merlin-ld (rebuild with scripts/build-riscv-tests.sh)
scripts/                test-data generators (riscv-tests, merlin-as golden encodings, merlin-objdump goldens)
docs/                   the ISA spec, the Avalon protocol, Pendragon's API, the Camelot system calls, filesystem and network stack, and merlin-dbg with Grail's GDB stub
```

## Running

You need [Rust](https://rustup.rs). No crates, no cross compiler, no QEMU.

```
cargo run --release -p grail -- examples/hello/hello.elf
```

```
Hello from Camelot!
[dbg] pc=80000018 a0 = 0x00000014 (20)
```

The kernel runs the same way: `cargo run --release -p grail -- kernel/kernel.elf`. It boots into a user-space shell that runs programs by name (`hello`, `echo a b`, `ticks`; `ding & dong` runs two side by side; `exit` stops the machine); with `--disk IMG` it lists the disk and runs a read/write self-test on it at boot, leaving the image as it was; when stdin is a terminal, Grail switches it to raw mode so each key reaches the guest as it is typed and the guest does the echoing (Windows consoles and Linux terminals; restored on exit and on Ctrl-C).

### Building the examples

The examples are built with our own assembler and linker, nothing else:

```
cargo xtask examples            # every example
cargo xtask examples hello -v   # one, printing the equivalent commands
```

```
merlin-as examples/hello/hello.S -o target/examples/hello/hello.o
merlin-ld -T examples/hello/link.ld target/examples/hello/hello.o -o examples/hello/hello.elf
built examples/hello/hello.elf
```

Each `examples/<dir>/<name>.elf` is rewritten (they are committed, so running them needs no build) and the objects land in `target/examples/`. `cargo xtask examples --check` builds in memory and reports any committed executable that is out of date; `cargo test` does the same check. The tools also work by hand: `cargo run --release -p merlin-as -- examples/hello/hello.S -o hello.o`, then `cargo run --release -p merlin-ld -- -T examples/hello/link.ld hello.o -o hello.elf`.

`examples/multiobj` links two objects (`main.S` calls `puts` in `print.S`, which counts bytes in its `.bss`; `main.S` keeps a string table and a counter in `.data`) and exits with code 42:

```
cargo run --release -p grail -- examples/multiobj/multiobj.elf
```

The top of each source also has the clang + ld.lld command that builds the same program, for comparison only; [spec section 10](docs/excalibur-spec.md) ("Examples") lists how the two executables differ (the loaded bytes don't).

`examples/uart-echo` is interrupt-driven: it sleeps in `wfi`, and every byte on stdin raises a machine external interrupt through the PLIC. It echoes until it receives EOT (0x04):

```
printf 'hi\n\004' | cargo run --release -p grail -- examples/uart-echo/uart-echo.elf
```

Options: `--trace` prints every instruction with its disassembly, `--max-steps N` stops a runaway program, `--ram MiB` sets RAM size (1 to 2048, default 4), `--clock FREQ` runs in real time at FREQ instructions per second (`10MHz`, `500kHz`, `1.5MHz`, `5Hz`; without it Grail runs at full speed and deterministically, reporting a nominal 10 MHz), `--disk IMG` attaches a disk image (repeat for up to 8 disks; `--disk-ro IMG` attaches one read-only), `--harts N` builds a machine of N harts (1 to 8, default 1; spec section 7, "Harts": they take turns one instruction at a time, deterministically, and the harts other than 0 wait in the boot ROM for an IPI). `examples/smp` starts every hart and has them race on shared counters: `cargo run --release -p grail -- --harts 4 examples/smp/smp.elf`. Grail's exit code is the program's (`xcal.halt`, the test device, or riscv-tests' `tohost`).

Software finds the RAM size, the clock and the devices in the Xcal sysinfo block (spec section 9.5). `examples/sysinfo` prints them:

```
cargo run --release -p grail -- --ram 64 --clock 2MHz examples/sysinfo/sysinfo.elf
```

```
sysinfo v1
RAM      64 MiB at 0x80000000
CPU      2000000 Hz (real time)
timebase 2000000 Hz
9 devices:
  test      0x00100000 + 0x00001000
  CLINT     0x02000000 + 0x00010000
...
```

As a library, Grail builds a `Machine` from a `MachineConfig` (RAM size, clock, disks); each machine has its own UART and debug sinks and disk backends, and can run on its own thread, with `run_for` to run it in slices.

To look inside an object or executable:

```
cargo run --release -p merlin-objdump -- -d examples/hello/hello.elf
```

```
80000000 <_start>:
80000000: 00010117     	auipc	sp, 0x10
80000004: 07010113     	addi	sp, sp, 0x70
...
80000010: 00000097     	auipc	ra, 0x0
80000014: 010080e7     	jalr	0x10(ra) <puts>
80000018: 0005000b     	xcal.dbg	a0
```

The options are llvm-objdump's: `-d` disassemble, `-r` relocations (under each instruction with `-dr`), `-t` symbols, `-h` sections, `-x` all headers, `-s` section contents, `-M no-aliases`, `--no-show-raw-insn`. The output is the same as llvm-objdump 21's apart from Xcal instructions and undecodable words (`.word`), so the two can be diffed.

### Debugging

`merlin-dbg` debugs a program running in Grail at the machine level, and by source line where the program has line tables ([docs/merlin-dbg.md](docs/merlin-dbg.md)). Given `--` and Grail's arguments, it starts the machine itself, stopped at its first instruction:

```
cargo run --release -p merlin-dbg -- -- examples/hello/hello.elf
```

```
(merlin-dbg) break puts
breakpoint 1 at 0x80000020 <puts>
(merlin-dbg) continue
breakpoint 1
=> 0x80000020 <puts>:  lui t0, 0x10000
(merlin-dbg) bt
#0  0x80000020 <puts>
#1  0x80000018 <_start+24>
```

It has breakpoints and watchpoints, `stepi`, `nexti` and `finish`, `regs`, `x/N`, `disas`, `bt` (by frame pointer) and `hart N`. Commands can also come from a file (`-x FILE`) or a pipe.

The kernel and its user programs are built with `merlin -g`, so in them it works by source line too: `break FILE:LINE`, `step` and `next` by line, `list`, and every stop shows its line. Run it in the repository root, where the line tables' paths lead to the sources:

```
cargo run --release -p merlin-dbg -- -- kernel/kernel.elf
```

```
(merlin-dbg) break kernel/main.c:273
breakpoint 1 at 0x80017668 <kmain>: kernel/main.c:273
(merlin-dbg) continue
breakpoint 1
=> 0x80017668 <kmain>:  addi sp, sp, -0x10
kernel/main.c:273: int kmain(void) {
(merlin-dbg) next
=> 0x80017678 <kmain+16>:  auipc ra, 0x23
kernel/main.c:275:     cpu_init();
```

A user program's line tables are in `target/kernel/obj/user/<name>.elf` after `cargo xtask kernel`: `add-symbol-file target/kernel/obj/user/sh.elf`, then `break kernel/user/sh.c:502`.

Behind it is Grail's GDB remote stub, which `grail --gdb ADDR` runs over TCP. `--gdb-wait` starts the machine stopped, waiting for a client. merlin-dbg connects to it with `--connect`, and so can GDB (`target remote 127.0.0.1:1234`) and LLDB (`gdb-remote 127.0.0.1:1234`):

```
cargo run --release -p grail -- --gdb-wait kernel/kernel.elf
cargo run --release -p merlin-dbg -- --connect 1234 kernel/kernel.elf
```

A debugged run executes exactly the instructions an undebugged one does (spec section 7, "Debugging").

### Framebuffer

Programs can draw into a 640×480 framebuffer (spec section 9.3). Grail is headless by default; `--fb` opens a native window showing it (Windows only, plain Win32 GDI through hand-written FFI; elsewhere Grail warns and stays headless). `--fb-dump DIR` writes `DIR/frame-NNNN.ppm` (at least four digits) on every `FB_PRESENT` and when the machine stops, with or without a window. `--headless` turns the window off even if `--fb` is given.

```
cargo run --release -p grail -- --fb examples/colorbars/colorbars.elf
cargo run --release -p grail -- --fb-dump frames examples/colorbars/colorbars.elf
```

Closing the window stops the emulator. When the program stops first, the window stays open on the last frame until you close it.

### Keyboard

Keys typed into the framebuffer window go to the Xcal keyboard device (spec section 9.4): a FIFO of press/release events with Shift and Ctrl flags, polled through `KBD_STATUS`/`KBD_DATA` or delivered as PLIC interrupts (source 2). Without a window the keyboard stays empty and stdin keeps feeding the UART. `examples/kbd-echo` echoes key presses to the terminal, polling (`kbd-echo.elf`) or interrupt-driven (`kbd-echo-irq.elf`); Esc stops it:

```
cargo run --release -p grail -- --fb examples/kbd-echo/kbd-echo.elf
```

Programs embedding Grail as a library feed it keys with `Machine::key_event`, the same entry point the window uses.

### Pointer

The mouse over the framebuffer window goes to the Xcal pointer device (spec section 9.7): a FIFO of events with the position in framebuffer pixels (absolute mode, the default, like a tablet; the mapping follows the window when it is resized) or the motion since the last event (relative mode), the left, right and middle buttons and the wheel, polled or delivered as PLIC interrupts (source 11). `examples/pointer` draws a cursor that follows the mouse and changes colour while a button is held; Esc stops it:

```
cargo run --release -p grail -- --fb examples/pointer/pointer.elf
```

Programs embedding Grail as a library feed it pointer input with `Machine::pointer_event`.

### Disks

`--disk IMG` gives the machine a disk backed by the raw image file IMG (a whole number of 512-byte sectors; the program's writes change the file), `--disk-ro IMG` a read-only one. Repeat either for up to 8 disks. Each disk is an Xcal block device (spec section 9.6): a page of registers where software names a sector range and a buffer in RAM, then a command (read, write, flush); the device copies the data itself and signals completion in a status register and on its own PLIC source (3 for the first disk). Software finds the disks in the sysinfo device table. `examples/disk` prints sector 0 of a disk as text and runs a write/read-back test that leaves the image as it was:

```
cargo run --release -p grail -- --disk examples/disk/disk.img examples/disk/disk.elf
```

```
disk 0x10001000, irq 3: 8 sectors
sector 0:
Hello from sector 0 of disk.img!
This text is on the disk image, not in the program.
read past the end: error 2
write/read-back of sector 7: ok
```

As a library, disks are `MachineConfig::disks` (image files or in-memory disks); a host with its own storage implements the `BlockBackend` trait (capacity, read, write, flush) and passes it to `Machine::with_backends`.

The Camelot kernel mounts disk 0 as its root filesystem when it holds one (format in [docs/camelot-fs.md](docs/camelot-fs.md)). `cargo xtask fs` builds such an image, `target/camelot-fs.img`, with the user programs in `/bin`, and makes it bootable: the image holds the whole system (its size is in [spec §10](docs/excalibur-spec.md), "Root image"), so Grail needs no program to run it. As on Linux, the kernel on the disk (`kernel/kernel-disk.elf`) has no programs of its own and runs them from `/bin` (it panics if there is no `/bin/sh`); `kernel/kernel.elf`, the one Grail runs as a program, carries a copy of each, like an initramfs, so that it works without a disk. Started with a disk and no ELF, Grail's boot ROM loads the disk's boot block (its first 1 KiB, checked by a signature and a checksum) into RAM and runs it, as a PC's BIOS runs the master boot record; Camelot's bootloader (`kernel/boot/boot.s`) then loads the kernel, stored after the filesystem (spec section 7, "Boot from disk"; docs/camelot-fs.md, "Booting"). A disk that is not bootable stops the machine with a message from the boot ROM (`boot ROM: disk 0 is not bootable (no boot signature)`, exit code 253). The `camelot-fs` command builds, checks and reads images on the host:

```
cargo xtask fs
cargo run --release -p grail -- --disk target/camelot-fs.img                   # boots from the disk (in 4 MiB)
cargo run --release -p grail -- --disk target/camelot-fs.img kernel/kernel.elf # or runs the kernel given
cargo run --release -p camelot-fs -- mkfs disk.img some/dir --size 4M   # a new image holding a copy of some/dir
cargo run --release -p camelot-fs -- mkfs root.img --boot kernel/boot/boot.elf --kernel kernel/kernel-disk.elf   # bootable
cargo run --release -p camelot-fs -- fsck disk.img                      # every consistency rule (--recover replays the log)
cargo run --release -p camelot-fs -- ls disk.img /bin
cargo run --release -p camelot-fs -- cat disk.img /README
```

### Network

`--net hub` gives the machine an Ethernet card (spec section 9.8) on a port of an in-process hub; repeat it for up to 4 cards, each with its own MAC address (`02:ca:00:00:00:01` for port 1, or `--net hub,mac=ADDR`). The card moves raw Ethernet II frames between two rings of descriptors in guest RAM and its backend, interrupts on PLIC source 12 (13-15 for the other cards), and drops and counts frames that arrive while the guest has no free receive buffer. `--pcap FILE` captures every frame the card of the last `--net` sends or receives into FILE, in the classic pcap format that Wireshark and tcpdump read:

```
cargo run --release -p grail -- --net hub --pcap ping.pcap examples/net/net-ping.elf
```

`examples/net` is a pair: `net-pong` waits for a frame on its receive interrupt and answers it, `net-ping` broadcasts one and polls for the answer. A single `grail` process runs one machine, so the ping above goes unanswered (exit code 3) but lands in the capture; `crates/grail/tests/net.rs` runs both on one hub in one process:

```
net 0x10009000, irq 12, mac 02:ca:00:00:00:01, link up
sent 60 bytes to ff:ff:ff:ff:ff:ff
received 60 bytes from 02:ca:00:00:00:02, type 0x88b5: Hello back from pong!
```

As a library, cards are `MachineConfig::nets` (`NetConfig::hub(&hub)`: machines whose configs name the same `grail::net::Hub` share a network); a host with its own network implements the `NetBackend` trait (send a frame, poll for a received one, both non-blocking, `Send` so frames can cross threads) and passes it to `Machine::with_devices` with the card's MAC address. `grail::net::Pcap` wraps any backend in a capture.

`--net user` puts the card behind a NAT gateway instead (spec section 9.8, "User networking"; `crates/gateway`), the way QEMU's user networking does, with no driver or TAP device on the host: the guest gets 10.0.2.15 by DHCP, DNS at 10.0.2.3 (answered from the host's resolver), and TCP and UDP to the internet through ordinary host sockets; 10.0.2.2 is the host's 127.0.0.1. `hostfwd` rules forward host ports to the guest, so a browser on the host can open `http://127.0.0.1:8080` to reach a web server on the guest's port 80:

```
cargo run --release -p grail -- --net user,hostfwd=tcp::8080-:80 my-server.elf
```

`examples/net/net-user` gets its address by DHCP and takes commands on the UART: `c PORT` connects to the host's port PORT through 10.0.2.2, `l PORT` accepts a forwarded connection and echoes it, `q` exits. With something listening on the host's port 8000 (`crates/grail/tests/net_user.rs` does both directions):

```
printf 'c 8000\nq\n' | cargo run --release -p grail -- --net user examples/net/net-user.elf
```

Camelot itself has sockets and network programs ([docs/camelot-net.md](docs/camelot-net.md)): with the root image, `httpd &` serves its files on the host's port 8080, and `fetch http://10.0.2.2:8000/FILE > FILE` downloads from a server on the host (`crates/grail/tests/kernel_sockets.rs` tests both, and `crates/avalon/tests/sockets.rs` runs `nc` between two VMs on a switch):

```
cargo xtask fs
cargo run --release -p grail -- --disk target/camelot-fs.img --net user,hostfwd=tcp::8080-:80 kernel/kernel.elf
```

## The Avalon platform

Grail runs one machine for one terminal. The Avalon platform runs many, as long-lived virtual machines managed over the network, and is named after its parts:

| Program | Crate | What it does | Status |
|---|---|---|---|
| **Avalon** | `crates/avalon` | the hypervisor server: creates and runs N Grail VMs, each with its own hardware profile (RAM, clock, disks, network cards), keeps each VM's console and screen, and connects their cards with virtual Ethernet switches | done |
| **Round Table** | `crates/round-table` | the control CLI: create, list, start, pause, resume, stop and destroy VMs; create, list, capture and delete switches, give them NAT gateways and plug cards in and out; create, list and delete Lake's disk images | done |
| **Pendragon** | `crates/pendragon` | the web console: Round Table's VM, switch and disk commands, from a browser, plus live consoles and an overview of both servers | done (the screen view is still to come) |
| **Herald** | `crates/herald` | attaches to one VM: its console, or its screen, keyboard and mouse; or connects to a TCP port (`--tcp`), a shell of its own from `shd` | done |
| **Lake** | `crates/lake` | the storage server: disk images served as block devices over the network | done |

They talk the Avalon protocol ([`docs/avalon-protocol.md`](docs/avalon-protocol.md)): length-prefixed binary frames over TCP, implemented once in `crates/avalon-proto` and shared by every program.

`cargo xtask up` starts Lake, Avalon and Pendragon together (see [Quick start](#quick-start)); the sections below run each one by hand. `up` runs the release binaries in `target/release` as child processes, Lake first, Avalon next, Pendragon last, each once the one before it is listening, and stops them in the opposite order on Ctrl-C, or when one of them ends by itself. The servers are in a process group of their own, so Ctrl-C reaches `up` alone, and on Windows also in a job object, so they end with `up` however it ends. A server told to listen on every address (`--lake 0.0.0.0`) is reached on loopback: that is the address `up` gives Pendragon, uses for the demo and prints, since Windows cannot connect to `0.0.0.0`. With port 0 (`--lake 127.0.0.1:0`) a server picks its port and `up` reads which from its output, as its test does (`crates/xtask/tests/up.rs`: the real binaries on ephemeral ports and a temporary images directory, the demo's VM booted to the shell with a second one beside it, all stopped again).

### Running Avalon

```
cargo run --release -p avalon
```

```
avalon: listening on 127.0.0.1:7400
```

Options: `--listen ADDR` (default `127.0.0.1:7400`; a host or IP address alone means port 7400) and `--max-ram MiB`, the total RAM of all VMs (default 4096): a create that would go over it is refused, and destroying a VM gives its RAM back. The protocol has no authentication, so Avalon listens on loopback unless told otherwise, and warns when it is not: anyone who can reach the port can create, control and read VMs, and open files on the host as their disks. Ctrl-C stops every VM, closes the connections and exits.

A VM is created from a name, a hardware profile and a program (an Excalibur ELF, sent with the request), and starts in state *created*. Then:

```
created --start--> running --pause--> paused --resume--> running
created, running, paused --stop--> stopped        running --program halts--> exited(code)
any state --destroy--> gone (its name and RAM are free again; ids are never reused)
```

Each running VM has a thread of its own and runs in slices of Grail's `run_for`, so a pause or stop takes effect at once (when the request is answered, the machine runs no more steps), a real-time VM (`clock_hz` in its profile) keeps to its clock, and a full-speed VM parked in `wfi` waiting for input sleeps instead of spinning. Each VM's guest finds its own RAM size, clock and devices in its sysinfo block, as under `grail --ram --clock --disk`.

Every VM's UART is its console. Avalon keeps the last 64 KiB of output; a client that attaches gets that scrollback, then the output live, and its input goes to the UART. Several clients can attach to one VM: they all see the output, and input from any of them reaches the guest. Attached clients also hear about state changes (`VmStateChanged`) and the VM's destruction (`Detached`). A client that stops reading while a VM writes to it is disconnected once 8 MiB are waiting for it, so it cannot make the server hold unbounded output.

A VM's disks are image files on Avalon's host or images on a Lake server (`lake://host:port/name`, see [Running Lake](#running-lake)), in the order its profile lists them.

A VM's network cards (up to 4, Grail's card of section 9.8 of the spec) plug into Avalon's virtual Ethernet switches, which a client creates by name. A switch is a learning switch, like the box under a desk: it learns which port each source address is on, sends a frame to the one port its destination was seen on, and floods broadcasts and unknown destinations to every other port; learned addresses age out after 300 seconds. Switches are separate networks, so a VM on one never sees another's frames. A card's frames go through the switch on its own VM's thread without ever waiting: each port queues at most 256 frames, and the ones past that are dropped and counted on that port, so a paused or slow VM loses its own frames and holds up nobody. A card with no address in the profile gets one made from the VM's name (`02:..`, the same every time). Cards are plugged in and out while VMs run (the guest sees its link go up and down), a switch can be captured into a pcap file on Avalon's host, and a card is unplugged when its VM's machine ends. A switch can also have a NAT gateway that gives its VMs addresses by DHCP, the host and the internet, and forwards host ports to them (see Round Table below). Section 6.7 of the protocol document has the details.

Every bad request gets a protocol error, never a crash: an unknown VM, a bad name, a profile Grail cannot build (bad RAM size, zero clock, a missing disk image, a Lake disk that does not exist, is attached by another VM or is on a server that cannot be reached: `InvalidProfile`, with Lake's own error in the message), a program that is not a loadable ELF (checked before anything is allocated for it), a request in the wrong state, a create over the RAM budget, malformed or misdirected frames.

A client attached to a VM's screen (a viewer, such as `herald --screen`) gets its framebuffer: one full frame, then only the rectangles that changed, looked for at most every 33 ms and only while the VM runs, so an unchanged screen sends nothing; a viewer that falls more than 2 MiB behind gets no updates until it catches up, then one full frame. Keys and pointer changes from a viewer go to the VM's keyboard and pointer devices between slices of its run. The details are in section 6.4 of the protocol document; `avalon_proto::Screen` is the diffing on the server and the screen a viewer keeps.

Round Table (below) gives the orders from the command line and Herald (below) attaches a terminal to a console or a window to a screen. Both are built on `avalon-proto`, and so can any other client, in a few lines of Rust:

```rust
use avalon_proto::{Connection, HardwareProfile, Message, VmRef};

let mut avalon = Connection::connect("127.0.0.1:7400", "example")?;
let program = std::fs::read("examples/sysinfo/sysinfo.elf")?;
let profile = HardwareProfile { clock_hz: Some(2_000_000), ..HardwareProfile::with_ram(64 << 20) };
let Message::VmCreated { id } = avalon.request(&Message::CreateVm { name: "demo".into(), profile, program })? else { panic!() };
avalon.request_ok(&Message::StartVm { vm: VmRef::Id(id) })?;
let Message::ConsoleAttached { scrollback, .. } = avalon.request(&Message::ConsoleAttach { vm: VmRef::Id(id) })? else { panic!() };
print!("{}", String::from_utf8_lossy(&scrollback)); // then ConsoleOutput events from avalon.recv()
```

As a library, `avalon::Server` binds an address (port 0 for an ephemeral one), runs on a thread of its own with `spawn`, and stops with `ServerThread::stop`; the tests run it that way.

### Running Lake

```
cargo run --release -p lake -- --dir images
```

```
lake: listening on 127.0.0.1:7401
lake: images in images
```

Lake keeps a directory of disk images (created if it does not exist) and serves them over the Avalon protocol: disk management (`DiskCreate`, `ListDisks`, `DiskDelete`: what [Round Table](#round-table)'s `disk create|list|delete` send; a `DiskCreate` with a source image makes a copy of it) and block I/O (`BlockOpen`, `BlockRead`, `BlockWrite`, `BlockFlush`, `BlockSize`, `BlockClose`). Each disk is a raw image file in the directory named after the disk, matched by its exact file name (names follow the protocol's rules: lowercase, no paths, no Windows device names); other files there are left alone. New images are sparse files, marked so explicitly on Windows: they take no host space until written. Option `--listen ADDR` (default `127.0.0.1:7401`; a host or IP address alone means port 7401): like Avalon, Lake listens on loopback unless told otherwise, and warns when it is not. Ctrl-C ends it; every write it has answered is already in the image files.

An image has **one writer at a time**: attaching it read-write is exclusive, and any number of read-only attachments may share an image that has no writer. A second writer, a writer while there are readers, or deleting an attached image gets `Busy`; a missing image `NoSuchDisk`, a sector range past the end `OutOfRange`, a write through a read-only attachment `ReadOnly`. A connection's attachments end when it closes.

An image can be **cloned**: a new image that starts as a copy of an existing one (`DiskCreate` with a `source`, protocol version 7; Round Table's `disk create NAME --from SRC`). The source may have readers but not a writer, since a copy of a disk being written is not a disk, and Lake attaches it read-only itself while it copies, so no writer can start meanwhile. The copy is written under a name that is not a disk name and renamed into place when it is complete, so it never shows half made, and runs of zeros are not written, so the copy of a sparse image is sparse too. This is how every Camelot VM gets a root disk of its own (one writer per image): `round-table create --camelot` and Pendragon's Camelot VMs copy the image `os-base`, which `cargo xtask up` puts in Lake's directory, to `NAME-disk0`.

A VM uses a Lake disk through Grail's network backend (`grail::lake::LakeDisk`): its profile names `lake://host:port/name` (the protocol's `DiskSource::Lake`; `DiskSource::parse` reads the argument), and Grail's `--disk lake://host:port/name` does the same for a single machine. The disk attaches the image when the machine is built (so a busy or missing image fails the create) and detaches it when the machine is dropped (a stopped, exited or destroyed VM), waiting for Lake to confirm: once `DestroyVm` is answered, another VM can attach the image. Each read, write and flush of the guest's disk is a request to Lake (transfers over 1 MiB are split), answered before the guest's next instruction; a Lake error is `BLK_ERROR` 5 for the guest, and so is a server that went away or does not answer within 10 s (the disk then stays failed).

```rust
use avalon_proto::{Connection, DeviceSpec, DiskSource, DiskSpec, HardwareProfile, Message};

let mut lake = Connection::connect("127.0.0.1:7401", "example")?;
lake.request_ok(&Message::DiskCreate { name: "root".into(), sectors: 2048, source: None })?; // 1 MiB
let source = DiskSource::parse("lake://127.0.0.1:7401/root")?;
let profile = HardwareProfile {
    devices: vec![DeviceSpec::Disk(DiskSpec { source, read_only: false })],
    ..HardwareProfile::with_ram(64 << 20)
};
// CreateVm with this profile, as above: the VM's disk 0 is Lake's image "root".
```

### Round Table

`round-table` gives the orders: it creates VMs on an Avalon server and controls them, and manages the disk images of a Lake server. With Avalon running, build it with `cargo build --release -p round-table` (the binary is `target/release/round-table`, or run it through `cargo run --release -p round-table --`):

```
round-table create --name demo --ram 64 --clock 2MHz --start examples/sysinfo/sysinfo.elf
round-table create --name echo --ram 16 examples/uart-echo/uart-echo.elf
round-table start echo
round-table list
```

```
created VM 1 (demo)
started VM 1 (demo)
created VM 2 (echo)
started VM echo
ID  NAME  STATE      RAM     CLOCK       DISKS  NETS
1   demo  exited(0)  64 MiB  2 MHz       0      0
2   echo  running    16 MiB  full speed  0      0
```

`round-table info demo` shows one VM in detail, with the device table its guest sees:

```
id       1
name     demo
state    exited(0)
RAM      64 MiB
clock    2 MHz (real time)
steps    6461
disks    none
nets     none
devices  9
  test      0x00100000 + 0x00001000
  CLINT     0x02000000 + 0x00010000
  PLIC      0x0c000000 + 0x04000000
  UART      0x10000000 + 0x00000100  irq 1
  fb-ctrl   0x30000000 + 0x00001000
  keyboard  0x30001000 + 0x00001000  irq 2
  sysinfo   0x30002000 + 0x00001000
  pointer   0x30003000 + 0x00001000  irq 11
  fb-vram   0x31000000 + 0x0012c000
```

The life cycle is `start`, `pause`, `resume`, `stop` and `destroy`, each taking a VM's id or its name (`round-table pause echo`, `round-table destroy 2`). `create` takes `--name` (required), `--ram MiB`, `--clock FREQ` and `--harts N` with exactly the syntax of `grail --ram`, `--clock` and `--harts` (the same parsers), `--disk SRC` and `--disk-ro SRC` (an image file on Avalon's host, or `lake://ADDR/NAME` for an image on a Lake server; repeat for more disks), `--start`, and `--camelot` instead of a program (below). `--server ADDR` picks the Avalon for the VM commands (default `127.0.0.1:7400`; a host alone means port 7400). `round-table --help` lists everything.

The disk commands talk to Lake (`--lake ADDR`, default `127.0.0.1:7401`; the VM commands do not take it, but for `create --camelot` below, nor do the disk commands take `--server`):

```
round-table disk create data --size 64MiB
round-table disk list
round-table disk delete data
```

```
created disk data (64 MiB, 131072 sectors)
NAME  SIZE    SECTORS  ATTACHED
data  64 MiB  131072   no
deleted disk data
```

Sizes take `K`, `M`, `G` (or `KiB`, `MiB`, `GiB`), `s` for 512-byte sectors, or no unit for bytes. `disk create NAME --from SRC` makes the new image a copy of the image SRC, as big as it, or as `--size` if that is given (no smaller than SRC; zeros after its end); SRC may not have a writer:

```
round-table disk create backup --from data
```

```
created disk backup (a copy of data, 64 MiB, 131072 sectors)
```

**Camelot VMs.** `create --name NAME --camelot` creates a VM that boots Camelot with no program to send: Round Table first has Lake copy the image `os-base` (the bootable root image that [`cargo xtask up`](#quick-start) puts in Lake's directory) to `NAME-disk0`, then creates the VM with that copy as its first disk, `lake://ADDR/NAME-disk0`, where ADDR is the Lake of `--lake` (default `127.0.0.1:7401`; Avalon reaches the disk at the same address). The other `create` options go with it (`--ram`, `--net`, more `--disk`s after the copy, `--start`):

```
round-table create --name web --camelot --net nat --start
```

```
created disk web-disk0 (a copy of os-base)
created VM 1 (web)
started VM 1 (web)
```

A Lake without the image says so: `the Lake at 127.0.0.1:7401 has no 'os-base' image to copy (start the platform with 'cargo xtask up', which puts it in Lake's images directory)`. A copy whose name is taken is refused (`already exists`), and so is a copy of an image that a VM writes (`busy`); when Avalon refuses the VM (its name is taken, say), the copy is deleted again. Destroying the VM leaves its disk on Lake, with what Camelot wrote to it, until `disk delete NAME-disk0`. A disk is attached to a VM when the VM is created (`--disk lake://127.0.0.1:7401/data`; a missing image, one another VM writes, or a Lake server that cannot be reached is refused as an invalid hardware profile) and detached when the VM's machine ends: when the VM is stopped, its program exits, or it is destroyed. A stopped or exited VM still lists its disks (`list`, `info`), but no longer holds them. `disk list` shows each image's attachments (one writer, or any number of readers), and an attached image cannot be deleted.

The switch commands go to Avalon too. `create --net SWITCH[,mac=ADDR]` gives a VM a network card plugged into SWITCH (`-` for a card that starts unplugged; repeat for up to 4 cards), and `list` and `info` show each card's address and switch. Two VMs that ping each other over a switch, captured:

```
round-table switch create lan
round-table create --name pong --net lan --start examples/net/net-pong.elf
round-table switch capture lan lan.pcap
round-table create --name ping --clock 10MHz --net lan --start examples/net/net-ping.elf
round-table switch show lan
round-table switch capture lan --stop
```

```
created switch lan
created VM 1 (pong)
started VM 1 (pong)
capturing switch lan into lan.pcap
created VM 2 (ping)
started VM 2 (ping)
switch   lan
capture  lan.pcap
ports    none
```

Once both programs have exited, `switch show` lists no ports, since a card is unplugged when its machine ends; while they run it lists one port per card, with frames and bytes in and out and frames dropped. `switch attach SWITCH VM [--card N]` plugs a card in, `switch detach VM [--card N]` unplugs it, `switch list` shows every switch, and `switch delete NAME` deletes one, unplugging what is left on it.

A switch can have a NAT gateway: the same one as `grail --net user` (spec section 9.8), on a port of the switch, serving every VM plugged in. Each card gets its own DHCP lease (10.0.2.15, .16, ... in the order they ask), 10.0.2.2 is the host's 127.0.0.1, DNS is at 10.0.2.3, and TCP and UDP reach the internet through host sockets. `--hostfwd` forwards host ports to a guest, 10.0.2.15 unless the rule names another address; the host ports are bound on 127.0.0.1 unless the rule says otherwise, a port that is taken fails the command, and deleting the switch closes them:

```
round-table switch create wan --nat --hostfwd tcp::8080-:80 --hostfwd tcp::2222-10.0.2.16:22
round-table create --name web --net wan --start my-server.elf
round-table switch show wan
```

```
created switch wan with a NAT gateway
forwarding tcp:127.0.0.1:8080-10.0.2.15:80
forwarding tcp:127.0.0.1:2222-10.0.2.16:22
created VM 1 (web)
started VM 1 (web)
```

`switch show` then lists the gateway (its network, forwarded ports and the lease of each card) and its port with the cards'. `switch nat NAME [--hostfwd RULE]...` gives an existing switch a gateway, and `switch nat NAME --off` takes it away, closing its connections and ports.

Errors are the server's, on stderr, with exit status 1 (`round-table: invalid state: VM 1 ('demo') is exited(0), not created`); an unreachable server is also 1, and a mistake on the command line is 2.

The commands themselves live in `crates/avalon-client` (`Client`: `create_vm`, `list_vms`, `vm_info`, `vm_action`, `create_switch`, `create_switch_with`, `set_switch_nat`, `list_switches`, `attach_card`, `detach_card`, `capture_switch`, `delete_switch`, `create_disk`, `list_disks`, `delete_disk`, plus the parsers and formatters for addresses, sizes, clocks, disk sources, network cards and MAC addresses); Round Table is a thin command line over it, and the [Pendragon](#pendragon) web console is another.

### Pendragon

Pendragon is the platform's web console: one page in a browser for the VMs and switches of an Avalon server and the disk images of a Lake server. It is a client of both, like Round Table, and goes through the same `avalon-client` layer for every operation; Avalon and Lake stay headless, and Lake can be on another machine. With both running:

```
cargo run --release -p pendragon
```

```
http://127.0.0.1:7402/?token=3f9c...e1
pendragon: open the URL above in a browser; Ctrl-C stops Pendragon
pendragon: Avalon at 127.0.0.1:7400, Lake at 127.0.0.1:7401
```

Open the URL it prints (the token in it changes at every launch). Options: `--listen ADDR` (default `127.0.0.1:7402`), `--avalon ADDR` and `--lake ADDR` (the servers, defaults `127.0.0.1:7400` and `127.0.0.1:7401`), `--avalon-max-ram MiB` (Avalon's budget, shown on the overview; default 4096, Avalon's own default), `--allow-host NAME` (see below). The pages:

- **Overview** (`#/`): Avalon and Lake, each up or down with its version or the error that says why; the RAM all VMs hold against Avalon's budget, as a bar; how many VMs are running, paused, created and ended; how many disks there are, how many are attached, and their total size. It refreshes every few seconds, as the other pages do.
- **VMs** (`#/vms`): a table of every VM with its id, name, state, RAM, clock and disks, and a button for each action its state allows (start for a created VM; pause, stop, destroy for a running one; resume for a paused one; destroy for a stopped or exited one). Stop and destroy ask first.
- **New VM** (`#/vms/new`): name, RAM in MiB (4 by default), clock (`10MHz`, `500kHz`, `5Hz`, or empty for full speed: the syntax of `--ram` and `--clock`), harts (1 to 8), what it boots: *Camelot (boot from disk)*, the choice when Lake has the image `os-base`, gives the VM its own copy of that image, `NAME-disk0`, as its first disk and boots it, as `round-table create --camelot` does; *Program (ELF)* uploads an ELF file chosen in the browser, which may be left out when the VM has a disk to boot from instead; any number of disks (each a `lake://ADDR/NAME` source, offered from Lake's list, or the path of an image file on Avalon's host, read-write or read-only), network cards (each on a switch, offered from Avalon's list, or unplugged, with an address or one Avalon makes up), and whether to start it at once. Mistakes come back as Round Table would say them (`ram: 0 MiB is out of range`).
- **One VM** (`#/vms/ID`): its facts (state, RAM, clock, steps run, disks, the device table its guest sees), its action buttons, and its console: the scrollback, then the output live as the guest writes it, and an input line whose text goes to the VM's UART when you press Enter, followed by LF, CR or nothing as you choose (a Ctrl-D button sends EOT). Several pages, and Herald, can watch one console at once.
- **Disks** (`#/disks`): a form to create an image (name and size: `64MiB`, `512K`, `1G`, `2048s`), and a table of Lake's images with their size, how they are attached (read-write, or how many readers) and which VMs hold them, with three buttons: *attach* opens the new-VM form with the image filled in (a VM's disks are part of its hardware, attached when it is created), *detach* stops the VMs that hold it (a VM lets go of its disks when it stops, and cannot be started again: the page says so first), and *delete*, for an image nobody holds.
- **Switches** (`#/switches`): a form to create a switch (its name, whether it has a NAT gateway, and the ports it forwards, one rule per line as `--hostfwd` takes them), a form to plug a VM's card into a switch, and a panel per switch: its gateway (network, forwarded ports as bound, and each lease with the VM it belongs to) with a button to add or remove it, a delete button, and its ports with frames and bytes in and out, frames dropped, and an *unplug* button per card.

Pendragon listens on loopback, but other web pages in the same browser could still send it requests, so it checks the `Host` of every request (which stops DNS rebinding), wants the launch's token on every API request and an `Origin` of its own on every change, and serves its page with a Content Security Policy that allows no inline script; the page sets all data as text, never as markup. `--allow-host NAME` adds a name to the accepted hosts, for `--listen` on another interface. Everything is std only: the HTTP server, the JSON, and the page (HTML, JavaScript and CSS built into the binary, no build step). [`docs/pendragon.md`](docs/pendragon.md) has the security model, the HTTP limits and the JSON API, which works with `curl` too:

```
curl -H "X-Pendragon-Token: $TOKEN" http://127.0.0.1:7402/api/vms
```

### Herald

Herald attaches a terminal to a VM's serial console, like a cable to its UART. With a VM running on Avalon (the Camelot kernel, say):

```
cargo run --release -p herald -- [--server ADDR] <vm>
```

```
herald: attached to VM 1 'vm001' (running) on 127.0.0.1:7400; Ctrl-] detaches
...
$ ticks
ticks: 512
$
herald: detached from VM 1 'vm001', which carries on
```

`<vm>` is a VM id (all digits) or name; `--server` defaults to `127.0.0.1:7400` (a host alone means port 7400; a host name is tried at each of its addresses, so `localhost` finds an Avalon on 127.0.0.1 even where it resolves to `::1` first). Herald prints the VM's scrollback (the last 64 KiB of its output), then its output live, and sends every key to the VM as it is typed: the terminal is in raw mode while Herald runs, so the guest does the echoing and line editing, as on a serial line (Windows consoles and Linux terminals; elsewhere input goes line by line). On Windows the console also gets VT input and output (arrows and other special keys reach the guest as the escape sequences a Unix terminal sends, and the guest's escape sequences are carried out), and output that is not UTF-8 is shown with U+FFFD rather than failing. The terminal's modes are put back however Herald ends: detaching, an error, a panic or Ctrl-C. The raw mode is Grail's own (`grail::console::RawMode`, with `RawOptions { vt: true }`), hand-written FFI to the Win32 console and to termios.

**Ctrl-]** detaches: Herald exits and the VM carries on, whatever its state. Ctrl-C also leaves Herald (not the VM), at once and without detaching first; the server sees the connection close. Herald ends by itself, saying why on stderr, when the VM stops, exits or is destroyed, and says so when it is paused or resumed. When stdin is not a terminal, its bytes go to the VM as they are, except that a Ctrl-] byte among them still detaches (and drops what follows it), and its end detaches, so `herald demo < NUL` (`< /dev/null`) just prints the scrollback. Any number of Heralds may attach to one VM: all of them see its output, and what any of them types reaches it. Exit codes: 0 when detached or when the VM ended, 1 on an error (no server, no such VM, connection lost, console output failed, `--screen` without a window), 2 for bad arguments; Ctrl-C gives an interrupted program's status (130 on Linux, `STATUS_CONTROL_C_EXIT` on Windows).

**`--screen`** shows the VM's screen in a window instead (Windows; elsewhere Herald says it has no window and exits with 1). Run `examples/colorbars` or `examples/pointer` on Avalon and:

```
cargo run --release -p herald -- --screen bars
```

The window is Grail's own (`grail::window`), titled `Herald: VM 1 'bars'`, at the screen's size. The keys typed and the mouse moved, clicked or wheeled over it go to the VM's keyboard and pointer devices, as in Grail's window (the pointer's position in the screen's pixels; runs of moves are sent as the last one, a press or release where it happened). Closing the window detaches, the VM carrying on. When the VM stops, exits or is destroyed, the window stays on its last frame, its title saying why, until it is closed; a pause and a resume show in the title too.

**`--tcp HOST:PORT`** connects straight to a TCP port instead, not through Avalon: Windows has no `telnet` or `nc` by default, and a Camelot shell from `shd` (kernel/user/README.md) is one, each connection a shell of its own (`cargo xtask up --demo` forwards 127.0.0.1:2323 to its VM's). A socket has no line discipline at the far end, so on a terminal Herald edits and echoes the line itself, as a telnet client in line mode does: printable characters go into the line, Backspace erases one, Ctrl-U the line, Ctrl-W a word, arrow keys and other escape sequences are ignored, and Enter sends the line with a newline. Ctrl-D on an empty line ends the input (Herald shuts down its sending side, which `shd` takes as a hang-up), and Ctrl-] leaves, closing the connection; Herald also ends when the far end closes. Piped input goes as it is, a Ctrl-] in it still leaving, and its end shuts the sending side down, Herald staying until the far end closes, so `herald --tcp 127.0.0.1:2323 < script` prints all the script's output (end the script with `exit`). `herald::tcp::TcpSession` is the streaming layer and `herald::tcp::LineEditor` the editor, both without a terminal; `crates/herald/tests/tcp.rs` runs them against small servers on loopback, and the binary through pipes.

As a library, `herald::Session::attach(connection, &vm)` attaches and `Session::run(input, output, notices)` streams: keys from a channel of bytes, the console to any `Write`. `herald::screen::ScreenSession::attach(connection, &vm)` and `ScreenSession::run(view)` do the same for the screen, with any `View`: something that can draw an `avalon_proto::Screen` and hand over input (`herald::window::WindowView` is the window). Neither knows of terminals or windows, which is how they are tested.

## Testing

```
cargo test --release
```

The whole suite, run locally before a pull request. GitHub's CI runs `cargo xtask ci`, the important tests that take minutes (`crates/xtask/src/ci.rs` lists them), and leaves the long integration tests to the local run. The whole suite covers the following.

Runs the decoder and device unit tests, every riscv-tests suite (except `rv32ui-p-ma_data`, which assumes hardware misaligned access; Excalibur traps instead), the examples (the UART echo is fed both from an in-process channel and through the `grail` binary's stdin), the kernel, the merlin compiler (every test program built with merlin, merlin-as and merlin-ld and run in Grail), merlin-ld against ld.lld (the loaded bytes of clang-built fixtures and of every riscv-tests program relinked from its object must be identical), and merlin-objdump against committed llvm-objdump output (`crates/merlin-objdump/tests/golden`; regenerate the fixtures and goldens with `scripts/gen-objdump-golden.sh`, which needs clang and llvm-objdump). The framebuffer test runs `examples/colorbars` headless and checks the dumped frames pixel by pixel and against a committed hash. The keyboard tests inject events through `Machine::key_event` and run both `examples/kbd-echo` programs; the pointer tests inject input through `Machine::pointer_event` and check `examples/pointer`'s cursor pixel by pixel. The window itself needs a desktop, so its tests (drawing, and key and mouse messages turned into events) are ignored by default: `cargo test --release -p grail -- --ignored window`. `crates/grail/tests/machine.rs` runs `examples/sysinfo` on machines with different RAM and clocks (two at once, on separate threads) and checks real-time pacing with lower bounds on elapsed time and generous upper ones. `crates/grail/tests/harts.rs` checks multi-hart machines: the boot ROM's park loop and IPI wake-up, the CLINT's and PLIC's per-hart registers, the turn order, a hart in `wfi` that holds no other up, and LR/SC reservations broken by other harts' stores; `crates/xtask/tests/examples.rs` runs `examples/smp` on 1 to 8 harts. `crates/grail/tests/atomics.rs` checks what riscv-tests rv32ua does not: what breaks an LR/SC reservation, misaligned and device addresses, and Sv32 permissions and A/D bits for atomics. `crates/grail/tests/block.rs` runs `examples/disk` on in-memory disks, temporary copies of `examples/disk/disk.img`, host-made backends (one that fails every request) and through `grail --disk`, and checks the committed image against the bytes it builds (`GRAIL_WRITE_DISK_IMG=1` rewrites it). `crates/grail/tests/net.rs` runs `examples/net`'s ping and pong on two machines on one hub (taking turns on one thread, and on two threads), drops a frame that arrives before pong has a buffer, checks a capture of the exchange byte for byte against `crates/grail/tests/golden/net-ping.pcap` (`GRAIL_WRITE_PCAP=1` rewrites it; the time stamps come from a fixed clock), and runs `grail --net --pcap`; the card's unit tests cover its registers, ring wraparound (of the slots and of the 32-bit counters), bad descriptors and DMA addresses (an error status, never a panic), dropped frames and the interrupt. `crates/grail/tests/net_user.rs` runs `examples/net/net-user` behind the NAT gateway with no internet: it gets its address by DHCP, connects through 10.0.2.2 to a listener of the test on 127.0.0.1 and echoes a connection the test makes to a `hostfwd` port, in process and through `grail --net user`; the gateway's unit tests (`crates/gateway`) drive it with crafted frames: ARP, DHCP, ICMP, DNS (with a made-up resolver), UDP and TCP through NAT to sockets on 127.0.0.1, forwarded ports, retransmission and window handling with made-up clocks, the limits, and every truncation and corrupted byte of good frames (dropped, never a panic). The kernel's user-space shell is driven through the `grail` binary's stdin and in-process (line editing, end of input, exit statuses, more input than the UART's ring buffer holds, no UART reads while it idles); it runs user programs that must take turns on timer ticks with every register intact, faulting ones that must be killed while the kernel carries on, programs that pass the system calls bad pointers (refused with `-EFAULT`) and one that checks every call; the system call document's tables are checked against the kernel's and the user library's numbers; with in-memory disks (and image files through `grail --disk`) the kernel's disk self-test must round-trip a block through the buffer cache, one interrupt-completed request at a time with no polling, sleeping in `wfi` while a held-back interrupt is outstanding, and leave the disk unchanged; `crates/xtask/tests/fs.rs` boots it on images built by mkfs, where its listing of `/` must be the host's, the files its transactions write must read back on the host and pass fsck, every commit must follow the log's protocol, and a crash after any sector written (the machine stopped there, every such disk checked, and the kernel rebooted on each one its recovery has work on) must leave a disk that fsck finds consistent once replayed, holding each transaction whole or not at all, which the kernel's own replay turns into exactly the host's (`crates/camelot-fs` tests mkfs round trips, the `camelot-fs` command and every kind of damage fsck must find); and on Windows `crates/grail/tests/console.rs` types into a real console (the one `cargo test` runs in, or a new one) to check raw mode end to end.

The debugger's tests:

- `crates/grail/tests/debug.rs` checks Grail's debug interface on hello, smp and the kernel: breakpoints, steps and watchpoints stop where they should, and multi-hart machines stop between harts' turns. Every debugged run is compared turn by turn with the undebugged one.
- The GDB stub's unit tests (`crates/grail/src/gdb`) cover packet framing, checksums and escapes, every supported command, and malformed or unsupported packets.
- `crates/grail/tests/gdb.rs` talks to `grail --gdb` over TCP, interrupting and killing a running kernel.
- `crates/merlin-dbg/tests/session.rs` drives the merlin-dbg binary on hello, smp (2 harts) and the kernel, by source line in the kernel and in the shell. It checks that each program's console output and exit code are those of an undebugged run. An ignored test has LLDB read the kernel's line tables.
- The line tables: `crates/merlin-as/tests/debug_line.rs` compares merlin-as's debug sections with clang's on a corpus (`scripts/gen-debug-line-golden.sh`), `crates/merlin/tests/debug.rs` checks that `-g` changes no code, `crates/merlin-ld/tests/link.rs` that they are linked at the code's addresses and strip to the link without them, and `crates/xtask/tests/debug_info.rs` that the committed kernels and the root image load the same bytes with and without `-g`.
- An ignored test runs LLDB against the stub: `cargo test --release -p grail --test gdb -- --ignored`. It needs LLDB and, on Windows, a Python runtime on the PATH (see docs/merlin-dbg.md).

`crates/avalon/tests` run an Avalon server in process on an ephemeral port and drive it through the protocol: two VMs with different RAM and clocks run `examples/sysinfo` and each prints its own profile (streamed live, and again from the scrollback), with their exit states and device tables; a looping guest (assembled in the test with merlin-as and merlin-ld) is paused (no more steps), resumed, stopped and destroyed, its clients get every state change and `Detached`, and its RAM and name are free again; `examples/uart-echo` takes input from several attached clients at full speed and in real time, and pauses with input waiting; a parked full-speed VM sleeps, and a real-time one keeps to its clock, its thread sleeping between batches (counted through a test-only hook on `ServerThread`); a guest that has presented a frame still runs full slices; a client that stops reading a chatty console is cut off while the VM and the other clients carry on; a client gets a limited time for its whole `Hello`, however slowly it trickles in; a create whose disk is slow to open (a FIFO, on Linux) holds up no other request; bad requests of every kind (unknown VMs, invalid names, profiles and programs, the RAM budget, wrong states, unserved groups, malformed and misdirected frames, oversize frames, bad handshakes) get the right error code and leave the server running; screens are streamed (`tests/screen.rs`: `examples/colorbars` decoded from its frames pixel for pixel and compared with Grail's framebuffer, for a viewer from the start and one that attaches late; an unchanged screen sends no frames while keys and pointer moves reach `examples/pointer`; a screen that changes all the time is sent at most every 33 ms, and nothing while paused once the last pixels drawn have gone out; a viewer that stops reading is held back, not cut off, and once it reads again (here while the VM is paused) catches up with one full frame that gives it the screen of a viewer that kept up; a stopped VM sends its last frame before the state change; a display that is off streams as black; input from a client that is not a viewer is dropped, input waits for a created or paused VM up to 256 keys and 256 pointer changes, and none is taken once the VM has ended; one connection views and attaches to the console of the same VM); the client example above is checked against the test that runs it; and shutdown stops the VMs and closes every connection. `tests/cli.rs` runs the `avalon` binary and shuts it down with Ctrl-Break (Windows) or SIGTERM (Unix). `tests/net.rs` runs `examples/net`'s ping and pong on two VMs on one switch (with a third VM on another switch that must see nothing, and a capture of the exchange), floods a VM that never runs until its port drops frames while the sender carries on, plugs cards in and out of running VMs, keeps generated addresses stable across re-creation, and checks every error the switch messages can give. The switch's unit tests (`src/switch/tests.rs`) cover MAC learning, flooding, filtering, ageing (with time passed in, not waited for), the table's size limit, full queues, address clashes, deleted switches and captures that fall behind.

`crates/lake/tests` run Lake in process on an ephemeral port, serving a temporary directory: disk management (names, sizes, listing order, images that outlive the server, files that are not disks left alone, even where the filesystem takes their names for disk names), block I/O up to the 1 MiB request limit, the one-writer-or-many-readers rule across connections, attachments released when a connection ends, the per-connection limit on attachments, and bad requests of every kind (unknown handles, out-of-range sectors, writes through read-only attachments, unserved groups, malformed and misdirected frames, oversize frames, bad handshakes), each with its error code and the server carrying on. `tests/grail.rs` runs `examples/disk` on a Lake disk (read-write, then read-only on two machines at once), splits big transfers into requests, checks that dropping a machine releases its image at once (with an explicit `BlockClose`, answered before the drop returns), and that a Lake server that goes away, answers nonsense or answers too late is `BLK_ERROR` 5 for the guest, not a hang or a panic (and a late answer is never taken for the next request's). `tests/avalon.rs` is the platform end to end, Lake and Avalon both in process: a VM writes to a Lake disk and is destroyed while still running; a new VM attaches the same image at once and reads the data back; meanwhile no second writer gets in, and three read-only VMs share the image. `tests/cli.rs` runs the `lake` binary.

`crates/round-table/tests` drive Round Table's command layer against an Avalon server running in process: VMs are created, listed and inspected (checking the exact tables), taken through every state transition by name and by id (each checked over a separate protocol connection, and a paused VM runs no more steps), destroyed and their names reused; disks from image files are attached at create time and `examples/disk` runs on them, and the image files are open exactly until the VM is stopped, exits or is destroyed; refusals (a name taken, the RAM budget, a program that is not an ELF, a Lake disk on a server nobody runs, an unknown VM, the wrong state, an unreachable server, a VM created that then fails to start) come back as the server's error. `tests/switch.rs` creates switches, runs ping and pong on one with a capture, and plugs cards in and out, checking the exact output and every refusal. The disk commands run against a stand-in Lake in the test that speaks the protocol's disk-management messages, and `tests/lake.rs` runs them against the real Lake server in process, with a VM created on a Lake disk holding it until it is destroyed. `tests/cli.rs` runs the `round-table` binary for its usage text, messages and exit statuses; the `avalon-client` library is also driven directly, as Pendragon uses it.

`crates/pendragon` tests its HTTP parser against malformed, oversize and slow requests, its JSON and base64 against every corner of their grammars, its random source and its guard (hosts, tokens, origins). `tests/api.rs` runs Avalon and Lake in process and drives every page operation over HTTP, as the browser does: disks created, listed and deleted; a VM created on a Lake disk with an uploaded program, inspected, paused and resumed, its console read (long polls that return as soon as there is output) and typed into; the disk shown held by it, refused for deletion and for a second writer, then detached; read-only images shared; a console that stays attached however long the guest is silent; refusals with their protocol codes; servers that are away. `tests/security.rs` checks the page's security headers and script, wrong `Host`s (DNS rebinding), missing and wrong tokens, foreign origins and form bodies, escaped output, and that malformed, oversize and slow requests are refused while Pendragon carries on. `tests/cli.rs` runs the `pendragon` binary and uses the URL it prints.

`crates/herald/tests` drive Herald against an in-process Avalon server. `session.rs` runs the streaming layer with no terminal (keys from a channel, the console into a buffer): it attaches to `examples/uart-echo` and to the kernel's shell, types, detaches (the VM carries on), attaches again and finds the earlier session in the scrollback; several sessions share one console; a pause and a resume are reported; stopping, exiting and destroying the VM end the session, and a VM that has already exited shows its scrollback and ends it at once; a long paste arrives whole and in order. `fake_server.rs` scripts a server for the rest: output queued while attaching, events for other VMs, events that do not decode and frames of unknown types, a detach that is never confirmed, confirmed by a reply that does not decode, met by a frame out of place or by a hang-up, keys typed while it waits, a reply out of turn, a lost connection. `cli.rs` runs the `herald` binary through pipes, and `terminal.rs` on a terminal of its own making, a pseudoconsole (ConPTY) on Windows and a pseudoterminal on Linux: raw mode is on while Herald is attached, keys typed on the terminal reach the VM, and the terminal's modes are back as they were after a detach, after an error (the server hanging up) and after Ctrl-C. `screen.rs` runs the screen layer with a view that only records: `examples/colorbars` reaches it pixel for pixel as Grail draws it; keys and mouse moves reach `examples/pointer` (its cursor moves, and Esc ends it), and a screen that has not changed is not redrawn; closing the view detaches, a pause and a destroy reach it, and a frame outside the screen or a pixel format Herald cannot show (from a scripted server) is an error. `window.rs` opens a real window and posts mouse and key messages to it (ignored by default, like Grail's window tests: `cargo test --release -p herald -- --ignored`).

`crates/avalon-proto` round-trips every Avalon protocol message through `encode`/`decode` and through a `Connection` over memory and TCP, checks a few layouts byte for byte against [`docs/avalon-protocol.md`](docs/avalon-protocol.md), rejects truncated, oversize and invalid frames, tests the version handshake and the screen diffing (a changed screen applied frame by frame comes out equal), and feeds the decoder random and mutated bytes from a fixed-seed generator (anything that decodes must re-encode to the same bytes). It also checks that its hardware profile still mirrors Grail's `MachineConfig`.

`merlin file.c -o file.s` compiles a C-subset program; `merlin --dump-ast file.c` prints the checked syntax tree. The compiler's end-to-end harness (`crates/merlin/tests/harness.rs`) runs every program in `crates/merlin/tests/cases/` through merlin, merlin-as and merlin-ld and checks its exit code in Grail, and a reference interpreter of the checked syntax tree (`tests/common/interp.rs`) must agree. The generated assembly of a few cases is committed in `crates/merlin/tests/snapshots/*.s` (`MERLIN_BLESS=1 cargo test -p merlin --test cli` regenerates them after an intended codegen change). With clang and ld.lld installed, `cargo test -p merlin --test harness -- --ignored` also checks the programs' expected exit codes against clang and that clang assembles merlin's output to the same code, data and relocations as merlin-as; `MERLIN_HARNESS_LLVM=1` builds merlin's output with clang and ld.lld instead of our tools. `crates/merlin/tests/abi.rs` checks the calling convention against clang: merlin-compiled code and a committed clang-compiled object (`tests/abi/clang_side.o`) pass structs of every size to each other, linked by merlin-ld and run in Grail (`MERLIN_BLESS=1 cargo test -p merlin --test abi -- --ignored clang_side` regenerates the object with clang).

`crates/xtask/tests/examples.rs` builds every example from source with merlin-as and merlin-ld, checks the committed `.elf` is exactly what comes out, and runs each in Grail with its input (UART bytes for `uart-echo`, key events for `kbd-echo`, pointer input for `pointer`, `examples/disk/disk.img` for `disk`), checking the output and the exit code. It also compares `hello` and `multiobj` with clang + ld.lld's build of the same sources (committed in `crates/merlin-ld/tests/fixtures`): same loaded bytes, sections, symbols and disassembly. None of this needs LLVM.

The test ELFs are committed. To rebuild them you need clang + lld (the official Windows LLVM installer works) and a riscv-tests checkout: `scripts/build-riscv-tests.sh path/to/riscv-tests tests/riscv-tests` (add suite names, such as `rv32ua`, to rebuild only those). The examples are rebuilt with `cargo xtask examples` (our tools only); clang is just a reference for them, with the command at the top of each source (`-include examples/xcal.inc` teaches clang the Xcal mnemonics). `cargo xtask hello` assembles `examples/hello/hello.S` with merlin-as and links it with ld.lld into `crates/merlin-as/tests/golden/hello.elf`, which `cargo test` runs in Grail (and, with ld.lld installed, checks is current). The kernel is rebuilt with `cargo xtask kernel` (merlin, merlin-as and merlin-ld, no LLVM; see [kernel/README.md](kernel/README.md)), which first builds the user programs the same way and embeds them as data (through a `userprogs.s` generated at build time, not committed); `cargo test` rebuilds it and checks the committed `kernel.elf` and `kernel-disk.elf` are current. `cargo xtask kernel --cc clang` is the clang `-O2` + ld.lld reference build (in `target/kernel/clang`), and `cargo test -p xtask --test kernel -- --ignored` checks that it behaves like ours and prints how much bigger and slower our code is. merlin-ld's test fixtures (clang-built objects, among them `examples/hello` and `examples/multiobj`, plus ld.lld's link of them with its `llvm-readelf` and `llvm-objdump -d` listings as the expected output) are rebuilt with `cargo xtask ld-fixtures`; the riscv-tests objects merlin-ld relinks come from `scripts/build-riscv-tests.sh`.
