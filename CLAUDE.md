# Camelot: notes for Claude Code sessions

Camelot is a learning project: the Excalibur ISA (RV32IM + Zicsr, M/S/U, Sv32, Xcal extensions), the Grail emulator, the Merlin toolchain (merlin-as, merlin-ld, merlin-objdump, merlin C-subset compiler) and the Camelot OS. The spec is `docs/excalibur-spec.md`; the roadmap is its §11 and the tracking issue on GitHub (label `roadmap`).

## Picking up work

- Work comes from GitHub issues. Each one has a goal, spec references, acceptance criteria and dependencies. Check that its dependencies are merged before starting.
- Any open issue can be implemented. Code in the core pieces (encoder, linker relocations, compiler codegen) should stay readable and well commented, since it doubles as study material.
- If the spec is ambiguous or you change a design decision, update `docs/excalibur-spec.md` in the same PR.

## Hard rules

- **Rust, std only.** No crates in any `Cargo.toml`, including dev-dependencies. `Cargo.lock` must list only workspace crates. Windows APIs go through hand-written `extern "system"` FFI.
- **Windows-native.** Everything must build and run with `cargo` on Windows (and Linux). No WSL, no QEMU, no GCC cross toolchain. Build helpers are Rust (tests, `cargo xtask`-style binaries), not shell scripts. Existing `scripts/*.sh` are only for regenerating committed reference files with LLVM.
- **LLVM is a reference, not a dependency.** clang, ld.lld, llvm-mc and llvm-objdump (official Windows LLVM installer) may be used to produce golden files and to validate output, but `cargo test` must pass without them. Commit expected outputs.
- **No AI attribution anywhere**: no `Co-Authored-By` trailers, no "Generated with" lines, no session links, no Claude footers in commits, PRs, issues, comments or code.

## Layout

```
crates/excalibur        encodings, decoder, disassembler (shared by every tool)
crates/elf32            ELF32 reader (grail, merlin-objdump, merlin-ld)
crates/grail            the emulator
crates/merlin           the C-subset compiler (front end + test harness; codegen is #14)
crates/merlin-as        the assembler
crates/merlin-ld        the linker (src/reloc.rs explains every relocation it applies)
crates/merlin-objdump   the disassembler; output matches llvm-objdump 21
crates/xtask            build tasks (`cargo xtask examples` builds every example with merlin-as + merlin-ld, no LLVM; `cargo xtask kernel` builds kernel/kernel.elf with clang + ld.lld; `cargo xtask hello` rebuilds merlin-as's ld.lld-linked hello reference; `cargo xtask ld-fixtures` regenerates merlin-ld's test fixtures)
kernel/                 the Camelot kernel (C + assembly); kernel.elf is committed, rebuild it after changes
docs/                   ISA spec
examples/               bare-metal programs; their .elf files are committed, rebuild them with `cargo xtask examples` after changes (`cargo test` checks)
tests/riscv-tests       prebuilt riscv-tests ELFs, and obj/ with their objects (merlin-ld relinks them)
```

Reuse `excalibur` (register names, CSR names, decoder, `Inst::format`) and `elf32` (reading ELF files, relocation names) instead of duplicating tables.

## Before pushing

CI runs on Linux only (Windows runners are expensive), so Windows is covered by running these checks locally on Windows before every push. Code behind `cfg(windows)`, such as the Win32 window, is only checked here:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --release
```

Formatting uses `rustfmt.toml` (max width 110). Branch per issue, PR description says `Closes #N`.
