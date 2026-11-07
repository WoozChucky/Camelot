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
//! `--profile` chooses what the image is for ([`Profile`]): its directory of
//! `kernel/profiles/` goes over `kernel/fs/`, with the `/etc/rc` init runs
//! (excalibur-spec section 10, "Init and the shell"). The server's, the
//! default, runs the console's shell and `shd`; the desktop's
//! (`target/camelot-desktop.img`, [`DESKTOP_IMAGE_SIZE`] blocks) keeps them
//! running and has the fonts in `/usr/share/fonts`. [`build`], the tests'
//! images, has no profile, so no `/etc/rc`: the shell is the first process.
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
/// compiler (#243). The server's ([`Profile::Server`]).
pub const IMAGE_SIZE: u32 = 8192;
/// The desktop image's size ([`Profile::Desktop`]), in 1 KiB blocks: 16 MiB,
/// room for the display server, the GUI library's programs, fonts and apps
/// (#183-#185) on top of what the server has.
pub const DESKTOP_IMAGE_SIZE: u32 = 16384;
/// Where the profiles' own files are (under the repository root): a tree for
/// each, `kernel/profiles/<name>/`, copied to `/` over [`ROOT_DIR`]'s.
pub const PROFILES_DIR: &str = "kernel/profiles";
/// The programs of `kernel/user` only the desktop image has: the server
/// image leaves them out of `/bin`. The desktop's programs join it as they
/// come (#183-#185).
pub const DESKTOP_PROGRAMS: &[&str] = &[
    "compositor",
    "files",
    "gears",
    "guidemo",
    "guitest",
    "panel",
    "sysmon",
    "term",
    "wltest",
];
/// Where the desktop image puts the fonts, as Linux distributions do.
pub const FONTS: &str = "/usr/share/fonts";
/// Where it puts the images the GUI library reads (QOI files).
pub const IMAGES: &str = "/usr/share/images";
/// Where it puts the applications the panel's launcher lists, a
/// freedesktop.org desktop entry each (`NAME.desktop`), as distributions do.
pub const APPLICATIONS: &str = "/usr/share/applications";
/// Their sources (under the repository root), copied byte for byte.
pub const APPLICATIONS_DIR: &str = "kernel/applications";

/// A root image's profile: one kernel, two images, told apart by their
/// programs, the files under `/etc` and `/usr/share`, and what init starts
/// (their `/etc/rc`), as Ubuntu Server and Ubuntu Desktop share a kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// The console's shell, whose end stops the machine, and `shd`.
    Server,
    /// The server's programs and the desktop's, the fonts, and an `/etc/rc`
    /// that keeps a console shell and `shd` running (the display server and
    /// the desktop join it as they come, #184 and #185).
    Desktop,
}

impl Profile {
    /// Every profile.
    pub const ALL: [Profile; 2] = [Profile::Server, Profile::Desktop];

    /// Its name, as `--profile` takes it.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Server => "server",
            Profile::Desktop => "desktop",
        }
    }

    /// The profile named `name`.
    pub fn parse(name: &str) -> Result<Profile, String> {
        Profile::ALL
            .into_iter()
            .find(|p| p.name() == name)
            .ok_or_else(|| format!("no profile '{name}' (server or desktop)"))
    }

    /// Its bootable image's size by default, in 1 KiB blocks: the one place
    /// each is given.
    pub fn image_size(self) -> u32 {
        match self {
            Profile::Server => IMAGE_SIZE,
            Profile::Desktop => DESKTOP_IMAGE_SIZE,
        }
    }

    /// Where `cargo xtask fs` writes its image by default (under `root`, the
    /// repository): the server's is [`DEFAULT_OUT`].
    pub fn default_out(self, root: &Path) -> PathBuf {
        match self {
            Profile::Server => root.join(DEFAULT_OUT),
            Profile::Desktop => root.join(DESKTOP_OUT),
        }
    }

    /// Whether its `/bin` has the program `name`.
    pub fn has_program(self, name: &str) -> bool {
        self == Profile::Desktop || !DESKTOP_PROGRAMS.contains(&name)
    }

    /// The names of the programs of `kernel/user` (`user`, as
    /// [`userprogs::user_dir`] gives it) its `/bin` has.
    pub fn programs(self, user: &Path) -> Result<Vec<String>, String> {
        let mut names = userprogs::program_names(user)?;
        names.retain(|n| self.has_program(n));
        Ok(names)
    }

    /// The tree of its own files, under `root`.
    pub fn dir(self, root: &Path) -> PathBuf {
        root.join(PROFILES_DIR).join(self.name())
    }
}
/// The host directory copied to `/`.
pub const ROOT_DIR: &str = "kernel/fs";
/// Where the image goes by default (under the repository root): the
/// server's, and the desktop's.
pub const DEFAULT_OUT: &str = "target/camelot-fs.img";
pub const DESKTOP_OUT: &str = "target/camelot-desktop.img";
/// Where the user programs go.
pub const BIN: &str = "/bin";
/// The device nodes' directory.
pub const DEV: &str = "/dev";
/// The directory of the sockets services listen on, Linux's `/run`: the
/// display server's `/run/wayland-0`.
pub const RUN: &str = "/run";
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

/// What the root image's `/usr` holds (path, contents): the headers of
/// [`userprogs::HEADERS`] in [`INCLUDE`], and in [`LIB`] the start code
/// (`crt0.o`), the C library ([`userprogs::LIBC`]), the GUI library
/// ([`userprogs::LIBGUI`]) and the programs' linker
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
    files.push((format!("{LIB}/{}", userprogs::LIBGUI), rt.gui_archive.clone()));
    files.push((format!("{LIB}/{}", userprogs::SCRIPT), read(userprogs::SCRIPT)?));
    Ok(files)
}

/// The desktop's `/usr/share` ([`SHARE_DIRS`] and the files in them): the
/// console's font (`kernel/fonts`), for the GUI library to draw text with, in
/// [`FONTS`], converted to PSF2 as the kernel embeds it
/// ([`crate::font::psf1_to_psf2`]), a directory a font, as distributions have
/// them, since a name has 14 bytes at most (docs/camelot-fs.md); the
/// images of `kernel/images` in [`IMAGES`] ([`crate::images`]); and the
/// desktop entries of [`APPLICATIONS_DIR`] in [`APPLICATIONS`], in the
/// order of their names.
pub fn share_files(root: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let read =
        |path: PathBuf| std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()));
    let fonts = root.join("kernel/fonts");
    let mut files = vec![
        (
            format!("{FONTS}/spleen/8x16.psfu"),
            crate::font::psf1_to_psf2(&read(root.join(crate::font::SOURCE))?)?,
        ),
        (
            format!("{FONTS}/spleen/LICENSE"),
            read(fonts.join("LICENSE.spleen"))?,
        ),
    ];
    for (name, qoi) in crate::images::build(root)? {
        files.push((format!("{IMAGES}/{name}"), qoi));
    }
    let apps = root.join(APPLICATIONS_DIR);
    let mut names: Vec<String> = std::fs::read_dir(&apps)
        .map_err(|e| format!("cannot read {}: {e}", apps.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".desktop"))
        .collect();
    names.sort();
    for name in names {
        files.push((format!("{APPLICATIONS}/{name}"), read(apps.join(&name))?));
    }
    Ok(files)
}

/// The directories of `/usr/share`, in the order they are made.
pub const SHARE_DIRS: &[&str] = &[
    "/usr/share",
    FONTS,
    "/usr/share/fonts/spleen",
    IMAGES,
    APPLICATIONS,
];

/// Makes [`SHARE_DIRS`] on `fs` (after `/usr`) and writes `files`
/// ([`share_files`]) there.
pub fn add_share(fs: &mut Mkfs, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    for d in SHARE_DIRS {
        fs.mkdir(d)?;
    }
    for (path, data) in files {
        fs.add_file(path, data)?;
    }
    Ok(())
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

/// Builds the root image every profile starts from: `opts`' shape,
/// `kernel/fs/` at `/`, the server's programs in `/bin` (the desktop's need
/// its `/usr/share`), `/dev/console`, `/dev/fb0`,
/// `/dev/input/event0` and `event1`, `/dev/ptmx` and `/dev/pts/0` to `15`,
/// and `/usr` ([`usr_files`]). No `/etc/rc`: its init runs the shell, as
/// pid 1, as if there were no init (the tests' images).
pub fn build(root: &Path, opts: &Options) -> Result<Vec<u8>, String> {
    build_with(root, opts, CCompiler::Merlin)
}

/// [`build`], with the programs' C compiled by `cc`: the clang reference
/// kernel checks that `/bin` holds the programs it embeds, so it needs an
/// image of its own.
pub fn build_with(root: &Path, opts: &Options, cc: CCompiler) -> Result<Vec<u8>, String> {
    build_image(root, opts, cc, None)
}

/// [`build`] for `profile`: its programs in `/bin`, its tree
/// ([`Profile::dir`]: `/etc/rc`) over `kernel/fs/`'s, and, on the desktop,
/// the fonts in [`FONTS`].
pub fn build_profile(root: &Path, opts: &Options, profile: Profile) -> Result<Vec<u8>, String> {
    build_image(root, opts, CCompiler::Merlin, Some(profile))
}

fn build_image(
    root: &Path,
    opts: &Options,
    cc: CCompiler,
    profile: Option<Profile>,
) -> Result<Vec<u8>, String> {
    let mut fs = Mkfs::new(opts)?;
    fs.add_host_dir(&root.join(ROOT_DIR), "/")?;
    if let Some(p) = profile {
        fs.add_host_dir(&p.dir(root), "/")?;
    }
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
    fs.mkdir(RUN)?;
    fs.mkdir(BIN)?;
    let user = userprogs::user_dir(root);
    let rt = userprogs::build_runtime(&user, cc)?;
    for prog in userprogs::programs(&user)? {
        let prog = userprogs::link_program(&user, &prog, cc, &rt)?;
        if profile.unwrap_or(Profile::Server).has_program(&prog.name) {
            fs.add_file(&format!("{BIN}/{}", prog.name), &prog.elf)?;
        }
    }
    add_usr(&mut fs, &usr_files(&user, &rt)?)?;
    if profile == Some(Profile::Desktop) {
        add_share(&mut fs, &share_files(root)?)?;
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

/// [`build_profile`], made bootable, `size` blocks in all
/// ([`Profile::image_size`] by default): the bootloader from
/// `kernel/boot/boot.S` in the boot block, [`KERNEL`] after the filesystem
/// (`camelot_fs::boot::install`), and the filesystem as big as the rest
/// ([`bootable_layout`]). Fails, saying so, when the files do not fit.
pub fn build_bootable(root: &Path, profile: Profile, size: u32) -> Result<Vec<u8>, String> {
    let kernel_path = root.join(KERNEL);
    let kernel =
        std::fs::read(&kernel_path).map_err(|e| format!("cannot read {}: {e}", kernel_path.display()))?;
    let opts = bootable_layout(size, &kernel)?;
    let mut image = build_profile(root, &opts, profile).map_err(|e| {
        // Only running out of blocks or inodes is cured by a bigger image;
        // anything else (a missing file, a program that does not compile, a
        // size too big for a filesystem) is reported as it is.
        if e.contains("no space left") || e.contains("no inode left") {
            format!(
                "the {} root filesystem does not fit in a {size}-block image, where the kernel leaves \
                 it {} blocks ({e}): give a bigger --size",
                profile.name(),
                opts.size
            )
        } else {
            format!("cannot build a {size}-block {} root image: {e}", profile.name())
        }
    })?;
    let bootloader = crate::boot::bootloader(root)?;
    camelot_fs::boot::install(&mut image, &bootloader, &kernel)?;
    debug_assert_eq!(image.len(), size as usize * BSIZE);
    Ok(image)
}
