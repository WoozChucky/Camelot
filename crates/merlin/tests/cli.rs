//! Runs the `merlin` binary: snapshots, error format, exit codes.
//!
//! Snapshots live in `tests/snapshots/`: `<case>.ast` is compared against
//! `merlin --dump-ast tests/cases/<case>.c` and `<case>.s` against the
//! assembly `merlin tests/cases/<case>.c` writes. To snapshot another case,
//! create an empty `<case>.ast` or `<case>.s` there and bless. After an
//! intended change to the tree, its printer or code generation, regenerate
//! them (merlin itself produces them, no external tools) with
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

/// Every `tests/snapshots/*.<ext>`, sorted.
fn snapshots(ext: &str) -> Vec<PathBuf> {
    let snap_dir = tests_dir().join("snapshots");
    let mut snaps: Vec<PathBuf> = std::fs::read_dir(&snap_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .collect();
    snaps.sort();
    snaps
}

/// Compares `got` with the snapshot file, or rewrites it under `MERLIN_BLESS`.
fn check_snapshot(snap: &Path, what: &str, got: &str) {
    if std::env::var_os("MERLIN_BLESS").is_some() {
        std::fs::write(snap, got).unwrap();
        return;
    }
    // Checkouts with autocrlf may turn the snapshot's line endings into CRLF.
    let want = std::fs::read_to_string(snap).unwrap().replace('\r', "");
    assert!(
        got == want,
        "{what} differs from {} (MERLIN_BLESS=1 to update)\n--- got ---\n{got}",
        snap.display()
    );
}

fn case_for(snap: &Path) -> (String, PathBuf) {
    let name = snap.file_stem().unwrap().to_string_lossy().into_owned();
    let case = tests_dir().join("cases").join(format!("{name}.c"));
    (name, case)
}

#[test]
fn dump_ast_snapshots() {
    let snaps = snapshots("ast");
    assert!(snaps.len() >= 3, "expected .ast snapshots");
    for snap in snaps {
        let (name, case) = case_for(&snap);
        let out = merlin(&[Path::new("--dump-ast"), &case]);
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        let got = String::from_utf8(out.stdout).unwrap().replace('\r', "");
        check_snapshot(&snap, &format!("{name}: --dump-ast"), &got);
    }
}

/// The generated assembly for a few cases is committed, so that every
/// change to code generation shows up in the diff of
/// `tests/snapshots/*.s`.
#[test]
fn assembly_snapshots() {
    let snaps = snapshots("s");
    assert!(snaps.len() >= 2, "expected .s snapshots");
    let dir = std::env::temp_dir().join(format!("merlin-cli-asm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for snap in snaps {
        let (name, case) = case_for(&snap);
        let out_s = dir.join(snap.file_name().unwrap());
        let out = merlin(&[&case, Path::new("-o"), &out_s]);
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        let got = std::fs::read_to_string(&out_s).unwrap();
        check_snapshot(&snap, &format!("{name}: generated assembly"), &got);
    }
    let _ = std::fs::remove_dir_all(&dir);
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
Prototype int add(int a, int b) <1:5>
Function int main(void) <2:5>
  Local #0 int x <3:9>
  Decl <3:5>
    Var int x #0 <3:9>
      Num 1 : int <3:13>
  For <4:5>
    NoInit <4:5>
    NoCond <4:5>
    NoStep <4:5>
    Break <4:14>
  Return <5:5>
    Binary || : int <5:23>
      Call add : int <5:12>
        Var x #0 : int <5:16>
        Unary - : int <5:19>
          Num 2 : int <5:20>
      Unary ! : int <5:26>
        Var x #0 : int <5:27>
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

/// A warning is printed as clang prints it, `file:line:col: warning: ...
/// [-Wflag]`, and the file still compiles: clang 23's for each array size
/// it folds where an array cannot be variable-length (a file-scope array,
/// a member, a `static` local, a local array with an initializer), none
/// for an enumerator or a `_Static_assert` folded the same way. One whose
/// operand has an error is not given, as clang gives it not.
#[test]
fn warnings_are_printed_as_clangs() {
    let t = TempSrc::new(
        "warnings",
        "w.c",
        "unsigned u(void);\nint a[1 ? 2 : u()];\nstruct s { int m[1 ? 3 : u()]; };\n\
         enum { E = 1 ? 4 : u() };\n_Static_assert(1 ? 1 : u(), \"folded silently\");\n\
         int f(void) {\n    static int b[1 ? 2 : u()];\n    int c[1 ? 2 : u()] = {1, 2};\n    \
         return a[0] + b[0] + c[0] + E;\n}\n",
    );
    let out = merlin(&[&t.file]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let file = t.file.display().to_string();
    let warning = "warning: variable length array folded to constant array as an extension \
                   [-Wgnu-folding-constant]";
    assert_eq!(
        stderr(&out),
        format!(
            "{file}:2:5: {warning}\n{file}:3:16: {warning}\n{file}:7:16: {warning}\n{file}:8:9: {warning}\n"
        )
    );
    assert!(t.dir.join("w.s").exists());
    // With an error in the operand folded: the error alone.
    std::fs::write(
        &t.file,
        "int f(void);\nint a[1 ? 2 : f(1, 2)];\nint b[1 ? 2 : f()];\n",
    )
    .unwrap();
    let out = merlin(&[&t.file]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        format!(
            "{file}:2:15: error: too many arguments to 'f': expected 0, have 2 (declared at 1:5)\n\
             {file}:3:5: {warning}\n"
        )
    );
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let limit = merlin::parser::MAX_DEPTH;
    // `return` and its expression take two levels, each `(` one more, so the
    // error points just past the 255th `(` (the first is at column 21).
    let parens = |n: usize| format!("int main() {{ return {}1{}; }}\n", "(".repeat(n), ")".repeat(n));
    let blocks = |n: usize| format!("int main() {{ {}{} return 0; }}\n", "{".repeat(n), "}".repeat(n));
    // At the limit, the whole pipeline (parser, sema, AST printer, codegen) still copes.
    for (name, src) in [("parens.c", parens(limit - 2)), ("blocks.c", blocks(limit))] {
        let t = TempSrc::new("deep-ok", name, &src);
        let out = merlin(&[Path::new("--dump-ast"), &t.file]);
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        let out = merlin(&[&*t.file]);
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
fn compiles_to_assembly() {
    let t = TempSrc::new("compile", "ok.c", "int main() { return 0; }\n");
    // -o names the output.
    let out_s = t.dir.join("custom.s");
    let out = merlin(&[&t.file, Path::new("-o"), &out_s]);
    assert!(out.status.success(), "{}", stderr(&out));
    let asm = std::fs::read_to_string(&out_s).unwrap();
    assert!(
        asm.contains(".globl main\n    .type main, @function\nmain:\n"),
        "{asm}"
    );
    // Without it, the output goes next to the input, with a .s extension.
    let out = merlin(&[&t.file]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(t.dir.join("ok.s")).unwrap(), asm);
}

#[test]
fn preprocessor_options() {
    let t = TempSrc::new(
        "pp",
        "main.c",
        "#include \"defs.h\"\nint main() { return VALUE + TWICE(1); }\n",
    );
    let inc = t.dir.join("inc");
    std::fs::create_dir_all(&inc).unwrap();
    std::fs::write(
        inc.join("defs.h"),
        "#define TWICE(x) ((x) * 2)\n#ifndef VALUE\n#define VALUE 7\n#endif\n",
    )
    .unwrap();
    std::fs::write(t.dir.join("second.c"), "int second(void) { return 2; }\n").unwrap();
    let flag = |s: &'static str| PathBuf::from(s);
    // -E prints the preprocessed tokens; -D defines, -U undefines, in order.
    let out = merlin(&[&flag("-E"), &flag("-I"), &inc, &flag("-DVALUE=40"), &t.file]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert!(text.contains("int main() { return 40 + ((1) * 2); }"), "{text}");
    let out = merlin(&[
        &flag("-E"),
        &flag("-DVALUE=40"),
        &flag("-UVALUE"),
        &flag("-I"),
        &inc,
        &t.file,
    ]);
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert!(text.contains("return 7 + ((1) * 2);"), "{text}");
    // Without -I the header is not found.
    let out = merlin(&[&t.file]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("1:10: error: 'defs.h' file not found"),
        "{}",
        stderr(&out)
    );
    // Several inputs: each gets its own .s.
    let second = t.dir.join("second.c");
    let out = merlin(&[&flag("-I"), &inc, &t.file, &second]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(t.dir.join("main.s").exists() && t.dir.join("second.s").exists());
    // An error in a header names the header.
    std::fs::write(inc.join("defs.h"), "int broken(;\n").unwrap();
    let out = merlin(&[&flag("-I"), &inc, &t.file]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("defs.h:1:12: error:"), "{err}");
}

/// A `.S` file (or `-x assembler-with-cpp`) is preprocessed as assembly with
/// -E (`__riscv_xlen` is 64), and is never compiled.
#[test]
fn assembly_is_only_preprocessed() {
    let src = "# A comment\n#if __riscv_xlen == 64\n    ld a0, 0(sp)\n#else\n    lw a0, 0(sp)\n#endif\n";
    let t = TempSrc::new("asm-cpp", "main.S", src);
    let file = t.file.display().to_string().replace('\\', "\\\\");
    let flag = |s: &'static str| PathBuf::from(s);
    let out = merlin(&[&flag("-E"), &t.file]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert_eq!(
        text,
        format!("# 1 \"{file}\"\n # A comment\n\n    ld a0, 0(sp)\n")
    );
    let out = merlin(&[&flag("-E"), &flag("-march=rv64ima_zicsr_zifencei"), &t.file]);
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert_eq!(
        text,
        format!("# 1 \"{file}\"\n # A comment\n\n    ld a0, 0(sp)\n")
    );
    // `-x` says so for any name; `-x c` makes the same file C, where `# A`
    // is no directive.
    let other = t.dir.join("main.asm");
    std::fs::write(&other, src).unwrap();
    let out = merlin(&[&flag("-E"), &flag("-x"), &flag("assembler-with-cpp"), &other]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = merlin(&[&flag("-E"), &flag("-xc"), &t.file]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("invalid preprocessing directive '#A'"),
        "{}",
        stderr(&out)
    );
    // Assembly is not compiled; merlin-as assembles it.
    let out = merlin(&[&t.file]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("assembler-with-cpp input is only preprocessed (-E)"));
    let out = merlin(&[&flag("-x"), &flag("fortran"), &t.file]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("merlin: language not recognized: 'fortran'"));
}

#[test]
fn codegen_errors_use_file_line_col_format() {
    let t = TempSrc::new(
        "codegen-error",
        "reg.c",
        "int main() { return 0; }\nint ra() { return 1; }\n",
    );
    let out = merlin(&[&t.file]);
    assert_eq!(out.status.code(), Some(1));
    let file = t.file.display().to_string();
    assert_eq!(
        stderr(&out),
        format!("{file}:2:5: error: 'ra' is a register name, which cannot be used as a symbol name\n")
    );
    assert!(!t.dir.join("reg.s").exists(), "no output on error");
}

#[test]
fn usage_errors() {
    let out = merlin(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("usage: merlin"));

    let out = merlin(&[Path::new("--bogus")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("merlin: unknown option --bogus"));

    let out = merlin(&[
        Path::new("-o"),
        Path::new("x.s"),
        Path::new("a.c"),
        Path::new("b.c"),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("merlin: -o needs a single input file"));

    for flag in ["-I", "-D", "-U", "-o"] {
        let out = merlin(&[Path::new(flag)]);
        assert_eq!(out.status.code(), Some(2));
        assert!(
            stderr(&out).starts_with(&format!("merlin: {flag} needs a")),
            "{flag}"
        );
    }

    let out = merlin(&[Path::new("does-not-exist.c")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).starts_with("merlin: cannot read does-not-exist.c"));

    let out = merlin(&[Path::new("--help")]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("--dump-ast"));

    // -march names the ISA, and -mabi the ABI that goes with it (anything
    // else is an unknown ABI to clang, as here). RV32's are refused as any
    // other.
    for isa in ["rv64gc", "rv32ima_zicsr_zifencei"] {
        let out = merlin(&[Path::new(&format!("-march={isa}")), Path::new("a.c")]);
        assert_eq!(out.status.code(), Some(2));
        assert!(
            stderr(&out).starts_with(&format!(
                "merlin: unsupported -march '{isa}' (merlin takes rv64ima_zicsr_zifencei)"
            )),
            "{}",
            stderr(&out)
        );
    }
    for args in [
        &["-mabi=ilp32"][..],
        &["-march=rv64ima_zicsr_zifencei", "-mabi=ilp32"],
        &["-mabi=lp64d"],
    ] {
        let mut args: Vec<&Path> = args.iter().map(Path::new).collect();
        args.push(Path::new("a.c"));
        let out = merlin(&args);
        assert_eq!(out.status.code(), Some(2));
        let abi = args[args.len() - 2].to_string_lossy().replace("-mabi=", "");
        assert!(
            stderr(&out).starts_with(&format!("merlin: unknown target ABI '{abi}'")),
            "{}",
            stderr(&out)
        );
    }
}

/// The target is RV64, LP64, with `-march` and `-mabi` or without them.
#[test]
fn the_target_is_lp64() {
    let t = TempSrc::new("march", "t.c", "unsigned long n = sizeof(long[2]);\n");
    let dump = |args: &[&str]| {
        let mut all: Vec<&Path> = args.iter().map(Path::new).collect();
        all.extend([Path::new("--dump-ast"), &t.file]);
        let out = merlin(&all);
        assert!(out.status.success(), "{}", stderr(&out));
        String::from_utf8_lossy(&out.stdout).replace('\r', "")
    };
    for args in [
        &[][..],
        &["-march=rv64ima_zicsr_zifencei"],
        &["-march=rv64ima_zicsr_zifencei", "-mabi=lp64"],
        &["-mabi=lp64", "-march=rv64ima_zicsr_zifencei"],
    ] {
        let ast = dump(args);
        assert!(ast.contains("Num 16 : unsigned long"), "{args:?}: {ast}");
    }
}
