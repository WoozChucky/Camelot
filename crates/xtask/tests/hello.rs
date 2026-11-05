//! `examples/hello/hello.S` built with merlin-as (acceptance criteria of #4).
//!
//! The committed image (`crates/merlin-as/tests/golden/hello.elf`: merlin-as
//! and ld.lld, rebuilt by `cargo xtask hello`) always runs on Grail, so this
//! passes without LLVM. With ld.lld installed the image is rebuilt from the
//! current merlin-as and must load the same; with clang and llvm-objdump too,
//! `llvm-objdump -dr` of our object must equal that of clang's
//! (`-mno-relax`), for every example source.
//! `crates/merlin-as/tests/encode_hello.rs` checks the object against
//! committed clang output without LLVM.

use grail::{Machine, MachineConfig, RunResult};
use std::path::{Path, PathBuf};
use std::process::Command;
use xtask::hello::{self, GOLDEN, OUTPUT};
use xtask::{examples, repo_root, tools};

const MAX_STEPS: u64 = 100_000;

/// Runs `elf` on a machine without firmware (`--bios none`: hello is a
/// bare-metal M-mode program) and checks what it prints.
fn assert_prints_hello(elf: &[u8]) {
    // hello reads nothing from the UART.
    let mut m = Machine::new(MachineConfig::default()).unwrap();
    m.cpu.bus.uart.capture = true;
    m.load_elf(elf).unwrap();
    let r = m.run(Some(MAX_STEPS), |_| {});
    assert_eq!(String::from_utf8_lossy(&m.cpu.bus.uart.captured), OUTPUT);
    assert_eq!(r, RunResult::Exited(0));
}

fn committed() -> Vec<u8> {
    let path = repo_root().join(GOLDEN);
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
    let elf = committed();
    assert_eq!(
        elf::Elf::parse(&elf).unwrap().class,
        elf::Class::Elf64,
        "{GOLDEN}"
    );
    assert_prints_hello(&elf);
}

#[test]
fn rebuilt_hello_prints_hello_and_matches_committed() {
    let Some(lld) = tools::find("ld.lld") else {
        eprintln!("skipped: ld.lld not found");
        return;
    };
    let elf = hello::build(&repo_root(), &lld, &scratch("lld"), true).unwrap();
    assert_prints_hello(&elf);
    assert!(
        hello::loaded_image(&elf).unwrap() == hello::loaded_image(&committed()).unwrap(),
        "{GOLDEN} is out of date: run `cargo xtask hello` and commit it"
    );
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

/// Every example source: merlin-as assembles what merlin's preprocessor makes
/// of it, and clang assembles it too, preprocessing it itself, with the Xcal
/// mnemonics from `examples/xcal.inc` and the flags of the reference builds
/// (`xtask::ld_fixtures::FLAGS`).
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
        for s in e.sources {
            sources.insert(format!("{}/{s}", e.dir));
        }
    }
    for example in sources {
        let src = repo_root().join("examples").join(&example);
        let name = example.replace(['/', '.'], "-");
        let obj = format!("{name}.o");

        let ours = scratch(&format!("objdump-merlin-{name}"));
        std::fs::create_dir_all(&ours).unwrap();
        let text = examples::assembly_of(
            &repo_root(),
            &format!("examples/{example}"),
            &[examples::INCLUDE_DIR],
        )
        .unwrap();
        let data = merlin_as::assemble_to_elf(&text).unwrap();
        std::fs::write(ours.join(&obj), data).unwrap();

        let theirs = scratch(&format!("objdump-clang-{name}"));
        std::fs::create_dir_all(&theirs).unwrap();
        let status = Command::new(&clang)
            .args(xtask::ld_fixtures::FLAGS)
            .arg(xtask::ld_fixtures::NO_RELAX)
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
        assert_eq!(ours, theirs, "{example}");
    }
}
