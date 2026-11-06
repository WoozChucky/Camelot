//! `cargo xtask fs`: the root filesystem image the kernel mounts at boot
//! (docs/camelot-fs.md), built with the `camelot-fs` crate's mkfs: the files
//! in `kernel/fs/` (a README) at `/`, the user programs of `kernel/user`,
//! built with merlin, merlin-as and merlin-ld exactly as the kernel embeds them, in
//! `/bin`, the device nodes, `/dev/console`, `/dev/fb0`,
//! `/dev/input/event0` and `event1`, `/dev/ptmx` and the pseudo-terminals'
//! `/dev/pts/0` to `15`, and what a C program is built from
//! ([`usr_files`]): the header in `/usr/include`, and in `/usr/lib` the
//! start code `crt0.o`, the C library `libc.a` and the linker script
//! `user.ld`, the very files the programs of `/bin` were linked from. No
//! LLVM needed.
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
/// A bootable image's size by default, in 1 KiB blocks: 8 MiB, the boot block,
/// the filesystem and the kernel together. The programs in `/bin` and what
/// `/usr` holds take what they need; the rest is the shell's to write files in
/// (`crates/xtask/tests/boot.rs` checks that at least a fifth is free).
/// Raised from 4 MiB when the programs outgrew it, with room for the C
/// compiler (#243) and the desktop's programs to come.
pub const IMAGE_SIZE: u32 = 8192;
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
/// The framebuffer's device number (`FB` in kernel/include/file.h).
pub const FB_MAJOR: u16 = 2;
/// The input devices' number (`INPUT` in kernel/include/file.h): minor 0
/// is the keyboard, `/dev/input/event0`, and 1 the pointer, `event1`.
pub const INPUT_MAJOR: u16 = 3;
/// `/dev/ptmx`'s device number (`PTMX` in kernel/include/file.h): opening
/// it makes a pseudo-terminal.
pub const PTMX_MAJOR: u16 = 4;
/// The pseudo-terminals' slaves' number (`PTS`): minor N is `/dev/pts/N`.
pub const PTS_MAJOR: u16 = 5;
/// The slaves' nodes the image has, `/dev/pts/0` up (`NPTY` in
/// kernel/include/pty.h): Camelot has no devpts to make them as they come.
pub const NPTY: u16 = 16;
/// Where the headers go, as on Linux: a compiler on Camelot looks there.
pub const INCLUDE: &str = "/usr/include";
/// Where the start code, the C library and the programs' linker script go.
pub const LIB: &str = "/usr/lib";

/// The default output path.
pub fn default_out(root: &Path) -> PathBuf {
    root.join(DEFAULT_OUT)
}

/// What the root image's `/usr` holds (path, contents): the headers of
/// [`userprogs::HEADERS`] in [`INCLUDE`], and in [`LIB`] the start code
/// (`crt0.o`), the C library ([`userprogs::LIBC`]) and the programs' linker
/// script ([`userprogs::SCRIPT`]), from `rt`, the runtime the programs of
/// `/bin` are linked with, and `user_dir` (`kernel/user`). The objects are
/// installed stripped of their debug information
/// ([`userprogs::Runtime::stripped`]), as distributions install a C
/// library. So on Camelot
///
/// ```text
/// ld -T /usr/lib/user.ld -o PROG /usr/lib/crt0.o PROG.o -L/usr/lib -lc
/// ```
///
/// links a program as the host links it: with `-S`, what `/bin` holds.
pub fn usr_files(user_dir: &Path, rt: &userprogs::Runtime) -> Result<Vec<(String, Vec<u8>)>, String> {
    let read = |name: &str| {
        let path = user_dir.join(name);
        std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    };
    let rt = rt.installed();
    let mut files = Vec::new();
    for h in userprogs::HEADERS {
        files.push((format!("{INCLUDE}/{h}"), read(h)?));
    }
    for o in &rt.objects {
        files.push((format!("{LIB}/{}", o.name), o.data.clone()));
    }
    files.push((format!("{LIB}/{}", userprogs::LIBC), rt.archive.clone()));
    files.push((format!("{LIB}/{}", userprogs::SCRIPT), read(userprogs::SCRIPT)?));
    Ok(files)
}

/// Makes `/usr`, `/usr/include` and `/usr/lib` on `fs` and writes `files`
/// ([`usr_files`]) there.
pub fn add_usr(fs: &mut Mkfs, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    for d in ["/usr", INCLUDE, LIB] {
        fs.mkdir(d)?;
    }
    for (path, data) in files {
        fs.add_file(path, data)?;
    }
    Ok(())
}

/// Builds the root image: `opts`' shape, `kernel/fs/` at `/`, the programs
/// in `/bin`, `/dev/console`, `/dev/fb0`, `/dev/input/event0` and `event1`,
/// `/dev/ptmx` and `/dev/pts/0` to `15`, and `/usr` ([`usr_files`]).
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
    fs.add_device(&format!("{DEV}/fb0"), FB_MAJOR, 0)?;
    fs.mkdir(&format!("{DEV}/input"))?;
    fs.add_device(&format!("{DEV}/input/event0"), INPUT_MAJOR, 0)?;
    fs.add_device(&format!("{DEV}/input/event1"), INPUT_MAJOR, 1)?;
    fs.add_device(&format!("{DEV}/ptmx"), PTMX_MAJOR, 0)?;
    fs.mkdir(&format!("{DEV}/pts"))?;
    for n in 0..NPTY {
        fs.add_device(&format!("{DEV}/pts/{n}"), PTS_MAJOR, n)?;
    }
    fs.mkdir(BIN)?;
    let user = userprogs::user_dir(root);
    let rt = userprogs::build_runtime(&user, cc)?;
    for prog in userprogs::programs(&user)? {
        let prog = userprogs::link_program(&user, &prog, cc, &rt)?;
        fs.add_file(&format!("{BIN}/{}", prog.name), &prog.elf)?;
    }
    add_usr(&mut fs, &usr_files(&user, &rt)?)?;
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
