//! The user programs embedded in the kernel (`kernel/user/`, spec section 10,
//! "Kernel processes").
//!
//! Every `kernel/user/*.s` but `lib.s` is one program. Each is assembled with
//! merlin-as together with `lib.s` (the shared runtime) and linked by
//! merlin-ld with `kernel/user/user.ld`, all in memory: our own tools, no
//! LLVM. The executables then go into `kernel/userprogs.s` as ELF images in
//! `.rodata`, with a table the kernel's loader looks programs up in. That file
//! is generated, but committed with the kernel sources, so building the
//! kernel needs nothing but its own directory; `cargo xtask kernel`
//! regenerates it, and `cargo test` checks that the committed file is what
//! the current programs and tools give.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The user programs' directory, relative to the repository root.
pub const USER_DIR: &str = "kernel/user";
/// The runtime linked into every program (not a program itself).
pub const LIB: &str = "lib.s";
/// The linker script every program is linked with.
pub const SCRIPT: &str = "user.ld";
/// The generated assembly file, relative to the repository root.
pub const GENERATED: &str = "kernel/userprogs.s";
/// The longest program name: the kernel keeps 15 characters and a NUL
/// (`struct proc`'s `name`, kernel/include/proc.h).
pub const MAX_NAME: usize = 15;

/// A linked user program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The source's stem: what the kernel's `run` command calls it.
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

/// The programs in `dir`: every `*.s` but [`LIB`], by name.
pub fn program_names(dir: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut names = Vec::new();
    for e in entries {
        let e = e.map_err(|e| e.to_string())?;
        let file = e.file_name();
        let Some(file) = file.to_str() else { continue };
        if file == LIB {
            continue;
        }
        let Some(stem) = file.strip_suffix(".s") else {
            continue;
        };
        if !valid_name(stem) {
            return Err(format!(
                "{}: a program name is 1 to {MAX_NAME} of a-z, 0-9 and _",
                dir.join(file).display()
            ));
        }
        names.push(stem.to_string());
    }
    names.sort();
    if names.is_empty() {
        return Err(format!("no programs in {}", dir.display()));
    }
    Ok(names)
}

/// Assembles `file` in `dir` with merlin-as. Errors name the file and line,
/// like merlin-as itself.
fn assemble(dir: &Path, file: &str) -> Result<Vec<u8>, String> {
    let path = dir.join(file);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
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

/// Builds program `name` from `dir`: `name.s` and [`LIB`] assembled, then
/// linked with [`SCRIPT`]. merlin-ld's warnings are errors here.
pub fn build_program(dir: &Path, name: &str) -> Result<Program, String> {
    let main = format!("{name}.s");
    let inputs = vec![
        merlin_ld::Input {
            name: format!("{name}.o"),
            data: assemble(dir, &main)?,
        },
        merlin_ld::Input {
            name: "lib.o".into(),
            data: assemble(dir, LIB)?,
        },
    ];
    let path = dir.join(SCRIPT);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let opts = merlin_ld::Options {
        script: Some(merlin_ld::ScriptSource {
            name: SCRIPT.into(),
            text,
        }),
        entry: None,
    };
    let linked = merlin_ld::link(&inputs, &opts).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(|e| format!("merlin-ld: error: {e}")).collect();
        format!("linking {name}:\n{}", lines.join("\n"))
    })?;
    if let Some(w) = linked.layout.warnings.first() {
        return Err(format!("linking {name}: merlin-ld: warning: {w}"));
    }
    Ok(Program {
        name: name.to_string(),
        elf: linked.elf,
    })
}

/// Builds every program in `dir`, by name.
pub fn build_all(dir: &Path) -> Result<Vec<Program>, String> {
    program_names(dir)?
        .iter()
        .map(|name| build_program(dir, name))
        .collect()
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
         # by merlin-as with kernel/user/lib.s and linked by merlin-ld with\n\
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
    fn lists_the_programs_but_not_the_runtime() {
        let names = program_names(&user_dir(&crate::repo_root())).unwrap();
        for want in ["ping", "pong", "badload", "badhalt"] {
            assert!(names.contains(&want.to_string()), "{want} missing from {names:?}");
        }
        assert!(!names.iter().any(|n| n == "lib"));
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);

        let dir = scratch("names");
        assert!(program_names(&dir).unwrap_err().contains("no programs"));
        std::fs::write(dir.join(LIB), "").unwrap();
        std::fs::write(dir.join("user.ld"), "").unwrap();
        assert!(program_names(&dir).unwrap_err().contains("no programs"));
        std::fs::write(dir.join("b.s"), "").unwrap();
        std::fs::write(dir.join("a.s"), "").unwrap();
        assert_eq!(program_names(&dir).unwrap(), ["a", "b"]);
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
    fn builds_a_program_and_reports_errors() {
        let dir = scratch("build");
        std::fs::write(dir.join(LIB), "    .globl exit\nexit:\n    li a7, 1\n    ecall\n").unwrap();
        std::fs::write(
            dir.join(SCRIPT),
            "ENTRY(_start)\nSECTIONS { . = 0x1000; .text : { *(.text*) } }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("ok.s"),
            "    .globl _start\n_start:\n    li a0, 0\n    call exit\n",
        )
        .unwrap();
        let p = build_program(&dir, "ok").unwrap();
        assert_eq!(p.name, "ok");
        let elf = elf32::Elf::parse(&p.elf).unwrap();
        assert_eq!(elf.header.entry, 0x1000);
        assert_eq!(build_all(&dir).unwrap(), [p]);

        std::fs::write(dir.join("bad.s"), "    nop\n    frobnicate a0\n").unwrap();
        let err = build_all(&dir).unwrap_err();
        assert!(err.starts_with("kernel/user/bad.s:2: error: "), "{err}");
        std::fs::write(
            dir.join("bad.s"),
            "    .globl _start\n_start:\n    call nowhere\n",
        )
        .unwrap();
        let err = build_program(&dir, "bad").unwrap_err();
        assert!(err.contains("undefined reference to 'nowhere'"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
