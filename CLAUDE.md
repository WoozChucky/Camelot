# Camelot: notes for Claude Code sessions

Camelot is Nuno's learning project: the Excalibur ISA (RV32IM + Zicsr, M/S/U, Sv32, Xcal extensions), the Grail emulator, the Merlin toolchain (merlin-as, merlin-ld, merlin-objdump, merlin C-subset compiler) and the Camelot OS. The spec is `docs/excalibur-spec.md`; the roadmap is its §11 and the tracking issue on GitHub (label `roadmap`).

## Picking up work

- Work comes from GitHub issues. Each one has a goal, spec references, acceptance criteria and dependencies. Check that its dependencies are merged before starting.
- Issues labeled **`nuno-writes`** are the parts Nuno writes to learn from (e.g. the merlin-as encoder, linker relocations, compiler codegen). On those, explain, review, write tests and answer questions, but do not write the implementation unless Nuno explicitly asks you to in the session.
- Issues labeled `scaffold` are plumbing you can implement.
- If the spec is ambiguous or you change a design decision, update `docs/excalibur-spec.md` in the same PR.

## Hard rules

- **Rust, std only.** No crates in any `Cargo.toml`, including dev-dependencies. `Cargo.lock` must list only workspace crates. Windows APIs go through hand-written `extern "system"` FFI.
- **Windows-native.** Everything must build and run with `cargo` on Windows (and Linux). No WSL, no QEMU, no GCC cross toolchain. Build helpers are Rust (tests, `cargo xtask`-style binaries), not shell scripts. Existing `scripts/*.sh` are only for regenerating committed reference files with LLVM.
- **LLVM is a reference, not a dependency.** clang, ld.lld, llvm-mc and llvm-objdump (official Windows LLVM installer) may be used to produce golden files and to validate output, but `cargo test` must pass without them. Commit expected outputs.
- **No AI attribution anywhere**: no `Co-Authored-By` trailers, no "Generated with" lines, no session links, no Claude footers in commits, PRs, issues, comments or code.

## Layout

```
crates/excalibur   encodings, decoder, disassembler (shared by every tool)
crates/grail       the emulator
crates/merlin-as   the assembler (once merged)
docs/              ISA spec
examples/          bare-metal programs
tests/riscv-tests  prebuilt riscv-tests ELFs
```

Reuse `excalibur` (register names, CSR names, decoder) instead of duplicating tables.

## Before pushing

CI runs on Windows and Linux; run the same checks locally:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --release
```

Formatting uses `rustfmt.toml` (max width 110). Branch per issue, PR description says `Closes #N`.
