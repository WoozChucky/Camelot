//! The user programs embedded in the kernel (`kernel/user/`, spec section 10,
//! "Kernel processes").
//!
//! Every `kernel/user/*.c` and `*.s` is one program, but for the runtime
//! linked into each of them ([`RUNTIME`]: `crt0.s`, the system call stubs
//! `usys.s` and the C library `ulib.c`). A program in assembly is assembled
//! from its source; one in C from the assembly clang made of it, which is
//! committed next to it in `kernel/user/clang/` (until Merlin, #15, compiles
//! this much C). merlin-as assembles everything, and merlin-ld links each
//! program with the runtime and `kernel/user/user.ld`, all in memory: our own
//! tools, no LLVM. The executables then go into `kernel/userprogs.s` as ELF
//! images in `.rodata`, with a table the kernel's loader looks programs up
//! in. That file is generated, but committed with the kernel sources, so
//! building the kernel needs nothing but its own directory.
//!
//! `cargo xtask kernel` first recompiles the C files with clang
//! ([`compile_c`], [`write_clang`]), then regenerates `userprogs.s`
//! ([`write`]). `cargo test` checks that the committed `userprogs.s` is what
//! the current sources and tools give (no LLVM needed), and, when the
//! installed clang is the one that built `kernel/kernel.elf`, that the
//! committed clang output is current too.

use crate::kernel::TARGET_FLAGS;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The user programs' directory, relative to the repository root.
pub const USER_DIR: &str = "kernel/user";
/// The runtime linked into every program (none of them is a program).
pub const RUNTIME: &[&str] = &["crt0.s", "usys.s", "ulib.c"];
/// The linker script every program is linked with.
pub const SCRIPT: &str = "user.ld";
/// Where clang's assembly for each `X.c` is committed, as `X.s`: a
/// subdirectory of [`USER_DIR`].
pub const CLANG_DIR: &str = "clang";
/// The generated assembly file, relative to the repository root.
pub const GENERATED: &str = "kernel/userprogs.s";
/// The longest program name: the kernel keeps 15 characters and a NUL
/// (`struct proc`'s `name`, kernel/include/proc.h).
pub const MAX_NAME: usize = 15;

/// clang's flags for the user C files, after [`TARGET_FLAGS`]: like the
/// kernel's (freestanding, no libc headers, no hidden runtime, nothing that
/// records the clang version), with `user.h` from the same directory. `-S`:
/// the output is assembly, for merlin-as.
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

/// The language of a source file in [`USER_DIR`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    C,
    Asm,
}

/// A source file in [`USER_DIR`]: a program's or the runtime's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The file's stem: for a program, its name.
    pub stem: String,
    pub lang: Lang,
}

impl Source {
    /// The source's file name, e.g. `sh.c`.
    pub fn file(&self) -> String {
        match self.lang {
            Lang::C => format!("{}.c", self.stem),
            Lang::Asm => format!("{}.s", self.stem),
        }
    }

    /// Where the assembly merlin-as reads is, relative to [`USER_DIR`]: the
    /// source itself, or clang's output for it.
    pub fn asm_file(&self) -> String {
        match self.lang {
            Lang::C => format!("{CLANG_DIR}/{}.s", self.stem),
            Lang::Asm => self.file(),
        }
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

/// A linked user program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The source's stem: what the shell calls it.
    pub name: String,
    /// The executable merlin-ld wrote.
    pub elf: Vec<u8>,
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

/// The programs in `dir`: every source but the [`RUNTIME`]'s, by name. A
/// name must be [`valid_name`] and belong to one source only.
pub fn programs(dir: &Path) -> Result<Vec<Source>, String> {
    let mut progs: Vec<Source> = sources(dir)?
        .into_iter()
        .filter(|s| !RUNTIME.contains(&s.file().as_str()))
        .collect();
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

/// Assembles `src` (from `dir`) with merlin-as. Errors name the file and
/// line, like merlin-as itself; a C file whose clang output is missing says
/// how to make it.
fn assemble(dir: &Path, src: &Source) -> Result<Vec<u8>, String> {
    let file = src.asm_file();
    let path = dir.join(&file);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if src.lang == Lang::C {
            format!(
                "cannot read {}: {e}\n(clang's assembly for {USER_DIR}/{}: run `cargo xtask kernel`, which needs clang)",
                path.display(),
                src.file()
            )
        } else {
            format!("cannot read {}: {e}", path.display())
        }
    })?;
    merlin_as::assemble_to_elf(&text).map_err(|errors| {
        let shown = format!("{USER_DIR}/{file}");
        let lines: Vec<String> = errors
            .iter()
            .map(|e| match e.line {
                0 => format!("{shown}: error: {}", e.msg),
                n => format!("{shown}:{n}: error: {}", e.msg),
            })
            .collect();
        lines.join("\n")
    })
}

/// The runtime's sources.
fn runtime() -> Vec<Source> {
    RUNTIME.iter().filter_map(|f| Source::from_file(f)).collect()
}

/// Builds program `prog` from `dir`: it and the [`RUNTIME`] assembled, then
/// linked with [`SCRIPT`], the program first. merlin-ld's warnings are errors
/// here.
pub fn build_program(dir: &Path, prog: &Source) -> Result<Program, String> {
    let mut inputs = Vec::new();
    for src in std::iter::once(prog.clone()).chain(runtime()) {
        inputs.push(merlin_ld::Input {
            name: format!("{}.o", src.file()),
            data: assemble(dir, &src)?,
        });
    }
    let path = dir.join(SCRIPT);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let opts = merlin_ld::Options {
        script: Some(merlin_ld::ScriptSource {
            name: SCRIPT.into(),
            text,
        }),
        entry: None,
    };
    let name = &prog.stem;
    let linked = merlin_ld::link(&inputs, &opts).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(|e| format!("merlin-ld: error: {e}")).collect();
        format!("linking {name}:\n{}", lines.join("\n"))
    })?;
    if let Some(w) = linked.layout.warnings.first() {
        return Err(format!("linking {name}: merlin-ld: warning: {w}"));
    }
    Ok(Program {
        name: name.clone(),
        elf: linked.elf,
    })
}

/// Builds the program called `name` in `dir`.
pub fn build_named(dir: &Path, name: &str) -> Result<Program, String> {
    let prog = programs(dir)?
        .into_iter()
        .find(|s| s.stem == name)
        .ok_or_else(|| format!("no program called {name} in {}", dir.display()))?;
    build_program(dir, &prog)
}

/// Builds every program in `dir`, by name.
pub fn build_all(dir: &Path) -> Result<Vec<Program>, String> {
    programs(dir)?
        .iter()
        .map(|prog| build_program(dir, prog))
        .collect()
}

/// The first line of every file in [`CLANG_DIR`].
const CLANG_HEADER: &str = "# Generated by `cargo xtask kernel` with clang: do not edit.";

/// What [`compile_c`] writes for `file` (e.g. `sh.c`), given clang's output.
fn clang_file_text(file: &str, asm: &str) -> String {
    format!("{CLANG_HEADER}\n# Source: {USER_DIR}/{file} (flags in crates/xtask/src/userprogs.rs).\n\n{asm}")
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

/// Compiles every C file in `dir` with `clang` to assembly, in `work` (a
/// scratch directory). Returns, for each, the path under [`CLANG_DIR`] its
/// output belongs at and the text to put there. clang runs inside `dir`
/// with relative names, so the output doesn't depend on where the
/// repository is.
pub fn compile_c(
    clang: &Path,
    dir: &Path,
    work: &Path,
    verbose: bool,
) -> Result<Vec<(String, String)>, String> {
    std::fs::create_dir_all(work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    let mut out = Vec::new();
    for src in sources(dir)?.into_iter().filter(|s| s.lang == Lang::C) {
        let asm = work.join(format!("{}.s", src.stem));
        let args = clang_args(&src, &asm);
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
        let text =
            std::fs::read_to_string(&asm).map_err(|e| format!("cannot read {}: {e}", asm.display()))?;
        // Windows clang writes CR LF line ends; the committed files have LF.
        let text = text.replace("\r\n", "\n");
        out.push((src.asm_file(), clang_file_text(&src.file(), &text)));
    }
    Ok(out)
}

/// The files in `dir`/[`CLANG_DIR`] that don't belong to any C file in `dir`.
pub fn stale_clang_files(dir: &Path) -> Result<Vec<String>, String> {
    let wanted: Vec<String> = sources(dir)?
        .into_iter()
        .filter(|s| s.lang == Lang::C)
        .map(|s| s.asm_file())
        .collect();
    let clang_dir = dir.join(CLANG_DIR);
    let Ok(entries) = std::fs::read_dir(&clang_dir) else {
        return Ok(Vec::new());
    };
    let mut stale = Vec::new();
    for e in entries {
        let e = e.map_err(|e| e.to_string())?;
        let rel = format!("{CLANG_DIR}/{}", e.file_name().to_string_lossy());
        if !wanted.contains(&rel) {
            stale.push(rel);
        }
    }
    stale.sort();
    Ok(stale)
}

/// Recompiles the C files under `root` with `clang` and writes their
/// assembly into [`CLANG_DIR`], leaving files that didn't change alone and
/// deleting any that no C file makes. Returns the files written or deleted.
pub fn write_clang(root: &Path, clang: &Path, verbose: bool) -> Result<Vec<String>, String> {
    let dir = user_dir(root);
    let work = root.join("target").join("userprogs");
    let compiled = compile_c(clang, &dir, &work, verbose)?;
    let clang_dir = dir.join(CLANG_DIR);
    std::fs::create_dir_all(&clang_dir).map_err(|e| format!("cannot create {}: {e}", clang_dir.display()))?;
    let mut changed = Vec::new();
    for (rel, text) in compiled {
        let path = dir.join(&rel);
        if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
            continue;
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        changed.push(format!("{USER_DIR}/{rel}"));
    }
    for rel in stale_clang_files(&dir)? {
        let path = dir.join(&rel);
        std::fs::remove_file(&path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        changed.push(format!("{USER_DIR}/{rel} (removed)"));
    }
    Ok(changed)
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

/// The text of `kernel/userprogs.s` for `programs`: the table
/// `user_programs` (one `struct user_program { const char *name; const u8
/// *elf; u32 size; }` per program, in order), `user_program_count`, then the
/// names and the images, each image 4-byte aligned under a global label
/// `user_elf_<name>`.
pub fn generate(programs: &[Program]) -> String {
    let mut s = String::new();
    s.push_str(
        "# Generated by `cargo xtask kernel` from kernel/user/: do not edit.\n\
         #\n\
         # The user programs as ELF images for the kernel's loader, each assembled\n\
         # by merlin-as (C programs from clang's assembly in kernel/user/clang)\n\
         # with the runtime (crt0.s, usys.s, ulib.c) and linked by merlin-ld with\n\
         # kernel/user/user.ld (crates/xtask/src/userprogs.rs). The table is the\n\
         # array kernel/include/proc.h declares:\n\
         #   struct user_program { const char *name; const u8 *elf; u32 size; };\n\
         \n\
         \x20   .section .rodata\n\
         \x20   .align 2\n\
         \x20   .globl user_programs\n\
         user_programs:\n",
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

/// Builds every program under `root` and returns the text of
/// `kernel/userprogs.s`.
pub fn generated(root: &Path) -> Result<String, String> {
    Ok(generate(&build_all(&user_dir(root))?))
}

/// Regenerates `kernel/userprogs.s` under `root`. Returns whether the file
/// changed (it is left alone when it is already up to date).
pub fn write(root: &Path) -> Result<bool, String> {
    let text = generated(root)?;
    let path = root.join(GENERATED);
    if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return Ok(false);
    }
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
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
    /// an ecall, and ulib.c's committed assembly has one function.
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
        std::fs::write(dir.join("ulib.c"), "int nothing(void) { return 0; }\n").unwrap();
        std::fs::create_dir_all(dir.join(CLANG_DIR)).unwrap();
        std::fs::write(
            dir.join(CLANG_DIR).join("ulib.s"),
            "    .globl nothing\nnothing:\n    li a0, 0\n    ret\n",
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
        let c = src("sh", Lang::C);
        assert_eq!((c.file(), c.asm_file()), ("sh.c".into(), "clang/sh.s".into()));
        let s = src("regs", Lang::Asm);
        assert_eq!((s.file(), s.asm_file()), ("regs.s".into(), "regs.s".into()));
        assert_eq!(Source::from_file("ulib.c"), Some(src("ulib", Lang::C)));
        assert_eq!(Source::from_file("user.h"), None);
        assert_eq!(Source::from_file("user.ld"), None);
        assert_eq!(runtime().len(), RUNTIME.len());
    }

    #[test]
    fn lists_the_programs_but_not_the_runtime() {
        let names = program_names(&user_dir(&crate::repo_root())).unwrap();
        for want in [
            "sh", "echo", "hello", "ticks", "ping", "pong", "badload", "badhalt",
        ] {
            assert!(names.contains(&want.to_string()), "{want} missing from {names:?}");
        }
        for runtime in ["crt0", "usys", "ulib", "user"] {
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
        // clang's output is not a program of its own.
        std::fs::write(dir.join(CLANG_DIR).join("b.s"), "").unwrap();
        assert_eq!(program_names(&dir).unwrap(), ["a", "b"]);
        assert_eq!(programs(&dir).unwrap(), [src("a", Lang::Asm), src("b", Lang::C)]);
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
            },
            Program {
                name: "two".into(),
                elf: vec![0; 4096],
            },
        ];
        let text = generate(&programs);
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
        assert_eq!(build_all(&dir).unwrap(), std::slice::from_ref(&p));

        // A C program: its committed clang output is what gets assembled.
        std::fs::write(dir.join("cprog.c"), "int main(void) { return 0; }\n").unwrap();
        let err = build_all(&dir).unwrap_err();
        assert!(
            err.contains("clang/cprog.s") && err.contains("run `cargo xtask kernel`"),
            "{err}"
        );
        std::fs::write(
            dir.join(CLANG_DIR).join("cprog.s"),
            "    .globl main\nmain:\n    li a0, 0\n    ret\n",
        )
        .unwrap();
        let built = build_all(&dir).unwrap();
        assert_eq!(
            built.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["cprog", "ok"]
        );

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
    fn finds_stale_clang_output() {
        let dir = scratch("stale");
        write_runtime(&dir);
        assert!(stale_clang_files(&dir).unwrap().is_empty());
        std::fs::write(dir.join("a.c"), "").unwrap();
        std::fs::write(dir.join(CLANG_DIR).join("a.s"), "").unwrap();
        std::fs::write(dir.join(CLANG_DIR).join("gone.s"), "").unwrap();
        std::fs::write(dir.join(CLANG_DIR).join("notes.txt"), "").unwrap();
        assert_eq!(
            stale_clang_files(&dir).unwrap(),
            ["clang/gone.s", "clang/notes.txt"]
        );
        // The repository's own: none.
        assert!(stale_clang_files(&user_dir(&crate::repo_root()))
            .unwrap()
            .is_empty());
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
        let text = clang_file_text("sh.c", "\t.text\n");
        assert!(text.starts_with(CLANG_HEADER));
        assert!(text.contains("kernel/user/sh.c"));
        assert!(text.ends_with("\n\n\t.text\n"));
    }
}
