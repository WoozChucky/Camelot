# Camelot

A learning project: design an instruction set, emulate it, build the toolchain for it, and write an operating system that runs on it. Everything builds and runs natively on Windows (and Linux/macOS) with nothing but Rust's standard library.

| Piece | What it is | Status |
|---|---|---|
| **Excalibur** | The ISA: RV32IM + Zicsr, M/S/U privilege, Sv32 paging, plus the Xcal extensions | Spec v0 in [`docs/excalibur-spec.md`](docs/excalibur-spec.md) |
| **Grail** | The emulator | Passes riscv-tests rv32ui/um/mi/si; CLINT, PLIC, UART with receive interrupts; Xcal framebuffer (Win32 window or PPM dumps) |
| **Merlin** | The toolchain: `merlin-as`, `merlin-ld`, `merlin-objdump`, and the `merlin` C-subset compiler | merlin-as: everything but the encoder ([guide](crates/merlin-as/ENCODER.md)). merlin-ld: everything but applying relocations ([src/reloc.rs](crates/merlin-ld/src/reloc.rs)). merlin: v0 front end (int-only C subset) and test harness, codegen next. merlin-objdump: matches llvm-objdump |
| **Camelot** | The kernel / OS | Boots on Grail in S-mode: UART, `kprintf`, trap handling, timer ticks ([kernel/](kernel/README.md)) |

## Layout

```
crates/excalibur        instruction encodings, decoder, disassembler (shared by every tool)
crates/elf32            ELF32 reader (shared by grail, merlin-objdump, merlin-ld)
crates/grail            the emulator
crates/merlin           the C-subset compiler (C → .s), with its test programs in tests/cases
crates/merlin-as        the assembler (source → ELF32 .o)
crates/merlin-ld        the linker (.o files → ELF32 executable)
crates/merlin-objdump   the disassembler / object dumper (llvm-objdump compatible output)
crates/xtask            build tasks: `cargo xtask kernel`, `cargo xtask ld-fixtures`
kernel/                 the Camelot kernel (C + assembly) and its prebuilt kernel.elf
examples/               bare-metal programs
tests/riscv-tests       prebuilt riscv-tests ELFs (rebuild with scripts/build-riscv-tests.sh)
scripts/                test-data generators (riscv-tests, merlin-as golden encodings, merlin-objdump goldens)
docs/                   the ISA spec
```

## Running

You need [Rust](https://rustup.rs). No crates, no cross compiler, no QEMU.

```
cargo run --release -p grail -- examples/hello/hello.elf
```

```
Hello from Camelot!
[dbg] pc=80000014 a0 = 0x00000014 (20)
```

The kernel runs the same way: `cargo run --release -p grail -- kernel/kernel.elf`.

`examples/uart-echo` is interrupt-driven: it sleeps in `wfi`, and every byte on stdin raises a machine external interrupt through the PLIC. It echoes until it receives EOT (0x04):

```
printf 'hi\n\004' | cargo run --release -p grail -- examples/uart-echo/uart-echo.elf
```

Options: `--trace` prints every instruction with its disassembly, `--max-steps N` stops a runaway program, `--ram MiB` sets RAM size. Grail's exit code is the program's (`xcal.halt`, the test device, or riscv-tests' `tohost`).

To look inside an object or executable:

```
cargo run --release -p merlin-objdump -- -d examples/hello/hello.elf
```

```
80000000 <_start>:
80000000: 00010117     	auipc	sp, 0x10
80000004: 07010113     	addi	sp, sp, 0x70
...
80000010: 00c000ef     	jal	0x8000001c <puts>
80000014: 0005000b     	xcal.dbg	a0
```

The options are llvm-objdump's: `-d` disassemble, `-r` relocations (under each instruction with `-dr`), `-t` symbols, `-h` sections, `-x` all headers, `-s` section contents, `-M no-aliases`, `--no-show-raw-insn`. The output is the same as llvm-objdump 21's apart from Xcal instructions and undecodable words (`.word`), so the two can be diffed.

### Framebuffer

Programs can draw into a 640×480 framebuffer (spec section 9.3). Grail is headless by default; `--fb` opens a native window showing it (Windows only, plain Win32 GDI through hand-written FFI; elsewhere Grail warns and stays headless). `--fb-dump DIR` writes `DIR/frame-NNNN.ppm` on every `FB_PRESENT` and when the machine stops, with or without a window. `--headless` turns the window off even if `--fb` is given.

```
cargo run --release -p grail -- --fb examples/colorbars/colorbars.elf
cargo run --release -p grail -- --fb-dump frames examples/colorbars/colorbars.elf
```

Closing the window stops the emulator. When the program stops first, the window stays open on the last frame until you close it.

## Testing

```
cargo test
```

Runs the decoder and device unit tests, every riscv-tests suite (except `rv32ui-p-ma_data`, which assumes hardware misaligned access; Excalibur traps instead), the examples (the UART echo is fed both from an in-process channel and through the `grail` binary's stdin), the kernel, the merlin front-end tests, and merlin-objdump against committed llvm-objdump output (`crates/merlin-objdump/tests/golden`; regenerate the fixtures and goldens with `scripts/gen-objdump-golden.sh`, which needs clang and llvm-objdump). The framebuffer test runs `examples/colorbars` headless and checks the dumped frames pixel by pixel and against a committed hash. The window itself needs a desktop, so its test is ignored by default: `cargo test --release -p grail -- --ignored window`.

`merlin --dump-ast file.c` prints the checked syntax tree. The compiler's end-to-end harness (`crates/merlin/tests/harness.rs`) runs every program in `crates/merlin/tests/cases/` through merlin, merlin-as and merlin-ld and checks its exit code in Grail; it is `#[ignore]`d until codegen and the linker exist. With clang and ld.lld installed, `cargo test -p merlin --test harness -- --ignored clang_reference` checks the same programs' expected exit codes against clang.

The test ELFs are committed. To rebuild them you need clang + lld (the official Windows LLVM installer works) and a riscv-tests checkout: `scripts/build-riscv-tests.sh path/to/riscv-tests tests/riscv-tests`. Until merlin-as and merlin-ld exist, examples are built with clang too; the command is at the top of each `.S` file. The kernel is rebuilt with `cargo xtask kernel` (clang + ld.lld, see [kernel/README.md](kernel/README.md)); `cargo test` also rebuilds it when clang is installed and checks the committed `kernel.elf` is current. merlin-ld's test fixtures (clang-built objects, plus `llvm-readelf` of ld.lld's link of them as the expected layout) are rebuilt with `cargo xtask ld-fixtures`.
