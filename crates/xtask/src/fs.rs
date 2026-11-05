//! `cargo xtask fs`: the root filesystem image the kernel mounts at boot
//! (docs/camelot-fs.md), built with the `camelot-fs` crate's mkfs: the files
//! in `kernel/fs/` (a README) at `/`, the user programs of `kernel/user`,
//! built with merlin, merlin-as and merlin-ld exactly as the kernel embeds them, in
//! `/bin`, and the console's device node, `/dev/console`. No LLVM needed.
//!
//! The image is bootable ([`build_bootable`]): Camelot's bootloader
//! (`kernel/boot/boot.s`, built with merlin-as + merlin-ld) in the boot
//! block and the committed disk kernel, `kernel/kernel-disk.elf` ([`KERNEL`]),
//! after the filesystem, so Grail needs no program to run it. The disk kernel
//! has no programs built in (the Linux way: the programs are on the root
//! filesystem, and only `kernel/kernel.elf`, run without a disk, carries its own,
//! as an initramfs would), so the whole system fits in one image of
//! [`IMAGE_SIZE`] blocks: the filesystem gets the blocks the kernel leaves.
//! The firmware loads the bootloader as its next stage, and a machine of 6 MiB
//! boots it:
//!
//! ```text
//! cargo xtask fs
//! cargo run --release -p grail -- --ram 6 --disk target/camelot-fs.img
//! cargo run --release -p grail -- --ram 8 --disk target/camelot-fs.img kernel/kernel.elf
//! ```

use crate::userprogs::{self, CCompiler};
use camelot_fs::mkfs::{Mkfs, Options};
use camelot_fs::BSIZE;
use std::path::{Path, PathBuf};

/// The kernel installed in a bootable image (under the repository root): the one
/// without built-in programs (`crate::kernel::disk_kernel`).
pub const KERNEL: &str = "kernel/kernel-disk.elf";
/// A bootable image's size by default, in 1 KiB blocks: 4 MiB, the boot block,
/// the filesystem and the kernel together. The programs in `/bin` take more
/// than half of the filesystem; the rest is the shell's to write files in
/// (`crates/xtask/tests/boot.rs` checks that at least a fifth is free).
pub const IMAGE_SIZE: u32 = 4096;
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

/// The shape of the filesystem of a bootable image of `size` blocks whose kernel
/// ELF is `kernel`: the default inodes and log, and every block the kernel leaves
/// (it takes what `camelot_fs::boot::install` stores of it, in whole blocks).
pub fn bootable_layout(size: u32, kernel: &[u8]) -> Result<Options, String> {
    let kernel_blocks = camelot_fs::boot::loadable(kernel)?.len().div_ceil(BSIZE);
    let kernel_blocks = u32::try_from(kernel_blocks).map_err(|_| "kernel: too big")?;
    let fs = size
        .checked_sub(kernel_blocks)
        .filter(|&b| b > 0)
        .ok_or_else(|| {
            format!(
                "a {size}-block image has no room for a filesystem: the kernel alone takes {kernel_blocks} \
             blocks"
            )
        })?;
    Ok(Options {
        size: fs,
        ..Options::default()
    })
}

/// [`build`], made bootable, `size` blocks in all ([`IMAGE_SIZE`] by default): the
/// bootloader from `kernel/boot/boot.S` in the boot block, [`KERNEL`] after the
/// filesystem (`camelot_fs::boot::install`), and the filesystem as big as the rest
/// ([`bootable_layout`]). Fails, saying so, when the files do not fit.
pub fn build_bootable(root: &Path, size: u32) -> Result<Vec<u8>, String> {
    let kernel_path = root.join(KERNEL);
    let kernel =
        std::fs::read(&kernel_path).map_err(|e| format!("cannot read {}: {e}", kernel_path.display()))?;
    let opts = bootable_layout(size, &kernel)?;
    let mut image = build(root, &opts).map_err(|e| {
        // Only running out of blocks or inodes is cured by a bigger image;
        // anything else (a missing file, a program that does not compile, a
        // size too big for a filesystem) is reported as it is.
        if e.contains("no space left") || e.contains("no inode left") {
            format!(
                "the root filesystem does not fit in a {size}-block image, where the kernel leaves it \
                 {} blocks ({e}): give a bigger --size",
                opts.size
            )
        } else {
            format!("cannot build a {size}-block root image: {e}")
        }
    })?;
    let bootloader = crate::boot::bootloader(root)?;
    camelot_fs::boot::install(&mut image, &bootloader, &kernel)?;
    debug_assert_eq!(image.len(), size as usize * BSIZE);
    Ok(image)
}
