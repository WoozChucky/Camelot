//! Runs the `merlin` binary: `--dump-ast` snapshots, error format, exit codes.
//!
//! Snapshots live in `tests/snapshots/<case>.ast` and are compared against
//! `merlin --dump-ast tests/cases/<case>.c`. After an intended change to the
//! tree or its printer, regenerate them with
//!
//!     MERLIN_BLESS=1 cargo test -p merlin --test cli

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn merlin(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_merlin"))
        .args(args)
        .output()
        .unwrap()
}

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).replace('\r', "")
}

/// A source file in a fresh temporary directory, removed when dropped.
struct TempSrc {
    dir: PathBuf,
    file: PathBuf,
}

impl TempSrc {
    fn new(test: &str, name: &str, src: &str) -> TempSrc {
        let dir = std::env::temp_dir().join(format!("merlin-cli-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(name);
        std::fs::write(&file, src).unwrap();
        TempSrc { dir, file }
    }
}

impl Drop for TempSrc {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn dump_ast_snapshots() {
    let bless = std::env::var_os("MERLIN_BLESS").is_some();
    let snap_dir = tests_dir().join("snapshots");
    let mut snaps: Vec<PathBuf> = std::fs::read_dir(&snap_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "ast"))
        .collect();
    snaps.sort();
    assert!(snaps.len() >= 3, "expected snapshots in {}", snap_dir.display());
    for snap in snaps {
        let name = snap.file_stem().unwrap().to_string_lossy().into_owned();
        let case = tests_dir().join("cases").join(format!("{name}.c"));
        let out = merlin(&[Path::new("--dump-ast"), &case]);
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        let got = String::from_utf8(out.stdout).unwrap().replace('\r', "");
        if bless {
            std::fs::write(&snap, &got).unwrap();
            continue;
        }
        // Checkouts with autocrlf may turn the snapshot's line endings into CRLF.
        let want = std::fs::read_to_string(&snap).unwrap().replace('\r', "");
        assert!(
            got == want,
            "{name}: --dump-ast differs from {} (MERLIN_BLESS=1 to update)\n--- got ---\n{got}",
            snap.display()
        );
    }
}

#[test]
fn dump_ast_reads_like_the_source() {
    let t = TempSrc::new(
        "dump",
        "small.c",
        "int add(int a, int b);\nint main() {\n    int x = 1;\n    for (;;) break;\n    return add(x, -2) || !x;\n}\n",
    );
    let out = merlin(&[Path::new("--dump-ast"), &t.file]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8(out.stdout).unwrap().replace('\r', "");
    assert_eq!(
        text,
        "\
Prototype add(a, b) <1:5>
Function main() <2:5>
  Local #0 x <3:9>
  Decl <3:5>
    Var x #0 <3:9>
      Num 1 <3:13>
  For <4:5>
    NoInit <4:5>
    NoCond <4:5>
    NoStep <4:5>
    Break <4:14>
  Return <5:5>
    Binary || <5:23>
      Call add <5:12>
        Var x #0 <5:16>
        Unary - <5:19>
          Num 2 <5:20>
      Unary ! <5:26>
        Var x #0 <5:27>
"
    );
}

#[test]
fn errors_use_file_line_col_format() {
    let t = TempSrc::new(
        "errors",
        "bad.c",
        "int main() {\n    int x = 1\n    return x;\n}\n",
    );
    let out = merlin(&[&t.file]);
    assert_eq!(out.status.code(), Some(1));
    let file = t.file.display().to_string();
    assert_eq!(
        stderr(&out),
        format!("{file}:2:14: error: expected ';' at end of declaration\n")
    );
    assert!(!t.dir.join("bad.s").exists(), "no output on error");
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let limit = merlin::parser::MAX_DEPTH;
    // `return` and its expression take two levels, each `(` one more, so the
    // error points just past the 255th `(` (the first is at column 21).
    let parens = |n: usize| format!("int main() {{ return {}1{}; }}\n", "(".repeat(n), ")".repeat(n));
    let blocks = |n: usize| format!("int main() {{ {}{} return 0; }}\n", "{".repeat(n), "}".repeat(n));
    // At the limit, the whole pipeline (parser, sema, AST printer) still copes.
    for (name, src) in [("parens.c", parens(limit - 2)), ("blocks.c", blocks(limit))] {
        let t = TempSrc::new("deep-ok", name, &src);
        let out = merlin(&[Path::new("--dump-ast"), &t.file]);
        assert!(out.status.success(), "{name}: {}", stderr(&out));
    }
    for (name, src, col) in [
        ("parens.c", parens(100_000), 21 + limit - 1),
        ("blocks.c", blocks(100_000), 14 + limit),
        (
            "minus.c",
            format!("int main() {{ return {}1; }}\n", "- ".repeat(100_000)),
            0,
        ),
    ] {
        let t = TempSrc::new("deep", name, &src);
        for args in [vec![Path::new("--dump-ast"), &t.file], vec![&*t.file]] {
            let out = merlin(&args);
            assert_eq!(out.status.code(), Some(1), "{name}: {}", stderr(&out));
            let err = stderr(&out);
            let file = t.file.display().to_string();
            let msg = format!("error: nesting too deep: merlin allows at most {limit} levels\n");
            if col > 0 {
                assert_eq!(err, format!("{file}:1:{col}: {msg}"));
            } else {
                assert!(
                    err.starts_with(&format!("{file}:1:")) && err.ends_with(&msg),
                    "{err}"
                );
            }
        }
    }
}

#[test]
fn every_semantic_error_is_reported() {
    let t = TempSrc::new(
        "sema",
        "sema.c",
        "int two(int a, int b) { return a + b; }\nint main() {\n    y = 1;\n    return two(1);\n}\n",
    );
    let out = merlin(&[Path::new("--dump-ast"), &t.file]);
    assert_eq!(out.status.code(), Some(1));
    let file = t.file.display().to_string();
    assert_eq!(
        stderr(&out),
        format!(
            "{file}:3:5: error: use of undeclared identifier 'y'\n\
             {file}:4:12: error: too few arguments to 'two': expected 2, have 1 (declared at 1:5)\n"
        )
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn compiles_or_reports_missing_codegen() {
    let t = TempSrc::new("compile", "ok.c", "int main() { return 0; }\n");
    let out_s = t.dir.join("custom.s");
    let out = merlin(&[&t.file, Path::new("-o"), &out_s]);
    if out.status.success() {
        let asm = std::fs::read_to_string(&out_s).unwrap();
        assert!(asm.contains("main:"), "{asm}");
    } else {
        // Until #14 lands.
        assert_eq!(out.status.code(), Some(1));
        let file = t.file.display().to_string();
        assert_eq!(
            stderr(&out),
            format!("{file}: error: not implemented yet: code generation (#14)\n")
        );
    }
}

#[test]
fn usage_errors() {
    let out = merlin(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("usage: merlin"));

    let out = merlin(&[Path::new("--bogus")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("merlin: unknown option --bogus"));

    let out = merlin(&[Path::new("a.c"), Path::new("b.c")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("merlin: one input file per invocation"));

    let out = merlin(&[Path::new("does-not-exist.c")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("merlin: cannot read does-not-exist.c"));

    let out = merlin(&[Path::new("--help")]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("--dump-ast"));
}
