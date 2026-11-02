# Camelot

A learning project: design an instruction set, emulate it, build the toolchain for it, and write an operating system that runs on it. Everything builds and runs natively on Windows (and Linux/macOS) with nothing but Rust's standard library.

| Piece | What it is | Status |
|---|---|---|
| **Excalibur** | The ISA: RV32IM + Zicsr, M/S/U privilege, Sv32 paging, plus the Xcal extensions | Spec v0 in [`docs/excalibur-spec.md`](docs/excalibur-spec.md) |
| **Grail** | The emulator | Passes riscv-tests rv32ui/um/mi/si; CLINT, PLIC, UART with receive interrupts; Xcal framebuffer (Win32 window or PPM dumps), keyboard (from the window, with PLIC interrupts), sysinfo and block devices (disk images, `--disk`); configurable RAM, real-time clock and disks |
| **Merlin** | The toolchain: `merlin-as`, `merlin-ld`, `merlin-objdump`, and the `merlin` C-subset compiler | Builds every example on its own, no LLVM (`cargo xtask examples`). merlin-as: complete; its objects match clang's ([encoder notes](crates/merlin-as/ENCODER.md)). merlin-ld: links clang objects to the same bytes as ld.lld except where ld.lld merges `SHF_MERGE` strings and constants (see [spec §10](docs/excalibur-spec.md)), no relaxation (relocations explained in [src/reloc.rs](crates/merlin-ld/src/reloc.rs)). merlin: v0 (int-only C subset) compiles to assembly that our own assembler and linker build into programs Grail runs ([code generation notes](crates/merlin/src/codegen.rs)). merlin-objdump: matches llvm-objdump |
| **Camelot** | The kernel / OS | Boots on Grail in S-mode: UART, `kprintf`, trap handling, timer ticks, page allocator and Sv32 paging ([kernel/](kernel/README.md)) |
| **Avalon** | The platform around Grail: the Avalon hypervisor server, the Round Table control CLI, the Herald VM client and the Lake storage server | Protocol v1 in [`docs/avalon-protocol.md`](docs/avalon-protocol.md), implemented by `crates/avalon-proto`; the programs come next |

## Layout

```
crates/excalibur        instruction encodings, decoder, disassembler (shared by every tool)
crates/elf32            ELF32 reader (shared by grail, merlin-objdump, merlin-ld)
crates/grail            the emulator
crates/avalon-proto     the Avalon protocol: frames, messages, blocking connection (docs/avalon-protocol.md)
crates/merlin           the C-subset compiler (C → .s), with its test programs in tests/cases
crates/merlin-as        the assembler (source → ELF32 .o)
crates/merlin-ld        the linker (.o files → ELF32 executable)
crates/merlin-objdump   the disassembler / object dumper (llvm-objdump compatible output)
crates/xtask            build tasks: `cargo xtask examples`, `cargo xtask kernel`, `cargo xtask hello`, `cargo xtask ld-fixtures`
kernel/                 the Camelot kernel (C + assembly) and its prebuilt kernel.elf
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

The kernel runs the same way: `cargo run --release -p grail -- kernel/kernel.elf`.

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
8 devices:
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

## Testing

```
cargo test
```

Runs the decoder and device unit tests, every riscv-tests suite (except `rv32ui-p-ma_data`, which assumes hardware misaligned access; Excalibur traps instead), the examples (the UART echo is fed both from an in-process channel and through the `grail` binary's stdin), the kernel, the merlin compiler (every test program built with merlin, merlin-as and merlin-ld and run in Grail), merlin-ld against ld.lld (the loaded bytes of clang-built fixtures and of every riscv-tests program relinked from its object must be identical), and merlin-objdump against committed llvm-objdump output (`crates/merlin-objdump/tests/golden`; regenerate the fixtures and goldens with `scripts/gen-objdump-golden.sh`, which needs clang and llvm-objdump). The framebuffer test runs `examples/colorbars` headless and checks the dumped frames pixel by pixel and against a committed hash. The keyboard tests inject events through `Machine::key_event` and run both `examples/kbd-echo` programs. The window itself needs a desktop, so its tests (drawing, and key messages turned into events) are ignored by default: `cargo test --release -p grail -- --ignored window`. `crates/grail/tests/machine.rs` runs `examples/sysinfo` on machines with different RAM and clocks (two at once, on separate threads) and checks real-time pacing with lower bounds on elapsed time and generous upper ones. `crates/grail/tests/block.rs` runs `examples/disk` on in-memory disks, temporary copies of `examples/disk/disk.img`, host-made backends (one that fails every request) and through `grail --disk`, and checks the committed image against the bytes it builds (`GRAIL_WRITE_DISK_IMG=1` rewrites it).

`crates/avalon-proto` round-trips every Avalon protocol message through `encode`/`decode` and through a `Connection` over memory and TCP, checks a few layouts byte for byte against [`docs/avalon-protocol.md`](docs/avalon-protocol.md), rejects truncated, oversize and invalid frames, tests the version handshake, and feeds the decoder random and mutated bytes from a fixed-seed generator (anything that decodes must re-encode to the same bytes). It also checks that its hardware profile still mirrors Grail's `MachineConfig`.

`merlin file.c -o file.s` compiles a C-subset program; `merlin --dump-ast file.c` prints the checked syntax tree. The compiler's end-to-end harness (`crates/merlin/tests/harness.rs`) runs every program in `crates/merlin/tests/cases/` through merlin, merlin-as and merlin-ld and checks its exit code in Grail. The generated assembly of a few cases is committed in `crates/merlin/tests/snapshots/*.s` (`MERLIN_BLESS=1 cargo test -p merlin --test cli` regenerates them after an intended codegen change). With clang and ld.lld installed, `cargo test -p merlin --test harness -- --ignored` also checks the programs' expected exit codes against clang and that clang assembles merlin's output to the same code as merlin-as; `MERLIN_HARNESS_LLVM=1` builds merlin's output with clang and ld.lld instead of our tools.

`crates/xtask/tests/examples.rs` builds every example from source with merlin-as and merlin-ld, checks the committed `.elf` is exactly what comes out, and runs each in Grail with its input (UART bytes for `uart-echo`, key events for `kbd-echo`, `examples/disk/disk.img` for `disk`), checking the output and the exit code. It also compares `hello` and `multiobj` with clang + ld.lld's build of the same sources (committed in `crates/merlin-ld/tests/fixtures`): same loaded bytes, sections, symbols and disassembly. None of this needs LLVM.

The test ELFs are committed. To rebuild them you need clang + lld (the official Windows LLVM installer works) and a riscv-tests checkout: `scripts/build-riscv-tests.sh path/to/riscv-tests tests/riscv-tests`. The examples are rebuilt with `cargo xtask examples` (our tools only); clang is just a reference for them, with the command at the top of each source (`-include examples/xcal.inc` teaches clang the Xcal mnemonics). `cargo xtask hello` assembles `examples/hello/hello.S` with merlin-as and links it with ld.lld into `crates/merlin-as/tests/golden/hello.elf`, which `cargo test` runs in Grail (and, with ld.lld installed, checks is current). The kernel is rebuilt with `cargo xtask kernel` (clang + ld.lld, see [kernel/README.md](kernel/README.md)); `cargo test` also rebuilds it when clang is installed and checks the committed `kernel.elf` is current. merlin-ld's test fixtures (clang-built objects, among them `examples/hello` and `examples/multiobj`, plus ld.lld's link of them with its `llvm-readelf` and `llvm-objdump -d` listings as the expected output) are rebuilt with `cargo xtask ld-fixtures`; the riscv-tests objects merlin-ld relinks come from `scripts/build-riscv-tests.sh`.
