# Camelot

A learning project: design an instruction set, emulate it, build the toolchain for it, and write an operating system that runs on it. Everything builds and runs natively on Windows (and Linux/macOS) with nothing but Rust's standard library.

| Piece | What it is | Status |
|---|---|---|
| **Excalibur** | The ISA: RV32IMA + Zicsr, M/S/U privilege, Sv32 paging, plus the Xcal extensions | Spec v0 in [`docs/excalibur-spec.md`](docs/excalibur-spec.md) |
| **Grail** | The emulator | Passes riscv-tests rv32ui/um/ua/mi/si; CLINT, PLIC, UART with receive interrupts; Xcal framebuffer (Win32 window or PPM dumps), keyboard and pointer (from the window, with PLIC interrupts), sysinfo and block devices (disk images, `--disk`); configurable RAM, real-time clock and disks |
| **Merlin** | The toolchain: `merlin-as`, `merlin-ld`, `merlin-objdump`, and the `merlin` C-subset compiler | Builds every example on its own, no LLVM (`cargo xtask examples`). merlin-as: complete; its objects match clang's ([encoder notes](crates/merlin-as/ENCODER.md)). merlin-ld: links clang objects to the same bytes as ld.lld except where ld.lld merges `SHF_MERGE` strings and constants (see [spec §10](docs/excalibur-spec.md)), no relaxation (relocations explained in [src/reloc.rs](crates/merlin-ld/src/reloc.rs)). merlin: a C99 subset (integer types, pointers, arrays, globals, strings; structs and the preprocessor next) compiles to assembly that our own assembler and linker build into programs Grail runs ([code generation notes](crates/merlin/src/codegen.rs)). merlin-objdump: matches llvm-objdump |
| **Camelot** | The kernel / OS | Boots on Grail in S-mode: `kprintf`, trap handling, timer ticks, page allocator and Sv32 paging, interrupt-driven UART console with a tiny shell, user programs in U-mode with their own page tables, switched round robin on timer ticks ([kernel/](kernel/README.md)) |
| **Avalon** | The platform around Grail: the Avalon hypervisor server, the Round Table control CLI, the Herald VM client and the Lake storage server | Protocol v1 in [`docs/avalon-protocol.md`](docs/avalon-protocol.md), implemented by `crates/avalon-proto`. The Avalon server runs N VMs with their own hardware profiles, a RAM budget and attachable consoles; Herald attaches a terminal to a VM's console; Round Table and Lake come next |

## Layout

```
crates/excalibur        instruction encodings, decoder, disassembler (shared by every tool)
crates/elf32            ELF32 reader (shared by grail, merlin-objdump, merlin-ld)
crates/grail            the emulator
crates/avalon-proto     the Avalon protocol: frames, messages, blocking connection (docs/avalon-protocol.md)
crates/avalon           the Avalon hypervisor server: runs N Grail VMs, serves the protocol
crates/herald           Herald, the Avalon client that attaches a terminal to one VM's console
crates/merlin           the C-subset compiler (C → .s), with its test programs in tests/cases
crates/merlin-as        the assembler (source → ELF32 .o)
crates/merlin-ld        the linker (.o files → ELF32 executable)
crates/merlin-objdump   the disassembler / object dumper (llvm-objdump compatible output)
crates/xtask            build tasks: `cargo xtask examples`, `cargo xtask kernel`, `cargo xtask hello`, `cargo xtask ld-fixtures`
kernel/                 the Camelot kernel (C + assembly) and its prebuilt kernel.elf; kernel/user holds the user programs it embeds
examples/               bare-metal programs, built with merlin-as + merlin-ld (the .elf files are committed)
tests/riscv-tests       prebuilt riscv-tests ELFs and their objects for merlin-ld (rebuild with scripts/build-riscv-tests.sh)
scripts/                test-data generators (riscv-tests, merlin-as golden encodings, merlin-objdump goldens)
docs/                   the ISA spec and the Avalon protocol
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

The kernel runs the same way: `cargo run --release -p grail -- kernel/kernel.elf`. It boots into a small shell (`help`, `ticks`, `run`, `halt`; `run ping pong` runs two user programs side by side); when stdin is a terminal, Grail switches it to raw mode so each key reaches the guest as it is typed and the guest does the echoing (Windows consoles and Linux terminals; restored on exit and on Ctrl-C).

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

Options: `--trace` prints every instruction with its disassembly, `--max-steps N` stops a runaway program, `--ram MiB` sets RAM size (1 to 2048), `--clock FREQ` runs in real time at FREQ instructions per second (`10MHz`, `500kHz`, `1.5MHz`, `5Hz`; without it Grail runs at full speed and deterministically, reporting a nominal 10 MHz), `--disk IMG` attaches a disk image (repeat for up to 8 disks; `--disk-ro IMG` attaches one read-only). Grail's exit code is the program's (`xcal.halt`, the test device, or riscv-tests' `tohost`).

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

### Framebuffer

Programs can draw into a 640×480 framebuffer (spec section 9.3). Grail is headless by default; `--fb` opens a native window showing it (Windows only, plain Win32 GDI through hand-written FFI; elsewhere Grail warns and stays headless). `--fb-dump DIR` writes `DIR/frame-NNNN.ppm` on every `FB_PRESENT` and when the machine stops, with or without a window. `--headless` turns the window off even if `--fb` is given.

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

## The Avalon platform

Grail runs one machine for one terminal. The Avalon platform runs many, as long-lived virtual machines managed over the network, and is named after its parts:

| Program | Crate | What it does | Status |
|---|---|---|---|
| **Avalon** | `crates/avalon` | the hypervisor server: creates and runs N Grail VMs, each with its own hardware profile (RAM, clock, disks), and keeps each VM's console | done |
| **Round Table** | `crates/round-table` | the control CLI: create, list, start, pause, resume, stop and destroy VMs | next |
| **Herald** | `crates/herald` | attaches to one VM: its console (and later its screen and keyboard) | console done |
| **Lake** | `crates/lake` | the storage server: disk images served as block devices over the network | next |

They talk the Avalon protocol ([`docs/avalon-protocol.md`](docs/avalon-protocol.md)): length-prefixed binary frames over TCP, implemented once in `crates/avalon-proto` and shared by every program.

### Running Avalon

```
cargo run --release -p avalon
```

```
avalon: listening on 127.0.0.1:7400
```

Options: `--listen ADDR` (default `127.0.0.1:7400`; a host or IP address alone means port 7400) and `--max-ram MiB`, the total RAM of all VMs (default 4096): a create that would go over it is refused, and destroying a VM gives its RAM back. Protocol v1 has no authentication, so Avalon listens on loopback unless told otherwise, and warns when it is not: anyone who can reach the port can create, control and read VMs, and open files on the host as their disks. Ctrl-C stops every VM, closes the connections and exits.

A VM is created from a name, a hardware profile and a program (an Excalibur ELF, sent with the request), and starts in state *created*. Then:

```
created --start--> running --pause--> paused --resume--> running
created, running, paused --stop--> stopped        running --program halts--> exited(code)
any state --destroy--> gone (its name and RAM are free again; ids are never reused)
```

Each running VM has a thread of its own and runs in slices of Grail's `run_for`, so a pause or stop takes effect at once (when the request is answered, the machine runs no more steps), a real-time VM (`clock_hz` in its profile) keeps to its clock, and a full-speed VM parked in `wfi` waiting for input sleeps instead of spinning. Each VM's guest finds its own RAM size, clock and devices in its sysinfo block, as under `grail --ram --clock --disk`.

Every VM's UART is its console. Avalon keeps the last 64 KiB of output; a client that attaches gets that scrollback, then the output live, and its input goes to the UART. Several clients can attach to one VM: they all see the output, and input from any of them reaches the guest. Attached clients also hear about state changes (`VmStateChanged`) and the VM's destruction (`Detached`). A client that stops reading while a VM writes to it is disconnected once 8 MiB are waiting for it, so it cannot make the server hold unbounded output.

Every bad request gets a protocol error, never a crash: an unknown VM, a bad name, a profile Grail cannot build (bad RAM size, zero clock, a missing disk image), a program that is not a loadable ELF (checked before anything is allocated for it), a request in the wrong state, a create over the RAM budget, malformed or misdirected frames. Screen and keyboard streaming (Herald's `--screen`) and disks on a Lake server are not served yet: their requests get `Unsupported` and `InvalidProfile`.

Round Table and Herald are built on `avalon-proto`, and so can any other client, in a few lines of Rust:

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

### Herald

Herald attaches a terminal to a VM's serial console, like a cable to its UART. With a VM running on Avalon (the Camelot kernel, say):

```
cargo run --release -p herald -- [--server ADDR] <vm>
```

```
herald: attached to VM 1 'camelot' (running) on 127.0.0.1:7400; Ctrl-] detaches
...
camelot> ticks
ticks: 512
camelot>
herald: detached from VM 1 'camelot', which carries on
```

`<vm>` is a VM id (all digits) or name; `--server` defaults to `127.0.0.1:7400` (a host alone means port 7400). Herald prints the VM's scrollback (the last 64 KiB of its output), then its output live, and sends every key to the VM as it is typed: the terminal is in raw mode while Herald runs, so the guest does the echoing and line editing, as on a serial line (Windows consoles and Linux terminals; elsewhere input goes line by line). On Windows the console also gets VT input and output (arrows and other special keys reach the guest as the escape sequences a Unix terminal sends, and the guest's escape sequences are carried out), and output that is not UTF-8 is shown with U+FFFD rather than failing. The terminal's modes are put back however Herald ends: detaching, an error, a panic or Ctrl-C. The raw mode is Grail's own (`grail::console::RawMode`, with `RawOptions { vt: true }`), hand-written FFI to the Win32 console and to termios.

**Ctrl-]** detaches: Herald exits and the VM carries on, whatever its state. Ctrl-C also leaves Herald (not the VM). Herald ends by itself, saying why on stderr, when the VM stops, exits or is destroyed, and says so when it is paused or resumed. When stdin is not a terminal, its bytes go to the VM as they are (Ctrl-] still detaches) and its end detaches, so `herald demo < NUL` (`< /dev/null`) just prints the scrollback. Any number of Heralds may attach to one VM: all of them see its output, and what any of them types reaches it. Exit codes: 0 when detached or when the VM ended, 1 on an error (no server, no such VM, connection lost), 2 for bad arguments.

As a library, `herald::Session::attach(connection, &vm)` attaches and `Session::run(input, output, notices)` streams: keys from a channel of bytes, the console to any `Write`. It knows nothing of terminals, which is how it is tested; the screen and keyboard will join it on the same connection.

## Testing

```
cargo test
```

Runs the decoder and device unit tests, every riscv-tests suite (except `rv32ui-p-ma_data`, which assumes hardware misaligned access; Excalibur traps instead), the examples (the UART echo is fed both from an in-process channel and through the `grail` binary's stdin), the kernel, the merlin compiler (every test program built with merlin, merlin-as and merlin-ld and run in Grail), merlin-ld against ld.lld (the loaded bytes of clang-built fixtures and of every riscv-tests program relinked from its object must be identical), and merlin-objdump against committed llvm-objdump output (`crates/merlin-objdump/tests/golden`; regenerate the fixtures and goldens with `scripts/gen-objdump-golden.sh`, which needs clang and llvm-objdump). The framebuffer test runs `examples/colorbars` headless and checks the dumped frames pixel by pixel and against a committed hash. The keyboard tests inject events through `Machine::key_event` and run both `examples/kbd-echo` programs; the pointer tests inject input through `Machine::pointer_event` and check `examples/pointer`'s cursor pixel by pixel. The window itself needs a desktop, so its tests (drawing, and key and mouse messages turned into events) are ignored by default: `cargo test --release -p grail -- --ignored window`. `crates/grail/tests/machine.rs` runs `examples/sysinfo` on machines with different RAM and clocks (two at once, on separate threads) and checks real-time pacing with lower bounds on elapsed time and generous upper ones. `crates/grail/tests/atomics.rs` checks what riscv-tests rv32ua does not: what breaks an LR/SC reservation, misaligned and device addresses, and Sv32 permissions and A/D bits for atomics. `crates/grail/tests/block.rs` runs `examples/disk` on in-memory disks, temporary copies of `examples/disk/disk.img`, host-made backends (one that fails every request) and through `grail --disk`, and checks the committed image against the bytes it builds (`GRAIL_WRITE_DISK_IMG=1` rewrites it). The kernel's shell is driven through the `grail` binary's stdin and in-process (line editing, more input than its ring buffer holds, no UART reads while it idles); its `run` command runs user programs that must take turns on timer ticks with every register intact, and faulting ones that must be killed while the kernel carries on; and on Windows `crates/grail/tests/console.rs` types into a real console (the one `cargo test` runs in, or a new one) to check raw mode end to end.

`crates/avalon/tests` run an Avalon server in process on an ephemeral port and drive it through the protocol: two VMs with different RAM and clocks run `examples/sysinfo` and each prints its own profile (streamed live, and again from the scrollback), with their exit states and device tables; a looping guest (assembled in the test with merlin-as and merlin-ld) is paused (no more steps), resumed, stopped and destroyed, its clients get every state change and `Detached`, and its RAM and name are free again; `examples/uart-echo` takes input from several attached clients at full speed and in real time, and pauses with input waiting; a parked full-speed VM sleeps, and a real-time one keeps to its clock, its thread sleeping between batches (counted through a test-only hook on `ServerThread`); a guest that has presented a frame still runs full slices; a client that stops reading a chatty console is cut off while the VM and the other clients carry on; a client gets a limited time for its whole `Hello`, however slowly it trickles in; a create whose disk is slow to open (a FIFO, on Linux) holds up no other request; bad requests of every kind (unknown VMs, invalid names, profiles and programs, the RAM budget, wrong states, unserved groups, malformed and misdirected frames, oversize frames, bad handshakes) get the right error code and leave the server running; the client example above is checked against the test that runs it; and shutdown stops the VMs and closes every connection. `tests/cli.rs` runs the `avalon` binary and shuts it down with Ctrl-Break (Windows) or SIGTERM (Unix).

`crates/herald/tests` drive Herald against an in-process Avalon server. `session.rs` runs the streaming layer with no terminal (keys from a channel, the console into a buffer): it attaches to `examples/uart-echo` and to the kernel's shell, types, detaches (the VM carries on), attaches again and finds the earlier session in the scrollback; several sessions share one console; a pause and a resume are reported; stopping, exiting and destroying the VM end the session, and a VM that has already exited shows its scrollback and ends it at once; a long paste arrives whole and in order. `fake_server.rs` scripts a server for the rest: output queued while attaching, events for other VMs and events that do not decode, a detach that is never confirmed, a reply out of turn, a lost connection. `cli.rs` runs the `herald` binary through pipes, and `terminal.rs` on a terminal of its own making, a pseudoconsole (ConPTY) on Windows and a pseudoterminal on Linux: raw mode is on while Herald is attached, keys typed on the terminal reach the VM, and the terminal's modes are back as they were after a detach, after an error (the server hanging up) and after Ctrl-C.

`crates/avalon-proto` round-trips every Avalon protocol message through `encode`/`decode` and through a `Connection` over memory and TCP, checks a few layouts byte for byte against [`docs/avalon-protocol.md`](docs/avalon-protocol.md), rejects truncated, oversize and invalid frames, tests the version handshake, and feeds the decoder random and mutated bytes from a fixed-seed generator (anything that decodes must re-encode to the same bytes). It also checks that its hardware profile still mirrors Grail's `MachineConfig`.

`merlin file.c -o file.s` compiles a C-subset program; `merlin --dump-ast file.c` prints the checked syntax tree. The compiler's end-to-end harness (`crates/merlin/tests/harness.rs`) runs every program in `crates/merlin/tests/cases/` through merlin, merlin-as and merlin-ld and checks its exit code in Grail, and a reference interpreter of the checked syntax tree (`tests/common/interp.rs`) must agree. The generated assembly of a few cases is committed in `crates/merlin/tests/snapshots/*.s` (`MERLIN_BLESS=1 cargo test -p merlin --test cli` regenerates them after an intended codegen change). With clang and ld.lld installed, `cargo test -p merlin --test harness -- --ignored` also checks the programs' expected exit codes against clang and that clang assembles merlin's output to the same code, data and relocations as merlin-as; `MERLIN_HARNESS_LLVM=1` builds merlin's output with clang and ld.lld instead of our tools.

`crates/xtask/tests/examples.rs` builds every example from source with merlin-as and merlin-ld, checks the committed `.elf` is exactly what comes out, and runs each in Grail with its input (UART bytes for `uart-echo`, key events for `kbd-echo`, pointer input for `pointer`, `examples/disk/disk.img` for `disk`), checking the output and the exit code. It also compares `hello` and `multiobj` with clang + ld.lld's build of the same sources (committed in `crates/merlin-ld/tests/fixtures`): same loaded bytes, sections, symbols and disassembly. None of this needs LLVM.

The test ELFs are committed. To rebuild them you need clang + lld (the official Windows LLVM installer works) and a riscv-tests checkout: `scripts/build-riscv-tests.sh path/to/riscv-tests tests/riscv-tests` (add suite names, such as `rv32ua`, to rebuild only those). The examples are rebuilt with `cargo xtask examples` (our tools only); clang is just a reference for them, with the command at the top of each source (`-include examples/xcal.inc` teaches clang the Xcal mnemonics). `cargo xtask hello` assembles `examples/hello/hello.S` with merlin-as and links it with ld.lld into `crates/merlin-as/tests/golden/hello.elf`, which `cargo test` runs in Grail (and, with ld.lld installed, checks is current). The kernel is rebuilt with `cargo xtask kernel` (clang + ld.lld, see [kernel/README.md](kernel/README.md)), which first builds the user programs in `kernel/user` with merlin-as + merlin-ld into the generated `kernel/userprogs.s`; `cargo test` checks that file is current (no LLVM needed), and also rebuilds the kernel when clang is installed and checks the committed `kernel.elf` is current. merlin-ld's test fixtures (clang-built objects, among them `examples/hello` and `examples/multiobj`, plus ld.lld's link of them with its `llvm-readelf` and `llvm-objdump -d` listings as the expected output) are rebuilt with `cargo xtask ld-fixtures`; the riscv-tests objects merlin-ld relinks come from `scripts/build-riscv-tests.sh`.
