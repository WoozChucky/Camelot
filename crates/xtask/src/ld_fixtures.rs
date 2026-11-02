//! `cargo xtask ld-fixtures`: regenerates merlin-ld's test fixtures in
//! `crates/merlin-ld/tests/fixtures` with clang, ld.lld and llvm-readobj.
//!
//! In every fixture directory:
//!
//! * each `name.S` is assembled by clang into the committed `name.o`;
//! * each `name.ld` is linked by ld.lld, and `llvm-readelf -S -l -s` of the
//!   result is written to `name.expected.txt`. The objects are listed in the
//!   script's first comment, `/* objects: norelocs/start.o layouts/edge.o */`
//!   (paths relative to the fixtures directory); without one, every `.o` in
//!   the script's directory is linked, in name order.
//!
//! merlin-ld's tests link the same objects with the same scripts and compare
//! section addresses, `PT_LOAD`s and symbol values with `name.expected.txt`.
//!
//! `hello/hello.S` and `hello/link.ld` are copied from `examples/hello` first,
//! so the fixture always follows the example (a test checks the copies).
//!
//! Objects are built with `-mno-relax`: merlin-ld does no linker relaxation,
//! and ld.lld must not shrink code either or the layouts would differ.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// clang flags for every fixture object.
pub const ASM_FLAGS: &[&str] = &[
    "--target=riscv32",
    "-march=rv32im_zicsr",
    "-mabi=ilp32",
    "-mno-relax",
    "-c",
];

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
    let hello = fixtures.join("hello");
    for f in ["hello.S", "link.ld"] {
        std::fs::copy(examples.join("hello").join(f), hello.join(f))
            .map_err(|e| format!("copying examples/hello/{f}: {e}"))?;
    }

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
            let mut args: Vec<OsString> = ASM_FLAGS.iter().map(OsString::from).collect();
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
                let mut args: Vec<OsString> = vec!["-T".into(), script.clone().into_os_string()];
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
}
