//! The user programs embedded in the kernel (`kernel/user/`, spec section 10,
//! "Kernel processes").
//!
//! Every `kernel/user/*.c` and `*.s` is one program, but for the runtime
//! linked into each of them ([`RUNTIME`]: `crt0.s` and the system call stubs
//! `usys.s`), and so is every subdirectory but `lib/`: a program of several
//! files (`kernel/user/as/*.c` is `as`), each compiled and assembled on its
//! own and linked together, in name order. The C library, `kernel/user/lib/` ([`LIB_DIR`]), is linked as
//! a library: each of its files is a member that goes into a program only
//! if the program needs something it defines (`merlin_ld::library`, like
//! an archive's members), so a program that never calls malloc carries
//! neither its code nor its data. Our own tools build them, in memory, with
//! no LLVM: merlin compiles each C file to assembly, merlin-as assembles
//! that and the `.s` files, and merlin-ld links each program with the
//! runtime, the library members it needs and `kernel/user/user.ld`. The
//! runtime and the library are built once for all the programs. The
//! executables then go into the kernel's `userprogs.s` ([`generate`]) as ELF
//! images in `.rodata`, with a table the kernel's loader looks programs up
//! in.
//!
//! That assembly is generated every time the kernel is built
//! (`crate::kernel::USERPROGS`), in memory, and is not committed: its
//! contents are in the committed `kernel/kernel.elf`, which `cargo test`
//! checks is what the current sources and tools give. `cargo xtask kernel`
//! leaves a copy next to the kernel's objects (`target/kernel/obj`). The
//! reference build, `cargo xtask kernel --cc clang`, has clang
//! compile the C files to assembly instead ([`compile_clang`], into a
//! scratch directory) and builds the same programs from that
//! ([`build_all_with`], [`CCompiler::Clang`]).

use crate::kernel::TARGET_FLAGS;
use crate::toolchain;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The user programs' directory, relative to the repository root.
pub const USER_DIR: &str = "kernel/user";
/// The runtime linked into every program (none of them is a program).
pub const RUNTIME: &[&str] = &["crt0.s", "usys.s"];
/// The C library's directory, under [`USER_DIR`]: every `.c` and `.s` file
/// in it is a library member.
pub const LIB_DIR: &str = "lib";
/// The linker script every program is linked with.
pub const SCRIPT: &str = "user.ld";
/// Programs the root image's `/bin` holds but the kernel does not build in:
/// they read and write files, which a machine without a disk has none of,
/// and their copies would not fit in the 2 MiB the kernel boots in (`as` is
/// over 100 KB).
pub const DISK_ONLY: &[&str] = &["as"];

/// The programs the kernel builds in: all but [`DISK_ONLY`], by name.
pub fn embedded(programs: &[Program]) -> Vec<&Program> {
    programs
        .iter()
        .filter(|p| !DISK_ONLY.contains(&p.name.as_str()))
        .collect()
}

/// The names of the programs the kernel builds in.
pub fn embedded_names(dir: &Path) -> Result<Vec<String>, String> {
    Ok(program_names(dir)?
        .into_iter()
        .filter(|n| !DISK_ONLY.contains(&n.as_str()))
        .collect())
}

/// The longest program name: the kernel keeps 15 characters and a NUL
/// (`struct proc`'s `name`, kernel/include/proc.h).
pub const MAX_NAME: usize = 15;

/// clang's flags for the user C files in the reference build, after
/// [`TARGET_FLAGS`]: like the kernel's (freestanding, no libc headers, no
/// hidden runtime, nothing that records the clang version), with `user.h`
/// from the same directory. `-S`: the output is assembly, for merlin-as.
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
    "-S",
];

/// The language of a source file in [`USER_DIR`], or a program's directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    C,
    Asm,
    /// A program of several files, in the subdirectory of its name: the
    /// `.c` and `.s` files there are its parts ([`Source::parts`]).
    Dir,
}

/// A source file in [`USER_DIR`]: a program's, the runtime's or the
/// library's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The file's stem, relative to [`USER_DIR`]: for a program, its name;
    /// for a library member, `lib/` and its name.
    pub stem: String,
    pub lang: Lang,
}

impl Source {
    /// The source's file name, e.g. `sh.c`.
    pub fn file(&self) -> String {
        match self.lang {
            Lang::C => format!("{}.c", self.stem),
            Lang::Asm => format!("{}.s", self.stem),
            Lang::Dir => format!("{}/", self.stem),
        }
    }

    /// The files a program is built from: itself, or, for a [`Lang::Dir`]
    /// program, the `.c` and `.s` files of its directory under `dir`, by
    /// name, their stems starting with the directory's (`as/lexer`).
    pub fn parts(&self, dir: &Path) -> Result<Vec<Source>, String> {
        if self.lang != Lang::Dir {
            return Ok(vec![self.clone()]);
        }
        let parts: Vec<Source> = sources(&dir.join(&self.stem))?
            .into_iter()
            .map(|s| Source {
                stem: format!("{}/{}", self.stem, s.stem),
                lang: s.lang,
            })
            .collect();
        if parts.is_empty() {
            return Err(format!("{}: no .c or .s files", dir.join(&self.stem).display()));
        }
        Ok(parts)
    }

    fn from_file(file: &str) -> Option<Source> {
        if let Some(stem) = file.strip_suffix(".c") {
            Some(Source {
                stem: stem.to_string(),
                lang: Lang::C,
            })
        } else {
            file.strip_suffix(".s").map(|stem| Source {
                stem: stem.to_string(),
                lang: Lang::Asm,
            })
        }
    }
}

/// What turns the C files into assembly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CCompiler<'a> {
    /// merlin, in memory: how the programs are built.
    Merlin,
    /// The reference build: clang already compiled each `X.c` to `X.s` in
    /// this directory ([`compile_clang`]).
    Clang(&'a Path),
}

/// A linked user program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The source's stem: what the shell calls it.
    pub name: String,
    /// The executable merlin-ld wrote, without its debug information: what
    /// the kernel embeds and `/bin` holds.
    pub elf: Vec<u8>,
    /// The same with its debug information (the line tables of `-g`), for
    /// a debugger: `cargo xtask kernel` writes it to `user/<name>.elf` in the
    /// object directory.
    pub debug: Vec<u8>,
}

/// `kernel/user` under `root`.
pub fn user_dir(root: &Path) -> PathBuf {
    root.join(USER_DIR)
}

/// Whether `name` can be a program name: 1 to [`MAX_NAME`] lowercase letters,
/// digits and underscores (it becomes part of a label in the generated file).
pub fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Every `.c` and `.s` file directly in `dir`, the runtime's included, by
/// file name.
pub fn sources(dir: &Path) -> Result<Vec<Source>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for e in entries {
        let e = e.map_err(|e| e.to_string())?;
        if !e.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let file = e.file_name();
        if let Some(src) = file.to_str().and_then(Source::from_file) {
            out.push(src);
        }
    }
    out.sort_by_key(Source::file);
    Ok(out)
}

/// The library's members in `dir` (`dir/lib`), by file name, their stems
/// starting with `lib/`. There may be none.
pub fn library(dir: &Path) -> Result<Vec<Source>, String> {
    let lib = dir.join(LIB_DIR);
    if !lib.is_dir() {
        return Ok(Vec::new());
    }
    Ok(sources(&lib)?
        .into_iter()
        .map(|s| Source {
            stem: format!("{LIB_DIR}/{}", s.stem),
            lang: s.lang,
        })
        .collect())
}

/// The programs in `dir`: every source but the [`RUNTIME`]'s, and every
/// subdirectory but [`LIB_DIR`] that holds `.c` or `.s` files, by name. A
/// name must be [`valid_name`] and belong to one source only.
pub fn programs(dir: &Path) -> Result<Vec<Source>, String> {
    let mut progs: Vec<Source> = sources(dir)?
        .into_iter()
        .filter(|s| !RUNTIME.contains(&s.file().as_str()))
        .collect();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for e in entries {
        let e = e.map_err(|e| e.to_string())?;
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if name == LIB_DIR || !e.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        if !sources(&e.path())?.is_empty() {
            progs.push(Source {
                stem: name,
                lang: Lang::Dir,
            });
        }
    }
    for s in &progs {
        if !valid_name(&s.stem) {
            return Err(format!(
                "{}: a program name is 1 to {MAX_NAME} of a-z, 0-9 and _",
                dir.join(s.file()).display()
            ));
        }
    }
    progs.sort_by(|a, b| a.stem.cmp(&b.stem));
    if let Some(w) = progs.windows(2).find(|w| w[0].stem == w[1].stem) {
        return Err(format!(
            "{}: both {} and {} define program {}",
            dir.display(),
            w[0].file(),
            w[1].file(),
            w[0].stem
        ));
    }
    if progs.is_empty() {
        return Err(format!("no programs in {}", dir.display()));
    }
    Ok(progs)
}

/// The programs' names in `dir`, in order.
pub fn program_names(dir: &Path) -> Result<Vec<String>, String> {
    Ok(programs(dir)?.into_iter().map(|s| s.stem).collect())
}

/// The assembly for `src` (in `dir`): the file itself, merlin's output for
/// a C file, or clang's. Also returns the name errors in it are shown under.
fn asm_text(dir: &Path, src: &Source, cc: CCompiler) -> Result<(String, String), String> {
    let read = |path: &Path| {
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    };
    match (src.lang, cc) {
        (Lang::Asm, _) => Ok((format!("{USER_DIR}/{}", src.file()), read(&dir.join(src.file()))?)),
        (Lang::C, CCompiler::Merlin) => {
            // Errors read `kernel/user/x.c:line:col: error: ...`.
            let asm = toolchain::compile_c(dir, &src.file(), &[], USER_DIR).map_err(|e| {
                e.lines()
                    .map(|l| format!("{USER_DIR}/{l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })?;
            Ok((format!("{USER_DIR}/{}.s (merlin's assembly)", src.file()), asm))
        }
        (Lang::Dir, _) => unreachable!("a program's parts are files"),
        (Lang::C, CCompiler::Clang(asm_dir)) => {
            let path = asm_dir.join(format!("{}.s", src.stem));
            Ok((path.display().to_string(), read(&path)?))
        }
    }
}

/// The runtime's sources.
fn runtime() -> Vec<Source> {
    RUNTIME.iter().filter_map(|f| Source::from_file(f)).collect()
}

/// `src` (in `dir`) compiled by `cc` if it is C, and assembled.
fn object(dir: &Path, src: &Source, cc: CCompiler) -> Result<merlin_ld::Input, String> {
    let (shown, text) = asm_text(dir, src, cc)?;
    Ok(merlin_ld::Input {
        name: format!("{}.o", src.file()),
        data: toolchain::assemble(&shown, &text)?,
    })
}

/// What every program is linked with: the runtime's objects, and the
/// library's, of which each program takes the ones it needs.
pub struct Runtime {
    pub objects: Vec<merlin_ld::Input>,
    pub library: Vec<merlin_ld::Input>,
}

/// Builds the [`RUNTIME`] and the library in `dir`, their C compiled by
/// `cc`.
pub fn build_runtime(dir: &Path, cc: CCompiler) -> Result<Runtime, String> {
    let objects = runtime()
        .iter()
        .map(|s| object(dir, s, cc))
        .collect::<Result<_, _>>()?;
    let library = library(dir)?
        .iter()
        .map(|s| object(dir, s, cc))
        .collect::<Result<_, _>>()?;
    Ok(Runtime { objects, library })
}

/// Links program `prog` from `dir`: it compiled by `cc` (C) and assembled,
/// then the runtime's objects and the library members it needs, linked
/// with [`SCRIPT`], the program first. merlin-ld's warnings are errors
/// here.
pub fn link_program(dir: &Path, prog: &Source, cc: CCompiler, rt: &Runtime) -> Result<Program, String> {
    let mut inputs = Vec::new();
    for part in prog.parts(dir)? {
        inputs.push(object(dir, &part, cc)?);
    }
    inputs.extend(rt.objects.iter().cloned());
    let inputs = merlin_ld::library::with_needed(&inputs, &rt.library).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(|e| format!("merlin-ld: error: {e}")).collect();
        format!("linking {}:\n{}", prog.stem, lines.join("\n"))
    })?;
    let debug = toolchain::link(dir, SCRIPT, &inputs, &prog.stem)?;
    Ok(Program {
        name: prog.stem.clone(),
        elf: merlin_ld::strip_debug(&debug).map_err(|e| format!("{}: {e}", prog.stem))?,
        debug,
    })
}

/// Builds program `prog` from `dir`, its C and the runtime's compiled by
/// `cc` ([`link_program`]).
pub fn build_program_with(dir: &Path, prog: &Source, cc: CCompiler) -> Result<Program, String> {
    link_program(dir, prog, cc, &build_runtime(dir, cc)?)
}

/// Builds program `prog` from `dir` with our own tools.
pub fn build_program(dir: &Path, prog: &Source) -> Result<Program, String> {
    build_program_with(dir, prog, CCompiler::Merlin)
}

/// Builds the program called `name` in `dir`.
pub fn build_named(dir: &Path, name: &str) -> Result<Program, String> {
    let prog = programs(dir)?
        .into_iter()
        .find(|s| s.stem == name)
        .ok_or_else(|| format!("no program called {name} in {}", dir.display()))?;
    build_program(dir, &prog)
}

/// Builds every program in `dir`, by name, its C compiled by `cc`; the
/// runtime and the library once for all of them.
pub fn build_all_with(dir: &Path, cc: CCompiler) -> Result<Vec<Program>, String> {
    let progs = programs(dir)?;
    let rt = build_runtime(dir, cc)?;
    progs
        .iter()
        .map(|prog| link_program(dir, prog, cc, &rt))
        .collect()
}

/// Builds every program in `dir` with our own tools, by name.
pub fn build_all(dir: &Path) -> Result<Vec<Program>, String> {
    build_all_with(dir, CCompiler::Merlin)
}

/// clang's arguments for `src` (run in [`USER_DIR`]), writing to `out`.
pub fn clang_args(src: &Source, out: &Path) -> Vec<String> {
    let mut args: Vec<String> = TARGET_FLAGS
        .iter()
        .chain(C_FLAGS)
        .map(|s| s.to_string())
        .collect();
    args.push(src.file());
    args.push("-o".into());
    args.push(out.display().to_string());
    args
}

/// The reference build's first step: compiles every C file in `dir` and
/// its library with `clang` to assembly, `X.c` to `out/X.s` and `lib/Y.c`
/// to `out/lib/Y.s`, for [`CCompiler::Clang`]. clang runs inside `dir` with
/// relative names, so the output doesn't depend on where the repository
/// is.
pub fn compile_clang(clang: &Path, dir: &Path, out: &Path, verbose: bool) -> Result<(), String> {
    std::fs::create_dir_all(out.join(LIB_DIR))
        .map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let out = std::path::absolute(out).map_err(|e| format!("bad path {}: {e}", out.display()))?;
    let mut all: Vec<Source> = sources(dir)?.into_iter().chain(library(dir)?).collect();
    for prog in programs(dir)?.iter().filter(|p| p.lang == Lang::Dir) {
        std::fs::create_dir_all(out.join(&prog.stem))
            .map_err(|e| format!("cannot create {}: {e}", out.display()))?;
        all.extend(prog.parts(dir)?);
    }
    for src in all.iter().filter(|s| s.lang == Lang::C) {
        let src = src.clone();
        let args = clang_args(&src, &out.join(format!("{}.s", src.stem)));
        if verbose {
            eprintln!("{} {}", clang.display(), args.join(" "));
        }
        let status = Command::new(clang)
            .args(&args)
            .current_dir(dir)
            .status()
            .map_err(|e| format!("cannot run {}: {e}", clang.display()))?;
        if !status.success() {
            return Err(format!("clang failed ({status}) on {USER_DIR}/{}", src.file()));
        }
    }
    Ok(())
}

/// Words of zeros from which a run is written as `.zero` rather than as
/// `.word 0x00000000`s. The images are mostly zeros: merlin-ld puts each
/// segment at a file offset congruent to its address modulo 4 KiB.
const ZERO_RUN_WORDS: usize = 4;
/// `.word`s per line.
const WORDS_PER_LINE: usize = 8;

/// Writes `bytes` as `.word`, `.zero` and (for the last 1-3 bytes) `.byte`
/// directives, which merlin-as and clang both assemble to the same bytes.
fn emit_bytes(out: &mut String, bytes: &[u8]) {
    let (chunks, tail) = bytes.as_chunks::<4>();
    let words: Vec<u32> = chunks.iter().map(|&c| u32::from_le_bytes(c)).collect();
    let mut line: Vec<String> = Vec::new();
    let flush = |out: &mut String, line: &mut Vec<String>| {
        if !line.is_empty() {
            let _ = writeln!(out, "    .word {}", line.join(", "));
            line.clear();
        }
    };
    let mut i = 0;
    while i < words.len() {
        let zeros = words[i..].iter().take_while(|&&w| w == 0).count();
        if zeros >= ZERO_RUN_WORDS {
            flush(out, &mut line);
            let _ = writeln!(out, "    .zero {}", 4 * zeros);
            i += zeros;
            continue;
        }
        line.push(format!("{:#010x}", words[i]));
        if line.len() == WORDS_PER_LINE {
            flush(out, &mut line);
        }
        i += 1;
    }
    flush(out, &mut line);
    if !tail.is_empty() {
        let tail: Vec<String> = tail.iter().map(|b| format!("{b:#04x}")).collect();
        let _ = writeln!(out, "    .byte {}", tail.join(", "));
    }
}

/// The text of the kernel's `userprogs.s` for `programs`: the table
/// `user_programs` (one `struct user_program { const char *name; const u8
/// *elf; u32 size; }` per program, in order), `user_program_count`, then the
/// names and the images, each image 4-byte aligned under a global label
/// `user_elf_<name>`. `cc` is what compiled their C, which the header says.
pub fn generate<P: std::borrow::Borrow<Program>>(programs: &[P], cc: CCompiler) -> String {
    let programs: &[&Program] = &programs.iter().map(|p| p.borrow()).collect::<Vec<_>>();
    let mut s = String::new();
    let cc = match cc {
        CCompiler::Merlin => "merlin",
        CCompiler::Clang(_) => "clang (the reference build)",
    };
    let _ = write!(
        s,
        "# Generated by `cargo xtask kernel` from kernel/user/: do not edit.\n\
         #\n\
         # The user programs as ELF images for the kernel's loader: each one's C\n\
         # compiled by {cc}, assembled by merlin-as and linked by merlin-ld with\n\
         # the runtime (crt0.s, usys.s), the members of the C library (lib/) it\n\
         # needs and kernel/user/user.ld (crates/xtask/src/userprogs.rs). The\n\
         # table is the array\n\
         # kernel/include/proc.h declares:\n\
         #   struct user_program {{ const char *name; const u8 *elf; u32 size; }};\n\
         \n\
         \x20   .section .rodata\n\
         \x20   .align 2\n\
         \x20   .globl user_programs\n\
         user_programs:\n"
    );
    for p in programs {
        let _ = writeln!(
            s,
            "    .word user_name_{0}, user_elf_{0}, {1}",
            p.name,
            p.elf.len()
        );
    }
    let _ = write!(
        s,
        "\n    .globl user_program_count\nuser_program_count:\n    .word {}\n",
        programs.len()
    );
    for p in programs {
        let _ = write!(s, "\nuser_name_{0}:\n    .asciz \"{0}\"\n", p.name);
    }
    for p in programs {
        let _ = write!(
            s,
            "\n    .align 2\n    .globl user_elf_{0}\nuser_elf_{0}:\n",
            p.name
        );
        emit_bytes(&mut s, &p.elf);
    }
    s
}

/// The `userprogs.s` of the disk kernel (`crate::kernel::disk_kernel`): the same
/// table and count as [`generate`]'s, with no programs, so the kernel's loader
/// finds none and runs everything from `/bin`.
pub fn empty() -> String {
    "# The disk kernel's user programs: none (crates/xtask/src/userprogs.rs).\n\
     # It runs every program from /bin on its root filesystem.\n\
     \n\
     \x20   .section .rodata\n\
     \x20   .align 2\n\
     \x20   .globl user_programs\n\
     user_programs:\n\
     \n\
     \x20   .globl user_program_count\n\
     user_program_count:\n\
     \x20   .word 0\n"
        .to_string()
}

/// Builds every program in `dir` (`kernel/user`) with our own tools and
/// returns the text of the kernel's `userprogs.s` ([`generate`]): all but
/// the [`DISK_ONLY`] ones.
pub fn generated(dir: &Path) -> Result<String, String> {
    Ok(generate(&embedded(&build_all(dir)?), CCompiler::Merlin))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-userprogs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal runtime in `dir`: `_start` calls main and exit, `exit` is
    /// an ecall, and the library has one function.
    fn write_runtime(dir: &Path) {
        std::fs::write(
            dir.join("crt0.s"),
            "    .section .text.start\n    .globl _start\n_start:\n    call main\n    call exit\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("usys.s"),
            "    .globl exit\nexit:\n    li a7, 1\n    ecall\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join(LIB_DIR)).unwrap();
        std::fs::write(
            dir.join(LIB_DIR).join("nothing.c"),
            "int nothing(void) { return 0; }\n",
        )
        .unwrap();
        // A member that needs another, and one nobody needs.
        std::fs::write(
            dir.join(LIB_DIR).join("twice.c"),
            "int nothing(void);\nint twice(void) { return 2 * nothing(); }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(LIB_DIR).join("unused.s"),
            "    .globl unused\nunused:\n    ret\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(SCRIPT),
            "ENTRY(_start)\nSECTIONS { . = 0x1000; .text : { *(.text.start) *(.text*) } }\n",
        )
        .unwrap();
    }

    fn src(stem: &str, lang: Lang) -> Source {
        Source {
            stem: stem.into(),
            lang,
        }
    }

    #[test]
    fn names() {
        for ok in ["ping", "a", "bad_ptr2", "abcdefghijklmno"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "Ping", "ping-pong", "a.b", "abcdefghijklmnop", "pïng"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn source_files() {
        assert_eq!(src("sh", Lang::C).file(), "sh.c");
        assert_eq!(src("regs", Lang::Asm).file(), "regs.s");
        assert_eq!(Source::from_file("printf.c"), Some(src("printf", Lang::C)));
        assert_eq!(Source::from_file("user.h"), None);
        assert_eq!(Source::from_file("user.ld"), None);
        assert_eq!(runtime().len(), RUNTIME.len());
    }

    #[test]
    fn lists_the_programs_but_not_the_runtime() {
        let names = program_names(&user_dir(&crate::repo_root())).unwrap();
        for want in [
            "sh", "echo", "hello", "ticks", "ding", "dong", "ping", "nc", "badload", "badhalt",
        ] {
            assert!(names.contains(&want.to_string()), "{want} missing from {names:?}");
        }
        for runtime in ["crt0", "usys", "user", "lib", "printf", "string", "malloc"] {
            assert!(!names.iter().any(|n| n == runtime), "{runtime} in {names:?}");
        }
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);

        let dir = scratch("names");
        assert!(program_names(&dir).unwrap_err().contains("no programs"));
        write_runtime(&dir);
        std::fs::write(dir.join("user.h"), "").unwrap();
        assert!(program_names(&dir).unwrap_err().contains("no programs"));
        std::fs::write(dir.join("b.c"), "").unwrap();
        std::fs::write(dir.join("a.s"), "").unwrap();
        // A subdirectory with no sources is no program; one with some is a
        // program of several files.
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("notes.txt"), "").unwrap();
        assert_eq!(program_names(&dir).unwrap(), ["a", "b"]);
        assert_eq!(programs(&dir).unwrap(), [src("a", Lang::Asm), src("b", Lang::C)]);
        std::fs::write(dir.join("sub").join("y.s"), "").unwrap();
        std::fs::write(dir.join("sub").join("x.c"), "").unwrap();
        assert_eq!(program_names(&dir).unwrap(), ["a", "b", "sub"]);
        let sub = programs(&dir).unwrap().pop().unwrap();
        assert_eq!(sub, src("sub", Lang::Dir));
        assert_eq!(
            sub.parts(&dir).unwrap(),
            [src("sub/x", Lang::C), src("sub/y", Lang::Asm)]
        );
        std::fs::remove_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.c"), "").unwrap();
        let err = program_names(&dir).unwrap_err();
        assert!(err.contains("both a.c and a.s define program a"), "{err}");
        std::fs::remove_file(dir.join("a.c")).unwrap();
        std::fs::write(dir.join("Bad-Name.s"), "").unwrap();
        assert!(program_names(&dir).unwrap_err().contains("a program name is"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_runs_become_zero_directives() {
        let mut s = String::new();
        // Three zero words stay words; four become a .zero.
        let mut bytes = vec![0x7f, b'E', b'L', b'F'];
        bytes.extend_from_slice(&[0; 12]);
        bytes.extend_from_slice(&[5, 0, 0, 0]);
        bytes.extend_from_slice(&[0; 16]);
        bytes.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 2, 3]);
        emit_bytes(&mut s, &bytes);
        assert_eq!(
            s,
            "    .word 0x464c457f, 0x00000000, 0x00000000, 0x00000000, 0x00000005\n\
             \x20   .zero 16\n\
             \x20   .word 0x00000001, 0x00000000\n\
             \x20   .byte 0x02, 0x03\n"
        );
        // Long lines are split.
        let mut s = String::new();
        emit_bytes(&mut s, &[1; 40]);
        assert_eq!(s.lines().count(), 2);
        assert_eq!(
            s.lines().next().unwrap().matches("0x01010101").count(),
            WORDS_PER_LINE
        );
    }

    #[test]
    fn generated_file_round_trips_through_merlin_as() {
        // Whatever bytes the images hold, the assembled .rodata has the table,
        // the names and the images, in that order.
        let programs = [
            Program {
                name: "one".into(),
                elf: (0..=255u8).chain([0; 64]).chain([9, 8, 7]).collect(),
                debug: Vec::new(),
            },
            Program {
                name: "two".into(),
                elf: vec![0; 4096],
                debug: Vec::new(),
            },
        ];
        let text = generate(&programs, CCompiler::Merlin);
        let obj = merlin_as::assemble(&text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
        let rodata = obj
            .sections
            .iter()
            .find(|s| s.name == ".rodata")
            .expect("no .rodata");
        let data = &rodata.data;
        let sym = |name: &str| {
            obj.symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no symbol {name}"))
                .value as usize
        };
        assert_eq!(sym("user_programs"), 0);
        // Three words per entry, the sizes in the third (the pointers are
        // relocations, still 0 in the object).
        assert_eq!(data[8..12], 323u32.to_le_bytes());
        assert_eq!(data[20..24], 4096u32.to_le_bytes());
        assert_eq!(data[sym("user_program_count")..][..4], 2u32.to_le_bytes());
        assert_eq!(&data[sym("user_name_one")..][..4], b"one\0");
        assert_eq!(&data[sym("user_name_two")..][..4], b"two\0");
        for p in &programs {
            let at = sym(&format!("user_elf_{}", p.name));
            assert_eq!(at % 4, 0);
            assert_eq!(&data[at..at + p.elf.len()], &p.elf[..]);
        }
        assert!(text.starts_with("# Generated by `cargo xtask kernel`"));
        assert!(text.contains("compiled by merlin,"), "{text}");
        // The reference build's says so; the data is the same.
        let clang = generate(&programs, CCompiler::Clang(Path::new("x")));
        assert!(
            clang.contains("compiled by clang (the reference build)"),
            "{clang}"
        );
        let body = |t: &str| t[t.find(".section").unwrap()..].to_string();
        assert_eq!(body(&clang), body(&text));
    }

    #[test]
    fn builds_programs_with_the_runtime_and_reports_errors() {
        let dir = scratch("build");
        write_runtime(&dir);
        std::fs::write(
            dir.join("ok.s"),
            "    .globl main\nmain:\n    call nothing\n    ret\n",
        )
        .unwrap();
        let p = build_named(&dir, "ok").unwrap();
        assert_eq!(p.name, "ok");
        let elf = elf32::Elf::parse(&p.elf).unwrap();
        // crt0's _start comes first and is the entry point.
        assert_eq!(elf.header.entry, 0x1000);
        let syms = elf.symbols().unwrap();
        for want in ["_start", "main", "exit", "nothing"] {
            assert!(syms.iter().any(|s| s.name == want), "{want} missing");
        }
        // Only the library members it needs.
        for unwanted in ["twice", "unused"] {
            assert!(!syms.iter().any(|s| s.name == unwanted), "{unwanted} linked");
        }
        assert_eq!(build_all(&dir).unwrap(), std::slice::from_ref(&p));
        // A member pulls in the members it needs.
        std::fs::write(
            dir.join("ok.s"),
            "    .globl main\nmain:\n    call twice\n    ret\n",
        )
        .unwrap();
        let twice = build_named(&dir, "ok").unwrap();
        let syms = elf32::Elf::parse(&twice.elf).unwrap().symbols().unwrap();
        for want in ["twice", "nothing"] {
            assert!(syms.iter().any(|s| s.name == want), "{want} missing");
        }
        assert!(!syms.iter().any(|s| s.name == "unused"));
        std::fs::write(
            dir.join("ok.s"),
            "    .globl main\nmain:\n    call nothing\n    ret\n",
        )
        .unwrap();

        // A C program, compiled by merlin with user.h from its directory.
        std::fs::write(dir.join("user.h"), "#define ZERO 0\nint nothing(void);\n").unwrap();
        std::fs::write(
            dir.join("cprog.c"),
            "#include \"user.h\"\nint main(void) { return nothing() + ZERO; }\n",
        )
        .unwrap();
        let built = build_all(&dir).unwrap();
        assert_eq!(
            built.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["cprog", "ok"]
        );
        // A merlin error names the file under kernel/user.
        std::fs::write(dir.join("cprog.c"), "int main(void) { return nope; }\n").unwrap();
        let err = build_all(&dir).unwrap_err();
        assert!(err.starts_with("kernel/user/cprog.c:1:"), "{err}");
        assert!(err.contains("nope"), "{err}");

        // The reference build reads clang's assembly from its directory.
        let asm = dir.join("asm");
        std::fs::create_dir_all(&asm).unwrap();
        let cprog = programs(&dir)
            .unwrap()
            .into_iter()
            .find(|p| p.stem == "cprog")
            .unwrap();
        // The library's assembly is in lib/ there.
        let err = build_program_with(&dir, &cprog, CCompiler::Clang(&asm)).unwrap_err();
        assert!(err.contains("cannot read") && err.contains("nothing.s"), "{err}");
        std::fs::create_dir_all(asm.join(LIB_DIR)).unwrap();
        std::fs::write(
            asm.join(LIB_DIR).join("nothing.s"),
            "    .globl nothing\nnothing:\n    li a0, 0\n    ret\n",
        )
        .unwrap();
        std::fs::write(
            asm.join(LIB_DIR).join("twice.s"),
            "    .globl twice\ntwice:\n    li a0, 0\n    ret\n",
        )
        .unwrap();
        let err = build_program_with(&dir, &cprog, CCompiler::Clang(&asm)).unwrap_err();
        assert!(err.contains("cannot read") && err.contains("cprog.s"), "{err}");
        std::fs::write(
            asm.join("cprog.s"),
            "    .globl main\nmain:\n    li a0, 0\n    ret\n",
        )
        .unwrap();
        let p = build_program_with(&dir, &cprog, CCompiler::Clang(&asm)).unwrap();
        assert!(elf32::Elf::parse(&p.elf).is_ok());
        std::fs::write(dir.join("cprog.c"), "int main(void) { return 0; }\n").unwrap();

        std::fs::write(dir.join("bad.s"), "    nop\n    frobnicate a0\n").unwrap();
        let err = build_all(&dir).unwrap_err();
        assert!(err.starts_with("kernel/user/bad.s:2: error: "), "{err}");
        std::fs::write(dir.join("bad.s"), "    .globl main\nmain:\n    call nowhere\n").unwrap();
        let err = build_named(&dir, "bad").unwrap_err();
        assert!(err.contains("undefined reference to 'nowhere'"), "{err}");
        // A program may not define what the runtime does.
        std::fs::write(
            dir.join("bad.s"),
            "    .globl main\nmain:\n    .globl exit\nexit:\n",
        )
        .unwrap();
        assert!(build_named(&dir, "bad").is_err());
        assert!(build_named(&dir, "nope")
            .unwrap_err()
            .contains("no program called nope"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clang_compiles_to_assembly_in_the_user_directory() {
        let args = clang_args(&src("sh", Lang::C), Path::new("out/sh.s"));
        for flag in [
            "--target=riscv32-unknown-elf",
            "-mno-relax",
            "-ffreestanding",
            "-nostdinc",
            "-fno-builtin",
            "-fno-common",
            "-S",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag} missing from {args:?}");
        }
        // The source by its bare name: clang runs in kernel/user.
        let n = args.len();
        assert_eq!(args[n - 3..], ["sh.c", "-o", "out/sh.s"]);
        let dir = scratch("noclang");
        std::fs::write(dir.join("a.c"), "").unwrap();
        let err = compile_clang(&dir.join("no-such-clang"), &dir, &dir.join("out"), false).unwrap_err();
        assert!(err.contains("cannot run"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
