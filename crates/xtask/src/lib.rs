//! Camelot build tasks, run as `cargo xtask <task>` (the alias lives in
//! `.cargo/config.toml`). Rust instead of shell scripts or Makefiles so they work
//! the same on Windows and Linux.
//!
//! * [`examples`]: builds every program in `examples/` with merlin-as +
//!   merlin-ld, no LLVM (the default way to build them).
//! * [`kernel`]: builds `kernel/kernel.elf` and `kernel/kernel-disk.elf` (the same
//!   kernel without its built-in programs) with merlin + merlin-as +
//!   merlin-ld, no LLVM, or the clang -O2 + ld.lld reference build.
//! * [`fs`]: builds the root filesystem image (the user programs and a
//!   README) with the `camelot-fs` crate's mkfs, bootable: 4 MiB with the disk kernel.
//! * [`boot`]: builds the boot ROM's disk loader and Camelot's bootloader
//!   with merlin-as + merlin-ld.
//! * [`toolchain`]: merlin, merlin-as and merlin-ld as libraries, for the
//!   kernel and its user programs.
//! * [`hello`]: builds `examples/hello/hello.S` with merlin-as + ld.lld into a
//!   committed reference image.
//! * [`ld_fixtures`]: regenerates merlin-ld's test fixtures (clang, ld.lld,
//!   llvm-readobj).
//! * [`tools`]: finds the LLVM tools.
//! * [`up`]: brings the Avalon platform up (Lake, Avalon and Pendragon, with
//!   Camelot's root image as Lake's image `os-base`) until Ctrl-C.
//! * [`userprogs`]: builds the user programs in `kernel/user/` with merlin,
//!   merlin-as and merlin-ld (or clang's C output, for the reference build)
//!   and generates the assembly that embeds them in the kernel (`userprogs.s`,
//!   made at build time, not committed).

pub mod boot;
pub mod examples;
pub mod fs;
pub mod hello;
pub mod kernel;
pub mod ld_fixtures;
pub mod toolchain;
pub mod tools;
pub mod up;
pub mod userprogs;

use std::path::{Path, PathBuf};

/// The repository root (this crate lives in `crates/xtask`).
pub fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}
