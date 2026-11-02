//! Camelot build tasks, run as `cargo xtask <task>` (the alias lives in
//! `.cargo/config.toml`). Rust instead of shell scripts or Makefiles so they work
//! the same on Windows and Linux.
//!
//! * [`examples`]: builds every program in `examples/` with merlin-as +
//!   merlin-ld, no LLVM (the default way to build them).
//! * [`kernel`]: builds `kernel/kernel.elf` with clang + ld.lld, or with
//!   merlin-as for the assembly files.
//! * [`hello`]: builds `examples/hello/hello.S` with merlin-as + ld.lld into a
//!   committed reference image.
//! * [`ld_fixtures`]: regenerates merlin-ld's test fixtures (clang, ld.lld,
//!   llvm-readobj).
//! * [`tools`]: finds the LLVM tools.

pub mod examples;
pub mod hello;
pub mod kernel;
pub mod ld_fixtures;
pub mod tools;

use std::path::{Path, PathBuf};

/// The repository root (this crate lives in `crates/xtask`).
pub fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}
