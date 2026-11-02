//! Camelot build tasks, run as `cargo xtask <task>` (the alias lives in
//! `.cargo/config.toml`). Rust instead of shell scripts or Makefiles so they work
//! the same on Windows and Linux.
//!
//! * [`kernel`]: builds `kernel/kernel.elf` with clang + ld.lld, or with
//!   merlin-as for the assembly files.
//! * [`ld_fixtures`]: regenerates merlin-ld's test fixtures (clang, ld.lld,
//!   llvm-readobj).
//! * [`tools`]: finds the LLVM tools.

pub mod kernel;
pub mod ld_fixtures;
pub mod tools;

use std::path::{Path, PathBuf};

/// The repository root (this crate lives in `crates/xtask`).
pub fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}
