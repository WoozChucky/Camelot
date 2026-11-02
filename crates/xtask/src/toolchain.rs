//! Our own toolchain as libraries: merlin (C to assembly), merlin-as
//! (assembly to an object) and merlin-ld (objects to an executable), the
//! same code as their command lines, run in memory. This is how the kernel
//! and its user programs are built (spec section 10, "Kernel build"): no
//! LLVM at all.
//!
//! Every path a tool sees is relative to the directory being built, so the
//! output doesn't depend on where the repository is checked out: a C file is
//! compiled as `main.c`, with `-I include`, and the files it includes are
//! read from under that directory.
//!
//! C is compiled with `-g` (debug information: line tables for merlin-dbg),
//! with a compilation directory relative to the repository root (`kernel`,
//! `kernel/user`), for the same reason. The debug information is never
//! loaded, so it changes no byte the machine runs ([`without_debug_info`]
//! builds without it, for the test that checks this).

use std::cell::Cell;
use std::path::{Path, PathBuf};

thread_local! {
    /// Whether [`compile_c`] passes `-g`: true but inside [`without_debug_info`].
    static DEBUG_INFO: Cell<bool> = const { Cell::new(true) };
}

/// Runs `f` with C compiled without `-g` on this thread: the build as it
/// was before debug information, for comparing loaded bytes.
pub fn without_debug_info<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            DEBUG_INFO.set(self.0);
        }
    }
    let _restore = Restore(DEBUG_INFO.replace(false));
    f()
}

/// Compiles `file`, a C file in `dir`, with merlin, the way
/// `merlin -g -fdebug-compilation-dir=<comp_dir> -I <inc>... file` run
/// inside `dir` would (`comp_dir` names `dir`, relative to the repository
/// root). `include_dirs` are relative to `dir`. Returns the assembly; errors
/// read `file.c:line:col: error: ...`, one per line.
pub fn compile_c(dir: &Path, file: &str, include_dirs: &[&str], comp_dir: &str) -> Result<String, String> {
    let path = dir.join(file);
    let src = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let opts = merlin::Options {
        include_dirs: include_dirs.iter().map(PathBuf::from).collect(),
        macros: Vec::new(),
        debug: DEBUG_INFO.get().then(|| merlin::DebugInfo {
            comp_dir: comp_dir.into(),
        }),
    };
    let load = |p: &Path| std::fs::read_to_string(dir.join(p)).ok();
    merlin::compile_with(file, &src, &opts, &load).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(|e| e.render(file)).collect();
        lines.join("\n")
    })
}

/// Assembles `text` (shown as `name` in errors) with merlin-as into an ELF
/// object. Errors read `name:line: error: ...`, one per line.
pub fn assemble(name: &str, text: &str) -> Result<Vec<u8>, String> {
    merlin_as::assemble_to_elf(text).map_err(|errors| {
        let lines: Vec<String> = errors
            .iter()
            .map(|e| match e.line {
                0 => format!("{name}: error: {}", e.msg),
                n => format!("{name}:{n}: error: {}", e.msg),
            })
            .collect();
        lines.join("\n")
    })
}

/// Links `inputs` with merlin-ld and the linker script `script` (read from
/// `dir`). merlin-ld's warnings are errors here: a kernel or a program
/// should link cleanly. `what` names the result in errors.
pub fn link(dir: &Path, script: &str, inputs: &[merlin_ld::Input], what: &str) -> Result<Vec<u8>, String> {
    let path = dir.join(script);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let opts = merlin_ld::Options {
        script: Some(merlin_ld::ScriptSource {
            name: script.into(),
            text,
        }),
        entry: None,
    };
    let linked = merlin_ld::link(inputs, &opts).map_err(|errors| {
        let lines: Vec<String> = errors.iter().map(|e| format!("merlin-ld: error: {e}")).collect();
        format!("linking {what}:\n{}", lines.join("\n"))
    })?;
    if let Some(w) = linked.layout.warnings.first() {
        return Err(format!("linking {what}: merlin-ld: warning: {w}"));
    }
    Ok(linked.elf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-toolchain-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("include")).unwrap();
        dir
    }

    #[test]
    fn compiles_with_headers_relative_to_the_directory() {
        let dir = scratch("compile");
        std::fs::write(dir.join("include").join("k.h"), "#define K 42\n").unwrap();
        std::fs::write(dir.join("near.h"), "int near(void);\n").unwrap();
        std::fs::write(
            dir.join("a.c"),
            "#include \"k.h\"\n#include \"near.h\"\nint f(void) { return K + near(); }\n",
        )
        .unwrap();
        let asm = compile_c(&dir, "a.c", &["include"], "x").unwrap();
        assert!(asm.contains("f:"), "{asm}");
        assert!(asm.contains("42"), "{asm}");
        // With debug information, which can be turned off.
        assert!(asm.starts_with("    .file 0 \"x\" \"a.c\"\n"), "{asm}");
        let plain = without_debug_info(|| compile_c(&dir, "a.c", &["include"], "x").unwrap());
        assert!(!plain.contains(".file") && !plain.contains(".loc"), "{plain}");
        assert!(compile_c(&dir, "a.c", &["include"], "x")
            .unwrap()
            .contains(".loc"));
        // Without -I include, k.h is not found, and the error names the file.
        let err = compile_c(&dir, "a.c", &[], "x").unwrap_err();
        assert!(err.starts_with("a.c:1:"), "{err}");
        assert!(err.contains("'k.h' file not found"), "{err}");
        assert!(compile_c(&dir, "missing.c", &[], "x")
            .unwrap_err()
            .contains("cannot read"));
        // The result assembles and links.
        let obj = assemble("a.s", &asm).unwrap();
        let start = assemble(
            "start.s",
            "    .globl _start\n_start:\n    call f\n    .globl near\nnear:\n    ret\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("t.ld"),
            "ENTRY(_start)\nSECTIONS { . = 0x1000; .text : { *(.text*) } }\n",
        )
        .unwrap();
        let inputs = [
            merlin_ld::Input {
                name: "start.o".into(),
                data: start,
            },
            merlin_ld::Input {
                name: "a.o".into(),
                data: obj,
            },
        ];
        let elf = link(&dir, "t.ld", &inputs, "t").unwrap();
        assert_eq!(elf32::Elf::parse(&elf).unwrap().header.entry, 0x1000);
        let err = link(&dir, "t.ld", &inputs[1..], "t").unwrap_err();
        assert!(
            err.contains("linking t:") && err.contains("undefined reference to 'near'"),
            "{err}"
        );
        assert!(link(&dir, "none.ld", &inputs, "t")
            .unwrap_err()
            .contains("cannot read"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn assembler_errors_name_the_file_and_line() {
        let err = assemble("x.s", "    nop\n    frobnicate a0\n").unwrap_err();
        assert!(err.starts_with("x.s:2: error: "), "{err}");
    }
}
