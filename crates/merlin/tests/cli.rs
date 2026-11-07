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
            "{file}:2:17: error: too many arguments to function call, expected 0, have 2\n\
             {file}:1:5: note: 'f' declared here\n\
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
             {file}:4:17: error: too few arguments to function call, expected 2, have 1\n\
             {file}:1:5: note: 'two' declared here\n"
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
        stderr(&out).contains("1:10: fatal error: 'defs.h' file not found"),
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

/// `@FILE` stands for the arguments of FILE, read as clang reads a response
/// file (excalibur::response): separated by blanks, tabs and line ends,
/// quoted with `'` or `"`, a backslash taking the character after it; a
/// nested `@FILE` is expanded, a missing one is an argument as it is (an
/// input that can't be read, here). How Camelot's `cc` driver passes cc1,
/// merlin in C, more arguments than `exec` takes. One that names itself is
/// an error.
#[test]
fn response_files() {
    let t = TempSrc::new("rsp", "main.c", "int main() { return VALUE + OTHER; }\n");
    let esc = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    let at = |p: &Path| PathBuf::from(format!("@{}", p.display()));
    let more = t.dir.join("more");
    std::fs::write(&more, "'-DOTHER=(1 + 1)'\n").unwrap();
    let args = t.dir.join("args");
    std::fs::write(
        &args,
        format!("-E\n  \"-DVALUE=40\"\t\"{}\" @{}\n", esc(&t.file), esc(&more)),
    )
    .unwrap();
    let out = merlin(&[&at(&args)]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert!(text.contains("int main() { return 40 + (1 + 1); }"), "{text}");
    // Among other arguments, in its place: -U after it undefines VALUE.
    let out = merlin(&[&at(&args), Path::new("-UVALUE")]);
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    assert!(text.contains("return VALUE + (1 + 1);"), "{text}");
    // One that names itself, through another.
    let a = t.dir.join("a");
    let b = t.dir.join("b");
    std::fs::write(&a, format!("@{}", esc(&b))).unwrap();
    std::fs::write(&b, format!("@{}", esc(&a))).unwrap();
    let out = merlin(&[&at(&a)]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        format!(
            "recursive expansion of: '{}'\n",
            excalibur::response::absolute(&a).display()
        )
    );
    // A missing one is an input like any other.
    let missing = at(&t.dir.join("missing"));
    let out = merlin(&[&missing]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).starts_with(&format!("merlin: cannot read {}", missing.display())),
        "{}",
        stderr(&out)
    );
}

/// The options clang's `-cc1` takes that change nothing merlin compiles:
/// `-O` levels (merlin does not optimize), `-std=` C99, `-ffreestanding`;
/// what clang says of a level or a standard it doesn't take; and `-w` and
/// `-W`, which leave warnings out or make them errors as clang does.
#[test]
fn optimization_standard_and_warning_options() {
    let t = TempSrc::new("opts", "w.c", "int f(void) {\n    return 1 << 40;\n}\n");
    let flag = |s: &'static str| PathBuf::from(s);
    let plain = merlin(&[&flag("-o"), &flag("-"), &t.file]);
    assert!(plain.status.success(), "{}", stderr(&plain));
    let warning = stderr(&plain);
    assert!(
        warning.contains("warning: shift count >= width of type [-Wshift-count-overflow]"),
        "{warning}"
    );
    // -O takes what clang's -cc1 takes silently: 0 to 4 (as a 32-bit
    // number, `00` and `03` too), s, z, g and nothing.
    for opts in [
        &["-O2"][..],
        &["-O0", "-Os", "-Oz", "-Og", "-O"],
        &["-O4", "-O00", "-O03"],
        &["-std=c99", "-ffreestanding"],
        &["-std=gnu99"],
    ] {
        let mut args: Vec<PathBuf> = opts.iter().map(PathBuf::from).collect();
        args.extend([flag("-o"), flag("-"), t.file.clone()]);
        let refs: Vec<&Path> = args.iter().map(PathBuf::as_path).collect();
        let out = merlin(&refs);
        assert!(out.status.success(), "{opts:?}: {}", stderr(&out));
        assert_eq!(out.stdout, plain.stdout, "{opts:?}");
        assert_eq!(stderr(&out), warning, "{opts:?}");
    }
    let said = |args: &[&str]| {
        let mut all: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        all.extend([flag("-o"), flag("-"), t.file.clone()]);
        let refs: Vec<&Path> = all.iter().map(PathBuf::as_path).collect();
        let out = merlin(&refs);
        (out.status.code(), stderr(&out), out.stdout.is_empty())
    };
    let file = t.file.display().to_string();
    // Anything but a 32-bit number is clang's error; a number it has no
    // level for its warning, which comes before the source's.
    for bad in ["x", "+3", "3x", "99999999999"] {
        assert_eq!(
            said(&[format!("-O{bad}").as_str()]),
            (
                Some(1),
                format!("error: invalid integral value '{bad}' in '-O{bad}'\n"),
                true
            )
        );
    }
    // (4 alone is clang's -O4 option; `04` its -O with a number to judge.)
    for odd in ["9", "-1", "04", "004"] {
        let o = said(&[format!("-O{odd}").as_str()]);
        assert_eq!(o.0, Some(0));
        assert_eq!(
            o.1,
            format!("warning: optimization level '-O{odd}' is not supported; using '-O3' instead\n{warning}")
        );
    }
    // A warning given as the options are read, before -w and -W apply, as
    // clang's -cc1 gives it.
    let clean = TempSrc::new("opts-clean", "c.c", "int f(void) {\n    return 1;\n}\n");
    for args in [&["-w", "-O9"][..], &["-Werror", "-O9"]] {
        let mut all: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        all.extend([flag("-o"), flag("-"), clean.file.clone()]);
        let refs: Vec<&Path> = all.iter().map(PathBuf::as_path).collect();
        let out = merlin(&refs);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert_eq!(
            stderr(&out),
            "warning: optimization level '-O9' is not supported; using '-O3' instead\n",
            "{args:?}"
        );
    }
    // -std=: an unknown standard is clang's error and its notes, one for
    // each C standard it knows; another language's, its other error.
    let foo = said(&["-std=foo"]);
    let notes: Vec<&str> = foo.1.lines().skip(1).collect();
    assert_eq!((foo.0, foo.2), (Some(1), true));
    assert!(
        foo.1.starts_with("error: invalid value 'foo' in '-std=foo'\n"),
        "{}",
        foo.1
    );
    assert_eq!(notes.len(), 13, "{}", foo.1);
    assert_eq!(
        notes[0],
        "note: use 'c89', 'c90', or 'iso9899:1990' for 'ISO C 1990' standard"
    );
    assert_eq!(
        notes[12],
        "note: use 'gnu2y' for 'Working Draft for ISO C2y with GNU extensions' standard"
    );
    assert!(said(&["-std=cuda"])
        .1
        .starts_with("error: invalid value 'cuda' in '-std=cuda'\nnote: "));
    for other in ["c++11", "gnu++26", "c++2c", "cl1.2", "clc++", "CL2.0", "hlsl2021"] {
        assert_eq!(
            said(&[format!("-std={other}").as_str()]),
            (
                Some(1),
                format!("error: invalid argument '-std={other}' not allowed with 'C'\n"),
                true
            )
        );
    }
    // Not clang's names: `c++` alone, a C++ standard it has none of, and
    // `CL1.0` (`cl1.0` is OpenCL's).
    for none in ["c++", "c++29", "CL1.0"] {
        let s = said(&[format!("-std={none}").as_str()]);
        assert!(
            s.1.starts_with(&format!("error: invalid value '{none}' in '-std={none}'\nnote: ")),
            "{}",
            s.1
        );
    }
    // Only the last -O and the last -std= are judged, the -std= first.
    assert_eq!(said(&["-O9", "-O2"]), (Some(0), warning.clone(), false));
    assert_eq!(said(&["-Ofoo", "-O2"]), (Some(0), warning.clone(), false));
    assert_eq!(said(&["-std=foo", "-std=c99"]), (Some(0), warning.clone(), false));
    assert_eq!(said(&["-std=c11", "-std=c99"]), (Some(0), warning.clone(), false));
    let both = said(&["-O9", "-std=c++11"]);
    assert_eq!(
        both,
        (
            Some(1),
            "error: invalid argument '-std=c++11' not allowed with 'C'\n\
             warning: optimization level '-O9' is not supported; using '-O3' instead\n"
                .into(),
            true
        )
    );
    assert_eq!(
        said(&["-std=c11"]),
        (
            Some(1),
            "error: merlin compiles C99: '-std=c11' is not supported\n".into(),
            true
        )
    );
    // -w and -Wno-FLAG leave the warning out; -WFLAG and -Wall change
    // nothing; -Werror makes it an error, which fails the compilation (no
    // output), and -Wno-error, or -Wno-error=FLAG, undoes it.
    for (args, said_text) in [
        (&["-w"][..], String::new()),
        (&["-Wno-shift-count-overflow"], String::new()),
        (&["-Wall", "-Wextra", "-Wshift-count-overflow"], warning.clone()),
        (&["-Werror", "-Wno-error"], warning.clone()),
        (&["-Werror", "-Wno-error=shift-count-overflow"], warning.clone()),
        (
            &["-Wno-shift-count-overflow", "-Wshift-count-overflow"],
            warning.clone(),
        ),
    ] {
        assert_eq!(said(args), (Some(0), said_text, false), "{args:?}");
    }
    let error =
        format!("{file}:2:14: error: shift count >= width of type [-Werror,-Wshift-count-overflow]\n");
    assert_eq!(said(&["-Werror"]), (Some(1), error.clone(), true));
    assert_eq!(said(&["-Werror=shift-count-overflow"]), (Some(1), error, true));
    assert_eq!(said(&["-w", "-Werror"]), (Some(0), String::new(), false));
}

/// A -W option names a warning's flag or one of clang's groups that hold
/// it (crates/merlin/src/warning_groups.txt, from `diagtool tree`), and the
/// options apply in command-line order, as clang's do: these are clang
/// 23's answers (`clang -cc1 -fno-caret-diagnostics`).
#[test]
fn warning_groups_and_order() {
    let t = TempSrc::new(
        "groups",
        "w.c",
        "int f(unsigned char c) { return c > 300; }\nint g(int x) { if (x) return 1; }\n",
    );
    let file = t.file.display().to_string();
    let compare = |error: bool| {
        format!(
            "{file}:1:35: {}: result of comparison of constant 300 with expression of type 'unsigned char' \
             is always false [{}-Wtautological-constant-out-of-range-compare]\n",
            if error { "error" } else { "warning" },
            if error { "-Werror," } else { "" }
        )
    };
    let returns = |error: bool| {
        format!(
            "{file}:2:33: {}: non-void function does not return a value in all control paths [{}-Wreturn-type]\n",
            if error { "error" } else { "warning" },
            if error { "-Werror," } else { "" }
        )
    };
    let said = |args: &[&str]| {
        let mut all: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        all.extend([PathBuf::from("-o"), PathBuf::from("-"), t.file.clone()]);
        let refs: Vec<&Path> = all.iter().map(PathBuf::as_path).collect();
        let out = merlin(&refs);
        (out.status.code(), stderr(&out))
    };
    let both = format!("{}{}", compare(false), returns(false));
    for (args, code, text) in [
        (&[][..], 0, both.clone()),
        // A group and the flag in it.
        (&["-Wno-tautological-compare"], 0, returns(false)),
        (
            &["-Werror=tautological-compare"],
            1,
            format!("{}{}", compare(true), returns(false)),
        ),
        (&["-Wno-everything"], 0, String::new()),
        (
            &["-Wall", "-Wno-error", "-Werror=all"],
            1,
            format!("{}{}", compare(true), returns(true)),
        ),
        // In order: -Werror=X turns X on again, and -Wno-X after it off.
        (
            &["-Wno-return-type", "-Werror=return-type"],
            1,
            format!("{}{}", compare(false), returns(true)),
        ),
        (&["-Werror=return-type", "-Wno-return-type"], 0, compare(false)),
        (
            &["-Werror", "-Wno-error=return-type"],
            1,
            format!("{}{}", compare(true), returns(false)),
        ),
        // Each warning's state, as clang keeps it: -WX does not undo an
        // error, but after -Wno-X it gives a warning again; -Wno-error=X
        // holds against a later -Werror; -Weverything turns on nothing
        // merlin gives (each is on by default) and undoes no -Wno-.
        (
            &["-Werror=return-type", "-Wreturn-type"],
            1,
            format!("{}{}", compare(false), returns(true)),
        ),
        (
            &["-Werror=return-type", "-Wno-return-type", "-Wreturn-type"],
            0,
            both.clone(),
        ),
        (
            &[
                "-Wno-error=return-type",
                "-Werror=return-type",
                "-Wno-return-type",
                "-Wreturn-type",
                "-Werror",
            ],
            1,
            format!("{}{}", compare(true), returns(false)),
        ),
        (&["-Wno-everything", "-Weverything"], 0, String::new()),
        (&["-Wno-everything", "-Wreturn-type"], 0, returns(false)),
        // `everything` is no group for -Werror= or -Wno-error= (clang warns
        // of an unknown option, which merlin leaves out).
        (&["-Werror=everything"], 0, both.clone()),
        (
            &["-Werror", "-Wno-error=everything"],
            1,
            format!("{}{}", compare(true), returns(true)),
        ),
    ] {
        assert_eq!(said(args), (Some(code), text), "{args:?}");
    }
    // An unflagged warning made an error is marked `[-Werror]`, as clang
    // marks it; #warning's flag, `#warnings`, is in clang's group `cpp`.
    let other = TempSrc::new("unflagged", "u.c", "int h = 'abcde';\n#warning hello\n");
    let file = other.file.display().to_string();
    let said = |args: &[&str]| {
        let mut all = vec![PathBuf::from("-Wno-multichar")];
        all.extend(args.iter().map(PathBuf::from));
        all.extend([PathBuf::from("-o"), PathBuf::from("-"), other.file.clone()]);
        let refs: Vec<&Path> = all.iter().map(PathBuf::as_path).collect();
        let out = merlin(&refs);
        (out.status.code(), stderr(&out))
    };
    let long = format!("{file}:1:9: warning: character constant too long for its type\n");
    let hello = format!("{file}:2:2: warning: hello [-W#warnings]\n");
    assert_eq!(said(&[]), (Some(0), format!("{long}{hello}")));
    assert_eq!(
        said(&["-Werror", "-Wno-error=cpp"]),
        (
            Some(1),
            format!("{file}:1:9: error: character constant too long for its type [-Werror]\n{hello}")
        )
    );
    assert_eq!(said(&["-Wno-cpp"]), (Some(0), long.clone()));
    assert_eq!(
        said(&["-Werror=cpp"]),
        (
            Some(1),
            format!("{long}{file}:2:2: error: hello [-Werror,-W#warnings]\n")
        )
    );
}

/// `-o -` writes to standard output, as clang's `-cc1` does, and `-` is
/// standard input, named `<stdin>` in diagnostics and `-` in the assembly
/// (its STT_FILE symbol, clang's `-main-file-name -`); without -o, standard
/// input's assembly goes to standard output and a file's to the file with
/// `.s` for its `.c` or `.i`. `-x cpp-output` (and a `.i` file) is C
/// already preprocessed, as clang's -cc1 takes it: every directive runs,
/// but macros (the predefined ones too) are expanded only in directives
/// (clang's `SetMacroExpansionOnlyInDirectives`). With -g, standard input
/// is `-` for the compile unit and `.file 0`, its code `<stdin>`'s, file 1.
#[test]
fn standard_input_and_output_and_preprocessed_c() {
    use std::io::Write;
    for args in [&["-o", "-", "-"][..], &["-"]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_merlin"))
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"int f(void) {\n    return 1 << 40;\n}\n")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{args:?}");
        let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
        assert!(text.starts_with("    .file \"-\"\n"), "{args:?}: {text}");
        assert_eq!(
            stderr(&out),
            "<stdin>:2:14: warning: shift count >= width of type [-Wshift-count-overflow]\n",
            "{args:?}"
        );
    }
    // With -g: the compile unit and `.file 0` are `-`, the code in file 1,
    // `<stdin>`, with the same checksum, as clang's (`clang -S -g -x c -`).
    let mut child = Command::new(env!("CARGO_BIN_EXE_merlin"))
        .args(["-g", "-fdebug-compilation-dir=/d", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"int f(int x) {\n    return x + 1;\n}\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    let md5 = "md5 0x35f70dd2f583fcb55046dff5372666f4";
    assert!(text.starts_with("    .file \"-\"\n"), "{text}");
    assert!(
        text.contains(&format!(
            "    .file 0 \"/d\" \"-\" {md5}\n    .file 1 \"<stdin>\" {md5}\n"
        )),
        "{text}"
    );
    assert!(text.contains("    .loc 1 1 0\n"), "{text}");
    assert!(text.contains("    .asciz \"-\"\n"), "{text}");
    let t = TempSrc::new(
        "cppout",
        "p.i",
        "# 1 \"orig.c\"\nint f(void) {\n    return 1 << 40;\n}\n",
    );
    for args in [
        vec![t.file.clone()],
        vec![PathBuf::from("-xcpp-output"), t.file.clone()],
    ] {
        let mut all = vec![PathBuf::from("-o"), PathBuf::from("-")];
        all.extend(args);
        let refs: Vec<&Path> = all.iter().map(PathBuf::as_path).collect();
        let out = merlin(&refs);
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(
            stderr(&out),
            "orig.c:2:14: warning: shift count >= width of type [-Wshift-count-overflow]\n"
        );
    }
    // p.i without -o: p.s, as clang names it.
    let out = merlin(&[&t.file]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(t.dir.join("p.s").exists());
    assert!(!t.dir.join("p.i.s").exists());
    // Macros are defined, the predefined ones too, but expanded only in
    // directives: X is undeclared in the text, __riscv an identifier.
    let pre = TempSrc::new(
        "cppdirs",
        "pp.i",
        "# 1 \"orig.c\"\n#define X 1\nint a = X;\nint __riscv = 2;\n#pragma once\n#line 40 \"z.c\"\nint b = Y;\n",
    );
    let out = merlin(&[Path::new("-o"), Path::new("-"), &pre.file]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "orig.c:2:9: error: use of undeclared identifier 'X'\n\
         orig.c:4:9: warning: #pragma once in main file [-Wpragma-once-outside-header]\n\
         z.c:40:9: error: use of undeclared identifier 'Y'\n"
    );
    // Every directive runs, its macros expanded, as clang's -cc1 runs them
    // in a preprocessed file: these are clang 23's answers.
    for (src, code, said) in [
        ("#if 0\nint a = ;\n#endif\nint b;\n", 0, String::new()),
        (
            "#define X 0\n#if X\nint a = ;\n#endif\nint b;\n",
            0,
            String::new(),
        ),
        (
            "#define F(x) x\n#if F(0)\nint a = ;\n#endif\n#line F(30) \"q.c\"\nint b = F;\n",
            1,
            "q.c:30:9: error: use of undeclared identifier 'F'\n".to_string(),
        ),
        (
            "#if __riscv\nint a = ;\n#endif\nint b;\n",
            1,
            "{}:2:9: error: expected expression\n".to_string(),
        ),
        (
            "#include \"nope.h\"\nint b;\n",
            1,
            "{}:1:10: fatal error: 'nope.h' file not found\n".to_string(),
        ),
        ("#error boom\nint b;\n", 1, "{}:1:2: error: boom\n".to_string()),
        (
            "#warning boom\nint b;\n",
            0,
            "{}:1:2: warning: boom [-W#warnings]\n".to_string(),
        ),
        (
            "#foo\nint b;\n",
            1,
            "{}:1:2: error: invalid preprocessing directive\n".to_string(),
        ),
    ] {
        let t = TempSrc::new("cppdirect", "d.i", src);
        let out = merlin(&[Path::new("-o"), Path::new("-"), &t.file]);
        let want = said.replace("{}", &t.file.display().to_string());
        assert_eq!((out.status.code(), stderr(&out)), (Some(code), want), "{src}");
    }
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
        stderr(&out).contains("1:3: error: invalid preprocessing directive\n"),
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

    // The driver's -Wp, -Wa, and -Wl, are no warning options: merlin, as
    // clang's -cc1, is given what they hold, never them.
    for opt in ["-Wp,-DFOO=1", "-Wa,--noexecstack", "-Wl,-S"] {
        let out = merlin(&[Path::new(opt), Path::new("a.c")]);
        assert_eq!(out.status.code(), Some(2), "{opt}");
        assert!(
            stderr(&out).starts_with(&format!("merlin: unknown option {opt}")),
            "{}",
            stderr(&out)
        );
    }

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

/// What clang's `-cc1 -v` says, committed (`tests/cc1-v.txt`) for the case
/// `-v_says_what_clang_cc1_says` runs: its lines but for clang's version
/// (`VERSION`) and its own system directories (the resource directory,
/// `/usr/include`), which merlin has none of.
const CC1_V: &str = "tests/cc1-v.txt";

/// The directories of `-v`'s case, in `dir`: `inc` and `inc2` there,
/// `nope` not, `inc` given twice.
fn v_case(dir: &Path) -> Vec<String> {
    std::fs::create_dir_all(dir.join("inc")).unwrap();
    std::fs::create_dir_all(dir.join("inc2")).unwrap();
    std::fs::write(dir.join("v.c"), "int x;\n").unwrap();
    ["-v", "-I", "inc", "-I", "nope", "-I", "inc", "-I", "inc2", "v.c"]
        .map(String::from)
        .to_vec()
}

/// `text` with its first line, the version, as `VERSION`, and clang's
/// system directories' lines left out.
fn v_lines(text: &str) -> String {
    let mut lines = text.lines();
    let mut out = vec!["VERSION".to_string()];
    lines.next();
    for l in lines {
        let system = l.contains("/usr/local/include")
            || l.contains("\"/usr/include\"")
            || (l.starts_with(' ') && l.to_ascii_lowercase().contains("clang"));
        if !system {
            out.push(l.to_string());
        }
    }
    out.join("\n") + "\n"
}

/// `-v` says merlin's version, then before each input the `#include` search
/// list as clang's `-cc1 -v` says it: the `-I` directories that are not
/// there, then those given again, left out and said so, then the two
/// lists, the quoted one with nothing of its own (merlin searches the
/// including file's directory first, which clang does not list either, and
/// has no `-iquote`), the angled one the `-I` directories in order. The
/// lines are clang's (`tests/cc1-v.txt`, which the ignored
/// `clang_cc1_v_says_what_merlin_says` holds clang to).
#[test]
fn v_says_what_clang_cc1_says() {
    let t = TempSrc::new("v", "unused.c", "");
    let args = v_case(&t.dir);
    let out = Command::new(env!("CARGO_BIN_EXE_merlin"))
        .args(&args)
        .args(["-o", "-"])
        .current_dir(&t.dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let said = stderr(&out);
    assert!(
        said.starts_with(&format!(
            "merlin version {} default target riscv64\n",
            env!("CARGO_PKG_VERSION")
        )),
        "{said}"
    );
    let want = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(CC1_V))
        .unwrap()
        .replace('\r', "");
    assert_eq!(v_lines(&said), want);

    // The options' own diagnostics come first, as clang's -cc1 gives them
    // as it reads its command line; an error there and it never starts, so
    // there is no version line.
    let opts = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_merlin"))
            .args(["-v"])
            .args(extra)
            .args(["-o", "-", "v.c"])
            .current_dir(&t.dir)
            .output()
            .unwrap()
    };
    let o9 = opts(&["-O9"]);
    assert!(o9.status.success());
    assert!(stderr(&o9).starts_with(&format!(
        "warning: optimization level '-O9' is not supported; using '-O3' instead\nmerlin version {} ",
        env!("CARGO_PKG_VERSION")
    )));
    let std = opts(&["-std=foo"]);
    assert_eq!(std.status.code(), Some(1));
    assert!(stderr(&std).starts_with("error: invalid value 'foo' in '-std=foo'\n"));
    assert!(!stderr(&std).contains("merlin version"));
}

/// clang's `-cc1 -v` says, for `-v`'s case, what `tests/cc1-v.txt` holds
/// (`MERLIN_BLESS=1` writes it). Needs clang on the PATH: `cargo test -p
/// merlin --test cli -- --ignored clang_cc1_v`.
#[test]
#[ignore]
fn clang_cc1_v_says_what_merlin_says() {
    let t = TempSrc::new("clang-v", "unused.c", "");
    let args = v_case(&t.dir);
    let out = Command::new("clang")
        .args(["-cc1", "-triple", "riscv64", "-fsyntax-only"])
        .args(&args)
        .current_dir(&t.dir)
        .output()
        .expect("clang on the PATH");
    assert!(out.status.success(), "{}", stderr(&out));
    let said = v_lines(&stderr(&out));
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(CC1_V);
    if std::env::var_os("MERLIN_BLESS").is_some() {
        std::fs::write(&path, &said).unwrap();
    }
    assert_eq!(said, std::fs::read_to_string(&path).unwrap().replace('\r', ""));
}
