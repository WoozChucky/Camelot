//! `cargo xtask fs`: the root filesystem image the kernel mounts at boot
//! (docs/camelot-fs.md), built with the `camelot-fs` crate's mkfs: the files
//! in `kernel/fs/` (a README) at `/`, and the user programs of `kernel/user`,
//! built with merlin-as + merlin-ld exactly as the kernel embeds them, in
//! `/bin`. No LLVM needed.
//!
//! ```text
//! cargo xtask fs
//! cargo run --release -p grail -- --disk target/camelot-fs.img kernel/kernel.elf
//! ```

use crate::userprogs;
use camelot_fs::mkfs::{Mkfs, Options};
use std::path::{Path, PathBuf};

/// The host directory copied to `/`.
pub const ROOT_DIR: &str = "kernel/fs";
/// Where the image goes by default (under the repository root).
pub const DEFAULT_OUT: &str = "target/camelot-fs.img";
/// Where the user programs go.
pub const BIN: &str = "/bin";

/// The default output path.
pub fn default_out(root: &Path) -> PathBuf {
    root.join(DEFAULT_OUT)
}

/// Builds the root image: `opts`' shape, `kernel/fs/` at `/`, the programs
/// in `/bin`.
pub fn build(root: &Path, opts: &Options) -> Result<Vec<u8>, String> {
    let mut fs = Mkfs::new(opts)?;
    fs.add_host_dir(&root.join(ROOT_DIR), "/")?;
    fs.mkdir(BIN)?;
    for prog in userprogs::build_all(&userprogs::user_dir(root))? {
        fs.add_file(&format!("{BIN}/{}", prog.name), &prog.elf)?;
    }
    Ok(fs.finish())
}
