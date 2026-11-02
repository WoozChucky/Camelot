# Camelot: notes for Claude Code sessions

Camelot is a learning project: the Excalibur ISA (RV32IMA + Zicsr, M/S/U, Sv32, Xcal extensions), the Grail emulator, the Merlin toolchain (merlin-as, merlin-ld, merlin-objdump, merlin C-subset compiler) and the Camelot OS. The spec is `docs/excalibur-spec.md`; the roadmap is its §11 and the tracking issue on GitHub (label `roadmap`).

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
crates/grail            the emulator (rom/diskboot.s: the boot ROM's disk loader, which boots a machine started with no program from disk 0)
crates/avalon-proto     Avalon protocol (framing, messages, Connection, Screen diffing), shared by the Avalon platform's programs; spec in docs/avalon-protocol.md
crates/gateway          user-mode NAT gateway (DHCP, DNS, TCP/UDP NAT, hostfwd) with its own frame send/recv API; grail wraps it as a NetBackend for `--net user` (spec 9.8). Must not depend on grail
crates/avalon           the Avalon hypervisor server: VM table, one thread per running VM (run_for slices), consoles, screen viewers and input, RAM budget, virtual Ethernet switches (src/switch.rs)
crates/avalon-client    the client layer: VM control, virtual network and disk management operations, parsers/formatters for addresses, sizes, clocks, disk sources, network cards (shared by Round Table and Pendragon; reuses grail::parse_ram_mib/parse_clock)
crates/round-table      the Round Table control CLI: argument parsing and text output over avalon-client (src/lib.rs `run`, main.rs is a thin wrapper)
crates/herald           Herald, the client that attaches to one VM: Session (the console streaming layer, terminal-free), screen::ScreenSession (the screen and input, window-free, drawing through a View) and the CLI, which puts the terminal in raw mode with grail::console::RawMode or shows the screen in grail::window (--screen)
crates/lake             the Lake storage server: a directory of disk images served over the protocol (one writer or many readers per image); Grail's client side is grail::lake::LakeDisk
crates/pendragon        the Pendragon web console: std-only HTTP/1.1 server, JSON API and an embedded page over avalon-client; API and security model in docs/pendragon.md
crates/camelot-fs       the Camelot filesystem on the host (docs/camelot-fs.md): format structs, mkfs, fsck and log replay, a reader, the `camelot-fs` command; the kernel's side is kernel/log.c and kernel/fs.c
crates/merlin           the C-subset compiler, C to .s (src/codegen.rs explains the stack frames and code generation, src/abi.rs the calling convention)
crates/merlin-as        the assembler
crates/merlin-ld        the linker (src/reloc.rs explains every relocation it applies)
crates/merlin-objdump   the disassembler; output matches llvm-objdump 21
crates/xtask            build tasks (`cargo xtask up` builds the platform and runs Lake, Avalon and Pendragon as child processes until Ctrl-C, after seeding Lake's images directory with the bootable root image as `os-base` (`--demo` adds the VM `vm001` on its copy `vm001-disk0`), see src/up.rs; `cargo xtask examples` builds every example with merlin-as + merlin-ld, no LLVM; `cargo xtask kernel` builds kernel/kernel.elf with merlin + merlin-as + merlin-ld, no LLVM, with the user programs of kernel/user built the same way and embedded through a userprogs.s generated at build time (in memory; a copy goes to target/kernel/obj, never committed), and kernel/kernel-disk.elf, the same kernel without the built-in programs, and `--cc clang` makes the clang -O2 + ld.lld reference build in target/kernel/clang; `cargo xtask fs` builds the root filesystem image target/camelot-fs.img from kernel/fs and the user programs, bootable (bootloader and kernel-disk.elf installed), 2 MiB in all, no LLVM; `cargo xtask boot` rebuilds the committed crates/grail/rom/diskboot.elf and kernel/boot/boot.elf with merlin-as + merlin-ld; `cargo xtask hello` rebuilds merlin-as's ld.lld-linked hello reference; `cargo xtask ld-fixtures` regenerates merlin-ld's test fixtures)
kernel/                 the Camelot kernel (C + assembly); kernel.elf and kernel-disk.elf (without built-in programs, for bootable images) are committed, rebuild them with `cargo xtask kernel` after changing the kernel, its user programs or merlin, merlin-as or merlin-ld (`cargo test` checks) (kernel/user: the user programs it embeds, C and assembly, and kernel/user/lib their C library, linked like an archive; system calls in docs/camelot-syscalls.md; kernel/net: the network stack, a file per layer, specified in docs/camelot-net.md; kernel/fs: the root image's files; kernel/boot: the bootloader of a bootable disk, which camelot-fs mkfs --boot installs)
docs/                   ISA spec, Avalon protocol, Pendragon API, Camelot system calls, Camelot filesystem format (camelot-fs.md), Camelot network stack (camelot-net.md)
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
