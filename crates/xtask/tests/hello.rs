//! `examples/hello/hello.S` built with merlin-as (acceptance criteria of #4),
//! for RV32 and for RV64 (#151).
//!
//! The committed images (`crates/merlin-as/tests/golden/hello.elf` and
//! `golden/rv64/hello.elf`: merlin-as and ld.lld, rebuilt by `cargo xtask
//! hello`) always run on Grail, so this passes without LLVM. With ld.lld
//! installed the images are rebuilt from the current merlin-as and must load
//! the same; with clang and llvm-objdump too, `llvm-objdump -dr` of our object
//! must equal that of clang's (`-mno-relax`), for every example source.
//! `crates/merlin-as/tests/encode_hello.rs` checks the object against
//! committed clang output without LLVM.

use grail::RunResult;
use std::path::{Path, PathBuf};
use std::process::Command;
use xtask::examples::Xlen;
use xtask::hello::{self, golden, output};
use xtask::{examples, kernel, repo_root, tools};

const MAX_STEPS: u64 = 100_000;

fn assert_prints_hello(elf: &[u8], xlen: Xlen) {
    // hello reads nothing from the UART. An ELF64 file makes the machine RV64.
    let (r, out) = kernel::run_on_grail(elf, b"", MAX_STEPS).unwrap();
    assert_eq!(out, output(xlen));
    assert_eq!(r, RunResult::Exited(0));
}

fn committed(xlen: Xlen) -> Vec<u8> {
    let path = repo_root().join(golden(xlen));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("hello-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn committed_hello_prints_hello() {
    for xlen in examples::XLENS {
        let elf = committed(xlen);
        let class = elf::Elf::parse(&elf).unwrap().class;
        let want = match xlen {
            Xlen::Rv32 => elf::Class::Elf32,
            Xlen::Rv64 => elf::Class::Elf64,
        };
        assert_eq!(class, want, "{}", golden(xlen));
        assert_prints_hello(&elf, xlen);
    }
}

#[test]
fn rebuilt_hello_prints_hello_and_matches_committed() {
    let Some(lld) = tools::find("ld.lld") else {
        eprintln!("skipped: ld.lld not found");
        return;
    };
    for xlen in examples::XLENS {
        let elf = hello::build(&repo_root(), &lld, &scratch(&format!("lld-{xlen:?}")), xlen, true).unwrap();
        assert_prints_hello(&elf, xlen);
        assert!(
            hello::loaded_image(&elf).unwrap() == hello::loaded_image(&committed(xlen)).unwrap(),
            "{} is out of date: run `cargo xtask hello` and commit it",
            golden(xlen)
        );
    }
}

/// `llvm-objdump -d -r` of `obj`, run inside its directory so the header
/// line names just the file.
fn objdump(objdump: &Path, obj: &Path) -> String {
    let out = Command::new(objdump)
        .args(["-d", "-r"])
        .arg(obj.file_name().unwrap())
        .current_dir(obj.parent().unwrap())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().replace('\r', "")
}

/// Every example source, at each width: merlin-as assembles what merlin's
/// preprocessor makes of it, and clang assembles it too, preprocessing it
/// itself, with the Xcal mnemonics from `examples/xcal.inc` (RV64 with the
/// flags of the RV64 reference builds, `xtask::ld_fixtures::FLAGS`).
#[test]
fn objdump_of_merlin_as_objects_matches_clang() {
    let tools = tools::find("clang").and_then(|clang| {
        let objdump = tools::find_beside("llvm-objdump", &clang)?;
        Some((clang, objdump))
    });
    let Some((clang, llvm_objdump)) = tools else {
        eprintln!("skipped: clang or llvm-objdump not found");
        return;
    };
    let mut sources = std::collections::BTreeSet::new();
    for e in examples::EXAMPLES {
        for xlen in examples::XLENS {
            for s in e.sources {
                sources.insert((xlen == examples::Xlen::Rv64, format!("{}/{s}", e.dir)));
            }
        }
    }
    for (rv64, example) in sources {
        let src = repo_root().join("examples").join(&example);
        let xlen = if rv64 {
            examples::Xlen::Rv64
        } else {
            examples::Xlen::Rv32
        };
        let name = format!("{}-{xlen:?}", example.replace(['/', '.'], "-"));
        let obj = format!("{name}.o");

        let ours = scratch(&format!("objdump-merlin-{name}"));
        std::fs::create_dir_all(&ours).unwrap();
        let text = examples::assembly_of(
            &repo_root(),
            &format!("examples/{example}"),
            &[examples::INCLUDE_DIR],
            xlen,
        )
        .unwrap();
        let opts = merlin_as::Options { xlen };
        let data = merlin_as::assemble_to_elf_with(&text, &opts).unwrap();
        std::fs::write(ours.join(&obj), data).unwrap();

        let theirs = scratch(&format!("objdump-clang-{name}"));
        std::fs::create_dir_all(&theirs).unwrap();
        let mut clang_cmd = Command::new(&clang);
        if rv64 {
            clang_cmd
                .args(xtask::ld_fixtures::FLAGS)
                .arg(xtask::ld_fixtures::NO_RELAX);
        } else {
            clang_cmd
                .args(kernel::TARGET_FLAGS)
                .arg("-march=rv32ima_zicsr_zifencei")
                .arg("-c");
        }
        let status = clang_cmd
            .arg("-include")
            .arg(repo_root().join(examples::XCAL_INCLUDE))
            .arg("-I")
            .arg(repo_root().join(examples::INCLUDE_DIR))
            .arg(&src)
            .arg("-o")
            .arg(theirs.join(&obj))
            .status()
            .unwrap();
        assert!(status.success());

        let (ours, theirs) = (
            objdump(&llvm_objdump, &ours.join(&obj)),
            objdump(&llvm_objdump, &theirs.join(&obj)),
        );
        assert_eq!(ours, theirs, "{example} ({xlen:?})");
    }
}
