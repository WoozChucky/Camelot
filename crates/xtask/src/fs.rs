//! `cargo xtask fs`: the root filesystem image the kernel mounts at boot
//! (docs/camelot-fs.md), built with the `camelot-fs` crate's mkfs: the files
//! in `kernel/fs/` (a README) at `/`, the user programs of `kernel/user`,
//! built with merlin, merlin-as and merlin-ld exactly as the kernel embeds them, in
//! `/bin`, and the console's device node, `/dev/console`. No LLVM needed.
//!
//! The image is bootable ([`build_bootable`]): Camelot's bootloader
//! (`kernel/boot/boot.s`, built with merlin-as + merlin-ld) in the boot
//! block and the committed `kernel/kernel.elf` after the filesystem, so
//! Grail needs no program to run it.
//!
//! ```text
//! cargo xtask fs
//! cargo run --release -p grail -- --disk target/camelot-fs.img
//! cargo run --release -p grail -- --disk target/camelot-fs.img kernel/kernel.elf
//! ```

use crate::userprogs::{self, CCompiler};
use camelot_fs::mkfs::{Mkfs, Options};
use std::path::{Path, PathBuf};

/// The kernel installed in a bootable image (under the repository root).
pub const KERNEL: &str = "kernel/kernel.elf";
/// The host directory copied to `/`.
pub const ROOT_DIR: &str = "kernel/fs";
/// Where the image goes by default (under the repository root).
pub const DEFAULT_OUT: &str = "target/camelot-fs.img";
/// Where the user programs go.
pub const BIN: &str = "/bin";
/// The device nodes' directory.
pub const DEV: &str = "/dev";
/// The console's device number (`CONSOLE` in kernel/include/file.h).
pub const CONSOLE_MAJOR: u16 = 1;

/// The default output path.
pub fn default_out(root: &Path) -> PathBuf {
    root.join(DEFAULT_OUT)
}

/// Builds the root image: `opts`' shape, `kernel/fs/` at `/`, the programs
/// in `/bin`, `/dev/console`.
pub fn build(root: &Path, opts: &Options) -> Result<Vec<u8>, String> {
    build_with(root, opts, CCompiler::Merlin)
}

/// [`build`], with the programs' C compiled by `cc`: the clang reference
/// kernel checks that `/bin` holds the programs it embeds, so it needs an
/// image of its own.
pub fn build_with(root: &Path, opts: &Options, cc: CCompiler) -> Result<Vec<u8>, String> {
    let mut fs = Mkfs::new(opts)?;
    fs.add_host_dir(&root.join(ROOT_DIR), "/")?;
    fs.mkdir(DEV)?;
    fs.add_device(&format!("{DEV}/console"), CONSOLE_MAJOR, 0)?;
    fs.mkdir(BIN)?;
    for prog in userprogs::build_all_with(&userprogs::user_dir(root), cc)? {
        fs.add_file(&format!("{BIN}/{}", prog.name), &prog.elf)?;
    }
    Ok(fs.finish())
}

/// [`build`], made bootable: the bootloader from `kernel/boot/boot.s` in the
/// boot block, [`KERNEL`] after the filesystem (`camelot_fs::boot::install`).
pub fn build_bootable(root: &Path, opts: &Options) -> Result<Vec<u8>, String> {
    let mut image = build(root, opts)?;
    let kernel_path = root.join(KERNEL);
    let kernel =
        std::fs::read(&kernel_path).map_err(|e| format!("cannot read {}: {e}", kernel_path.display()))?;
    camelot_fs::boot::install(&mut image, &crate::boot::bootloader(root)?, &kernel)?;
    Ok(image)
}
