//! merlin's diagnostics are clang 23's: every diagnostic case of merlin's
//! tests is a row of the table in `tests/diagnostics/`, a source (with the
//! headers it includes) and the diagnostics clang gives on it, the first
//! line of each as clang prints it with `-fno-caret-diagnostics` (the
//! position, `error:` or `warning:`, the message and the `[-W...]` flag),
//! and the notes merlin gives with it. The file is `t.c`, left out of the
//! lines; a line in a header names it (`h.h:1:9: error: ...`).
//!
//! * `merlin_gives_the_tables_diagnostics` (run by `cargo test`, no
//!   external tools): merlin, compiling each source for RV32 and RV64,
//!   gives exactly the diagnostics of its row.
//! * `clang_gives_the_tables_diagnostics` (ignored: needs clang on `PATH`):
//!   clang 23 (`clang --target=riscv32 -mabi=ilp32` and `--target=riscv64
//!   -mabi=lp64`, with `-std=c99 -ffreestanding -fsyntax-only`, merlin's
//!   language) gives the same errors and warnings, with the notes the row
//!   has among its own; and so, merlin's diagnostics are clang's.
//!
//!       cargo test -p merlin --test diagnostics -- --ignored
//!
//! * `clang_warns_as_merlin_does_on_the_programs` (ignored: needs clang):
//!   on every whole program of merlin's tests (`tests/cases`,
//!   `tests/cases-merlin`, `tests/multi`) and every C file of the Camelot
//!   kernel and its user programs, merlin and clang give the same
//!   diagnostics, the default warnings clang gives on them included.
//!
//! A row may say three more things (the spec, "Diagnostics", says why each
//! is so):
//!
//! * `.then(...)`: what clang reports after a syntax error, at which
//!   merlin's preprocessor and parser stop (clang recovers and goes on).
//! * `.merlin(why)`: a diagnostic of merlin's own, for what clang takes and
//!   merlin's language has not (a variable-length array, `float`): clang
//!   gives no error on the source.
//! * `.rv32()`, `.rv64()`: a row of one target, where clang's diagnostics
//!   differ with the width.
//!
//! merlin's implementation limits (how deep expressions and statements
//! nest, how many parameters, how large a frame or a function) are not
//! clang's, and are checked by the tests of the parser and the code
//! generator.

#[path = "diagnostics/codegen.rs"]
mod codegen;
#[path = "diagnostics/lexer.rs"]
mod lexer;
#[path = "diagnostics/parser.rs"]
mod parser;
#[path = "diagnostics/preprocessor.rs"]
mod preprocessor;
#[path = "diagnostics/sema.rs"]
mod sema;
#[path = "diagnostics/warnings.rs"]
mod warnings;

use merlin::types::Xlen;
use std::path::{Component, Path};
use std::process::Command;

/// One row: a source and its diagnostics.
pub struct Case {
    src: String,
    /// The headers it includes, by name (next to `t.c`) and text.
    files: Vec<(&'static str, &'static str)>,
    /// The diagnostics, as clang prints them (with `t.c:` left out).
    want: Vec<String>,
    /// clang's errors after merlin's syntax error, where merlin stops.
    then: Vec<String>,
    /// Why the diagnostic is merlin's own: clang takes the source.
    merlin: Option<&'static str>,
    /// The one target the row is for, or both.
    xlen: Option<Xlen>,
    /// `-D` and `-U` options, in order (`-DN=1`, `-UN`).
    args: Vec<&'static str>,
    /// Where the row is written, for `MERLIN_BLESS`.
    at: &'static std::panic::Location<'static>,
}

/// A row of both targets.
#[track_caller]
pub fn case(src: impl Into<String>, want: &[&str]) -> Case {
    Case {
        at: std::panic::Location::caller(),
        src: src.into(),
        files: Vec::new(),
        want: want.iter().map(|s| s.to_string()).collect(),
        then: Vec::new(),
        merlin: None,
        xlen: None,
        args: Vec::new(),
    }
}

impl Case {
    pub fn files(self, files: &[(&'static str, &'static str)]) -> Case {
        Case {
            files: files.to_vec(),
            ..self
        }
    }

    pub fn then(self, then: &[&str]) -> Case {
        Case {
            then: then.iter().map(|s| s.to_string()).collect(),
            ..self
        }
    }

    pub fn merlin(self, why: &'static str) -> Case {
        Case {
            merlin: Some(why),
            ..self
        }
    }

    pub fn rv32(self) -> Case {
        Case {
            xlen: Some(Xlen::Rv32),
            ..self
        }
    }

    pub fn rv64(self) -> Case {
        Case {
            xlen: Some(Xlen::Rv64),
            ..self
        }
    }

    pub fn args(self, args: &[&'static str]) -> Case {
        Case {
            args: args.to_vec(),
            ..self
        }
    }

    fn widths(&self) -> Vec<Xlen> {
        match self.xlen {
            Some(x) => vec![x],
            None => vec![Xlen::Rv32, Xlen::Rv64],
        }
    }
}

fn table() -> Vec<Case> {
    [
        lexer::cases(),
        preprocessor::cases(),
        parser::cases(),
        sema::cases(),
        warnings::cases(),
        codegen::cases(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn width(x: Xlen) -> &'static str {
    match x {
        Xlen::Rv32 => "rv32",
        Xlen::Rv64 => "rv64",
    }
}

/// A path as the table writes it: `/` between its parts, no `.` among them.
fn plain(path: &str) -> String {
    let parts: Vec<String> = Path::new(&path.replace('\\', "/"))
        .components()
        .filter(|c| *c != Component::CurDir)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// A diagnostic's line as the table writes it: without `t.c:`, a header's
/// path plain.
fn table_line(line: &str) -> String {
    match line.strip_prefix("t.c:") {
        Some(rest) => rest.to_string(),
        None => match line.split_once(':') {
            Some((file, rest)) if !file.is_empty() && !file.starts_with('<') => {
                format!("{}:{rest}", plain(file))
            }
            _ => line.to_string(),
        },
    }
}

/// What merlin says of a row's source for the target `x`: every line of
/// every diagnostic, notes included.
fn merlin_lines(c: &Case, x: Xlen) -> Vec<String> {
    let files: Vec<(String, &str)> = c.files.iter().map(|&(n, t)| (plain(n), t)).collect();
    let load = move |p: &Path| {
        let p = plain(&p.to_string_lossy());
        files.iter().find(|(n, _)| *n == p).map(|(_, t)| t.to_string())
    };
    let macros = c
        .args
        .iter()
        .map(|a| match (a.strip_prefix("-D"), a.strip_prefix("-U")) {
            (Some(d), _) => merlin::CmdMacro::Define(d.to_string()),
            (_, Some(u)) => merlin::CmdMacro::Undef(u.to_string()),
            _ => panic!("{a}: -D or -U"),
        })
        .collect();
    let opts = merlin::Options {
        xlen: x,
        macros,
        ..merlin::Options::default()
    };
    let diagnostics = match merlin::compile_warned("t.c", &c.src, &opts, &load) {
        Ok((_, warnings)) => warnings,
        Err(errors) => errors,
    };
    diagnostics
        .iter()
        .flat_map(|d| d.render("t.c").lines().map(table_line).collect::<Vec<_>>())
        .collect()
}

#[test]
fn merlin_gives_the_tables_diagnostics() {
    let mut failures = Vec::new();
    let mut count = 0;
    for c in table() {
        for x in c.widths() {
            count += 1;
            let got = merlin_lines(&c, x);
            if got != c.want {
                failures.push(format!(
                    "{}:{}: {} ({}):\n  want {:?}\n  got  {:?}",
                    c.at.file(),
                    c.at.line(),
                    c.src,
                    width(x),
                    c.want,
                    got
                ));
            }
        }
    }
    assert!(count > 400, "only {count} rows");
    assert!(
        failures.is_empty(),
        "{} of {count} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// clang's options for the target `x`, as the harness gives them, with
/// merlin's language: C99, freestanding.
fn clang(x: Xlen) -> Command {
    let mut c = Command::new("clang");
    match x {
        Xlen::Rv32 => c.args(["--target=riscv32", "-march=rv32ima_zicsr_zifencei", "-mabi=ilp32"]),
        Xlen::Rv64 => c.args([
            "--target=riscv64",
            "-march=rv64ima_zicsr_zifencei",
            "-mabi=lp64",
            "-mcmodel=medany",
        ]),
    };
    c.args([
        "-fsyntax-only",
        "-fno-caret-diagnostics",
        "-fno-color-diagnostics",
    ]);
    c
}

/// The diagnostics in clang's output, each with its notes, as the table
/// writes them: `-fno-caret-diagnostics` leaves a line per diagnostic and
/// per note, and lines of context (`In file included from`), left out.
fn diagnostics(stderr: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in stderr.lines() {
        let line = table_line(line.trim_end());
        let Some(kind) = ["error", "fatal error", "warning", "note"]
            .into_iter()
            .find(|k| line.contains(&format!(": {k}: ")))
        else {
            continue;
        };
        if kind == "note" {
            if let Some(last) = out.last_mut() {
                last.1.push(line);
            }
        } else {
            out.push((line, Vec::new()));
        }
    }
    out
}

/// The table's lines grouped as [`diagnostics`] groups clang's.
fn grouped(lines: &[String]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for l in lines {
        if l.contains(": note: ") {
            out.last_mut()
                .expect("a note follows a diagnostic")
                .1
                .push(l.clone());
        } else {
            out.push((l.clone(), Vec::new()));
        }
    }
    out
}

/// What clang says of a row's source for the target `x`, written to a
/// directory of its own under `scratch`.
fn clang_diagnostics(c: &Case, x: Xlen, scratch: &Path, n: usize) -> Vec<(String, Vec<String>)> {
    let dir = scratch.join(format!("{n}-{}", width(x)));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in &c.files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    std::fs::write(dir.join("t.c"), &c.src).unwrap();
    let out = clang(x)
        .args(["-std=c99", "-ffreestanding"])
        .args(&c.args)
        .arg("t.c")
        .current_dir(&dir)
        .output()
        .expect("clang on PATH");
    diagnostics(&String::from_utf8_lossy(&out.stderr))
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("merlin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Whether clang's diagnostics `got` are what the row says: its errors
/// and warnings, in clang's order (then those merlin stops before), each of
/// its notes among those clang gives with the diagnostic; for a row of
/// merlin's own, no error.
fn agrees(c: &Case, got: &[(String, Vec<String>)]) -> bool {
    if c.merlin.is_some() {
        return !got
            .iter()
            .any(|(l, _)| l.contains(": error: ") || l.contains(": fatal error: "));
    }
    let want = grouped(&[c.want.clone(), c.then.clone()].concat());
    want.len() == got.len()
        && want.iter().zip(got).all(|((w, wn), (g, gn))| {
            w == g && {
                let mut gn = gn.iter();
                wn.iter().all(|n| gn.any(|g| g == n))
            }
        })
}

/// Each row's diagnostics are clang's, for each of its targets. With
/// `MERLIN_BLESS=1`, a row that clang disagrees with is rewritten to say
/// what clang says (with the notes merlin gives that clang gives too),
/// unless it has `.then` or `.merlin`, or clang's differ with the width,
/// or the row is not a `case("...", &[...])` on one line: those are left
/// to edit by hand.
#[test]
#[ignore = "needs clang on PATH"]
fn clang_gives_the_tables_diagnostics() {
    let dir = scratch("diagnostics");
    let rows = table();
    let jobs: Vec<_> = rows
        .iter()
        .enumerate()
        .flat_map(|(n, c)| c.widths().into_iter().map(move |x| (n, x)))
        .collect();
    type Got = Vec<(String, Vec<String>)>;
    let results: Vec<(usize, Xlen, Got)> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .chunks(jobs.len().div_ceil(16))
            .map(|chunk| {
                let (dir, rows) = (&dir, &rows);
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|&(n, x)| (n, x, clang_diagnostics(&rows[n], x, dir, n)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let _ = std::fs::remove_dir_all(&dir);
    let bless_mode = std::env::var_os("MERLIN_BLESS").is_some();
    let mut failures = Vec::new();
    let mut blessed = Vec::new();
    for (n, c) in rows.iter().enumerate() {
        let got: Vec<_> = results.iter().filter(|r| r.0 == n).collect();
        let wrong: Vec<_> = got.iter().filter(|(_, _, g)| !agrees(c, g)).collect();
        let same = got.iter().all(|r| r.2 == got[0].2);
        // Blessing rewrites a row clang agrees with too, when merlin gives
        // more of clang's notes than the row has.
        if bless_mode && c.then.is_empty() && c.merlin.is_none() && same {
            let merlin = merlin_lines(c, got[0].1);
            let lines: Vec<String> = got[0]
                .2
                .iter()
                .flat_map(|(l, notes)| {
                    std::iter::once(l.clone()).chain(notes.iter().filter(|n| merlin.contains(n)).cloned())
                })
                .collect();
            if lines == c.want {
                continue;
            }
            match bless(c.at, &lines) {
                Ok(()) => {
                    blessed.push(c.at);
                    continue;
                }
                Err(e) => failures.push(format!("{}:{}: cannot bless: {e}", c.at.file(), c.at.line())),
            }
        }
        if wrong.is_empty() {
            continue;
        }
        for (_, x, g) in wrong {
            failures.push(format!(
                "{}:{}: {} ({}){}:\n  want {:?}{}\n  clang {:?}",
                c.at.file(),
                c.at.line(),
                c.src,
                width(*x),
                c.merlin.map(|_| " (merlin's own)").unwrap_or_default(),
                c.want,
                if c.then.is_empty() {
                    String::new()
                } else {
                    format!(" then {:?}", c.then)
                },
                g
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} differ from clang{}:\n{}",
        failures.len(),
        if blessed.is_empty() {
            String::new()
        } else {
            format!(" ({} rows blessed)", blessed.len())
        },
        failures.join("\n")
    );
}

/// A whole C program file to compare: where it is (from the workspace's
/// root), the directories its headers are in (from its own), and whether
/// it is the kernel's or a user program's, which have no C library
/// (`-fno-builtin -nostdinc`, as their builds have).
struct Program {
    path: std::path::PathBuf,
    include: Vec<&'static str>,
    camelot: bool,
}

/// Every C file of merlin's whole-program tests (`tests/cases`,
/// `tests/cases-merlin`, `tests/multi`), and of the Camelot kernel and its
/// user programs.
fn programs() -> Vec<Program> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let c_files = |dir: &str| -> Vec<std::path::PathBuf> {
        let mut files: Vec<_> = std::fs::read_dir(root.join(dir))
            .unwrap_or_else(|e| panic!("{dir}: {e}"))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "c"))
            .map(|p| Path::new(dir).join(p.file_name().unwrap()))
            .collect();
        files.sort();
        files
    };
    let mut out = Vec::new();
    for dir in ["crates/merlin/tests/cases", "crates/merlin/tests/cases-merlin"] {
        for path in c_files(dir) {
            out.push(Program {
                path,
                include: Vec::new(),
                camelot: false,
            });
        }
    }
    let mut multi: Vec<_> = std::fs::read_dir(root.join("crates/merlin/tests/multi"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    multi.sort();
    for name in multi {
        let dir = format!("crates/merlin/tests/multi/{name}");
        let include = if root.join(&dir).join("include").is_dir() {
            vec!["include"]
        } else {
            Vec::new()
        };
        for path in c_files(&dir) {
            out.push(Program {
                path,
                include: include.clone(),
                camelot: false,
            });
        }
    }
    for (dir, include) in [
        ("kernel", vec!["include"]),
        ("kernel/net", vec!["../include"]),
        ("kernel/user", vec![]),
        ("kernel/user/lib", vec![".."]),
        ("kernel/user/as", vec![".."]),
        ("kernel/user/ld", vec![".."]),
    ] {
        for path in c_files(dir) {
            out.push(Program {
                path,
                include: include.clone(),
                camelot: true,
            });
        }
    }
    out
}

/// Every line merlin gives for the program `p` for the target `x`, as
/// [`table_line`] writes them, the file named as it is from its directory.
fn merlin_program_lines(p: &Program, x: Xlen) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join(p.path.parent().unwrap());
    let name = p.path.file_name().unwrap().to_string_lossy().into_owned();
    let src = std::fs::read_to_string(root.join(&p.path)).unwrap();
    let opts = merlin::Options {
        xlen: x,
        include_dirs: p.include.iter().map(std::path::PathBuf::from).collect(),
        ..merlin::Options::default()
    };
    let load = |path: &Path| std::fs::read_to_string(dir.join(path)).ok();
    let diagnostics = match merlin::compile_warned(&name, &src, &opts, &load) {
        Ok((_, warnings)) => warnings,
        Err(errors) => errors,
    };
    diagnostics
        .iter()
        .flat_map(|d| {
            d.render(&name)
                .lines()
                .map(|l| plain_line(&table_line(l)))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Every line clang gives for the program `p` for the target `x`, as
/// [`merlin_program_lines`] writes merlin's.
fn clang_program_lines(p: &Program, x: Xlen) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = root.join(p.path.parent().unwrap());
    let mut c = clang(x);
    c.args(["-std=c99", "-ffreestanding"]);
    if p.camelot {
        c.args(["-fno-builtin", "-nostdinc"]);
    }
    for i in &p.include {
        c.arg(format!("-I{i}"));
    }
    let out = c
        .arg(p.path.file_name().unwrap())
        .current_dir(&dir)
        .output()
        .expect("clang on PATH");
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| {
            [": error: ", ": fatal error: ", ": warning: ", ": note: "]
                .iter()
                .any(|k| l.contains(k))
        })
        .map(|l| plain_line(&table_line(l.trim_end())))
        .collect()
}

/// A diagnostic's line with the file in it named by its plain path, `t.c`
/// left in.
fn plain_line(line: &str) -> String {
    match line.split_once(':') {
        Some((file, rest)) if !file.is_empty() && !file.starts_with(['<', ' ']) => {
            format!("{}:{rest}", plain(file))
        }
        _ => line.to_string(),
    }
}

/// On every whole program of merlin's tests and every C file of the kernel
/// and its user programs, for each target, merlin gives the diagnostics
/// clang gives, its default warnings above all (the programs compile).
#[test]
#[ignore = "needs clang on PATH"]
fn clang_warns_as_merlin_does_on_the_programs() {
    let programs = programs();
    assert!(programs.len() > 150, "only {} programs", programs.len());
    let jobs: Vec<(usize, Xlen)> = (0..programs.len())
        .flat_map(|n| [(n, Xlen::Rv32), (n, Xlen::Rv64)])
        .collect();
    let failures: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .chunks(jobs.len().div_ceil(16))
            .map(|chunk| {
                let programs = &programs;
                s.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|&(n, x)| {
                            let p = &programs[n];
                            let (want, got) = (clang_program_lines(p, x), merlin_program_lines(p, x));
                            (want != got).then(|| {
                                format!(
                                    "{} ({}):\n  clang  {want:?}\n  merlin {got:?}",
                                    p.path.display(),
                                    width(x)
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    assert!(
        failures.is_empty(),
        "{} differ from clang:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The length of the string literal that starts `s` (at its `"`), its
/// closing quote included.
fn literal_len(s: &str) -> Option<usize> {
    let mut chars = s.char_indices().skip(1);
    while let Some((i, ch)) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '"' => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// Rewrites the row written at `at` to want `lines`: the `&[...]` after
/// the source literal of the `case(` at that line and column.
fn bless(at: &std::panic::Location<'static>, lines: &[String]) -> Result<(), String> {
    // The location is relative to the workspace's root.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(at.file());
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out: Vec<String> = text.lines().map(str::to_string).collect();
    let line = out.get_mut(at.line() as usize - 1).ok_or("no such line")?;
    let col = at.column() as usize - 1;
    let start = col + line[col..].find("case(\"").ok_or("no case(\"...\" there")? + 5;
    let end = start + literal_len(&line[start..]).ok_or("the source runs past the line")?;
    let rest = line[end..].strip_prefix(", &[").ok_or("no &[ after the source")?;
    let mut i = 0;
    let close = loop {
        match rest.as_bytes().get(i) {
            Some(b'"') => i += literal_len(&rest[i..]).ok_or("a line runs past the line")?,
            Some(b']') => break i,
            Some(_) => i += 1,
            None => return Err("no ] closing the lines".into()),
        }
    };
    let wants: Vec<String> = lines
        .iter()
        .map(|l| format!("\"{}\"", l.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    *line = format!("{}, &[{}{}", &line[..end], wants.join(", "), &rest[close..]);
    let mut text = out.join("\n");
    text.push('\n');
    std::fs::write(&path, text).map_err(|e| e.to_string())
}
