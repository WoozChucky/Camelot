//! Building and running the Camelot kernel (`kernel/`).
//!
//! Every `*.c`, `*.S` and `*.s` file directly in `kernel/` or in one of its
//! source subdirectories ([`SUBDIRS`], e.g. `net/arp.c`) is one object (a
//! `.S` file is assembly that goes through the C preprocessor first, #193),
//! and so is
//! [`USERPROGS`], `userprogs.s`: the user programs of `kernel/user/` as data,
//! which is not a file in `kernel/` but generated at build time
//! ([`crate::userprogs`]). The objects are linked in name order with
//! `kernel/kernel.lds.S` ([`SCRIPT`]), a linker script that goes through the
//! C preprocessor first, as Linux's `vmlinux.lds.S` (`merlin -E -P`,
//! [`linker_script`]). Our own tools build it ([`Toolchain::Merlin`],
//! [`build_merlin`]): merlin compiles each C file, merlin-as assembles it and
//! the assembly files and merlin-ld links them, all in memory, with no LLVM.
//! That is how the committed `kernel/kernel.elf` is made, and `cargo test`
//! checks it is what the current sources and tools give.
//!
//! The kernel is a higher-half kernel linked at 0xffff_ffff_8000_0000 and
//! loaded at 0x8020_0000 after the firmware (#154 and #174), which embeds the
//! user programs (#155, [`crate::userprogs::generated`]).
//!
//! The reference build ([`Toolchain::Clang`], `cargo xtask kernel --cc
//! clang`) compiles the C with clang `-O2`, assembles with clang (or
//! merlin-as) and links with ld.lld, so the two can be compared. Either way
//! every tool sees paths relative to `kernel/` (clang runs inside it), so the
//! output doesn't depend on where the repository is checked out.

use crate::toolchain;
use grail::{Bios, Machine, MachineConfig, RunResult};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};

/// Flags shared by every clang invocation: the Excalibur target, with the
/// LP64 ABI. No linker relaxation, since merlin-as and merlin-ld won't do it
/// either.
pub const TARGET_FLAGS: &[&str] = &[
    "--target=riscv64-unknown-elf",
    "-march=rv64ima_zicsr",
    "-mabi=lp64",
    "-mno-relax",
];

/// Extra flags for C files in the reference build: freestanding, no libc
/// headers, no hidden runtime (stack protector, unwind tables), and no
/// `.comment` with the clang version. `medany` addresses globals
/// pc-relatively (`auipc`): the kernel runs at its physical address before
/// paging is on, where only pc-relative addressing reaches its symbols (#154),
/// and merlin always addresses them so.
pub const C_FLAGS: &[&str] = &[
    "-mcmodel=medany",
    "-std=c11",
    "-O2",
    "-ffreestanding",
    "-fno-builtin",
    "-nostdinc",
    "-nostdlib",
    "-fno-pic",
    "-fno-common",
    "-fno-ident",
    "-fno-stack-protector",
    "-fno-asynchronous-unwind-tables",
    "-fno-unwind-tables",
    "-Wall",
    "-Wextra",
    "-Werror",
    "-Iinclude",
];

/// Generous step budget for running the kernel non-interactively: booting
/// it to its shell, mounting the root filesystem and checking that `/bin`
/// holds the kernel's own programs, and a few shell commands. Nearly all of
/// the boot goes to putting every page on the free list, mapping RAM and the
/// allocator self-test that takes every page and gives it back, so it grows
/// with the RAM; merlin does not optimize, and the clang `-O2` reference
/// build needs far fewer.
pub const BOOT_MAX_STEPS: u64 = 200_000_000;

/// The RAM of the machines `cargo xtask kernel --run` and the tests boot the
/// kernel on: the firmware's 2 MiB at the start of RAM, the image's from
/// 0x8020_0000, and room for the pages of a few processes.
pub const RAM_SIZE: usize = 8 << 20;

/// The machine the kernel runs on here: Camelot's firmware (`grail --bios
/// default`), which starts it in S-mode at 0x8020_0000, and [`RAM_SIZE`] of
/// RAM.
pub fn machine_config() -> MachineConfig {
    MachineConfig {
        ram_size: RAM_SIZE,
        bios: Bios::Default,
        ..MachineConfig::default()
    }
}

/// The banner the kernel prints first at boot, before `Camelot vX booting...`
/// (`banner` in kernel/main.c), as a string literal so tests can `concat!` it
/// into their expected output.
#[macro_export]
macro_rules! banner {
    () => {
        concat!(
            "\n",
            r"    ____      _      __  __   _____   _        ___    _____",
            "\n",
            r"   / ___|    / \    |  \/  | | ____| | |      / _ \  |_   _|",
            "\n",
            r"  | |       / _ \   | |\/| | |  _|   | |     | | | |   | |",
            "\n",
            r"  | |___   / ___ \  | |  | | | |___  | |___  | |_| |   | |",
            "\n",
            r"   \____| /_/   \_\ |_|  |_| |_____| |_____|  \___/    |_|",
            "\n",
            "\n",
        )
    };
}

/// [`banner!`] as a constant.
pub const BANNER: &str = banner!();

/// The include directory of the kernel's C files, relative to the kernel
/// directory.
pub const INCLUDE_DIR: &str = "include";
/// Where the kernel's `.S` files find `<asm.h>` (the repository's
/// `include/`, #193), relative to the kernel directory, and the kernel's own
/// headers (for constants shared with C).
pub const ASM_INCLUDE_DIRS: &[&str] = &["../include", INCLUDE_DIR];
/// The kernel directory as the debug information names it (the compilation
/// directory of `-g`): `kernel`, relative to the repository root, so a
/// debugger run there finds `kernel/main.c`.
pub const DEBUG_DIR: &str = "kernel";
/// The kernel's linker script, in the kernel directory, preprocessed
/// ([`linker_script`]), as Linux's `vmlinux.lds.S`: it links high and loads
/// at 0x8020_0000, after the firmware (`AT`).
pub const SCRIPT: &str = "kernel.lds.S";
/// The preprocessed script, in the object directory, for ld.lld (and for
/// reading: merlin-ld links with it in memory).
pub const SCRIPT_OUT: &str = "kernel.lds";

/// The text of [`SCRIPT`]: `merlin -E -P` (no line markers, which a linker
/// script can't have) with the assembly's include directories, so it may
/// include the headers that assembly does.
pub fn linker_script(kernel_dir: &Path) -> Result<String, String> {
    toolchain::preprocess_lds(kernel_dir, SCRIPT, ASM_INCLUDE_DIRS)
}

/// Links `inputs` with [`linker_script`] (`what` in errors).
fn link_kernel(kernel_dir: &Path, inputs: &[merlin_ld::Input], what: &str) -> Result<Vec<u8>, String> {
    let text = linker_script(kernel_dir)?;
    toolchain::link_script(SCRIPT, text, inputs, what, false)
}
/// The generated source that holds the user programs
/// ([`crate::userprogs::generate`]). It is a source of the kernel, linked in
/// its name order like the others, but no file in the kernel directory: it is
/// built from [`USER_DIR`] every time, and a file of that name in the kernel
/// directory (a stale copy from when it was committed) is an error.
pub const USERPROGS: &str = "userprogs.s";
/// The user programs' directory, in the kernel directory
/// ([`crate::userprogs::USER_DIR`] from the repository root). A kernel
/// directory without one has no [`USERPROGS`].
pub const USER_DIR: &str = "user";
/// The kernel with its user programs built in, in the kernel directory: what
/// Grail runs as a program (`grail kernel/kernel.elf`), with or without a disk.
pub const KERNEL_ELF: &str = "kernel.elf";
/// The kernel without them ([`disk_kernel`]), in the kernel directory: what a
/// bootable root image holds (`cargo xtask fs`).
pub const DISK_KERNEL: &str = "kernel-disk.elf";

/// What assembles the `.s` files in the reference build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assembler {
    /// clang's integrated assembler.
    Clang,
    /// Our own merlin-as (as a library). Bootstrapping step 1: our `.o`
    /// linked by ld.lld must behave the same.
    Merlin,
}

/// Which tools build the kernel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Toolchain {
    /// merlin, merlin-as and merlin-ld, in memory: how `kernel/kernel.elf`
    /// is built. No LLVM.
    Merlin,
    /// The reference build: clang compiles the C, `assembler` assembles the
    /// `.s` files and ld.lld links.
    Clang {
        clang: PathBuf,
        lld: PathBuf,
        assembler: Assembler,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    C,
    /// Assembly: a `.S` file, preprocessed first, or a `.s` one.
    Asm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// File name inside the kernel directory, e.g. `main.c`.
    pub name: String,
    pub lang: Lang,
}

impl Source {
    /// Object file name: `main.c` → `main.c.o` (keeps `x.c` and `x.s` apart).
    pub fn object_name(&self) -> String {
        format!("{}.o", self.name)
    }

    /// Where merlin's assembly for a C file goes next to the objects:
    /// `main.c` → `main.c.s`.
    pub fn asm_name(&self) -> String {
        format!("{}.s", self.name)
    }

    /// Whether this is [`USERPROGS`], generated at build time rather than
    /// read from the kernel directory.
    pub fn is_generated(&self) -> bool {
        self.name == USERPROGS
    }

    /// Whether this goes through the C preprocessor before the assembler: a
    /// `.S` file.
    pub fn is_preprocessed(&self) -> bool {
        self.name.ends_with(".S")
    }
}

/// The subdirectories of the kernel directory whose `*.c`, `*.S` and `*.s`
/// files are kernel sources too, named `dir/file` (the others hold the user
/// programs, the bootloader, the headers and the root image's files).
pub const SUBDIRS: &[&str] = &["net"];

/// Lists the kernel's sources (`*.c`, `*.S`, `*.s`, in `kernel_dir` and its
/// [`SUBDIRS`], and the generated [`USERPROGS`] if `kernel_dir` has a
/// [`USER_DIR`]) in name order. A `userprogs.s` file in `kernel_dir` is an
/// error rather than a second copy of the programs.
pub fn sources(kernel_dir: &Path) -> Result<Vec<Source>, String> {
    let mut out = Vec::new();
    list_sources(kernel_dir, "", &mut out)?;
    if out.iter().any(Source::is_generated) {
        return Err(format!(
            "{}: the user programs' assembly is generated at build time from {USER_DIR}/ \
             and is not a source file any more; delete this stale copy",
            kernel_dir.join(USERPROGS).display()
        ));
    }
    if kernel_dir.join(USER_DIR).is_dir() {
        out.push(Source {
            name: USERPROGS.into(),
            lang: Lang::Asm,
        });
    }
    for sub in SUBDIRS {
        let dir = kernel_dir.join(sub);
        if dir.is_dir() {
            list_sources(&dir, &format!("{sub}/"), &mut out)?;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    if out.is_empty() {
        return Err(format!("no .c, .S or .s files in {}", kernel_dir.display()));
    }
    Ok(out)
}

/// Adds the `*.c`, `*.S` and `*.s` files of `dir` to `out`, named `prefix` +
/// their file name.
fn list_sources(dir: &Path, prefix: &str, out: &mut Vec<Source>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for e in entries {
        let e = e.map_err(|e| e.to_string())?;
        if !e.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        let lang = match Path::new(&name).extension().and_then(|x| x.to_str()) {
            Some("c") => Lang::C,
            // A linker script that is preprocessed (kernel.lds.S) is no
            // assembly.
            Some("S") if name.ends_with(".lds.S") => continue,
            Some("s" | "S") => Lang::Asm,
            _ => continue,
        };
        out.push(Source {
            name: format!("{prefix}{name}"),
            lang,
        });
    }
    Ok(())
}

/// One source of the kernel built by our own tools.
#[derive(Clone, Debug)]
pub struct Object {
    pub source: Source,
    /// merlin's assembly, for a C file, or the preprocessed text of a `.S`
    /// file: what merlin-as assembled.
    pub asm: Option<String>,
    /// The object merlin-as made.
    pub obj: Vec<u8>,
}

/// A kernel built by our own tools.
#[derive(Clone, Debug)]
pub struct Built {
    /// In link order: the sources' name order.
    pub objects: Vec<Object>,
    pub elf: Vec<u8>,
}

/// The text of [`USERPROGS`] for the kernel in `kernel_dir`: every program in
/// its [`USER_DIR`] built by merlin, merlin-as and merlin-ld, in memory
/// ([`crate::userprogs::generated`]).
pub fn userprogs(kernel_dir: &Path) -> Result<String, String> {
    crate::userprogs::generated(&kernel_dir.join(USER_DIR))
}

/// The assembly merlin-as assembles for the assembly source `source` of the
/// kernel: a `.S` file preprocessed by merlin ([`ASM_INCLUDE_DIRS`]), a `.s`
/// one as it is.
pub fn assembly_of(kernel_dir: &Path, source: &Source) -> Result<String, String> {
    if source.is_preprocessed() {
        return toolchain::preprocess_asm(kernel_dir, &source.name, ASM_INCLUDE_DIRS);
    }
    let path = kernel_dir.join(&source.name);
    std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Builds the kernel in `kernel_dir` with merlin, merlin-as and merlin-ld,
/// in memory: merlin compiles each C file (`-I include`), merlin-as
/// assembles that and each assembly file (a `.S` one preprocessed by merlin
/// first), and merlin-ld links the objects in name order with
/// [`linker_script`]. [`USERPROGS`] is `userprogs` if given (the text of
/// another build of the programs), or else [`userprogs()`]. No LLVM; nothing
/// is written.
pub fn build_merlin(kernel_dir: &Path, userprogs: Option<&str>) -> Result<Built, String> {
    let mut objects = Vec::new();
    for source in sources(kernel_dir)? {
        let (asm, obj) = match source.lang {
            Lang::C => {
                let asm = toolchain::compile_c(kernel_dir, &source.name, &[INCLUDE_DIR], DEBUG_DIR)?;
                let obj = toolchain::assemble(&format!("{} (merlin's assembly)", source.asm_name()), &asm)?;
                (Some(asm), obj)
            }
            Lang::Asm if source.is_generated() => {
                let text = match userprogs {
                    Some(text) => text.to_string(),
                    None => self::userprogs(kernel_dir)?,
                };
                (None, toolchain::assemble(&source.name, &text)?)
            }
            Lang::Asm => {
                let text = assembly_of(kernel_dir, &source)?;
                // A preprocessed source keeps its text, as merlin's assembly
                // of a C file is kept: what merlin-as saw.
                let asm = source.is_preprocessed().then(|| text.clone());
                (asm, toolchain::assemble(&source.name, &text)?)
            }
        };
        objects.push(Object { source, asm, obj });
    }
    let inputs: Vec<merlin_ld::Input> = objects
        .iter()
        .map(|o| merlin_ld::Input {
            name: o.source.object_name(),
            data: o.obj.clone(),
        })
        .collect();
    let elf = link_kernel(kernel_dir, &inputs, "the kernel")?;
    Ok(Built { objects, elf })
}

/// The disk kernel (`kernel/kernel-disk.elf`, [`DISK_KERNEL`]): the objects of
/// `built` linked again with `kernel_dir`'s linker script, but with a `userprogs.s`
/// that has no programs ([`crate::userprogs::empty`]), so the kernel runs every
/// program from `/bin` on its root filesystem.
///
/// This is the Linux split: `kernel/kernel.elf` carries its programs, most of its
/// size, the way an initramfs carries the programs of a machine with no root
/// disk; the kernel a bootable image holds (`cargo xtask fs`) does not need them,
/// since the same programs are in the image's `/bin`, and is a fraction of the
/// size. Booted with no shell in `/bin` (or with no root filesystem at all) it
/// panics: there is nothing else to run.
pub fn disk_kernel(kernel_dir: &Path, built: &Built) -> Result<Vec<u8>, String> {
    if !built.objects.iter().any(|o| o.source.is_generated()) {
        return Err(format!("the kernel has no {USERPROGS} to leave out"));
    }
    // The same link, in the same order, with the empty table's object in place
    // of the programs'.
    let empty = toolchain::assemble(USERPROGS, &crate::userprogs::empty())?;
    let inputs: Vec<merlin_ld::Input> = built
        .objects
        .iter()
        .map(|o| merlin_ld::Input {
            name: o.source.object_name(),
            data: if o.source.is_generated() {
                empty.clone()
            } else {
                o.obj.clone()
            },
        })
        .collect();
    link_kernel(kernel_dir, &inputs, "the disk kernel")
}

/// The disk kernel's path for a kernel written to `out`: `kernel.elf` gives
/// `kernel-disk.elf` next to it.
pub fn disk_kernel_path(out: &Path) -> PathBuf {
    let stem = out.file_stem().unwrap_or_default().to_string_lossy();
    let stem = format!("{stem}-disk");
    let name = match out.extension() {
        Some(ext) => format!("{stem}.{}", ext.to_string_lossy()),
        None => stem,
    };
    out.with_file_name(name)
}

/// Everything needed for one kernel build. The tools run with `kernel_dir` as
/// their working directory, so relative `obj_dir`/`out` paths are taken
/// relative to it; pass absolute ones.
#[derive(Clone, Debug)]
pub struct Build {
    /// Directory holding the sources, `include/` and [`SCRIPT`].
    pub kernel_dir: PathBuf,
    /// Where object files (and merlin's assembly) go.
    pub obj_dir: PathBuf,
    /// The linked ELF.
    pub out: PathBuf,
    pub toolchain: Toolchain,
    /// The file assembled as [`USERPROGS`]: the reference build embeds user
    /// programs built from clang's assembly. Without one, [`Build::run`]
    /// generates it from the user programs (merlin, merlin-as, merlin-ld) and
    /// writes it to the object directory, where every tool can read it.
    pub userprogs: Option<PathBuf>,
    /// Print each command (or its command-line equivalent) before running it.
    pub verbose: bool,
}

impl Build {
    /// The [`USERPROGS`] file the tools read: [`Build::userprogs`], or the
    /// generated one in the object directory.
    pub fn userprogs_path(&self) -> PathBuf {
        match &self.userprogs {
            Some(p) => p.clone(),
            None => self.obj_dir.join(USERPROGS),
        }
    }

    /// What a tool reads for `src`: its path relative to the kernel
    /// directory, or [`Build::userprogs_path`] for [`USERPROGS`].
    fn input(&self, src: &Source) -> OsString {
        if src.is_generated() {
            self.userprogs_path().into()
        } else {
            src.name.clone().into()
        }
    }

    /// clang arguments for one source (run with the kernel directory as cwd).
    pub fn compile_args(&self, src: &Source) -> Vec<OsString> {
        let mut args: Vec<OsString> = TARGET_FLAGS.iter().map(OsString::from).collect();
        if src.lang == Lang::C {
            args.extend(C_FLAGS.iter().map(OsString::from));
        } else if src.is_preprocessed() {
            // clang preprocesses a .S file itself; <asm.h> is in ../include.
            args.extend(ASM_INCLUDE_DIRS.iter().map(|d| OsString::from(format!("-I{d}"))));
        }
        args.push("-c".into());
        args.push(self.input(src));
        args.push("-o".into());
        args.push(self.obj_dir.join(src.object_name()).into());
        args
    }

    /// Linker arguments (run with the kernel directory as cwd): ld.lld's, and
    /// merlin-ld's, which takes the same.
    /// The script is [`SCRIPT_OUT`], preprocessed into the object directory
    /// ([`Build::write_script`]).
    pub fn link_args(&self, sources: &[Source]) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "-T".into(),
            self.obj_dir.join(SCRIPT_OUT).into(),
            "-o".into(),
            self.out.clone().into(),
        ];
        args.extend(sources.iter().map(|s| self.obj_dir.join(s.object_name()).into()));
        args
    }

    /// Compiles, assembles and links; returns the output path.
    pub fn run(&self) -> Result<PathBuf, String> {
        let sources = sources(&self.kernel_dir)?;
        let obj_dir = self.kernel_dir.join(&self.obj_dir);
        // `net/arp.c` becomes `net/arp.c.o` there.
        for dir in std::iter::once(obj_dir.clone()).chain(SUBDIRS.iter().map(|s| obj_dir.join(s))) {
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let out = self.kernel_dir.join(&self.out);
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        // The user programs first: their images are part of the kernel.
        let userprogs = self.userprogs(&sources)?;
        match &self.toolchain {
            Toolchain::Merlin => self.run_merlin(&sources, userprogs.as_deref())?,
            Toolchain::Clang {
                clang,
                lld,
                assembler,
            } => {
                for src in &sources {
                    if src.lang == Lang::Asm && *assembler == Assembler::Merlin {
                        self.merlin_as(src)?;
                    } else {
                        self.exec(clang, self.compile_args(src))?;
                    }
                }
                self.write_script()?;
                self.exec(lld, self.link_args(&sources))?;
            }
        }
        Ok(self.out.clone())
    }

    /// Writes [`linker_script`] to [`SCRIPT_OUT`] in the object directory,
    /// for ld.lld.
    fn write_script(&self) -> Result<(), String> {
        if self.verbose {
            let incs: Vec<String> = ASM_INCLUDE_DIRS.iter().map(|d| format!("-I {d}")).collect();
            eprintln!(
                "merlin -E -P {} {SCRIPT} -o {}",
                incs.join(" "),
                self.obj_dir.join(SCRIPT_OUT).display()
            );
        }
        let path = self.kernel_dir.join(&self.obj_dir).join(SCRIPT_OUT);
        std::fs::write(&path, linker_script(&self.kernel_dir)?)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// [`USERPROGS`]'s text, if the kernel has one: [`Build::userprogs`]'s, or
    /// generated from the user programs and written to [`Build::userprogs_path`]
    /// for the tools that read files, with each program's debug ELF in
    /// `user/<name>.elf` under the object directory.
    fn userprogs(&self, sources: &[Source]) -> Result<Option<String>, String> {
        if !sources.iter().any(Source::is_generated) {
            return Ok(None);
        }
        let path = self.kernel_dir.join(self.userprogs_path());
        if self.userprogs.is_some() {
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            return Ok(Some(text));
        }
        if self.verbose {
            eprintln!(
                "(every program in {USER_DIR}/, built with merlin, merlin-as and merlin-ld, as data) > {}",
                self.userprogs_path().display()
            );
        }
        let user = self.kernel_dir.join(USER_DIR);
        let programs = crate::userprogs::build_all_with(&user, crate::userprogs::CCompiler::Merlin)?;
        let text = crate::userprogs::generate(
            &crate::userprogs::embedded(&programs),
            crate::userprogs::CCompiler::Merlin,
        );
        std::fs::write(&path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        // The programs with their debug information, for merlin-dbg's
        // `add-symbol-file` (the kernel embeds them without).
        let dir = self.kernel_dir.join(&self.obj_dir).join(USER_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        for p in &programs {
            let file = dir.join(format!("{}.elf", p.name));
            std::fs::write(&file, &p.debug).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        }
        Ok(Some(text))
    }

    /// [`build_merlin`] with `userprogs` ([`Build::userprogs`]'s text), then
    /// writes merlin's assembly and the objects to the object directory (for
    /// merlin-objdump and for comparing with the reference build) and the
    /// kernel to the output.
    fn run_merlin(&self, sources: &[Source], userprogs: Option<&str>) -> Result<(), String> {
        if self.verbose {
            // What the same build is from the command line, inside kernel/.
            let obj = |name: String| self.obj_dir.join(name).display().to_string();
            for src in sources {
                let input = self.input(src).to_string_lossy().into_owned();
                if src.lang == Lang::C {
                    eprintln!(
                        "merlin -g -fdebug-compilation-dir={DEBUG_DIR} -I {INCLUDE_DIR} {input} -o {}",
                        obj(src.asm_name())
                    );
                    eprintln!("merlin-as {} -o {}", obj(src.asm_name()), obj(src.object_name()));
                } else if src.is_preprocessed() {
                    let incs: Vec<String> = ASM_INCLUDE_DIRS.iter().map(|d| format!("-I {d}")).collect();
                    eprintln!("merlin -E {} {input} -o {}", incs.join(" "), obj(src.asm_name()));
                    eprintln!("merlin-as {} -o {}", obj(src.asm_name()), obj(src.object_name()));
                } else {
                    eprintln!("merlin-as {input} -o {}", obj(src.object_name()));
                }
            }
        }
        // The script merlin-ld links with, for reading (and the command
        // line's merlin-ld).
        self.write_script()?;
        if self.verbose {
            let args: Vec<String> = self
                .link_args(sources)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            eprintln!("merlin-ld {}", args.join(" "));
        }
        let built = build_merlin(&self.kernel_dir, userprogs)?;
        let write = |path: PathBuf, bytes: &[u8]| {
            std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
        };
        let obj_dir = self.kernel_dir.join(&self.obj_dir);
        for o in &built.objects {
            if let Some(asm) = &o.asm {
                write(obj_dir.join(o.source.asm_name()), asm.as_bytes())?;
            }
            write(obj_dir.join(o.source.object_name()), &o.obj)?;
        }
        let out = self.kernel_dir.join(&self.out);
        write(out.clone(), &built.elf)?;
        // The same kernel without its programs, for bootable images.
        let disk = disk_kernel_path(&out);
        if self.verbose {
            eprintln!("merlin-ld (again, with no user programs) -o {}", disk.display());
        }
        write(disk, &disk_kernel(&self.kernel_dir, &built)?)
    }

    fn merlin_as(&self, src: &Source) -> Result<(), String> {
        let path = self.kernel_dir.join(self.input(src));
        if self.verbose {
            eprintln!("merlin-as {}", path.display());
        }
        let text = if src.is_generated() {
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?
        } else {
            assembly_of(&self.kernel_dir, src)?
        };
        let elf = toolchain::assemble(&src.name, &text)?;
        let obj = self.kernel_dir.join(&self.obj_dir).join(src.object_name());
        std::fs::write(&obj, elf).map_err(|e| format!("cannot write {}: {e}", obj.display()))
    }

    fn exec(&self, tool: &Path, args: Vec<OsString>) -> Result<(), String> {
        let line = || {
            let mut s = tool.display().to_string();
            for a in &args {
                s.push(' ');
                s.push_str(&a.to_string_lossy());
            }
            s
        };
        if self.verbose {
            eprintln!("{}", line());
        }
        let status = Command::new(tool)
            .args(&args)
            .current_dir(&self.kernel_dir)
            .status()
            .map_err(|e| format!("cannot run {}: {e}", tool.display()))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("command failed ({status}):\n  {}", line()))
        }
    }
}

/// Where the reference build goes, under the repository root: the user
/// programs' assembly from clang (`user/`), their `userprogs.s`, the
/// objects (`obj/`) and `kernel.elf`.
pub const REFERENCE_DIR: &str = "target/kernel/clang";

/// The reference build (`cargo xtask kernel --cc clang`) of the kernel under
/// `root`, into `work` (absolute): clang compiles the user programs' C to
/// assembly, merlin-as and merlin-ld build the programs from it as usual
/// (into `work/userprogs.s`), then clang compiles
/// the kernel's C, `assembler` assembles its assembly files and ld.lld links
/// `out`. Returns `out`.
pub fn build_reference(
    root: &Path,
    clang: &Path,
    lld: &Path,
    assembler: Assembler,
    work: &Path,
    out: &Path,
    verbose: bool,
) -> Result<PathBuf, String> {
    use crate::userprogs::{self, CCompiler};
    std::fs::create_dir_all(work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    let progs = work.join(USERPROGS);
    let user = userprogs::user_dir(root);
    let asm = work.join("user");
    userprogs::compile_clang(clang, &user, &asm, verbose)?;
    let programs = userprogs::build_all_with(&user, CCompiler::Clang(&asm))?;
    let text = userprogs::generate(&userprogs::embedded(&programs), CCompiler::Clang(&asm));
    std::fs::write(&progs, text).map_err(|e| format!("cannot write {}: {e}", progs.display()))?;
    Build {
        kernel_dir: root.join("kernel"),
        obj_dir: work.join("obj"),
        out: out.to_path_buf(),
        toolchain: Toolchain::Clang {
            clang: clang.to_path_buf(),
            lld: lld.to_path_buf(),
            assembler,
        },
        userprogs: Some(progs),
        verbose,
    }
    .run()
}

/// First line of `clang --version`, e.g. `clang version 21.1.5`: what the
/// reference build says it was made with.
pub fn clang_version(clang: &Path) -> Result<String, String> {
    let out = Command::new(clang)
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot run {}: {e}", clang.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| out.status.success() && !l.is_empty())
        .ok_or_else(|| format!("{} --version printed nothing", clang.display()))
}

/// A UART input channel that holds `bytes` and then ends, as if they had all
/// been typed before the machine started.
pub fn typed(bytes: &[u8]) -> Receiver<u8> {
    let (tx, rx) = mpsc::channel();
    for &b in bytes {
        tx.send(b).unwrap();
    }
    rx
}

/// Runs a kernel image on Grail ([`machine_config`]) with `input` waiting on
/// the UART (commands for the shell, which runs until it reads `exit`) and
/// the UART captured. Returns how it ended and everything it printed (UART
/// and `xcal.dbg`).
pub fn run_on_grail(elf: &[u8], input: &[u8], max_steps: u64) -> Result<(RunResult, String), String> {
    let mut m = Machine::new(machine_config())?;
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf)?;
    m.set_uart_input(typed(input));
    let r = m.run(Some(max_steps), |_| {});
    let out = std::mem::take(&mut m.cpu.bus.uart.captured);
    Ok((r, String::from_utf8_lossy(&out).into_owned()))
}

/// What `cargo xtask kernel --run` does once the image is built: boots `elf` on a
/// [`machine_config`] machine for at most `max_steps` steps (`None`: until it halts), with
/// the UART reading `input` and writing to `uart`, and `xcal.dbg` output going to
/// `debug`. Returns the result and the machine in its final state.
pub fn run_with_output(
    elf: &[u8],
    max_steps: Option<u64>,
    input: Receiver<u8>,
    uart: impl Write + Send + 'static,
    debug: impl Write + Send + 'static,
) -> Result<(RunResult, Machine), String> {
    let mut m = Machine::new(machine_config())?;
    m.load_elf(elf)?;
    m.set_uart_input(input);
    m.set_uart_output(uart);
    m.set_debug_output(debug);
    let r = m.run(max_steps, |_| {});
    Ok((r, m))
}

/// Replaces the program counter in `[dbg] pc=XXXXXXXXXXXXXXXX` lines with dots, so
/// output can be compared across builds whose code layout differs.
pub fn mask_dbg_pc(out: &str) -> String {
    out.split_inclusive('\n')
        .map(|line| match line.strip_prefix("[dbg] pc=") {
            Some(rest) if rest.len() >= 16 && rest.is_char_boundary(16) => {
                format!("[dbg] pc=................{}", &rest[16..])
            }
            _ => line.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> Build {
        Build {
            kernel_dir: PathBuf::from("kernel"),
            obj_dir: PathBuf::from("obj"),
            out: PathBuf::from("out/kernel.elf"),
            toolchain: Toolchain::Clang {
                clang: PathBuf::from("clang"),
                lld: PathBuf::from("ld.lld"),
                assembler: Assembler::Clang,
            },
            userprogs: None,
            verbose: false,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn lists_kernel_sources_in_order() {
        let srcs = sources(&crate::repo_root().join("kernel")).unwrap();
        let names: Vec<&str> = srcs.iter().map(|s| s.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        for want in ["entry.S", "kprintf.c", "main.c", "uart.c", USERPROGS] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        // userprogs.s is generated, not read from kernel/ (where it is not).
        let generated: Vec<&str> = srcs
            .iter()
            .filter(|s| s.is_generated())
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(generated, [USERPROGS]);
        assert!(!crate::repo_root().join("kernel").join(USERPROGS).exists());
        assert!(srcs
            .iter()
            .all(|s| (s.lang == Lang::Asm) == (s.name.ends_with(".s") || s.name.ends_with(".S"))));
        // The kernel's own assembly is preprocessed (one source for both
        // widths); only the generated userprogs.s is not.
        assert!(srcs
            .iter()
            .filter(|s| s.lang == Lang::Asm)
            .all(|s| s.is_preprocessed() != s.is_generated()));
        // The linker script (a .S file, but no assembly), kernel.elf and
        // include/ are not sources.
        assert!(crate::repo_root().join("kernel").join(SCRIPT).is_file());
        assert!(!names
            .iter()
            .any(|n| *n == SCRIPT || n.ends_with(".elf") || *n == "include"));
    }

    #[test]
    fn no_sources_is_an_error() {
        let dir = std::env::temp_dir().join(format!("xtask-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(sources(&dir).unwrap_err().contains("no .c, .S or .s files"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(sources(&dir).unwrap_err().contains("cannot read"));
    }

    #[test]
    fn object_names_keep_extension() {
        let c = Source {
            name: "x.c".into(),
            lang: Lang::C,
        };
        let s = Source {
            name: "x.s".into(),
            lang: Lang::Asm,
        };
        assert_ne!(c.object_name(), s.object_name());
        assert_eq!(c.object_name(), "x.c.o");
        assert_eq!(c.asm_name(), "x.c.s");
    }

    #[test]
    fn the_reference_build_can_embed_other_user_programs() {
        let mut b = build();
        let progs = Source {
            name: USERPROGS.into(),
            lang: Lang::Asm,
        };
        // By default the generated file, in the object directory (never in
        // the kernel directory).
        let generated = b.obj_dir.join(USERPROGS).display().to_string();
        let args = strings(&b.compile_args(&progs));
        assert!(args.contains(&generated), "{args:?}");
        assert!(!args.contains(&"userprogs.s".to_string()), "{args:?}");
        b.userprogs = Some(PathBuf::from("/elsewhere/userprogs.s"));
        let args = strings(&b.compile_args(&progs));
        assert!(!args.contains(&generated), "{args:?}");
        assert!(args.contains(&"/elsewhere/userprogs.s".to_string()), "{args:?}");
        // Other sources are unaffected.
        let args = strings(&b.compile_args(&Source {
            name: "entry.s".into(),
            lang: Lang::Asm,
        }));
        assert!(args.contains(&"entry.s".to_string()), "{args:?}");
    }

    #[test]
    fn merlin_build_writes_its_assembly_objects_and_kernel() {
        let dir = std::env::temp_dir().join(format!("xtask-merlin-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let b = Build {
            kernel_dir: crate::repo_root().join("kernel"),
            obj_dir: dir.join("obj"),
            out: dir.join("out").join("kernel.elf"),
            toolchain: Toolchain::Merlin,
            userprogs: None,
            verbose: false,
        };
        assert_eq!(b.run().unwrap(), b.out);
        let elf = std::fs::read(&b.out).unwrap();
        let built = build_merlin(&b.kernel_dir, None).unwrap();
        assert_eq!(elf, built.elf);
        // The disk kernel, next to it: the same kernel with no built-in programs
        // (`user_program_count` 0 where the full kernel has them all).
        let disk = std::fs::read(dir.join("out").join("kernel-disk.elf")).unwrap();
        let programs = |elf: &[u8]| {
            let mut m = Machine::new(machine_config()).unwrap();
            m.load_elf(elf).unwrap();
            // A link address in the upper half, loaded 0x8020_0000 on.
            let at = grail::elf::parse(elf)
                .unwrap()
                .symbol("user_program_count")
                .unwrap();
            m.cpu.bus.peek(at - 0xffff_ffff_8000_0000 + 0x8020_0000).unwrap()
        };
        assert!(programs(&elf) > 0);
        assert_eq!(programs(&disk), 0);
        assert!(disk.len() < elf.len());
        for f in [
            "main.c.s",
            "main.c.o",
            "entry.S.s",
            "entry.S.o",
            "userprogs.s",
            "userprogs.s.o",
        ] {
            assert!(b.obj_dir.join(f).is_file(), "{f}");
        }
        // The generated programs, written where the objects go.
        assert_eq!(
            std::fs::read_to_string(b.obj_dir.join(USERPROGS)).unwrap(),
            userprogs(&b.kernel_dir).unwrap()
        );
        assert!(!b.obj_dir.join("userprogs.s.s").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merlin_build_errors_name_the_file() {
        let dir = std::env::temp_dir().join(format!("xtask-merlin-errors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(INCLUDE_DIR)).unwrap();
        std::fs::write(dir.join("a.c"), "int f(void) { return g(); }\n").unwrap();
        let err = build_merlin(&dir, None).unwrap_err();
        assert!(err.starts_with("a.c:1:"), "{err}");
        std::fs::write(dir.join("a.c"), "int f(void) { return 0; }\n").unwrap();
        std::fs::write(dir.join("b.s"), "    bogus\n").unwrap();
        assert!(build_merlin(&dir, None).unwrap_err().starts_with("b.s:1: error"));
        std::fs::write(dir.join("b.s"), "    .globl _start\n_start:\n    call f\n").unwrap();
        let err = build_merlin(&dir, None).unwrap_err();
        assert!(err.contains("cannot read") && err.contains(SCRIPT), "{err}");
        std::fs::write(
            dir.join(SCRIPT),
            "ENTRY(_start)\nSECTIONS { . = 0x80000000; .text : { *(.text*) } }\n",
        )
        .unwrap();
        let built = build_merlin(&dir, None).unwrap();
        let names: Vec<&str> = built.objects.iter().map(|o| o.source.name.as_str()).collect();
        assert_eq!(names, ["a.c", "b.s"]);
        assert!(built.objects[0].asm.is_some() && built.objects[1].asm.is_none());
        // With a user/ directory, userprogs.s is generated from it, or given.
        std::fs::create_dir_all(dir.join(USER_DIR)).unwrap();
        let err = build_merlin(&dir, None).unwrap_err();
        assert!(err.contains("no programs in"), "{err}");
        let err = build_merlin(&dir, Some("    bogus\n")).unwrap_err();
        assert!(err.starts_with("userprogs.s:1: error"), "{err}");
        let built = build_merlin(&dir, Some("    .text\n    nop\n")).unwrap();
        let names: Vec<&str> = built.objects.iter().map(|o| o.source.name.as_str()).collect();
        assert_eq!(names, ["a.c", "b.s", USERPROGS]);
        // A stale userprogs.s file in the kernel directory is an error, not a
        // second copy of the programs.
        std::fs::write(dir.join(USERPROGS), "    .text\n    nop\n").unwrap();
        let err = build_merlin(&dir, None).unwrap_err();
        assert!(
            err.contains("generated at build time") && err.contains("delete"),
            "{err}"
        );
        assert!(sources(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn c_compile_line() {
        let b = build();
        let args = strings(&b.compile_args(&Source {
            name: "main.c".into(),
            lang: Lang::C,
        }));
        for flag in [
            "--target=riscv64-unknown-elf",
            "-march=rv64ima_zicsr",
            "-mabi=lp64",
            "-mno-relax",
            "-ffreestanding",
            "-nostdinc",
            "-nostdlib",
            "-Iinclude",
            "-c",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag} missing from {args:?}");
        }
        // The source is passed relative to the kernel directory, so the
        // object doesn't record where the repository lives.
        let c = args.iter().position(|a| a == "-c").unwrap();
        assert_eq!(args[c + 1], "main.c");
        assert_eq!(args[c + 2], "-o");
        assert_eq!(PathBuf::from(&args[c + 3]), Path::new("obj").join("main.c.o"));
    }

    #[test]
    fn asm_compile_line_has_no_c_flags() {
        let args = strings(&build().compile_args(&Source {
            name: "entry.s".into(),
            lang: Lang::Asm,
        }));
        assert!(args.iter().any(|a| a == "-mno-relax"));
        assert!(!args
            .iter()
            .any(|a| a == "-std=c11" || a == "-Werror" || a == "-mcmodel=medany"));
    }

    #[test]
    fn link_line() {
        let srcs = [
            Source {
                name: "entry.s".into(),
                lang: Lang::Asm,
            },
            Source {
                name: "main.c".into(),
                lang: Lang::C,
            },
        ];
        let args = strings(&build().link_args(&srcs));
        assert_eq!(args[0], "-T");
        assert_eq!(PathBuf::from(&args[1]), Path::new("obj").join("kernel.lds"));
        assert_eq!(args[2], "-o");
        assert_eq!(PathBuf::from(&args[3]), Path::new("out/kernel.elf"));
        assert_eq!(PathBuf::from(&args[4]), Path::new("obj").join("entry.s.o"));
        assert_eq!(PathBuf::from(&args[5]), Path::new("obj").join("main.c.o"));
        assert_eq!(args.len(), 6);
    }

    #[test]
    fn the_disk_kernel_goes_next_to_the_kernel() {
        assert_eq!(
            disk_kernel_path(Path::new("kernel/kernel.elf")),
            Path::new("kernel/kernel-disk.elf")
        );
        assert_eq!(disk_kernel_path(Path::new(KERNEL_ELF)), Path::new(DISK_KERNEL));
        assert_eq!(disk_kernel_path(Path::new("out/k")), Path::new("out/k-disk"));
        // Without a userprogs.s there is nothing to leave out.
        let built = Built {
            objects: Vec::new(),
            elf: Vec::new(),
        };
        let err = disk_kernel(Path::new("kernel"), &built).unwrap_err();
        assert!(err.contains("no userprogs.s"), "{err}");
    }

    #[test]
    fn masks_dbg_pc() {
        let out = "hi\n[dbg] pc=ffffffff80000123 a0 = 0x0000000000000000 (0)\n[dbg] pc=8\n";
        assert_eq!(
            mask_dbg_pc(out),
            "hi\n[dbg] pc=................ a0 = 0x0000000000000000 (0)\n[dbg] pc=8\n"
        );
        assert_eq!(mask_dbg_pc(""), "");
    }

    #[test]
    fn missing_tool_is_reported() {
        let mut b = build();
        b.kernel_dir = crate::repo_root().join("kernel");
        b.obj_dir = std::env::temp_dir().join(format!("xtask-missing-tool-{}", std::process::id()));
        b.out = b.obj_dir.join("kernel.elf");
        b.toolchain = Toolchain::Clang {
            clang: b.obj_dir.join("no-such-clang"),
            lld: b.obj_dir.join("no-such-lld"),
            assembler: Assembler::Clang,
        };
        let err = b.run().unwrap_err();
        assert!(err.contains("cannot run"), "{err}");
        let _ = std::fs::remove_dir_all(&b.obj_dir);
    }
}
