//! merlin's preprocessor in assembler-with-cpp mode against clang's (#193):
//! every `.S` source of the repository (the examples, the boot ROM) and
//! `include/asm.h`, preprocessed for RV32 and for RV64 with
//! `-I include`, must come out of `merlin -E` exactly as out of
//! `clang -E -x assembler-with-cpp`, but for two things clang writes that
//! merlin does not:
//!
//! * the line markers of clang's own predefined macros and command line
//!   (`# 1 "<built-in>" 1`, `# 1 "<command line>" 1` ... and the `# 1
//!   "file.S" 2` that comes back from them);
//! * on Windows, the `\` clang joins an include directory and a header's
//!   name with (`include\\asm.h`), where merlin writes `/`.
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

/// The sources: every `.S` of `examples/*/` and `crates/grail/rom`, and the
/// header, relative to the repository root with `/`.
fn sources(root: &Path) -> Vec<String> {
    let mut found = vec!["include/asm.h".to_string()];
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join("examples"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.push(root.join("crates/grail/rom"));
    for dir in dirs {
        for f in std::fs::read_dir(&dir).unwrap() {
            let p = f.unwrap().path();
            if p.extension().is_some_and(|e| e == "S") {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                found.push(rel);
            }
        }
    }
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

fn clang_e(root: &Path, source: &str, x: Xlen) -> String {
    let (target, march, mabi) = match x {
        Xlen::Rv32 => ("riscv32", "rv32ima_zicsr_zifencei", "ilp32"),
        Xlen::Rv64 => ("riscv64", "rv64ima_zicsr_zifencei", "lp64"),
    };
    let out = Command::new("clang")
        .args([
            &format!("--target={target}"),
            &format!("-march={march}"),
            &format!("-mabi={mabi}"),
            "-E",
            "-x",
            "assembler-with-cpp",
            "-I",
            "include",
            source,
        ])
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

fn merlin_e(root: &Path, source: &str, x: Xlen) -> String {
    let text = std::fs::read_to_string(root.join(source)).unwrap();
    let opts = merlin::Options {
        xlen: x,
        include_dirs: vec![PathBuf::from("include")],
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
    assert!(sources.len() > 15, "{sources:?}");
    let mut wrong = Vec::new();
    for source in &sources {
        for x in [Xlen::Rv32, Xlen::Rv64] {
            let clang = normalized(&clang_e(&root, source, x));
            let ours = merlin_e(&root, source, x);
            if ours != clang {
                let line = ours
                    .lines()
                    .zip(clang.lines())
                    .position(|(a, b)| a != b)
                    .unwrap_or(ours.lines().count().min(clang.lines().count()));
                wrong.push(format!(
                    "{source} ({x:?}), line {}:\n  merlin: {:?}\n  clang:  {:?}",
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
