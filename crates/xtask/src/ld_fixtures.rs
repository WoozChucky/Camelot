//! `cargo xtask ld-fixtures`: regenerates merlin-ld's test fixtures in
//! `crates/merlin-ld/tests/fixtures` ([`FIXTURES`]) with clang, ld.lld,
//! llvm-readobj and llvm-objdump. The committed files were made by LLVM
//! 23.1.4 (clang and LLD 23.1.4, from the official Windows installer): the
//! version shows in the `.comment` section of every `.lld.elf` and in the
//! names of the mapping symbols (`$xrv32i2p1_m2p0_...`), neither of which
//! merlin-ld's tests compare (they check the allocated sections and the
//! global symbols).
//!
//! There are two sets of fixtures ([`SETS`]), each a directory of fixture
//! directories built with its own clang flags: RV32 ([`RV32`]) in
//! `fixtures/` itself and RV64 ([`RV64`]) in `fixtures/rv64/`. The RV64 set
//! has sources of its own (`rv64/relocs`: what RV64 code can reach is not
//! what RV32's can; `rv64/at`: a higher-half program) and copies of the
//! RV32 sources that work on both widths unchanged ([`SHARED_WITH_RV64`]),
//! which [`run`] makes first, as it copies the examples, and a test checks.
//!
//! In every fixture directory:
//!
//! * each `name.S` is assembled by clang into the committed `name.o`;
//! * each `name.ld` is linked by ld.lld into the committed `name.lld.elf`,
//!   with `llvm-readelf -S -l -s` of it in `name.expected.txt` and
//!   `llvm-objdump -d` of it in `name.objdump.txt`. The objects are listed in
//!   the script's first comment, `/* objects: norelocs/start.o layouts/edge.o */`
//!   (paths relative to the set's directory); without one, every `.o` in
//!   the script's directory is linked, in name order.
//!
//! merlin-ld's tests link the same objects with the same scripts and compare
//! section addresses, `PT_LOAD`s and symbol values with `name.expected.txt`,
//! the loadable bytes with `name.lld.elf`, and merlin-objdump's disassembly of
//! the result with `name.objdump.txt`.
//!
//! The fixtures in [`FROM_EXAMPLES`] (`hello`, `multiobj`) are copied from
//! `examples/` first, so they always follow the examples (a test checks the
//! copies), and the RV64 set shares them ([`SHARED_WITH_RV64`]): an example
//! has one source for both widths (#193), preprocessed by clang with
//! `__riscv_xlen` for each. Their ld.lld links are the references the
//! examples built with our own tools are compared with
//! (`crates/xtask/tests/examples.rs`). Every source is assembled with
//! `-include examples/xcal.inc`, which teaches clang the Xcal mnemonics the
//! examples use, and `-I include`, where `asm.h` is.
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

/// A set of fixtures: a directory of fixture directories, and how its
/// objects are built.
#[derive(Debug, PartialEq, Eq)]
pub struct Set {
    /// Its directory, relative to [`FIXTURES`] (`""`: the fixtures directory
    /// itself).
    pub dir: &'static str,
    /// clang flags for every object of the set, followed by [`NO_RELAX`]
    /// unless the source asks for relaxation ([`asm_flags`]).
    pub flags: &'static [&'static str],
}

/// The RV32 set, in the fixtures directory itself.
pub const RV32: Set = Set {
    dir: "",
    flags: &["--target=riscv32", "-march=rv32im_zicsr", "-mabi=ilp32", "-c"],
};

/// The RV64 set, in `fixtures/rv64`: the target and ABI of RV64 Camelot,
/// with the code model its programs need at 0x8000_0000 (`medany`:
/// pc-relative addressing, as `lui` cannot reach there on RV64). The model
/// changes no byte of an assembled `.S`; it is there so that the rule holds
/// for everything built for RV64.
pub const RV64: Set = Set {
    dir: "rv64",
    flags: &[
        "--target=riscv64",
        "-march=rv64ima_zicsr_zifencei",
        "-mabi=lp64",
        "-mcmodel=medany",
        "-c",
    ],
};

/// Every set, in the order [`run`] builds them.
pub const SETS: &[Set] = &[RV32, RV64];

/// Fixture directories of the RV32 set copied from `examples/<same name>`,
/// with the files copied (the sources and the linker script; objects are
/// linked in name order, which is also each example's source order).
pub const FROM_EXAMPLES: &[(&str, &[&str])] = &[
    ("hello", &["hello.S", "link.ld"]),
    ("multiobj", &["main.S", "print.S", "link.ld"]),
];

/// Fixture directories of the RV32 set whose sources (`.S` and `.ld`) the
/// RV64 set has too, copied into `fixtures/rv64/<same name>`: they assemble,
/// link and run on both widths (`multiobj` through `asm.h`'s `RISCV_PTR`
/// and `REG_L`, as the example does). Not `relocs`, `data64` (RV32's
/// R_RISCV_64) or `at` (RV64 has its own, a higher-half program).
pub const SHARED_WITH_RV64: &[&str] = &[
    "hello", "layouts", "merge", "multiobj", "norelocs", "rejects", "symbols",
];

/// The default: no `R_RISCV_RELAX` or `R_RISCV_ALIGN` in the object.
pub const NO_RELAX: &str = "-mno-relax";

/// A line that makes a fixture source build with relaxation (`-mrelax`).
pub const RELAX_MARKER: &str = "# xtask: relax";

/// Whether a fixture source asks to be assembled with relaxation.
pub fn wants_relax(source: &str) -> bool {
    source.lines().any(|l| l.trim_end() == RELAX_MARKER)
}

/// The clang arguments that assemble a fixture source of `set` (before its
/// file names).
pub fn asm_flags(set: &Set, source: &str) -> Vec<&'static str> {
    let mut flags = set.flags.to_vec();
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

/// Regenerates everything under `fixtures` (see the module documentation):
/// the copies of the examples ([`FROM_EXAMPLES`]) and of the shared sources
/// ([`copy_shared`]), then every set's objects and links ([`SETS`]).
/// `root` is the repository's root, where `examples/` and `include/` are.
/// Returns the files written.
pub fn run(fixtures: &Path, root: &Path, tools: &Tools) -> Result<Vec<PathBuf>, String> {
    let examples = root.join("examples");
    for (dir, copied) in FROM_EXAMPLES {
        let to = fixtures.join(dir);
        std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        for f in *copied {
            std::fs::copy(examples.join(dir).join(f), to.join(f))
                .map_err(|e| format!("copying examples/{dir}/{f}: {e}"))?;
        }
    }
    // After the examples: `hello` and `multiobj` are shared too.
    copy_shared(fixtures)?;
    let xcal = examples.join("xcal.inc");
    let include = root.join(crate::examples::INCLUDE_DIR);
    let mut written = Vec::new();
    for set in SETS {
        written.extend(build_set(fixtures, set, &xcal, &include, tools)?);
    }
    Ok(written)
}

/// The hand-written sources (`.S` and `.ld`) directly in `dir`, in name order.
pub fn sources(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut v = files(dir, "S")?;
    v.extend(files(dir, "ld")?);
    v.sort();
    Ok(v)
}

/// Gives each directory of [`SHARED_WITH_RV64`] in the RV64 set the
/// sources of the RV32 one, `fixtures/<dir>` into `fixtures/rv64/<dir>`:
/// copies them, and removes any other source there (one the RV32 set no
/// longer has), so the two hold the same files.
pub fn copy_shared(fixtures: &Path) -> Result<(), String> {
    for dir in SHARED_WITH_RV64 {
        let (from, to) = (fixtures.join(dir), fixtures.join(RV64.dir).join(dir));
        std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        let wanted = sources(&from)?;
        for old in sources(&to)? {
            if !wanted.iter().any(|w| w.file_name() == old.file_name()) {
                std::fs::remove_file(&old).map_err(|e| format!("{}: {e}", old.display()))?;
            }
        }
        for src in wanted {
            let target = to.join(file_name(&src));
            std::fs::copy(&src, &target).map_err(|e| format!("copying {}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// The fixture directories of `set`: the directories in its own, but not
/// another set's (the RV64 set's `rv64` is in the RV32 set's directory),
/// in name order.
pub fn set_dirs(fixtures: &Path, set: &Set) -> Result<Vec<PathBuf>, String> {
    let root = fixtures.join(set.dir);
    let other_sets: Vec<PathBuf> = SETS
        .iter()
        .filter(|s| s.dir != set.dir)
        .map(|s| fixtures.join(s.dir))
        .collect();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && !other_sets.contains(p))
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// Builds one set: every source's object with the set's flags, then every
/// script's link, listing and disassembly.
fn build_set(
    fixtures: &Path,
    set: &Set,
    xcal: &Path,
    include: &Path,
    tools: &Tools,
) -> Result<Vec<PathBuf>, String> {
    let root = fixtures.join(set.dir);
    let dirs = set_dirs(fixtures, set)?;
    let mut written = Vec::new();

    // Objects first: scripts may use objects from other directories.
    for dir in &dirs {
        for src in files(dir, "S")? {
            let obj = src.with_extension("o");
            let source = std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let mut args: Vec<OsString> = asm_flags(set, &source).into_iter().map(OsString::from).collect();
            args.extend(["-include".into(), xcal.as_os_str().to_os_string()]);
            args.extend(["-I".into(), include.as_os_str().to_os_string()]);
            args.extend([file_name(&src), "-o".into(), file_name(&obj)]);
            tools.exec(&tools.clang, &args, dir)?;
            written.push(obj);
        }
    }

    let set_name = if set.dir.is_empty() { "rv32" } else { set.dir };
    let tmp = std::env::temp_dir().join(format!("xtask-ld-fixtures-{set_name}-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let result = (|| {
        for dir in &dirs {
            for script in files(dir, "ld")? {
                let text =
                    std::fs::read_to_string(&script).map_err(|e| format!("{}: {e}", script.display()))?;
                let objects: Vec<PathBuf> = match script_objects(&text) {
                    Some(list) => list.iter().map(|o| root.join(o)).collect(),
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
                tools.exec(&tools.lld, &args, &root)?;
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
            // One source for both widths: the RV64 set has the same copy.
            assert!(SHARED_WITH_RV64.contains(dir), "{dir}");
        }
    }

    /// The committed fixtures as the sets see them: the RV32 set's
    /// directories leave out the RV64 set's, and every directory the RV64
    /// set shares has sources in the RV32 one.
    #[test]
    fn sets_of_the_committed_fixtures() {
        let fixtures = crate::repo_root().join(FIXTURES);
        let names = |set: &Set| -> Vec<String> {
            set_dirs(&fixtures, set)
                .unwrap()
                .iter()
                .map(|d| file_name(d).to_string_lossy().into_owned())
                .collect()
        };
        let (rv32, rv64) = (names(&RV32), names(&RV64));
        assert!(!rv32.iter().any(|d| d == RV64.dir), "{rv32:?}");
        assert!(rv32.iter().any(|d| d == "relocs") && rv64.iter().any(|d| d == "relocs"));
        for dir in SHARED_WITH_RV64 {
            assert!(
                rv32.iter().any(|d| d == dir) && rv64.iter().any(|d| d == dir),
                "{dir}"
            );
            assert!(!sources(&fixtures.join(dir)).unwrap().is_empty(), "{dir}");
        }
    }

    #[test]
    fn relax_marker() {
        for set in SETS {
            let plain = asm_flags(set, "    .text\n    nop\n");
            assert_eq!(plain.last(), Some(&"-mno-relax"));
            assert_eq!(&plain[..plain.len() - 1], set.flags);
            let relax = asm_flags(set, "# xtask: relax\r\n# built with relaxation\n    nop\n");
            assert_eq!(relax.last(), Some(&"-mrelax"));
            assert!(!relax.contains(&"-mno-relax"));
        }
        assert!(asm_flags(&RV64, "").contains(&"-mabi=lp64"));
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
