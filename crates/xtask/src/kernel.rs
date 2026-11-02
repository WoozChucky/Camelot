//! Building and running the Camelot kernel (`kernel/`).
//!
//! Every `*.c` and `*.s` file directly in `kernel/` or in one of its source
//! subdirectories ([`SUBDIRS`], e.g. `net/arp.c`) is one object, and the
//! objects are linked in name order with `kernel/kernel.ld`. Our own tools
//! build it ([`Toolchain::Merlin`], [`build_merlin`]): merlin compiles each C
//! file, merlin-as assembles it and the `.s` files and merlin-ld links them,
//! all in memory, with no LLVM. That is how the committed
//! `kernel/kernel.elf` is made, and `cargo test` checks it is what the
//! current sources and tools give.
//!
//! The reference build ([`Toolchain::Clang`], `cargo xtask kernel --cc
//! clang`) compiles the C with clang `-O2`, assembles with clang (or
//! merlin-as) and links with ld.lld, so the two can be compared. Either way
//! every tool sees paths relative to `kernel/` (clang runs inside it), so the
//! output doesn't depend on where the repository is checked out.

use crate::toolchain;
use grail::{Machine, RunResult};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};

/// Flags shared by every clang invocation: the Excalibur target. No linker
/// relaxation, since merlin-as and merlin-ld won't do it either.
pub const TARGET_FLAGS: &[&str] = &[
    "--target=riscv32-unknown-elf",
    "-march=rv32im_zicsr",
    "-mabi=ilp32",
    "-mno-relax",
];

/// Extra flags for C files in the reference build: freestanding, no libc
/// headers, no hidden runtime (stack protector, unwind tables), and no
/// `.comment` with the clang version. `medany` addresses globals
/// pc-relatively (`auipc`), which works for a kernel linked at 0x8000_0000
/// (merlin always does).
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

/// Generous step budget for running the kernel non-interactively. Booting
/// the kernel with the default 128 MiB of RAM takes about 36 million steps
/// (6 million for the clang `-O2` reference build: merlin does not
/// optimize), nearly all of them spent putting every page on the free list,
/// mapping RAM and MMIO page by page, and the allocator self-test that takes
/// every page and gives it back; mounting the root filesystem and checking
/// `/bin` takes about 30 million more, and a few shell commands a few
/// hundred thousand.
pub const BOOT_MAX_STEPS: u64 = 200_000_000;

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
/// The kernel's linker script, in the kernel directory.
pub const SCRIPT: &str = "kernel.ld";
/// The generated source that holds the user programs
/// (`crate::userprogs::GENERATED`), in the kernel directory.
pub const USERPROGS: &str = "userprogs.s";

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
}

/// The subdirectories of the kernel directory whose `*.c` and `*.s` files
/// are kernel sources too, named `dir/file` (the others hold the user
/// programs, the bootloader, the headers and the root image's files).
pub const SUBDIRS: &[&str] = &["net"];

/// Lists the kernel's sources (`*.c`, `*.s`, in `kernel_dir` and its
/// [`SUBDIRS`]) in name order.
pub fn sources(kernel_dir: &Path) -> Result<Vec<Source>, String> {
    let mut out = Vec::new();
    list_sources(kernel_dir, "", &mut out)?;
    for sub in SUBDIRS {
        let dir = kernel_dir.join(sub);
        if dir.is_dir() {
            list_sources(&dir, &format!("{sub}/"), &mut out)?;
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    if out.is_empty() {
        return Err(format!("no .c or .s files in {}", kernel_dir.display()));
    }
    Ok(out)
}

/// Adds the `*.c` and `*.s` files of `dir` to `out`, named `prefix` + their
/// file name.
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
            Some("s") => Lang::Asm,
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
    /// merlin's assembly, for a C file.
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

/// Builds the kernel in `kernel_dir` with merlin, merlin-as and merlin-ld, in
/// memory: merlin compiles each C file (`-I include`), merlin-as assembles
/// that and each `.s` file, and merlin-ld links the objects in name order
/// with `kernel.ld`. `userprogs`, if given, is assembled instead of
/// `kernel_dir/userprogs.s`. No LLVM; nothing is written.
pub fn build_merlin(kernel_dir: &Path, userprogs: Option<&Path>) -> Result<Built, String> {
    let mut objects = Vec::new();
    for source in sources(kernel_dir)? {
        let (asm, obj) = match source.lang {
            Lang::C => {
                let asm = toolchain::compile_c(kernel_dir, &source.name, &[INCLUDE_DIR])?;
                let obj = toolchain::assemble(&format!("{} (merlin's assembly)", source.asm_name()), &asm)?;
                (Some(asm), obj)
            }
            Lang::Asm => {
                let path = match userprogs {
                    Some(p) if source.name == USERPROGS => p.to_path_buf(),
                    _ => kernel_dir.join(&source.name),
                };
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                (None, toolchain::assemble(&source.name, &text)?)
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
    let elf = toolchain::link(kernel_dir, SCRIPT, &inputs, "the kernel")?;
    Ok(Built { objects, elf })
}

/// Everything needed for one kernel build. The tools run with `kernel_dir` as
/// their working directory, so relative `obj_dir`/`out` paths are taken
/// relative to it; pass absolute ones.
#[derive(Clone, Debug)]
pub struct Build {
    /// Directory holding the sources, `include/` and `kernel.ld`.
    pub kernel_dir: PathBuf,
    /// Where object files (and merlin's assembly) go.
    pub obj_dir: PathBuf,
    /// The linked ELF.
    pub out: PathBuf,
    pub toolchain: Toolchain,
    /// Assembled instead of `kernel_dir/userprogs.s`: the reference build
    /// embeds user programs built from clang's assembly.
    pub userprogs: Option<PathBuf>,
    /// Print each command (or its command-line equivalent) before running it.
    pub verbose: bool,
}

impl Build {
    /// What a tool reads for `src`, relative to the kernel directory unless
    /// [`Build::userprogs`] replaces it.
    fn input(&self, src: &Source) -> OsString {
        match &self.userprogs {
            Some(p) if src.name == USERPROGS => p.clone().into(),
            _ => src.name.clone().into(),
        }
    }

    /// clang arguments for one source (run with the kernel directory as cwd).
    pub fn compile_args(&self, src: &Source) -> Vec<OsString> {
        let mut args: Vec<OsString> = TARGET_FLAGS.iter().map(OsString::from).collect();
        if src.lang == Lang::C {
            args.extend(C_FLAGS.iter().map(OsString::from));
        }
        args.push("-c".into());
        args.push(self.input(src));
        args.push("-o".into());
        args.push(self.obj_dir.join(src.object_name()).into());
        args
    }

    /// Linker arguments (run with the kernel directory as cwd): ld.lld's, and
    /// merlin-ld's, which takes the same.
    pub fn link_args(&self, sources: &[Source]) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["-T".into(), SCRIPT.into(), "-o".into(), self.out.clone().into()];
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
        match &self.toolchain {
            Toolchain::Merlin => self.run_merlin(&sources)?,
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
                self.exec(lld, self.link_args(&sources))?;
            }
        }
        Ok(self.out.clone())
    }

    /// [`build_merlin`], then writes merlin's assembly and the objects to
    /// the object directory (for merlin-objdump and for comparing with the
    /// reference build) and the kernel to the output.
    fn run_merlin(&self, sources: &[Source]) -> Result<(), String> {
        if self.verbose {
            // What the same build is from the command line, inside kernel/.
            let obj = |name: String| self.obj_dir.join(name).display().to_string();
            for src in sources {
                let input = self.input(src).to_string_lossy().into_owned();
                if src.lang == Lang::C {
                    eprintln!("merlin -I {INCLUDE_DIR} {input} -o {}", obj(src.asm_name()));
                    eprintln!("merlin-as {} -o {}", obj(src.asm_name()), obj(src.object_name()));
                } else {
                    eprintln!("merlin-as {input} -o {}", obj(src.object_name()));
                }
            }
            let args: Vec<String> = self
                .link_args(sources)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            eprintln!("merlin-ld {}", args.join(" "));
        }
        let userprogs = self.userprogs.as_ref().map(|p| self.kernel_dir.join(p));
        let built = build_merlin(&self.kernel_dir, userprogs.as_deref())?;
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
        write(self.kernel_dir.join(&self.out), &built.elf)
    }

    fn merlin_as(&self, src: &Source) -> Result<(), String> {
        let path = self.kernel_dir.join(self.input(src));
        if self.verbose {
            eprintln!("merlin-as {}", path.display());
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
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
/// (into `work/userprogs.s`, not the committed file), then clang compiles
/// the kernel's C, `assembler` assembles its `.s` files and ld.lld links
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
    let user = userprogs::user_dir(root);
    let asm = work.join("user");
    userprogs::compile_clang(clang, &user, &asm, verbose)?;
    let programs = userprogs::build_all_with(&user, CCompiler::Clang(&asm))?;
    let progs = work.join(USERPROGS);
    std::fs::write(&progs, userprogs::generate(&programs, CCompiler::Clang(&asm)))
        .map_err(|e| format!("cannot write {}: {e}", progs.display()))?;
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

/// Runs a kernel image on Grail with `input` waiting on the UART (commands
/// for the shell, which runs until it reads `exit`) and the UART
/// captured. Returns how it ended and everything it printed (UART and
/// `xcal.dbg`).
pub fn run_on_grail(elf: &[u8], input: &[u8], max_steps: u64) -> Result<(RunResult, String), String> {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf)?;
    m.set_uart_input(typed(input));
    let r = m.run(Some(max_steps), |_| {});
    let out = std::mem::take(&mut m.cpu.bus.uart.captured);
    Ok((r, String::from_utf8_lossy(&out).into_owned()))
}

/// What `cargo xtask kernel --run` does once the image is built: boots `elf` on a
/// default Grail machine for at most `max_steps` steps (`None`: until it halts), with
/// the UART reading `input` and writing to `uart`, and `xcal.dbg` output going to
/// `debug`. Returns the result and the machine in its final state.
pub fn run_with_output(
    elf: &[u8],
    max_steps: Option<u64>,
    input: Receiver<u8>,
    uart: impl Write + Send + 'static,
    debug: impl Write + Send + 'static,
) -> Result<(RunResult, Machine), String> {
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(elf)?;
    m.set_uart_input(input);
    m.set_uart_output(uart);
    m.set_debug_output(debug);
    let r = m.run(max_steps, |_| {});
    Ok((r, m))
}

/// Replaces the program counter in `[dbg] pc=XXXXXXXX` lines with dots, so
/// output can be compared across builds whose code layout differs.
pub fn mask_dbg_pc(out: &str) -> String {
    out.split_inclusive('\n')
        .map(|line| match line.strip_prefix("[dbg] pc=") {
            Some(rest) if rest.len() >= 8 && rest.is_char_boundary(8) => {
                format!("[dbg] pc=........{}", &rest[8..])
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
        for want in ["entry.s", "kprintf.c", "main.c", "uart.c"] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        assert!(srcs
            .iter()
            .all(|s| (s.lang == Lang::Asm) == s.name.ends_with(".s")));
        // kernel.ld, kernel.elf and include/ are not sources.
        assert!(!names
            .iter()
            .any(|n| n.ends_with(".ld") || n.ends_with(".elf") || *n == "include"));
    }

    #[test]
    fn no_sources_is_an_error() {
        let dir = std::env::temp_dir().join(format!("xtask-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(sources(&dir).unwrap_err().contains("no .c or .s files"));
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
        let args = strings(&b.compile_args(&progs));
        assert!(args.contains(&"userprogs.s".to_string()), "{args:?}");
        b.userprogs = Some(PathBuf::from("/elsewhere/userprogs.s"));
        let args = strings(&b.compile_args(&progs));
        assert!(!args.contains(&"userprogs.s".to_string()), "{args:?}");
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
        assert_eq!(elf, build_merlin(&b.kernel_dir, None).unwrap().elf);
        for f in ["main.c.s", "main.c.o", "entry.s.o", "userprogs.s.o"] {
            assert!(b.obj_dir.join(f).is_file(), "{f}");
        }
        assert!(!b.obj_dir.join("entry.s.s").exists());
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
        // userprogs.s can be replaced by another file.
        std::fs::write(dir.join(USERPROGS), "    bogus\n").unwrap();
        assert!(build_merlin(&dir, None).is_err());
        std::fs::write(dir.join("other.s"), "    .text\n    nop\n").unwrap();
        assert!(build_merlin(&dir, Some(&dir.join("other.s"))).is_ok());
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
            "--target=riscv32-unknown-elf",
            "-march=rv32im_zicsr",
            "-mabi=ilp32",
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
        assert_eq!(args[..3], ["-T", "kernel.ld", "-o"]);
        assert_eq!(PathBuf::from(&args[3]), Path::new("out/kernel.elf"));
        assert_eq!(PathBuf::from(&args[4]), Path::new("obj").join("entry.s.o"));
        assert_eq!(PathBuf::from(&args[5]), Path::new("obj").join("main.c.o"));
        assert_eq!(args.len(), 6);
    }

    #[test]
    fn masks_dbg_pc() {
        let out = "hi\n[dbg] pc=80000123 a0 = 0x00000000 (0)\n[dbg] pc=8\n";
        assert_eq!(
            mask_dbg_pc(out),
            "hi\n[dbg] pc=........ a0 = 0x00000000 (0)\n[dbg] pc=8\n"
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
