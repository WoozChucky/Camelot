//! `cargo xtask ld-fixtures`: regenerates merlin-ld's test fixtures in
//! `crates/merlin-ld/tests/fixtures` with clang, ld.lld, llvm-readobj and
//! llvm-objdump.
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

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

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
}
