//! `cargo xtask ld-fixtures`: regenerates merlin-ld's test fixtures in
//! `crates/merlin-ld/tests/fixtures` ([`FIXTURES`]) with clang, ld.lld,
//! llvm-readobj and llvm-objdump. The committed files were made by LLVM
//! 23.1.4 (clang and LLD 23.1.4, from the official Windows installer): the
//! version shows in the `.comment` section of every `.lld.elf` and in the
//! names of the mapping symbols (`$xrv32i2p1_m2p0_...`), neither of which
//! merlin-ld's tests compare (they check the allocated sections and the
//! global symbols).
//!
//! In every fixture directory:
//!
//! * each `name.S` is assembled by clang into the committed `name.o`;
//! * each `name.ld` is linked by ld.lld into the committed `name.lld.elf`,
//!   with `llvm-readelf -S -l -s` of it in `name.expected.txt` and
//!   `llvm-objdump -d` of it in `name.objdump.txt`. The objects are listed in
//!   the script's first comment, `/* objects: norelocs/start.o layouts/edge.o */`
//!   (paths relative to the fixtures directory); without one, every `.o` in
//!   the script's directory is linked, in name order.
//!
//! merlin-ld's tests link the same objects with the same scripts and compare
//! section addresses, `PT_LOAD`s and symbol values with `name.expected.txt`,
//! the loadable bytes with `name.lld.elf`, and merlin-objdump's disassembly of
//! the result with `name.objdump.txt`.
//!
//! The fixtures in [`FROM_EXAMPLES`] (`hello`, `multiobj`) are copied from
//! `examples/` first, so they always follow the examples (a test checks the
//! copies). Their ld.lld links are the references the examples built with our
//! own tools are compared with (`crates/xtask/tests/examples.rs`). Every
//! source is assembled with `-include examples/xcal.inc`, which teaches clang
//! the Xcal mnemonics the examples use.
//!
//! Objects are built with `-mno-relax` and linked with `--no-relax`: merlin-ld
//! does no linker relaxation, and ld.lld must not shrink code either or the
//! outputs would differ. A source containing the line `# xtask: relax` is
//! assembled with relaxation instead ([`RELAX_MARKER`]), to check that
//! merlin-ld ignores the `R_RISCV_RELAX` hints that come with it.
//!
//! `--out DIR` regenerates into `DIR` instead, from copies of the sources
//! ([`prepare_out`]): `diff -r` of the two trees then shows what a new LLVM
//! would change, with the committed files untouched.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The fixtures directory, relative to the repository root.
pub const FIXTURES: &str = "crates/merlin-ld/tests/fixtures";

/// clang flags for every fixture object, followed by [`NO_RELAX`] unless the
/// source asks for relaxation.
pub const ASM_FLAGS: &[&str] = &["--target=riscv32", "-march=rv32im_zicsr", "-mabi=ilp32", "-c"];

/// Fixture directories copied from `examples/<same name>`, with the files
/// copied (the sources and the linker script; objects are linked in name
/// order, which is also each example's source order).
pub const FROM_EXAMPLES: &[(&str, &[&str])] = &[
    ("hello", &["hello.S", "link.ld"]),
    ("multiobj", &["main.S", "print.S", "link.ld"]),
];

/// The default: no `R_RISCV_RELAX` or `R_RISCV_ALIGN` in the object.
pub const NO_RELAX: &str = "-mno-relax";

/// A line that makes a fixture source build with relaxation (`-mrelax`).
pub const RELAX_MARKER: &str = "# xtask: relax";

/// Whether a fixture source asks to be assembled with relaxation.
pub fn wants_relax(source: &str) -> bool {
    source.lines().any(|l| l.trim_end() == RELAX_MARKER)
}

/// The clang arguments that assemble a fixture source (before its file names).
pub fn asm_flags(source: &str) -> Vec<&'static str> {
    let mut flags = ASM_FLAGS.to_vec();
    flags.push(if wants_relax(source) { "-mrelax" } else { NO_RELAX });
    flags
}

/// The objects a fixture script lists in its `/* objects: ... */` comment.
pub fn script_objects(script: &str) -> Option<Vec<String>> {
    let start = script.find("/*")? + 2;
    let end = start + script[start..].find("*/")?;
    let list = script[start..end].trim().strip_prefix("objects:")?;
    Some(list.split_whitespace().map(str::to_string).collect())
}

pub struct Tools {
    pub clang: PathBuf,
    pub lld: PathBuf,
    pub readobj: PathBuf,
    pub objdump: PathBuf,
    pub verbose: bool,
}

impl Tools {
    fn exec(&self, tool: &Path, args: &[OsString], dir: &Path) -> Result<Vec<u8>, String> {
        if self.verbose {
            let shown: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
            eprintln!("[{}] {} {}", dir.display(), tool.display(), shown.join(" "));
        }
        let out = Command::new(tool)
            .args(args)
            .current_dir(dir)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", tool.display()))?;
        if !out.status.success() {
            return Err(format!(
                "{} failed in {}:\n{}",
                tool.display(),
                dir.display(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(out.stdout)
    }
}

/// Files in `dir` with extension `ext`, in name order.
fn files(dir: &Path, ext: &str) -> Result<Vec<PathBuf>, String> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
        .collect();
    v.sort();
    Ok(v)
}

fn file_name(p: &Path) -> OsString {
    p.file_name().unwrap_or_default().to_os_string()
}

/// Whether a file is a hand-written fixture source (`.S` or `.ld`), as
/// opposed to an output of [`run`].
fn is_source(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "S" || e == "ld")
}

/// Copies the hand-written sources of the fixtures in `from` (every `.S` and
/// `.ld`, in every directory below it) into the same places under `to`, so
/// that [`run`] can regenerate the outputs there (`cargo xtask ld-fixtures
/// --out DIR`). Returns the files copied.
pub fn copy_sources(from: &Path, to: &Path) -> Result<Vec<PathBuf>, String> {
    let mut copied = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(from)
        .map_err(|e| format!("{}: {e}", from.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        let target = to.join(file_name(&path));
        if path.is_dir() {
            copied.extend(copy_sources(&path, &target)?);
        } else if is_source(&path) {
            std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
            std::fs::copy(&path, &target).map_err(|e| format!("copying {}: {e}", path.display()))?;
            copied.push(target);
        }
    }
    Ok(copied)
}

/// Gets `out` ready for [`run`] in place of the committed `fixtures`
/// (`cargo xtask ld-fixtures --out DIR`): a new or empty directory, into
/// which the sources are copied ([`copy_sources`]). Regenerating there and
/// comparing the two trees shows what a new LLVM (or a changed source) would
/// change, without touching the committed files. An `out` that is the
/// fixtures directory itself is returned as it is: that is a plain run.
/// Returns `out` made absolute (the tools run in other directories).
pub fn prepare_out(fixtures: &Path, out: &Path) -> Result<PathBuf, String> {
    let out = std::path::absolute(out).map_err(|e| format!("{}: {e}", out.display()))?;
    if out.exists() {
        let same =
            |a: &Path, b: &Path| matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b);
        if same(&out, fixtures) {
            return Ok(out);
        }
        let mut contents = std::fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        if contents.next().is_some() {
            // Old outputs or sources left there would make the comparison lie.
            return Err(format!(
                "{} is not empty (--out needs a new or empty directory)",
                out.display()
            ));
        }
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    copy_sources(fixtures, &out)?;
    Ok(out)
}

/// Regenerates everything under `fixtures` (see the module documentation).
/// `examples` is the repository's `examples` directory. Returns the files
/// written.
pub fn run(fixtures: &Path, examples: &Path, tools: &Tools) -> Result<Vec<PathBuf>, String> {
    let mut written = Vec::new();
    for (dir, copied) in FROM_EXAMPLES {
        let to = fixtures.join(dir);
        std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        for f in *copied {
            std::fs::copy(examples.join(dir).join(f), to.join(f))
                .map_err(|e| format!("copying examples/{dir}/{f}: {e}"))?;
        }
    }
    let xcal = examples.join("xcal.inc");

    let mut dirs: Vec<PathBuf> = std::fs::read_dir(fixtures)
        .map_err(|e| format!("{}: {e}", fixtures.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    // Objects first: scripts may use objects from other directories.
    for dir in &dirs {
        for src in files(dir, "S")? {
            let obj = src.with_extension("o");
            let source = std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let mut args: Vec<OsString> = asm_flags(&source).into_iter().map(OsString::from).collect();
            args.extend(["-include".into(), xcal.clone().into_os_string()]);
            args.extend([file_name(&src), "-o".into(), file_name(&obj)]);
            tools.exec(&tools.clang, &args, dir)?;
            written.push(obj);
        }
    }

    let tmp = std::env::temp_dir().join(format!("xtask-ld-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let result = (|| {
        for dir in &dirs {
            for script in files(dir, "ld")? {
                let text =
                    std::fs::read_to_string(&script).map_err(|e| format!("{}: {e}", script.display()))?;
                let objects: Vec<PathBuf> = match script_objects(&text) {
                    Some(list) => list.iter().map(|o| fixtures.join(o)).collect(),
                    None => files(dir, "o")?,
                };
                let stem = script
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let elf = tmp.join(format!("{stem}.elf"));
                let mut args: Vec<OsString> =
                    vec!["--no-relax".into(), "-T".into(), script.clone().into_os_string()];
                args.extend(objects.into_iter().map(PathBuf::into_os_string));
                args.extend(["-o".into(), elf.clone().into_os_string()]);
                tools.exec(&tools.lld, &args, fixtures)?;
                let args: Vec<OsString> = ["--elf-output-style=GNU", "-S", "-l", "-s"]
                    .iter()
                    .map(OsString::from)
                    .chain([file_name(&elf)])
                    .collect();
                let listing = tools.exec(&tools.readobj, &args, &tmp)?;
                let out = dir.join(format!("{stem}.expected.txt"));
                std::fs::write(&out, listing).map_err(|e| format!("{}: {e}", out.display()))?;
                written.push(out);

                let copy = dir.join(format!("{stem}.lld.elf"));
                std::fs::copy(&elf, &copy).map_err(|e| format!("{}: {e}", copy.display()))?;
                written.push(copy.clone());
                let args: Vec<OsString> = vec!["-d".into(), file_name(&copy)];
                let listing = tools.exec(&tools.objdump, &args, dir)?;
                let out = dir.join(format!("{stem}.objdump.txt"));
                std::fs::write(&out, listing).map_err(|e| format!("{}: {e}", out.display()))?;
                written.push(out);
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result.map(|()| written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_comment() {
        assert_eq!(
            script_objects("/* objects: a/x.o  b/y.o */\nSECTIONS {}"),
            Some(vec!["a/x.o".to_string(), "b/y.o".to_string()])
        );
        assert_eq!(script_objects("/* a comment */ /* objects: x.o */"), None);
        assert_eq!(script_objects("SECTIONS {}"), None);
    }

    #[test]
    fn copied_examples_follow_the_examples_table() {
        for (dir, files) in FROM_EXAMPLES {
            let ex = crate::examples::find(dir).unwrap();
            let mut want = ex.sources.to_vec();
            want.extend(ex.script);
            assert_eq!(*files, want.as_slice(), "{dir}");
            // ld.lld gets the objects in name order: it must be the build's order.
            let mut sorted = ex.sources.to_vec();
            sorted.sort_unstable();
            assert_eq!(sorted, ex.sources, "{dir}");
        }
    }

    #[test]
    fn relax_marker() {
        let plain = asm_flags("    .text\n    nop\n");
        assert_eq!(plain.last(), Some(&"-mno-relax"));
        assert_eq!(&plain[..plain.len() - 1], ASM_FLAGS);
        let relax = asm_flags("# xtask: relax\r\n# built with relaxation\n    nop\n");
        assert_eq!(relax.last(), Some(&"-mrelax"));
        assert!(!relax.contains(&"-mno-relax"));
        // Only as a line of its own.
        assert!(!wants_relax("# see `# xtask: relax` in relax.S\n"));
    }

    /// A new scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-ld-fixtures-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn out_gets_the_sources_only() {
        let dir = scratch("out");
        let fixtures = dir.join("fixtures");
        for (f, text) in [
            ("a/x.S", "nop\n"),
            ("a/x.o", "object"),
            ("a/link.ld", "SECTIONS {}\n"),
            ("a/link.expected.txt", "listing"),
            ("a/link.lld.elf", "elf"),
            ("b/c/y.S", "ret\n"),
        ] {
            let path = fixtures.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let out = prepare_out(&fixtures, &dir.join("out")).unwrap();
        assert!(out.is_absolute());
        // Only the hand-written sources, at every depth.
        let mut got = Vec::new();
        let mut todo = vec![out.clone()];
        while let Some(d) = todo.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    todo.push(p);
                } else {
                    got.push(p.strip_prefix(&out).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        got.sort();
        assert_eq!(got, ["a/link.ld", "a/x.S", "b/c/y.S"]);
        assert_eq!(std::fs::read_to_string(out.join("b/c/y.S")).unwrap(), "ret\n");

        // A directory that is not empty is refused; the fixtures themselves
        // are a plain run, nothing copied.
        let err = prepare_out(&fixtures, &out).unwrap_err();
        assert!(
            err.ends_with("is not empty (--out needs a new or empty directory)"),
            "{err}"
        );
        assert_eq!(prepare_out(&fixtures, &fixtures).unwrap(), fixtures);
        assert_eq!(std::fs::read_to_string(fixtures.join("a/x.o")).unwrap(), "object");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
