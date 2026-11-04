//! A reference build of the hello example: `examples/hello/hello.S`
//! assembled by merlin-as and linked by ld.lld with `examples/hello/link.ld`
//! (bootstrapping step 1, spec section 10), for RV32 and for RV64 (#151):
//! hello.S is the same source at both widths. The example itself is built
//! with merlin-ld instead ([`crate::examples`]); both load the same image.
//!
//! `cargo xtask hello` writes the results to [`golden`]'s files, which are
//! committed so that tests can run them in Grail without LLVM; with ld.lld
//! installed, the tests also rebuild them and check that the loaded images
//! are unchanged.

use crate::examples::Xlen;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The committed RV32 image, relative to the repository root.
pub const GOLDEN: &str = "crates/merlin-as/tests/golden/hello.elf";

/// The committed RV64 image, next to merlin-as's other RV64 references.
pub const GOLDEN_RV64: &str = "crates/merlin-as/tests/golden/rv64/hello.elf";

/// The committed image of `xlen`: [`GOLDEN`] or [`GOLDEN_RV64`].
pub fn golden(xlen: Xlen) -> &'static str {
    match xlen {
        Xlen::Rv32 => GOLDEN,
        Xlen::Rv64 => GOLDEN_RV64,
    }
}

/// What the image prints in Grail: the UART text, then `xcal.dbg a0` with the
/// byte count, at the same pc as in `examples/hello/hello.elf` (neither link
/// relaxes `call puts` into a single `jal`).
pub const OUTPUT: &str = "Hello from Camelot!\n[dbg] pc=80000018 a0 = 0x00000014 (20)\n";

/// What the RV64 image prints: the same, with `xcal.dbg`'s 64-bit pc and
/// register.
pub const OUTPUT_RV64: &str = "Hello from Camelot!\n[dbg] pc=0000000080000018 a0 = 0x0000000000000014 (20)\n";

/// What the image of `xlen` prints: [`OUTPUT`] or [`OUTPUT_RV64`].
pub fn output(xlen: Xlen) -> &'static str {
    match xlen {
        Xlen::Rv32 => OUTPUT,
        Xlen::Rv64 => OUTPUT_RV64,
    }
}

/// Assembles hello.S with merlin-as for `xlen` into `work_dir/hello.o`, links
/// it into `work_dir/hello.elf` with `lld` (which takes the width from the
/// object), and returns the executable's bytes.
pub fn build(root: &Path, lld: &Path, work_dir: &Path, xlen: Xlen, verbose: bool) -> Result<Vec<u8>, String> {
    let example = root.join("examples").join("hello");
    let src_path = example.join("hello.S");
    let src =
        std::fs::read_to_string(&src_path).map_err(|e| format!("cannot read {}: {e}", src_path.display()))?;
    let obj = merlin_as::assemble_to_elf_with(&src, &merlin_as::Options { xlen }).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("hello.S: {e}")).collect();
        format!("merlin-as failed:\n{}", lines.join("\n"))
    })?;
    std::fs::create_dir_all(work_dir).map_err(|e| format!("cannot create {}: {e}", work_dir.display()))?;
    let obj_path = work_dir.join("hello.o");
    std::fs::write(&obj_path, obj).map_err(|e| format!("cannot write {}: {e}", obj_path.display()))?;

    let out: PathBuf = work_dir.join("hello.elf");
    let mut cmd = Command::new(lld);
    cmd.arg("-T")
        .arg(example.join("link.ld"))
        .arg(&obj_path)
        .arg("-o")
        .arg(&out);
    if verbose {
        eprintln!("{cmd:?}");
    }
    let status = cmd
        .status()
        .map_err(|e| format!("cannot run {}: {e}", lld.display()))?;
    if !status.success() {
        return Err(format!("{} failed ({status})", lld.display()));
    }
    std::fs::read(&out).map_err(|e| format!("cannot read {}: {e}", out.display()))
}

/// What Grail loads from an executable: the entry point and every `PT_LOAD`
/// segment as (address, file bytes, memory size).
#[derive(Debug, PartialEq, Eq)]
pub struct LoadedImage {
    pub entry: u64,
    pub segments: Vec<(u64, Vec<u8>, u64)>,
}

/// The [`LoadedImage`] of an executable. Two links that load the same can
/// still differ elsewhere (ld.lld writes its version into `.comment`).
pub fn loaded_image(elf: &[u8]) -> Result<LoadedImage, String> {
    let e = grail::elf::parse(elf).map_err(|e| e.to_string())?;
    let segments = e
        .segments
        .into_iter()
        .map(|s| (s.paddr, s.data, s.mem_size))
        .collect();
    Ok(LoadedImage {
        entry: e.entry,
        segments,
    })
}
