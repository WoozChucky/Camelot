//! merlin's calling convention against clang's: structs of every size class
//! passed and returned both ways between merlin-compiled and clang-compiled
//! code (see `tests/abi/clang_side.c` for the cases).
//!
//! Run by `cargo test`, without LLVM: `tests/abi/clang_side.o` is committed.
//!
//! * `merlin_and_clang_call_each_other`: merlin compiles
//!   `tests/abi/merlin_side.c`; merlin-ld links it with the committed clang
//!   object and `tests/crt0.s`; Grail runs it. Exit code 30 means every
//!   check passed.
//! * `merlin_calls_merlin`: the same program with both sides compiled by
//!   merlin.
//!
//! Ignored, since they need clang on `PATH`:
//!
//! * `clang_side`: compiles `clang_side.c` again and checks that the
//!   committed `clang_side.s` matches; with `MERLIN_BLESS=1` it rewrites
//!   `clang_side.s` and `clang_side.o` instead (after changing the source):
//!
//!       MERLIN_BLESS=1 cargo test -p merlin --test abi -- --ignored clang_side
//!
//! * `clang_calls_clang`: both sides compiled by clang, which checks the
//!   program itself.

use grail::{Machine, RunResult};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The exit code when every check passes.
const EXPECT: u8 = 30;

/// How the clang side is compiled (and the merlin side, for `clang_calls_clang`).
const CLANG_FLAGS: &[&str] = &[
    "--target=riscv32",
    "-march=rv32im",
    "-mabi=ilp32",
    "-std=c99",
    "-O1",
    "-mno-relax",
    "-msmall-data-limit=0",
    "-ffreestanding",
    "-fno-builtin",
    "-fno-addrsig",
];

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn abi_dir() -> PathBuf {
    tests_dir().join("abi")
}

/// Assembles with merlin-as.
fn assemble(asm: &str) -> Vec<u8> {
    merlin_as::assemble_to_elf(asm).unwrap_or_else(|e| panic!("merlin-as: {e:?}"))
}

/// Compiles a file of `tests/abi` with merlin, then assembles it.
fn merlin_object(file: &str) -> Vec<u8> {
    let src = std::fs::read_to_string(abi_dir().join(file)).unwrap();
    let asm = merlin::compile(&src).unwrap_or_else(|errs| {
        let msgs: Vec<String> = errs.iter().map(|e| e.render(file)).collect();
        panic!("{}", msgs.join("\n"))
    });
    assemble(&asm)
}

/// Links `tests/crt0.s` and the objects with merlin-ld, runs the program in
/// Grail and returns its exit code.
fn link_and_run(objs: Vec<(&str, Vec<u8>)>) -> u8 {
    let crt0 = std::fs::read_to_string(tests_dir().join("crt0.s")).unwrap();
    let mut inputs = vec![merlin_ld::Input {
        name: "crt0.o".into(),
        data: assemble(&crt0),
    }];
    for (name, data) in objs {
        inputs.push(merlin_ld::Input {
            name: name.into(),
            data,
        });
    }
    let linked = merlin_ld::link(&inputs, &merlin_ld::Options::default())
        .unwrap_or_else(|e| panic!("merlin-ld: {e:?}"));
    let mut m = Machine::with_ram(grail::DEFAULT_RAM_SIZE);
    m.load_elf(&linked.elf).unwrap();
    match m.run(Some(10_000_000), |_| {}) {
        RunResult::Exited(code) => code as u8,
        RunResult::StepLimit => panic!("still running at pc={:#010x}", m.cpu.pc),
    }
}

#[test]
fn merlin_and_clang_call_each_other() {
    let clang_side = std::fs::read(abi_dir().join("clang_side.o")).unwrap();
    let code = link_and_run(vec![
        ("merlin_side.o", merlin_object("merlin_side.c")),
        ("clang_side.o", clang_side),
    ]);
    assert_eq!(code, EXPECT, "{} checks failed", EXPECT as i32 - code as i32);
}

#[test]
fn merlin_calls_merlin() {
    let code = link_and_run(vec![
        ("merlin_side.o", merlin_object("merlin_side.c")),
        ("clang_side.o", merlin_object("clang_side.c")),
    ]);
    assert_eq!(code, EXPECT);
}

/// Runs clang in `tests/abi` on `file` (a relative name, so no absolute path
/// ends up in the output) with the extra arguments.
fn clang(file: &str, extra: &[&str]) {
    let out = Command::new("clang")
        .current_dir(abi_dir())
        .args(CLANG_FLAGS)
        .args(extra)
        .arg(file)
        .output()
        .expect("running clang");
    assert!(
        out.status.success(),
        "clang failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("merlin-abi-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "needs clang on PATH"]
fn clang_side() {
    let scratch = Scratch::new("regen");
    let (s, o) = (scratch.0.join("clang_side.s"), scratch.0.join("clang_side.o"));
    clang("clang_side.c", &["-S", "-o", s.to_str().unwrap()]);
    clang("clang_side.c", &["-c", "-o", o.to_str().unwrap()]);
    let asm = std::fs::read_to_string(&s).unwrap();
    if std::env::var_os("MERLIN_BLESS").is_some() {
        std::fs::write(abi_dir().join("clang_side.s"), &asm).unwrap();
        std::fs::copy(&o, abi_dir().join("clang_side.o")).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(abi_dir().join("clang_side.s")).unwrap();
    assert_eq!(
        committed.replace("\r\n", "\n"),
        asm.replace("\r\n", "\n"),
        "tests/abi/clang_side.s is out of date (MERLIN_BLESS=1 to regenerate)"
    );
}

#[test]
#[ignore = "needs clang on PATH"]
fn clang_calls_clang() {
    let scratch = Scratch::new("clang");
    let mut objs = Vec::new();
    for (c, o) in [
        ("clang_side.c", "clang_side.o"),
        ("merlin_side.c", "merlin_side.o"),
    ] {
        let path = scratch.0.join(o);
        clang(c, &["-c", "-o", path.to_str().unwrap()]);
        objs.push((o, std::fs::read(&path).unwrap()));
    }
    assert_eq!(link_and_run(objs), EXPECT);
}
