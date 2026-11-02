# Camelot

A learning project: design an instruction set, emulate it, build the toolchain for it, and write an operating system that runs on it. Everything builds and runs natively on Windows (and Linux/macOS) with nothing but Rust's standard library.

| Piece | What it is | Status |
|---|---|---|
| **Excalibur** | The ISA: RV32IM + Zicsr, M/S/U privilege, Sv32 paging, plus the Xcal extensions | Spec v0 in [`docs/excalibur-spec.md`](docs/excalibur-spec.md) |
| **Grail** | The emulator | Passes riscv-tests rv32ui/um/mi/si |
| **Merlin** | The toolchain: `merlin-as`, `merlin-ld`, `merlin-objdump`, and the `merlin` C-subset compiler | merlin-as: everything but the encoder ([guide](crates/merlin-as/ENCODER.md)) |
| **Camelot** | The kernel / OS | Not started |

## Layout

```
crates/excalibur   instruction encodings, decoder, disassembler (shared by every tool)
crates/grail       the emulator
crates/merlin-as   the assembler (source → ELF32 .o)
examples/          bare-metal programs
tests/riscv-tests  prebuilt riscv-tests ELFs (rebuild with scripts/build-riscv-tests.sh)
scripts/           test-data generators (riscv-tests, merlin-as golden encodings)
docs/              the ISA spec
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

Options: `--trace` prints every instruction with its disassembly, `--max-steps N` stops a runaway program, `--ram MiB` sets RAM size. Grail's exit code is the program's (`xcal.halt`, the test device, or riscv-tests' `tohost`).

## Testing

```
cargo test
```

Runs the decoder unit tests, every riscv-tests suite (except `rv32ui-p-ma_data`, which assumes hardware misaligned access; Excalibur traps instead), and the examples.

The test ELFs are committed. To rebuild them you need clang + lld (the official Windows LLVM installer works) and a riscv-tests checkout: `scripts/build-riscv-tests.sh path/to/riscv-tests tests/riscv-tests`. Until merlin-as and merlin-ld exist, examples are built with clang too; the command is at the top of each `.S` file.
