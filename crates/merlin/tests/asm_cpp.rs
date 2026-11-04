//! merlin's preprocessor in assembler-with-cpp mode against clang's (#193):
//! every `.S` source of the repository (the examples, the boot ROM, the
//! kernel's assembly and linker script, merlin-ld's and merlin-objdump's
//! test fixtures: every `.S` file outside `target/` and hidden directories)
//! and `include/asm.h`, preprocessed for RV32 and for RV64 with
//! `-I include` (and `-I kernel/include` for the kernel's, whose headers it
//! shares with the C), must come out of `merlin -E` exactly as out of
//! `clang -E -x assembler-with-cpp`, but for two things clang writes that
//! merlin does not:
//!
//! * the line markers of clang's own predefined macros and command line
//!   (`# 1 "<built-in>" 1`, `# 1 "<command line>" 1` ... and the `# 1
//!   "file.S" 2` that comes back from them);
//! * on Windows, the `\` clang joins an include directory and a header's
//!   name with (`include\\asm.h`), where merlin writes `/`.
//!
//! With `-P` (no line markers, as `cargo xtask kernel` preprocesses the
//! kernel's linker script, `kernel/kernel.lds.S`) the two must be the same
//! with nothing taken out.
//!
//! Needs clang on the PATH (the official LLVM installer's, 23.1.4 when this
//! was written), so it is ignored by default:
//!
//!     cargo test -p merlin --test asm_cpp -- --ignored

use merlin::types::Xlen;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The sources: every `.S` file of the repository (but in `target/` and in
/// directories whose name starts with `.`, `.git` and worktrees), and the
/// header, relative to the repository root with `/`.
fn sources(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy();
            if p.is_dir() {
                if name != "target" && !name.starts_with('.') {
                    walk(root, &p, found);
                }
            } else if p.extension().is_some_and(|e| e == "S") {
                found.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut found = vec!["include/asm.h".to_string()];
    walk(root, root, &mut found);
    found.sort();
    found
}

/// clang's output with what merlin leaves out taken out (see the top).
fn normalized(clang: &str) -> String {
    let mut out = Vec::new();
    let mut after_builtin = false;
    for line in clang.lines() {
        let marker = line.starts_with("# ") && line.contains(" \"");
        if marker && (line.contains("\"<built-in>\"") || line.contains("\"<command line>\"")) {
            after_builtin = true;
            continue;
        }
        if std::mem::take(&mut after_builtin) && marker && line.ends_with(" 2") {
            continue;
        }
        if marker {
            out.push(line.replace("\\\\", "/"));
        } else {
            out.push(line.to_string());
        }
    }
    out.iter().map(|l| format!("{l}\n")).collect()
}

/// The `-I` directories for `source`, relative to the repository root:
/// `include`, and the kernel's headers for a kernel source.
fn include_dirs(source: &str) -> Vec<&'static str> {
    if source.starts_with("kernel/") {
        vec!["include", "kernel/include"]
    } else {
        vec!["include"]
    }
}

fn clang_e(root: &Path, source: &str, x: Xlen, line_markers: bool) -> String {
    let (target, march, mabi) = match x {
        Xlen::Rv32 => ("riscv32", "rv32ima_zicsr_zifencei", "ilp32"),
        Xlen::Rv64 => ("riscv64", "rv64ima_zicsr_zifencei", "lp64"),
    };
    let out = Command::new("clang")
        .args((!line_markers).then_some("-P"))
        .args([
            &format!("--target={target}"),
            &format!("-march={march}"),
            &format!("-mabi={mabi}"),
            "-E",
            "-x",
            "assembler-with-cpp",
        ])
        .args(include_dirs(source).iter().flat_map(|d| ["-I", d]))
        .arg(source)
        .current_dir(root)
        .output()
        .expect("clang on the PATH");
    assert!(
        out.status.success(),
        "clang -E {source}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().replace("\r\n", "\n")
}

fn merlin_e(root: &Path, source: &str, x: Xlen, line_markers: bool) -> String {
    let text = std::fs::read_to_string(root.join(source)).unwrap();
    let opts = merlin::Options {
        xlen: x,
        include_dirs: include_dirs(source).into_iter().map(PathBuf::from).collect(),
        line_markers,
        ..merlin::Options::default()
    };
    let load = |p: &Path| std::fs::read_to_string(root.join(p)).ok();
    merlin::preprocess_asm(source, &text, &opts, &load).unwrap_or_else(|e| panic!("{source}: {e:?}"))
}

#[test]
#[ignore = "needs clang on PATH"]
fn assembler_with_cpp_matches_clang() {
    let root = repo_root();
    let sources = sources(&root);
    // The examples, the boot ROM and the fixtures of both tools.
    for dir in [
        "examples/",
        "crates/grail/rom/",
        "crates/merlin-ld/tests/",
        "crates/merlin-objdump/tests/",
    ] {
        assert!(sources.iter().any(|s| s.starts_with(dir)), "{dir} in {sources:?}");
    }
    let mut wrong = Vec::new();
    for source in &sources {
        for (x, line_markers) in [
            (Xlen::Rv32, true),
            (Xlen::Rv64, true),
            (Xlen::Rv32, false),
            (Xlen::Rv64, false),
        ] {
            let clang = clang_e(&root, source, x, line_markers);
            let clang = if line_markers { normalized(&clang) } else { clang };
            let ours = merlin_e(&root, source, x, line_markers);
            if ours != clang {
                let line = ours
                    .lines()
                    .zip(clang.lines())
                    .position(|(a, b)| a != b)
                    .unwrap_or(ours.lines().count().min(clang.lines().count()));
                wrong.push(format!(
                    "{source} ({x:?}{}), line {}:\n  merlin: {:?}\n  clang:  {:?}",
                    if line_markers { "" } else { ", -P" },
                    line + 1,
                    ours.lines().nth(line),
                    clang.lines().nth(line)
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The normalization drops only what the top says.
#[test]
fn clangs_own_markers_are_dropped() {
    let clang = "# 1 \"a.S\"\n# 1 \"<built-in>\" 1\n# 1 \"<built-in>\" 3\n# 401 \"<built-in>\" 3\n\
                 # 1 \"<command line>\" 1\n# 1 \"<built-in>\" 2\n# 1 \"a.S\" 2\nx\n\
                 # 1 \"include\\\\asm.h\" 1\n# 2 \"a.S\" 2\n \"a\\\\b\"\n";
    assert_eq!(
        normalized(clang),
        "# 1 \"a.S\"\nx\n# 1 \"include/asm.h\" 1\n# 2 \"a.S\" 2\n \"a\\\\b\"\n"
    );
}
